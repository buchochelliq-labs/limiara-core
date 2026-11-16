//! One behavioural suite every object store must pass (ADR 0013, ADR 0015).
//! It drives the real `ObjectService`, so it checks what callers rely on.

#![allow(dead_code)]

pub mod automations;
pub mod bulk;
pub mod client_ids;
pub mod composite;
pub mod conflicts;
pub mod definitions;
pub mod formulas;
pub mod jobs;
pub mod lease;
pub mod plugin_objects;
pub mod push;
pub mod query;
pub mod quick_action_replicas;
pub mod quick_actions;
pub mod related;
pub mod related_actions;
pub mod related_to;
pub mod relationships;
pub mod replica;
pub mod reports;
pub mod rollups;
pub mod rules;
pub mod search;
pub mod sources;
pub mod sync;
pub mod timeline;

use platform_objects::service::ListRecords;
use platform_objects::*;
use serde_json::{json, Map, Value};
use std::sync::Arc;
use uuid::Uuid;

pub struct Report {
    pub tenant_a: Uuid,
    pub tenant_b: Uuid,
    /// Successful mutations in workspace A, each of which left exactly one
    /// audit row and one outbox row.
    pub mutations_in_a: i64,
}

const ADMIN_PERMISSIONS: [&str; 2] = [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION];

fn caller(tenant: Uuid, user: Uuid, permissions: &[&str], roles: &[Uuid]) -> ObjectCaller {
    ObjectCaller::new(
        tenant,
        user,
        permissions
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        roles.to_vec(),
    )
}

fn service(store: &Arc<dyn ObjectStore>, caller: ObjectCaller) -> ObjectService {
    ObjectService::new(store.clone(), caller)
}

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

fn body(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object body")
}

fn object(name: &str, label: &str) -> CreateObject {
    CreateObject {
        name: name.into(),
        label: label.into(),
        plural_label: format!("{label}s"),
        description: None,
        name_label: None,
        sharing_model: None,
        grant_hierarchy: None,
        track_history: None,
        searchable: None,
        in_navigation: None,
    }
}

fn field(name: &str, kind: FieldType) -> CreateField {
    CreateField {
        name: name.into(),
        label: name.replace('_', " "),
        field_type: kind,
        description: None,
        required: false,
        unique: false,
        indexed: false,
        searchable: false,
        track_history: false,
        max_length: None,
        scale: None,
        options: Vec::new(),
        default_value: None,
        reference_object: None,
        reference_objects: None,
        delete_behaviour: None,
        formula: None,
        formula_type: None,
        rollup: None,
        lookup_filter: None,
    }
}

fn option(value: &str, default: bool) -> PicklistOption {
    PicklistOption {
        value: value.into(),
        label: value.into(),
        active: true,
        default,
    }
}

fn codes(error: &ObjectError) -> Vec<&'static str> {
    match error {
        ObjectError::Validation(issues) => issues.iter().map(|issue| issue.code).collect(),
        _ => Vec::new(),
    }
}

