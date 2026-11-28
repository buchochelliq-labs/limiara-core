//! Importing records with their identity on every backend (ADR 0035 G4).
//! Only the kernel's migrator may import. Each record keeps its internal
//! and public id, stamps, version, owner and archive and recycle-bin state;
//! values are checked but validation rules and automations do not run; the
//! change log carries every record, and history keeps backdated `system`
//! entries. A batch is all or nothing, and the object's record sequence
//! moves past every imported id. Suspended objects take imports.

use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use chrono::{DateTime, Duration, Utc};
use platform_id::PublicId;
use platform_objects::import::{ImportedHistory, ImportedRecord, IMPORT_EXISTS};
use platform_objects::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::Arc;
use uuid::Uuid;

const ORG: &str = "custom:org";
const PERSON: &str = "custom:person";

/// What the store test checks against its own tables.
pub struct Report {
    /// Public ids of every imported record.
    pub imported: Vec<String>,
}

fn values(value: Value) -> Map<String, Value> {
    value.as_object().expect("an object").clone()
}

fn record(prefix: &str, sequence: u64, values_: Value, at: DateTime<Utc>) -> ImportedRecord {
    ImportedRecord {
        id: Uuid::now_v7(),
        public_id: PublicId::new(prefix, sequence)
            .expect("id")
            .as_str()
            .to_owned(),
        values: values(values_),
        owner_id: None,
        version: 1,
        created_at: at,
        created_by: None,
        updated_at: at,
        updated_by: None,
        archived_at: None,
        deleted_at: None,
        history: Vec::new(),
    }
}

