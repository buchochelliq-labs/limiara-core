//! The Contacts-to-engine migration on every backend pair (ADR 0035 §2):
//! the dry run, a blocked start, a copy that resumes after an
//! interruption, and the copied records as the engine serves them.

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use contacts_engine_migration::*;
use contacts_plugin::store::{Actor, ContactsStore};
use contacts_plugin::{
    ContactCustomFieldTarget, ContactCustomFieldType, ContactsAuthorization,
    ContactsMutationContext, ContactsService, CreateCustomFieldDefinition, CreateOrganisation,
    CreatePerson, DataClassification, EngineMigration, EngineMigrationPhase, UpdatePerson,
    ORGANISATIONS_READ_PERMISSION, ORGANISATIONS_WRITE_PERMISSION, PEOPLE_READ_PERMISSION,
    PEOPLE_WRITE_PERMISSION, SETTINGS_MANAGE_PERMISSION,
};
use platform_objects::*;
use plugin_contract::Manifest;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use uuid::Uuid;

fn contacts(store: &Arc<dyn ContactsStore>, tenant: Uuid, member: Uuid) -> ContactsService {
    ContactsService::with_store(
        store.clone(),
        ContactsAuthorization::new(
            tenant,
            member,
            [
                PEOPLE_READ_PERMISSION,
                PEOPLE_WRITE_PERMISSION,
                ORGANISATIONS_READ_PERMISSION,
                ORGANISATIONS_WRITE_PERMISSION,
                SETTINGS_MANAGE_PERMISSION,
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ),
    )
}

fn context() -> ContactsMutationContext {
    ContactsMutationContext::new(Uuid::now_v7())
}

fn person(name: &str, organisation: Option<&str>) -> CreatePerson {
    CreatePerson {
        full_name: name.into(),
        email: None,
        telephone: None,
        organisation_id: organisation.map(str::to_owned),
        tags: Vec::new(),
        custom_fields: BTreeMap::new(),
    }
}

fn organisation(name: &str) -> CreateOrganisation {
    CreateOrganisation {
        name: name.into(),
        email: None,
        telephone: None,
        custom_fields: BTreeMap::new(),
    }
}

/// Stores keep times to the microsecond.
fn micros(at: DateTime<Utc>) -> i64 {
    at.timestamp_micros()
}

async fn install(objects: &Arc<dyn ObjectStore>, tenant: Uuid, member: Uuid) {
    let manifest =
        Manifest::parse_toml(include_str!("../../../plugin.toml")).expect("the Contacts manifest");
    let installer = ObjectService::new(
        objects.clone(),
        ObjectCaller::plugin_installer(tenant, member, "platform.contacts"),
    );
    installer
        .install_plugin_objects(&manifest, MutationContext::new(Uuid::now_v7()))
        .await
        .expect("install");
    installer
        .resume_plugin_objects("platform.contacts", MutationContext::new(Uuid::now_v7()))
        .await
        .expect("enable");
}

pub async fn run(
    store: Arc<dyn ContactsStore>,
    objects: Arc<dyn ObjectStore>,
    tenant: Uuid,
    other_tenant: Uuid,
) {
    let member = Uuid::now_v7();
    let former = Uuid::now_v7();
    install(&objects, tenant, member).await;
    let service = contacts(&store, tenant, member);
    let by_former = contacts(&store, tenant, former);

    // --- A workspace as Contacts holds it ---------------------------------
    let acme = service
        .create_organisation(organisation("Acme"), context())
        .await
        .expect("acme");
    let globex = by_former
        .create_organisation(organisation("Globex"), context())
        .await
        .expect("globex");
    let mut ada = person("  Ada   Lovelace ", Some(&acme.public_id));
    ada.email = Some("ada@example.org".into());
    ada.tags = vec!["VIP".into(), "Speaker".into()];
    ada.telephone = Some("ext twelve".into());
    let ada = service.create_person(ada, context()).await.expect("ada");
    let plato = by_former
        .create_person(person("Plato", None), context())
        .await
        .expect("plato");
    let ludwig = service
        .create_person(
            person("Ludwig van Beethoven", Some(&globex.public_id)),
            context(),
        )
        .await
        .expect("ludwig");
    let ludwig = service
        .archive_person(&ludwig.public_id, ludwig.version, context())
        .await
        .expect("archive");
    let binned = service
        .create_person(person("Binned Person", None), context())
        .await
        .expect("binned");
    let binned = service
        .delete_person(&binned.public_id, binned.version, context())
        .await
        .expect("delete");

    let members = BTreeSet::from([member]);
    let migration = Migration::new(
        store.clone(),
        objects.clone(),
        tenant,
        member,
        members.clone(),
    );

    // --- The dry run reports, writes nothing, and blocks on a refusal ----
    let report = migration.dry_run().await.expect("dry run");
    assert_eq!(
        (
            report.organisations.active,
            report.organisations.without_owner
        ),
        (2, 1),
        "Globex's creator is no longer a member"
    );
    assert_eq!(
        (
            report.people.active,
            report.people.archived,
            report.people.deleted,
            report.people.without_owner
        ),
        (2, 1, 1, 1)
    );
    assert_eq!(report.long_names, vec![ludwig.public_id.clone()]);
    assert_eq!(
        report
            .people
            .refused
            .iter()
            .map(|refusal| (refusal.public_id.as_str(), refusal.field.as_str()))
            .collect::<Vec<_>>(),
        vec![(ada.public_id.as_str(), "platform.contacts:telephone")],
        "Contacts never checked telephone numbers"
    );
    assert_eq!(
        report.blockers.iter().map(|b| b.code).collect::<Vec<_>>(),
        vec![BLOCKED_REFUSED]
    );
    assert!(matches!(
        migration.copy().await,
        Err(MigrationError::Blocked(blocked)) if blocked.blockers[0].code == BLOCKED_REFUSED
    ));
    assert_eq!(
        migration.state().await.expect("state"),
        None,
        "nothing written"
    );

    // Fixed through the Contacts API, it passes.
    let ada = service
        .update_person(
            &ada.public_id,
            UpdatePerson {
                expected_version: ada.version,
                full_name: ada.full_name.clone(),
                email: ada.email.clone(),
                telephone: Some("+44 20 7946 0018".into()),
                organisation_id: Some(acme.public_id.clone()),
                tags: ada.tags.clone(),
                custom_fields: BTreeMap::new(),
            },
            context(),
        )
        .await
        .expect("fix the telephone");
    assert!(migration.dry_run().await.expect("again").can_start());

    // --- An interrupted copy resumes ---------------------------------------
    // As if a run finished the organisations, wrote Ada's batch and stopped
    // before its checkpoint.
    let engine = ObjectService::new(objects.clone(), ObjectCaller::migrator(tenant, member));
    engine
        .import_records(
            ORGANISATION,
            vec![
                organisation_record(&acme, &[], &members),
                organisation_record(&globex, &[], &members),
            ],
            0,
            MutationContext::new(Uuid::now_v7()),
        )
        .await
        .expect("the organisations' pass");
    engine
        .import_records(
            PERSON,
            vec![person_record(&ada, &[], &members)],
            0,
            MutationContext::new(Uuid::now_v7()),
        )
        .await
        .expect("an earlier batch");
    {
        let mut tx = store.begin(tenant, Actor::System).await.expect("begin");
        tx.save_engine_migration(
            tenant,
            &EngineMigration {
                phase: EngineMigrationPhase::Copying,
                checkpoint: json!({
                    "organisations_done": true,
                    "organisations_after": acme.id.max(globex.id),
                }),
                checksums: json!({}),
                cutover_at: None,
                rollback_deadline: None,
                version: 0,
                updated_at: Utc::now(),
            },
            None,
        )
        .await
        .expect("an interrupted state");
        tx.commit().await.expect("commit");
    }
    // Contacts stays writable meanwhile: an organisation the pass did not
    // see, and a person pointing at it.
    let initech = service
        .create_organisation(organisation("Initech"), context())
        .await
        .expect("initech");
    let peter = service
        .create_person(person("Peter Gibbons", Some(&initech.public_id)), context())
        .await
        .expect("peter");
    let copied = migration.copy().await.expect("copy");
    assert_eq!(
        (copied.organisations, copied.people, copied.already_there),
        (1, 4, 1),
        "Initech comes with Peter; Ada's batch is not written twice"
    );
    assert_eq!(copied.state.people.count, 5);
    let state = migration.state().await.expect("state").expect("written");
    assert_eq!(state.phase, EngineMigrationPhase::Copied);
    assert_eq!(
        state.checksums,
        serde_json::to_value(&copied.state).expect("json")
    );
    assert!(matches!(
        migration.copy().await,
        Err(MigrationError::Phase(EngineMigrationPhase::Copied))
    ));

    // --- The engine holds them, as ADR 0035 §1 maps them -----------------
    let installer = ObjectService::new(
        objects.clone(),
        ObjectCaller::plugin_installer(tenant, member, "platform.contacts"),
    );
    installer
        .wake_plugin_objects(
            "platform.contacts",
            &[PERSON, ORGANISATION],
            MutationContext::new(Uuid::now_v7()),
        )
        .await
        .expect("wake, as a cutover will");
    let admin = ObjectService::new(
        objects.clone(),
        ObjectCaller::new(
            tenant,
            member,
            [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Vec::new(),
        ),
    );
    let read = admin
        .get_record(PERSON, &ada.public_id, true)
        .await
        .expect("ada");
    assert_eq!(read.id, ada.id);
    assert_eq!(read.name, "Ada Lovelace", "composed from the split parts");
    assert_eq!(read.values["platform.contacts:first_name"], json!("Ada"));
    assert_eq!(
        read.values["platform.contacts:last_name"],
        json!("Lovelace")
    );
    assert_eq!(
        read.values["platform.contacts:organisation"],
        json!(acme.public_id)
    );
    assert_eq!(
        read.values["platform.contacts:tags"],
        json!(["VIP", "Speaker"])
    );
    assert_eq!(
        read.values["platform.contacts:email"],
        json!("ada@example.org")
    );
    assert_eq!(read.owner_id, Some(member));
    assert_eq!(read.version, ada.version);
    assert_eq!(micros(read.created_at), micros(ada.created_at));
    assert_eq!(micros(read.updated_at), micros(ada.updated_at));
    let read = admin
        .get_record(PERSON, &peter.public_id, true)
        .await
        .expect("peter");
    assert_eq!(
        read.values["platform.contacts:organisation"],
        json!(initech.public_id)
    );
    admin
        .get_record(ORGANISATION, &initech.public_id, true)
        .await
        .expect("initech");
    let read = admin
        .get_record(PERSON, &plato.public_id, true)
        .await
        .expect("plato");
    assert_eq!(read.name, "Plato");
    assert_eq!(read.owner_id, None, "his creator left");
    let read = admin
        .get_record(PERSON, &ludwig.public_id, true)
        .await
        .expect("ludwig");
    assert_eq!(
        read.values["platform.contacts:first_name"],
        json!("Ludwig van")
    );
    assert_eq!(read.archived_at.map(micros), ludwig.archived_at.map(micros));
    let read = admin
        .get_record(PERSON, &binned.public_id, true)
        .await
        .expect("binned");
    assert_eq!(read.deleted_at.map(micros), binned.deleted_at.map(micros));
    // The engine numbers new records past every id Contacts issued.
    let next = admin
        .create_record(
            PERSON,
            &json!({"platform.contacts:last_name": "Newcomer"})
                .as_object()
                .cloned()
                .expect("body"),
            MutationContext::new(Uuid::now_v7()),
        )
        .await
        .expect("a new person");
    assert_eq!(
        platform_id::PublicId::parse(&next.public_id)
            .expect("id")
            .sequence(),
        Some(6),
        "five people were numbered in Contacts"
    );

    // --- An empty workspace without the engine objects is not ready ------
    let empty = Migration::new(
        store.clone(),
        objects.clone(),
        Uuid::now_v7(),
        member,
        members.clone(),
    )
    .dry_run()
    .await
    .expect("dry run");
    assert_eq!(
        empty.blockers.iter().map(|b| b.code).collect::<Vec<_>>(),
        vec![BLOCKED_OBJECTS_MISSING]
    );

    // --- Custom fields wait for the next part ----------------------------
    install(&objects, other_tenant, member).await;
    let other = contacts(&store, other_tenant, member);
    other
        .create_definition(
            CreateCustomFieldDefinition {
                applies_to: ContactCustomFieldTarget::Person,
                api_name: "badge__c".into(),
                label: "Badge".into(),
                description: None,
                field_type: ContactCustomFieldType::Text,
                required: false,
                unique_per_tenant: false,
                indexed: false,
                options: Vec::new(),
                validation_schema: json!({}),
                data_classification: DataClassification::Internal,
                purpose: Some("Building access".into()),
                retention_days: None,
                encrypted: false,
            },
            context(),
        )
        .await
        .expect("a definition");
    let blocked = Migration::new(store, objects, other_tenant, member, members)
        .dry_run()
        .await
        .expect("dry run");
    assert_eq!(
        blocked.blockers.iter().map(|b| b.code).collect::<Vec<_>>(),
        vec![BLOCKED_CUSTOM_FIELDS]
    );
}