/// Runs the suite in two existing, empty workspaces.
pub async fn run(store: Arc<dyn ObjectStore>, tenant_a: Uuid, tenant_b: Uuid) -> Report {
    let admin_id = Uuid::now_v7();
    let reader_role = Uuid::now_v7();
    let admin = service(&store, caller(tenant_a, admin_id, &ADMIN_PERMISSIONS, &[]));
    let mut mutations = 0;

    // --- Objects -----------------------------------------------------------
    let account = admin
        .create_object(object("account", "Account"), context())
        .await
        .expect("account object");
    mutations += 1;
    let project = admin
        .create_object(object("project", "Project"), context())
        .await
        .expect("project object");
    mutations += 1;
    assert_eq!(project.api_name, "custom:project");
    assert_eq!(project.origin, Origin::Workspace);
    assert!(
        naming::is_workspace_prefix(&project.key_prefix),
        "workspace objects take digit-first prefixes"
    );
    assert_ne!(project.key_prefix, account.key_prefix);
    assert!(matches!(
        admin
            .create_object(object("project", "Again"), context())
            .await,
        Err(ObjectError::Conflict("objects.duplicate_api_name"))
    ));
    let invalid = admin
        .create_object(object("Bad Name", ""), context())
        .await
        .expect_err("invalid object");
    assert!(codes(&invalid).contains(&"objects.name_invalid"));
    assert!(codes(&invalid).contains(&"objects.label_invalid"));

    // --- Fields --------------------------------------------------------------
    let mut status = field("status", FieldType::Picklist);
    status.required = true;
    status.options = vec![
        option("Open", true),
        option("Won", false),
        option("Lost", false),
    ];
    admin
        .create_field("custom:project", status, context())
        .await
        .expect("status");
    let mut code = field("code", FieldType::Text);
    code.unique = true;
    code.max_length = Some(20);
    admin
        .create_field("custom:project", code, context())
        .await
        .expect("code");
    admin
        .create_field(
            "custom:project",
            field("budget", FieldType::Currency),
            context(),
        )
        .await
        .expect("budget");
    admin
        .create_field("custom:project", field("due", FieldType::Date), context())
        .await
        .expect("due");
    admin
        .create_field("custom:project", field("site", FieldType::Url), context())
        .await
        .expect("site");
    let mut account_link = field("account", FieldType::Lookup);
    account_link.reference_object = Some("custom:account".into());
    account_link.delete_behaviour = Some(DeleteBehaviour::Clear);
    admin
        .create_field("custom:project", account_link, context())
        .await
        .expect("account lookup");
    mutations += 6;
    let bad_lookup = {
        let mut bad = field("nowhere", FieldType::Lookup);
        bad.reference_object = Some("custom:missing".into());
        admin
            .create_field("custom:project", bad, context())
            .await
            .expect_err("bad lookup")
    };
    assert!(codes(&bad_lookup).contains(&"objects.reference_object_unknown"));
    let bad_unique = {
        let mut bad = field("flag", FieldType::Boolean);
        bad.unique = true;
        admin
            .create_field("custom:project", bad, context())
            .await
            .expect_err("bad unique")
    };
    assert!(codes(&bad_unique).contains(&"objects.unique_not_allowed"));
    let (described, _) = admin.describe("custom:project").await.expect("describe");
    let names: Vec<_> = described
        .fields
        .iter()
        .map(|field| field.api_name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "custom:status",
            "custom:code",
            "custom:budget",
            "custom:due",
            "custom:site",
            "custom:account"
        ]
    );
    assert_eq!(
        described
            .field("custom:account")
            .and_then(|field| field.reference_object.as_deref()),
        Some("custom:account")
    );

    // --- Records -------------------------------------------------------------
    let acme = admin
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("acme");
    mutations += 1;
    assert!(acme.public_id.starts_with(&account.key_prefix));
    assert_eq!(acme.created_by, Some(admin_id));
    assert_eq!(acme.owner_id, Some(admin_id));

    let invalid = admin
        .create_record(
            "custom:project",
            &body(json!({"custom:site": "javascript:alert(1)", "custom:nope": 1, "custom:budget": 1.234})),
            context(),
        )
        .await
        .expect_err("invalid record");
    let found = codes(&invalid);
    for code in [
        "records.name_required",
        "records.url_invalid",
        "records.field_unknown",
        "records.number_scale",
    ] {
        assert!(found.contains(&code), "{code} in {found:?}");
    }
    let apollo = admin
        .create_record(
            "custom:project",
            &body(json!({
                "name": "Apollo",
                "custom:code": "AP-1",
                "custom:budget": "1200.50",
                "custom:due": "2026-12-01",
                "custom:account": acme.public_id,
            })),
            context(),
        )
        .await
        .expect("apollo");
    mutations += 1;
    assert_eq!(
        apollo.values["custom:status"],
        json!("Open"),
        "picklist default"
    );
    assert_eq!(apollo.values["custom:budget"], json!(1200.5));
    assert!(apollo.public_id.starts_with(&project.key_prefix));
    assert!(
        matches!(
            admin
                .create_record(
                    "custom:project",
                    &body(json!({"name": "Clash", "custom:code": "ap-1"})),
                    context()
                )
                .await,
            Err(ObjectError::Conflict("records.duplicate_value"))
        ),
        "unique values ignore case"
    );
    let wrong_target = admin
        .create_record(
            "custom:project",
            &body(json!({"name": "Wrong", "custom:account": apollo.public_id})),
            context(),
        )
        .await
        .expect_err("lookup to the wrong object");
    assert!(codes(&wrong_target).contains(&"records.reference_not_found"));
    for name in ["Zephyr", "Borealis", "Cassini"] {
        admin
            .create_record(
                "custom:project",
                &body(json!({"name": name, "custom:status": "Won"})),
                context(),
            )
            .await
            .expect("project");
        mutations += 1;
    }

    // Partial updates with versions.
    let updated = admin
        .update_record(
            "custom:project",
            &apollo.public_id,
            apollo.version,
            &body(json!({"custom:due": null, "custom:status": "Lost"})),
            context(),
        )
        .await
        .expect("update");
    mutations += 1;
    assert_eq!(updated.version, apollo.version + 1);
    assert!(!updated.values.contains_key("custom:due"));
    assert_eq!(
        updated.values["custom:code"],
        json!("AP-1"),
        "untouched values stay"
    );
    assert!(matches!(
        admin
            .update_record(
                "custom:project",
                &apollo.public_id,
                apollo.version,
                &body(json!({"name": "Late"})),
                context()
            )
            .await,
        Err(ObjectError::VersionConflict)
    ));
    let cleared = admin
        .update_record(
            "custom:project",
            &apollo.public_id,
            updated.version,
            &body(json!({"custom:status": null})),
            context(),
        )
        .await
        .expect_err("required field cleared");
    assert!(codes(&cleared).contains(&"records.value_required"));

    // Lists: sort, keyset paging, filters, search.
    let first = admin
        .list_records(
            "custom:project",
            ListRecords {
                limit: Some(2),
                ..ListRecords::default()
            },
        )
        .await
        .expect("page one");
    let names: Vec<_> = first
        .records
        .iter()
        .map(|record| record.name.as_str())
        .collect();
    assert_eq!(names, ["Apollo", "Borealis"]);
    let second = admin
        .list_records(
            "custom:project",
            ListRecords {
                limit: Some(2),
                cursor: first.next_cursor.clone(),
                ..ListRecords::default()
            },
        )
        .await
        .expect("page two");
    let names: Vec<_> = second
        .records
        .iter()
        .map(|record| record.name.as_str())
        .collect();
    assert_eq!(names, ["Cassini", "Zephyr"]);
    assert!(second.next_cursor.is_none());
    let won = admin
        .list_records(
            "custom:project",
            ListRecords {
                filters: vec![("custom:status".into(), "Won".into())],
                ..ListRecords::default()
            },
        )
        .await
        .expect("filter");
    assert_eq!(won.records.len(), 3);
    let not_indexed = admin
        .list_records(
            "custom:project",
            ListRecords {
                filters: vec![("custom:site".into(), "x".into())],
                ..ListRecords::default()
            },
        )
        .await
        .expect_err("non-indexed filter");
    assert!(codes(&not_indexed).contains(&"records.filter_not_indexed"));
    let searched = admin
        .list_records(
            "custom:project",
            ListRecords {
                search: Some("ASSI".into()),
                ..ListRecords::default()
            },
        )
        .await
        .expect("search");
    assert_eq!(searched.records.len(), 1);
    let newest = admin
        .list_records(
            "custom:project",
            ListRecords {
                sort: RecordSort::CreatedAt,
                direction: SortDirection::Desc,
                limit: Some(1),
                ..ListRecords::default()
            },
        )
        .await
        .expect("newest");
    assert_eq!(newest.records[0].name, "Cassini");
    let wrong_cursor = admin
        .list_records(
            "custom:project",
            ListRecords {
                sort: RecordSort::UpdatedAt,
                cursor: first.next_cursor,
                ..ListRecords::default()
            },
        )
        .await
        .expect_err("cursor from another sort");
    assert!(codes(&wrong_cursor).contains(&"records.cursor_invalid"));

    // Indexing a field later makes it filterable for existing records.
    let (described, _) = admin.describe("custom:project").await.expect("describe");
    let site = described.field("custom:site").expect("site").clone();
    admin
        .update_record(
            "custom:project",
            &won.records[0].public_id,
            won.records[0].version,
            &body(json!({"custom:site": "https://example.org"})),
            context(),
        )
        .await
        .expect("set site");
    mutations += 1;
    admin
        .update_field(
            "custom:project",
            "custom:site",
            UpdateField {
                expected_version: site.version,
                label: None,
                description: None,
                required: None,
                indexed: Some(true),
                searchable: None,
                track_history: None,
                max_length: None,
                options: None,
                default_value: None,
                position: None,
                formula: None,
                lookup_filter: None,
                reference_objects: None,
            },
            context(),
        )
        .await
        .expect("index site");
    mutations += 1;
    let by_site = admin
        .list_records(
            "custom:project",
            ListRecords {
                filters: vec![("custom:site".into(), "https://EXAMPLE.org".into())],
                ..ListRecords::default()
            },
        )
        .await
        .expect("filter on newly indexed field");
    assert_eq!(by_site.records.len(), 1);

    // --- Delete, lookups, recycle bin -------------------------------------
    assert!(
        matches!(
            admin
                .delete_object("custom:account", account.version, context())
                .await,
            Err(ObjectError::Conflict("objects.object_referenced"))
        ),
        "lookups from other objects block deleting the object"
    );
    let acme_now = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("acme");
    admin
        .delete_record(
            "custom:account",
            &acme.public_id,
            acme_now.version,
            context(),
        )
        .await
        .expect("delete acme");
    mutations += 2; // the cleared lookup on Apollo and the delete
    let apollo_now = admin
        .get_record("custom:project", &apollo.public_id, false)
        .await
        .expect("apollo");
    assert!(
        !apollo_now.values.contains_key("custom:account"),
        "clear behaviour"
    );
    assert!(matches!(
        admin
            .get_record("custom:account", &acme.public_id, false)
            .await,
        Err(ObjectError::NotFound)
    ));
    let binned = admin
        .get_record("custom:account", &acme.public_id, true)
        .await
        .expect("include_deleted");
    assert!(binned.deleted_at.is_some());
    let bin = admin
        .list_records(
            "custom:account",
            ListRecords {
                status: RecordStatus::Deleted,
                ..ListRecords::default()
            },
        )
        .await
        .expect("bin");
    assert_eq!(bin.records.len(), 1);
    let restored = admin
        .undelete_record("custom:account", &acme.public_id, binned.version, context())
        .await
        .expect("undelete");
    mutations += 1;
    assert!(restored.deleted_at.is_none());

    // The recycle bin releases unique values; undelete reclaims them or
    // fails when another record took one meanwhile.
    let vega = admin
        .create_record(
            "custom:project",
            &body(json!({"name": "Vega", "custom:code": "VE-1"})),
            context(),
        )
        .await
        .expect("vega");
    let vega = admin
        .delete_record("custom:project", &vega.public_id, vega.version, context())
        .await
        .expect("delete vega");
    admin
        .create_record(
            "custom:project",
            &body(json!({"name": "Vega two", "custom:code": "ve-1"})),
            context(),
        )
        .await
        .expect("binned unique values are free");
    mutations += 3;
    assert!(matches!(
        admin
            .undelete_record("custom:project", &vega.public_id, vega.version, context())
            .await,
        Err(ObjectError::Conflict("records.duplicate_value"))
    ));

    // Restrict blocks deletion while a record points at the target.
    let mut sponsor = field("sponsor", FieldType::Lookup);
    sponsor.reference_object = Some("custom:account".into());
    sponsor.delete_behaviour = Some(DeleteBehaviour::Restrict);
    admin
        .create_field("custom:project", sponsor, context())
        .await
        .expect("sponsor");
    mutations += 1;
    let apollo_now = admin
        .get_record("custom:project", &apollo.public_id, false)
        .await
        .expect("apollo");
    admin
        .update_record(
            "custom:project",
            &apollo.public_id,
            apollo_now.version,
            &body(json!({"custom:sponsor": acme.public_id})),
            context(),
        )
        .await
        .expect("sponsor set");
    mutations += 1;
    assert!(matches!(
        admin
            .delete_record(
                "custom:account",
                &acme.public_id,
                restored.version,
                context()
            )
            .await,
        Err(ObjectError::Conflict("records.delete_restricted"))
    ));

    // --- Access ----------------------------------------------------------
    admin
        .set_object_permission(
            "custom:project",
            reader_role,
            AccessFlags {
                read: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("grant read");
    mutations += 1;
    let permissions = admin
        .object_permissions("custom:project")
        .await
        .expect("permissions");
    assert_eq!(permissions.len(), 1);
    let reader = service(
        &store,
        caller(tenant_a, Uuid::now_v7(), &[], &[reader_role]),
    );
    let listed: Vec<_> = reader
        .list_objects()
        .await
        .expect("reader objects")
        .into_iter()
        .map(|(object, _)| object.api_name)
        .collect();
    assert_eq!(
        listed,
        ["custom:project"],
        "only objects the reader may read"
    );
    reader
        .get_record("custom:project", &apollo.public_id, false)
        .await
        .expect("reader reads");
    assert!(matches!(
        reader
            .create_record("custom:project", &body(json!({"name": "Nope"})), context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(
        matches!(
            reader
                .get_record("custom:account", &acme.public_id, false)
                .await,
            Err(ObjectError::NotFound)
        ),
        "an unreadable object looks missing"
    );
    assert!(matches!(
        reader
            .create_object(object("sneaky", "Sneaky"), context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    let stranger = service(&store, caller(tenant_a, Uuid::now_v7(), &[], &[]));
    assert!(stranger.list_objects().await.expect("stranger").is_empty());

    // An editor who cannot read a lookup's target may change other fields
    // but cannot point the lookup anywhere it cannot see.
    admin
        .set_object_permission(
            "custom:project",
            reader_role,
            AccessFlags {
                edit: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("grant edit");
    mutations += 1;
    let apollo_now = reader
        .get_record("custom:project", &apollo.public_id, false)
        .await
        .expect("apollo");
    let renamed = reader
        .update_record(
            "custom:project",
            &apollo.public_id,
            apollo_now.version,
            &body(json!({"name": "Apollo Prime"})),
            context(),
        )
        .await
        .expect("untouched lookups are not re-checked");
    mutations += 1;
    assert_eq!(renamed.values["custom:sponsor"], json!(acme.public_id));
    let cleared = reader
        .update_record(
            "custom:project",
            &apollo.public_id,
            renamed.version,
            &body(json!({"custom:sponsor": null})),
            context(),
        )
        .await
        .expect("clearing a lookup");
    mutations += 1;
    assert!(matches!(
        reader
            .update_record(
                "custom:project",
                &apollo.public_id,
                cleared.version,
                &body(json!({"custom:sponsor": acme.public_id})),
                context(),
            )
            .await,
        Err(ObjectError::Validation(issues)) if issues[0].code == "records.reference_not_found"
    ));

    // --- Field deletion hides values -------------------------------------
    let (described, _) = admin.describe("custom:project").await.expect("describe");
    let budget = described.field("custom:budget").expect("budget").clone();
    admin
        .delete_field("custom:project", "custom:budget", budget.version, context())
        .await
        .expect("delete budget");
    mutations += 1;
    let apollo_now = admin
        .get_record("custom:project", &apollo.public_id, false)
        .await
        .expect("apollo");
    assert!(!apollo_now.values.contains_key("custom:budget"));

    // --- Sharing (ADR 0016) ----------------------------------------------
    // A private object: Bob reports to Alice; Carol and Dave are peers.
    let sales = Uuid::now_v7();
    let (alice, bob, carol, dave) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let mut deal = object("deal", "Deal");
    deal.sharing_model = Some(SharingModel::Private);
    admin.create_object(deal, context()).await.expect("deal");
    mutations += 1;
    admin
        .set_object_permission(
            "custom:deal",
            sales,
            AccessFlags {
                create: true,
                delete: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("sales access");
    mutations += 1;
    let member = |user: Uuid, reports: Vec<Uuid>, permissions: &[&str]| {
        service(
            &store,
            caller(tenant_a, user, permissions, &[sales]).with_reports(reports),
        )
    };
    let alice_s = member(alice, vec![bob], &[]);
    let bob_s = member(bob, Vec::new(), &[]);
    let carol_s = member(carol, Vec::new(), &[]);
    let dave_s = member(dave, Vec::new(), &[]);
    let bobs = bob_s
        .create_record("custom:deal", &body(json!({"name": "Bob deal"})), context())
        .await
        .expect("bob deal");
    let carols = carol_s
        .create_record(
            "custom:deal",
            &body(json!({"name": "Carol deal"})),
            context(),
        )
        .await
        .expect("carol deal");
    mutations += 2;
    let names = |page: RecordPage| {
        page.records
            .into_iter()
            .map(|record| record.name)
            .collect::<Vec<_>>()
    };
    let listed = |service: &ObjectService| {
        let service = service.clone();
        async move {
            names(
                service
                    .list_records("custom:deal", ListRecords::default())
                    .await
                    .expect("deals"),
            )
        }
    };
    assert_eq!(
        listed(&bob_s).await,
        ["Bob deal"],
        "private: owners see their own"
    );
    assert_eq!(
        listed(&alice_s).await,
        ["Bob deal"],
        "managers see their reports'"
    );
    assert!(listed(&dave_s).await.is_empty(), "peers see nothing");
    assert_eq!(
        listed(&admin).await,
        ["Bob deal", "Carol deal"],
        "modify all sees everything"
    );
    assert!(
        matches!(
            dave_s
                .get_record("custom:deal", &bobs.public_id, false)
                .await,
            Err(ObjectError::NotFound)
        ),
        "an invisible record is missing"
    );
    assert!(matches!(
        dave_s
            .update_record(
                "custom:deal",
                &bobs.public_id,
                bobs.version,
                &body(json!({"name": "x"})),
                context()
            )
            .await,
        Err(ObjectError::NotFound)
    ));
    let (_, rights) = alice_s
        .get_record_with_rights("custom:deal", &bobs.public_id, false)
        .await
        .expect("alice reads bob's deal");
    assert!(rights.edit && rights.delete && rights.share && !rights.transfer);
    let bobs = alice_s
        .update_record(
            "custom:deal",
            &bobs.public_id,
            bobs.version,
            &body(json!({"name": "Bob deal"})),
            context(),
        )
        .await
        .expect("managers edit their reports' records");
    mutations += 1;
    assert!(
        matches!(
            alice_s
                .change_owner(
                    "custom:deal",
                    &bobs.public_id,
                    alice,
                    bobs.version,
                    context()
                )
                .await,
            Err(ObjectError::PermissionDenied)
        ),
        "moving a report's record needs records.transfer"
    );

    // Manual shares: read, then edit; never delete or reshare.
    assert!(matches!(
        dave_s.record_sharing("custom:deal", &bobs.public_id).await,
        Err(ObjectError::NotFound)
    ));
    bob_s
        .share_record(
            "custom:deal",
            &bobs.public_id,
            GranteeKind::User,
            dave,
            ShareAccess::Read,
            context(),
        )
        .await
        .expect("share read");
    mutations += 1;
    assert_eq!(
        listed(&dave_s).await,
        ["Bob deal"],
        "a share shows the record"
    );
    assert!(matches!(
        dave_s
            .update_record(
                "custom:deal",
                &bobs.public_id,
                bobs.version,
                &body(json!({"name": "x"})),
                context()
            )
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    bob_s
        .share_record(
            "custom:deal",
            &bobs.public_id,
            GranteeKind::User,
            dave,
            ShareAccess::Edit,
            context(),
        )
        .await
        .expect("upgrade to edit");
    mutations += 1;
    let bobs = dave_s
        .update_record(
            "custom:deal",
            &bobs.public_id,
            bobs.version,
            &body(json!({"name": "Bob deal"})),
            context(),
        )
        .await
        .expect("an edit share edits");
    mutations += 1;
    assert!(
        matches!(
            dave_s
                .delete_record("custom:deal", &bobs.public_id, bobs.version, context())
                .await,
            Err(ObjectError::PermissionDenied)
        ),
        "an edit share never deletes"
    );
    assert!(
        matches!(
            dave_s.record_sharing("custom:deal", &bobs.public_id).await,
            Err(ObjectError::PermissionDenied)
        ),
        "grantees cannot reshare"
    );
    let (_, _, shares) = bob_s
        .record_sharing("custom:deal", &bobs.public_id)
        .await
        .expect("shares");
    assert_eq!(shares.len(), 1);
    assert_eq!(
        (shares[0].grantee_id, shares[0].access),
        (dave, ShareAccess::Edit)
    );
    assert!(matches!(
        bob_s
            .share_record("custom:deal", &bobs.public_id, GranteeKind::User, bob, ShareAccess::Read, context())
            .await,
        Err(ObjectError::Validation(issues)) if issues[0].code == "records.share_owner"
    ));

    // A role share reaches every holder of the role.
    carol_s
        .share_record(
            "custom:deal",
            &carols.public_id,
            GranteeKind::Role,
            sales,
            ShareAccess::Read,
            context(),
        )
        .await
        .expect("role share");
    mutations += 1;
    assert_eq!(listed(&dave_s).await, ["Bob deal", "Carol deal"]);
    carol_s
        .unshare_record(
            "custom:deal",
            &carols.public_id,
            GranteeKind::Role,
            sales,
            context(),
        )
        .await
        .expect("unshare");
    mutations += 1;
    assert_eq!(listed(&dave_s).await, ["Bob deal"]);
    assert!(matches!(
        carol_s
            .unshare_record(
                "custom:deal",
                &carols.public_id,
                GranteeKind::Role,
                sales,
                context()
            )
            .await,
        Err(ObjectError::NotFound)
    ));

    // Lookups and their names follow visibility.
    let mut note = object("note", "Note");
    note.sharing_model = Some(SharingModel::ReadWrite);
    admin.create_object(note, context()).await.expect("note");
    mutations += 1;
    let mut about = field("deal", FieldType::Lookup);
    about.reference_object = Some("custom:deal".into());
    admin
        .create_field("custom:note", about, context())
        .await
        .expect("lookup");
    mutations += 1;
    admin
        .set_object_permission(
            "custom:note",
            sales,
            AccessFlags {
                create: true,
                edit: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("note access");
    mutations += 1;
    assert!(
        matches!(
            dave_s
                .create_record("custom:note", &body(json!({"name": "Peek", "custom:deal": carols.public_id})), context())
                .await,
            Err(ObjectError::Validation(issues)) if issues[0].code == "records.reference_not_found"
        ),
        "a lookup cannot point at an invisible record"
    );
    let carol_note = carol_s
        .create_record(
            "custom:note",
            &body(json!({"name": "Mine", "custom:deal": carols.public_id})),
            context(),
        )
        .await
        .expect("carol's note");
    mutations += 1;
    assert_eq!(
        carol_s
            .related_names("custom:note", std::slice::from_ref(&carol_note))
            .await
            .expect("names")
            .len(),
        1
    );
    assert!(
        dave_s
            .related_names("custom:note", std::slice::from_ref(&carol_note))
            .await
            .expect("names")
            .is_empty(),
        "names of invisible records are withheld"
    );

    // The hierarchy can be switched off per object.
    let (deal_definition, _) = admin.describe("custom:deal").await.expect("deal");
    admin
        .update_object(
            "custom:deal",
            UpdateObject {
                expected_version: deal_definition.version,
                label: None,
                plural_label: None,
                description: None,
                name_label: None,
                sharing_model: None,
                grant_hierarchy: Some(false),
                track_history: None,
                searchable: None,
                in_navigation: None,
            },
            context(),
        )
        .await
        .expect("no hierarchy");
    mutations += 1;
    assert!(
        listed(&alice_s).await.is_empty(),
        "without hierarchy, managers see only their own"
    );

    // Transfer: the owner moves the record and the shares go with them.
    let moved = bob_s
        .change_owner(
            "custom:deal",
            &bobs.public_id,
            carol,
            bobs.version,
            context(),
        )
        .await
        .expect("transfer");
    mutations += 1;
    assert_eq!(moved.owner_id, Some(carol));
    assert_eq!(moved.version, bobs.version + 1);
    assert!(
        listed(&bob_s).await.is_empty(),
        "the previous owner loses a private record"
    );
    assert!(
        listed(&dave_s).await.is_empty(),
        "a transfer removes manual shares"
    );
    assert_eq!(listed(&carol_s).await, ["Bob deal", "Carol deal"]);
    let transferrer = member(dave, Vec::new(), &[RECORDS_TRANSFER_PERMISSION]);
    assert!(
        matches!(
            transferrer
                .change_owner(
                    "custom:deal",
                    &moved.public_id,
                    dave,
                    moved.version,
                    context()
                )
                .await,
            Err(ObjectError::NotFound)
        ),
        "records.transfer moves only records the caller can see and edit"
    );

    // --- Field history (ADR 0017) ----------------------------------------
    let mut ticket = object("ticket", "Ticket");
    ticket.track_history = Some(true);
    admin
        .create_object(ticket, context())
        .await
        .expect("ticket");
    let mut priority = field("priority", FieldType::Text);
    priority.track_history = true;
    admin
        .create_field("custom:ticket", priority, context())
        .await
        .expect("priority");
    admin
        .create_field("custom:ticket", field("notes", FieldType::Text), context())
        .await
        .expect("notes");
    mutations += 3;
    let issue = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Printer", "custom:priority": "low", "custom:notes": "a"})),
            context(),
        )
        .await
        .expect("ticket record");
    let issue = admin
        .update_record(
            "custom:ticket",
            &issue.public_id,
            issue.version,
            &body(json!({"name": "Printer jam", "custom:priority": "high", "custom:notes": "b"})),
            context(),
        )
        .await
        .expect("ticket update");
    let issue = admin
        .delete_record("custom:ticket", &issue.public_id, issue.version, context())
        .await
        .expect("ticket delete");
    let issue = admin
        .undelete_record("custom:ticket", &issue.public_id, issue.version, context())
        .await
        .expect("ticket undelete");
    let issue = admin
        .change_owner(
            "custom:ticket",
            &issue.public_id,
            bob,
            issue.version,
            context(),
        )
        .await
        .expect("ticket transfer");
    mutations += 5;
    let (page, _) = admin
        .record_history("custom:ticket", &issue.public_id, None, None)
        .await
        .expect("history");
    let summary: Vec<(HistoryKind, Option<String>)> = page
        .entries
        .iter()
        .map(|entry| (entry.kind, entry.field.clone()))
        .collect();
    assert_eq!(
        summary,
        [
            (HistoryKind::Changed, Some("owner".to_owned())),
            (HistoryKind::Undeleted, None),
            (HistoryKind::Deleted, None),
            (HistoryKind::Changed, Some("custom:priority".to_owned())),
            (HistoryKind::Changed, Some("name".to_owned())),
            (HistoryKind::Created, None),
        ]
        .iter()
        .map(|(kind, field)| (*kind, field.clone()))
        .collect::<Vec<_>>(),
        "newest first; untracked fields are not kept"
    );
    let priority_change = &page.entries[3];
    assert_eq!(
        (
            priority_change.old_value.clone(),
            priority_change.new_value.clone()
        ),
        (Some(json!("low")), Some(json!("high")))
    );
    assert_eq!(page.entries[0].new_value, Some(json!(bob.to_string())));
    assert!(page
        .entries
        .iter()
        .all(|entry| entry.changed_by == Some(admin_id)));
    let (first, _) = admin
        .record_history("custom:ticket", &issue.public_id, None, Some(2))
        .await
        .expect("first page");
    assert_eq!(first.entries.len(), 2);
    let (second, _) = admin
        .record_history(
            "custom:ticket",
            &issue.public_id,
            first.next_cursor.as_deref(),
            Some(10),
        )
        .await
        .expect("second page");
    assert_eq!(second.entries.len(), 4);
    assert_eq!(
        second.entries[0].kind,
        HistoryKind::Deleted,
        "pages continue where they stopped"
    );
    assert!(second.next_cursor.is_none());
    assert!(
        matches!(
            dave_s
                .record_history("custom:ticket", &issue.public_id, None, None)
                .await,
            Err(ObjectError::NotFound)
        ),
        "no access to the object, no history"
    );
    let (untracked, _) = admin
        .record_history("custom:project", &apollo.public_id, None, None)
        .await
        .expect("untracked history");
    assert!(
        untracked.entries.is_empty(),
        "objects that do not track history keep none"
    );

    // --- Change data capture and sync (ADR 0018, #287) --------------------
    let start = admin
        .list_changes(None, Some(200), None)
        .await
        .expect("change log");
    assert!(start.head > 0, "earlier mutations are in the log");
    let mut position = start.head;
    // Drain to the head so the next page holds only the changes below.
    loop {
        let page = admin
            .list_changes(Some(position), Some(200), None)
            .await
            .expect("drain");
        if page.next_after == position {
            break;
        }
        position = page.next_after;
    }
    let before_sync = chrono::Utc::now();
    let synced = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Scanner", "custom:priority": "low"})),
            context(),
        )
        .await
        .expect("sync ticket");
    let synced = admin
        .update_record(
            "custom:ticket",
            &synced.public_id,
            synced.version,
            &body(json!({"custom:priority": "high"})),
            context(),
        )
        .await
        .expect("sync update");
    let synced = admin
        .delete_record(
            "custom:ticket",
            &synced.public_id,
            synced.version,
            context(),
        )
        .await
        .expect("sync delete");
    mutations += 3;
    let page = admin
        .list_changes(Some(position), Some(200), None)
        .await
        .expect("new changes");
    let seen: Vec<(i64, ChangeKind, Vec<String>)> = page
        .changes
        .iter()
        .map(|(object, change)| {
            assert_eq!(object, "custom:ticket");
            assert_eq!(change.public_id, synced.public_id);
            (change.position, change.kind, change.fields.clone())
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                position + 1,
                ChangeKind::Created,
                vec!["name".to_owned(), "custom:priority".to_owned()]
            ),
            (
                position + 2,
                ChangeKind::Updated,
                vec!["custom:priority".to_owned()]
            ),
            (position + 3, ChangeKind::Deleted, Vec::new()),
        ],
        "gap-free positions in commit order, field names without values"
    );
    assert_eq!((page.next_after, page.head), (position + 3, position + 3));
    let hidden = dave_s
        .list_changes(Some(position), Some(200), None)
        .await
        .expect("dave's view of the log");
    assert!(
        hidden.changes.is_empty(),
        "changes to invisible records are withheld"
    );
    assert_eq!(
        hidden.next_after,
        position + 3,
        "but the position still advances"
    );
    assert!(admin
        .list_changes(Some(position), Some(200), Some("custom:project"))
        .await
        .expect("filtered")
        .changes
        .is_empty());
    assert!(matches!(
        admin.list_changes(Some(position + 4), None, None).await,
        Err(ObjectError::Validation(issues)) if issues[0].code == "changes.position_invalid"
    ));

    // The metadata version moves with definitions, never with records
    // (ADR 0024 §3), so a synced device knows when to refetch them.
    assert!(
        start.metadata_version > 0,
        "objects and fields were defined"
    );
    assert_eq!(
        page.metadata_version, start.metadata_version,
        "record changes leave the metadata version alone"
    );
    let (ticket, _) = admin.describe("custom:ticket").await.expect("ticket");
    let priority = ticket.field("custom:priority").expect("priority").clone();
    admin
        .update_field(
            "custom:ticket",
            "custom:priority",
            serde_json::from_value(json!({
                "expected_version": priority.version,
                "label": "Urgency",
            }))
            .expect("relabel"),
            context(),
        )
        .await
        .expect("relabel priority");
    mutations += 1;
    let relabelled = admin
        .list_changes(Some(position + 3), Some(200), None)
        .await
        .expect("after relabel");
    assert_eq!(
        (relabelled.metadata_version, relabelled.head),
        (start.metadata_version + 1, position + 3),
        "a field change moves the metadata version and leaves no record change"
    );

    // Sync: everything touched since a moment, tombstones included.
    let touched = admin
        .list_records(
            "custom:ticket",
            ListRecords {
                status: RecordStatus::Any,
                sort: RecordSort::TouchedAt,
                touched_after: Some(before_sync),
                ..ListRecords::default()
            },
        )
        .await
        .expect("touched since");
    assert_eq!(touched.records.len(), 1);
    assert!(
        touched.records[0].deleted_at.is_some(),
        "deleted records come back as tombstones"
    );
    assert!(admin
        .list_records(
            "custom:ticket",
            ListRecords {
                status: RecordStatus::Any,
                touched_after: Some(chrono::Utc::now()),
                ..ListRecords::default()
            },
        )
        .await
        .expect("nothing newer")
        .records
        .is_empty());

    // Expiry: an expired position must resynchronise.
    let mut tx = store.begin(tenant_a).await.expect("expiry tx");
    tx.expire_changes(chrono::Utc::now() + chrono::TimeDelta::hours(1))
        .await
        .expect("expire");
    tx.commit().await.expect("expiry commit");
    assert!(matches!(
        admin.list_changes(Some(position), None, None).await,
        Err(ObjectError::Conflict("changes.position_expired"))
    ));
    let resumed = admin
        .list_changes(None, None, None)
        .await
        .expect("resume at the oldest kept");
    assert!(resumed.changes.is_empty());
    assert_eq!(resumed.head, position + 3);

    // --- Workspace isolation ---------------------------------------------
    let other = service(
        &store,
        caller(tenant_b, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    assert!(matches!(
        other.describe("custom:project").await,
        Err(ObjectError::NotFound)
    ));
    assert!(matches!(
        other
            .get_record("custom:project", &apollo.public_id, true)
            .await,
        Err(ObjectError::NotFound)
    ));
    let theirs = other
        .create_object(object("project", "Project"), context())
        .await
        .expect("the same name in another workspace");
    assert!(other
        .list_records("custom:project", ListRecords::default())
        .await
        .expect("empty")
        .records
        .is_empty());
    assert_eq!(theirs.api_name, "custom:project");

    Report {
        tenant_a,
        tenant_b,
        mutations_in_a: mutations,
    }
}