fn code(error: &ObjectError) -> Vec<(String, &'static str)> {
    match error {
        ObjectError::Validation(issues) => issues
            .iter()
            .map(|issue| (issue.field.clone(), issue.code))
            .collect(),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> Report {
    let member = Uuid::now_v7();
    let owner = Uuid::now_v7();
    let admin = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    let migrator = service(&store, ObjectCaller::migrator(tenant, member));
    for (name, label) in [("org", "Org"), ("person", "Person")] {
        admin
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    let (org, _) = admin.describe(ORG).await.expect("org");
    let (person, _) = admin.describe(PERSON).await.expect("person");
    let mut employer = field("employer", FieldType::Lookup);
    employer.reference_object = Some(ORG.into());
    let mut email = field("email", FieldType::Email);
    email.unique = true;
    // A second unique field, after email: a record can take an email and
    // then collide on its badge.
    let mut badge = field("badge", FieldType::Text);
    badge.unique = true;
    for definition in [employer, email, badge] {
        admin
            .create_field(PERSON, definition, context())
            .await
            .expect("field");
    }
    // A rule an import must not run, and an automation it must not fire.
    admin
        .create_validation_rule(
            PERSON,
            serde_json::from_value(json!({
                "name": "needs_email", "condition": "ISBLANK(custom:email)",
                "message": "Give an email"
            }))
            .expect("rule"),
            context(),
        )
        .await
        .expect("rule");
    // Stores keep times to the microsecond.
    let earlier =
        DateTime::from_timestamp_micros((Utc::now() - Duration::days(400)).timestamp_micros())
            .expect("a time");

    // --- Only the migrator imports ------------------------------------------
    let one = || vec![record(&org.key_prefix, 1, json!({"name": "Acme"}), earlier)];
    for refused in [
        admin.clone(),
        service(
            &store,
            ObjectCaller::plugin_installer(tenant, member, "example.x"),
        ),
        service(&store, ObjectCaller::system(tenant)),
    ] {
        assert!(matches!(
            refused.import_records(ORG, one(), 0, context()).await,
            Err(ObjectError::PermissionDenied)
        ));
    }

    // --- Organisations: active, archived and deleted, with their stamps ---
    let mut acme = record(&org.key_prefix, 41, json!({"name": "Acme"}), earlier);
    acme.owner_id = Some(owner);
    acme.version = 7;
    acme.created_by = Some(owner);
    acme.updated_at = earlier + Duration::days(30);
    acme.updated_by = Some(owner);
    acme.history = vec![ImportedHistory {
        kind: HistoryKind::Created,
        field: None,
        old_value: None,
        new_value: None,
        changed_at: earlier,
        changed_by: Some(owner),
    }];
    let mut shelved = record(&org.key_prefix, 42, json!({"name": "Shelved"}), earlier);
    shelved.archived_at = Some(earlier + Duration::days(1));
    let mut binned = record(&org.key_prefix, 43, json!({"name": "Binned"}), earlier);
    binned.deleted_at = Some(earlier + Duration::days(2));
    let report = migrator
        .import_records(
            ORG,
            vec![acme.clone(), shelved.clone(), binned.clone()],
            100,
            context(),
        )
        .await
        .expect("import organisations");
    assert_eq!(report.imported, 3);
    assert_eq!(report.record_sequence, 100, "the source counter wins");
    let read = admin
        .get_record(ORG, &acme.public_id, false)
        .await
        .expect("acme");
    // The stamps are the source's, the touch the import's (ADR 0024 §3).
    assert!(read.touched_at > acme.updated_at, "touched when imported");
    assert_eq!(read.id, acme.id);
    assert_eq!(read.version, 7);
    assert_eq!(read.owner_id, Some(owner));
    assert_eq!(read.created_at, acme.created_at);
    assert_eq!(read.updated_at, acme.updated_at);
    assert_eq!(read.updated_by, Some(owner));
    let archived = admin
        .get_record(ORG, &shelved.public_id, false)
        .await
        .expect("archived");
    assert_eq!(archived.archived_at, shelved.archived_at);
    assert!(admin
        .get_record(ORG, &binned.public_id, false)
        .await
        .is_err());
    let in_bin = admin
        .get_record(ORG, &binned.public_id, true)
        .await
        .expect("in the recycle bin");
    assert_eq!(in_bin.deleted_at, binned.deleted_at);
    // Backdated history, marked as the platform's.
    let (history, _) = admin
        .record_history(ORG, &acme.public_id, None, None)
        .await
        .expect("history");
    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].changed_at, earlier);
    assert_eq!(history.entries[0].changed_by, Some(owner));
    assert_eq!(
        history.entries[0].source.as_ref().map(|source| source.kind),
        Some(SourceKind::System)
    );
    // The change log carries them; the binned one as deleted.
    let changes = admin
        .list_changes(None, Some(200), Some(ORG))
        .await
        .expect("changes");
    let kinds: Vec<(String, ChangeKind)> = changes
        .changes
        .iter()
        .map(|(_, change)| (change.public_id.clone(), change.kind))
        .collect();
    assert!(kinds.contains(&(acme.public_id.clone(), ChangeKind::Created)));
    assert!(kinds.contains(&(binned.public_id.clone(), ChangeKind::Deleted)));
    assert!(
        changes
            .changes
            .iter()
            .all(|(_, change)| change.changed_at > earlier + Duration::days(300)),
        "logged at the import, not at the source's old times"
    );
    // The next record the engine numbers follows the imported ones.
    let next = admin
        .create_record(ORG, &body(json!({"name": "New"})), context())
        .await
        .expect("a new organisation");
    let sequence = PublicId::parse(&next.public_id)
        .expect("id")
        .sequence()
        .expect("server range");
    assert_eq!(sequence, 101);

    // --- People: lookups to imported records, no rules run ------------------
    let ada = record(
        &person.key_prefix,
        900,
        json!({"name": "Ada", "custom:employer": acme.public_id}),
        earlier,
    );
    // A lookup to an archived or deleted target is kept as the source had it.
    let bea = record(
        &person.key_prefix,
        901,
        json!({"name": "Bea", "custom:employer": binned.public_id, "custom:email": "bea@example.com"}),
        earlier,
    );
    let report = migrator
        .import_records(PERSON, vec![ada.clone(), bea.clone()], 0, context())
        .await
        .expect("people, though the rule would refuse Ada");
    assert_eq!(report.record_sequence, 901, "past every imported id");
    // A later batch of lower ids reports the counter as it stands.
    let cy = record(&person.key_prefix, 50, json!({"name": "Cy"}), earlier);
    let later = migrator
        .import_records(PERSON, vec![cy.clone()], 0, context())
        .await
        .expect("a lower id");
    assert_eq!(
        later.record_sequence, 901,
        "never lowered, and reported as stored"
    );
    let read = admin
        .get_record(PERSON, &ada.public_id, false)
        .await
        .expect("ada");
    assert_eq!(read.values["custom:employer"], json!(acme.public_id));
    // Imported references feed related lists like any other.
    let employees = admin
        .query(QueryRequest {
            q: format!(
                "SELECT name FROM custom:person WHERE custom:employer = '{}'",
                acme.public_id
            ),
            cursor: None,
            page_size: None,
            include_deleted: false,
            include_archived: false,
        })
        .await
        .expect("query");
    assert_eq!(employees.rows.len(), 1);

    // --- Refusals, and a batch is all or nothing ---------------------------
    let fresh = |sequence| {
        record(
            &person.key_prefix,
            sequence,
            json!({"name": "Someone"}),
            earlier,
        )
    };
    let mut wrong_prefix = fresh(910);
    wrong_prefix.public_id = PublicId::new(&org.key_prefix, 910)
        .expect("id")
        .as_str()
        .to_owned();
    let mut device = fresh(911);
    device.public_id = PublicId::new_client(&person.key_prefix, 5)
        .expect("id")
        .as_str()
        .to_owned();
    let mut dangling = fresh(912);
    dangling.values.insert(
        "custom:employer".into(),
        json!(PublicId::new(&org.key_prefix, 999).expect("id").as_str()),
    );
    let mut unversioned = fresh(913);
    unversioned.version = 0;
    let mut backwards = fresh(914);
    backwards.updated_at = earlier - Duration::days(1);
    let mut bad_email = fresh(915);
    bad_email
        .values
        .insert("custom:email".into(), json!("not an email"));
    for (bad, expected) in [
        (wrong_prefix, "records.import_id_invalid"),
        (device, "records.import_id_invalid"),
        (dangling, "records.import_reference_missing"),
        (unversioned, "records.import_version_invalid"),
        (backwards, "records.import_stamps_invalid"),
        (bad_email, "records.email_invalid"),
    ] {
        let good = fresh(920);
        let error = migrator
            .import_records(PERSON, vec![good.clone(), bad], 0, context())
            .await
            .expect_err("refused");
        let found = code(&error);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(
            found[0].0.split('/').nth(1),
            Some("1"),
            "the second record: {found:?}"
        );
        assert_eq!(found[0].1, expected, "{found:?}");
        assert!(
            admin
                .get_record(PERSON, &good.public_id, true)
                .await
                .is_err(),
            "nothing of a refused batch is written"
        );
    }
    let mut again = fresh(930);
    again.id = ada.id;
    assert!(matches!(
        migrator.import_records(PERSON, vec![again], 0, context()).await,
        Err(ObjectError::Conflict(code)) if code == IMPORT_EXISTS
    ));
    let mut twin = fresh(931);
    twin.public_id = ada.public_id.clone();
    assert!(matches!(
        migrator.import_records(PERSON, vec![twin], 0, context()).await,
        Err(ObjectError::Conflict(code)) if code == IMPORT_EXISTS
    ));
    // Uniqueness holds.
    let mut clash = fresh(932);
    clash
        .values
        .insert("custom:email".into(), json!("BEA@example.com"));
    assert!(matches!(
        migrator
            .import_records(PERSON, vec![clash], 0, context())
            .await,
        Err(ObjectError::Conflict(_))
    ));
    assert!(matches!(
        migrator
            .import_records("custom:nothing", Vec::new(), 0, context())
            .await,
        Err(ObjectError::NotFound)
    ));

    // --- A suspended plugin object takes imports ---------------------------
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, "example.tracker"),
    );
    installer
        .install_plugin_objects(&super::plugin_objects::fixture(), context())
        .await
        .expect("install, suspended");
    let ticket_prefix = super::plugin_objects::fixture()
        .objects
        .iter()
        .find(|object| object.name == "ticket")
        .expect("ticket")
        .key_prefix
        .clone();
    let ticket = record(
        &ticket_prefix,
        5,
        json!({"name": "Imported ticket"}),
        earlier,
    );
    migrator
        .import_records("example.tracker:ticket", vec![ticket.clone()], 0, context())
        .await
        .expect("into a suspended object");
    installer
        .resume_plugin_objects("example.tracker", context())
        .await
        .expect("resume");
    assert_eq!(
        admin
            .get_record("example.tracker:ticket", &ticket.public_id, false)
            .await
            .expect("there once in service")
            .name,
        "Imported ticket"
    );

    // --- What an interrupted batch wrote ----------------------------------
    let unknown = Uuid::now_v7();
    assert_eq!(
        migrator
            .imported_ids(&[acme.id, binned.id, unknown])
            .await
            .expect("imported ids"),
        BTreeSet::from([acme.id, binned.id]),
        "any state counts; an id never imported does not"
    );
    assert!(matches!(
        admin.imported_ids(&[acme.id]).await,
        Err(ObjectError::PermissionDenied)
    ));

    // --- A check imports nothing, and reports every refusal ---------------
    assert!(matches!(
        admin.check_import().await.err(),
        Some(ObjectError::PermissionDenied)
    ));
    let globex = record(&org.key_prefix, 700, json!({"name": "Globex"}), earlier);
    let dee = record(
        &person.key_prefix,
        700,
        // Points at an organisation the same check brought in.
        json!({"name": "Dee", "custom:employer": globex.public_id}),
        earlier,
    );
    let mut wrong = record(&person.key_prefix, 701, json!({"name": "Wrong"}), earlier);
    wrong
        .values
        .insert("custom:email".into(), json!("not an email"));
    let again = record(
        &person.key_prefix,
        702,
        json!({"name": "Ada again"}),
        earlier,
    );
    let again = ImportedRecord {
        id: ada.id,
        ..again
    };
    let fine = record(&person.key_prefix, 703, json!({"name": "Fine"}), earlier);
    let holder = record(
        &person.key_prefix,
        704,
        json!({"name": "Holder", "custom:email": "holder@example.com", "custom:badge": "B-1"}),
        earlier,
    );
    // Takes a fresh email, then collides on the badge.
    let clash = record(
        &person.key_prefix,
        705,
        json!({"name": "Clash", "custom:email": "shared@example.com", "custom:badge": "B-1"}),
        earlier,
    );
    // The email the refused record no longer holds.
    let later = record(
        &person.key_prefix,
        706,
        json!({"name": "Later", "custom:email": "shared@example.com"}),
        earlier,
    );
    {
        let mut check = migrator.check_import().await.expect("check");
        assert!(check
            .check(ORG, vec![globex.clone()])
            .await
            .expect("organisations")
            .is_empty());
        let refused = check
            .check(
                PERSON,
                vec![
                    dee.clone(),
                    wrong.clone(),
                    again.clone(),
                    fine.clone(),
                    holder.clone(),
                    clash.clone(),
                    later.clone(),
                ],
            )
            .await
            .expect("people");
        let refused: Vec<_> = refused
            .iter()
            .map(|refusal| {
                (
                    refusal.public_id.clone(),
                    refusal
                        .issues
                        .iter()
                        .map(|issue| issue.code)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            refused,
            vec![
                (wrong.public_id.clone(), vec!["records.email_invalid"]),
                (again.public_id.clone(), vec![IMPORT_EXISTS]),
                (clash.public_id.clone(), vec!["records.duplicate_value"]),
            ],
            "the others pass, the check goes on after a refusal, and a refused \
             record's partial writes are undone"
        );
    }
    for (object, public_id) in [(ORG, &globex.public_id), (PERSON, &fine.public_id)] {
        assert!(
            matches!(
                admin.get_record(object, public_id, true).await,
                Err(ObjectError::NotFound)
            ),
            "a check writes nothing"
        );
    }

    Report {
        imported: vec![
            acme.public_id,
            shelved.public_id,
            binned.public_id,
            ada.public_id,
            bea.public_id,
            cy.public_id,
            ticket.public_id,
        ],
    }
}
