//! Plugin-declared objects on every backend (ADR 0033 §2). Installing a
//! plugin's manifest creates its objects through the engine with
//! `origin = plugin`, idempotently; upgrades add fields and change labels
//! and options but never drop fields or change types; administrators extend
//! plugin objects but cannot change what the plugin owns; suspending hides
//! the objects everywhere without breaking references to them, and resuming
//! brings everything back unchanged.

use super::{body, caller, codes, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::plugin_objects::{
    PLUGIN_FIELD_TYPE_CHANGED, PLUGIN_KEY_PREFIX_CHANGED, PLUGIN_KEY_PREFIX_TAKEN,
};
use platform_objects::service::{ListRecords, PLUGIN_OWNED};
use platform_objects::*;
use plugin_contract::{Manifest, ObjectFieldDecl, ObjectFieldOption, ObjectFieldType};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const PLUGIN: &str = "example.tracker";
const TICKET: &str = "example.tracker:ticket";
const MILESTONE: &str = "example.tracker:milestone";

/// What the store test checks against its own tables.
pub struct Report {
    /// Ids of records of the plugin's objects, kept through removal.
    pub record_ids: Vec<Uuid>,
}

pub fn fixture() -> Manifest {
    Manifest::parse_toml(include_str!(
        "../../../../fixtures/plugins/tracker/plugin.toml"
    ))
    .expect("the tracker fixture parses")
}

fn declared_field(name: &str, kind: ObjectFieldType) -> ObjectFieldDecl {
    ObjectFieldDecl {
        name: name.into(),
        label: name.into(),
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
        reference_object: None,
        reference_objects: Vec::new(),
        delete_behaviour: None,
    }
}

fn ticket_field<'a>(manifest: &'a mut Manifest, name: &str) -> &'a mut ObjectFieldDecl {
    manifest.objects[1]
        .fields
        .iter_mut()
        .find(|field| field.name == name)
        .expect("declared field")
}

async fn query(who: &ObjectService, q: &str) -> Result<QueryResult, ObjectError> {
    who.query(QueryRequest {
        q: q.into(),
        cursor: None,
        page_size: None,
        include_deleted: false,
    })
    .await
}

fn conflict(result: Result<impl std::fmt::Debug, ObjectError>) -> &'static str {
    match result {
        Err(ObjectError::Conflict(code)) => code,
        other => panic!("expected a conflict, got {other:?}"),
    }
}

async fn listed(who: &ObjectService) -> Vec<String> {
    who.list_objects()
        .await
        .expect("list objects")
        .into_iter()
        .map(|(object, _)| object.api_name)
        .collect()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> Report {
    let member = Uuid::now_v7();
    let admin = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, PLUGIN),
    );
    let stranger = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, "example.other"),
    );
    let manifest = fixture();

    // --- Only the plugin's installer installs it ----------------------------
    assert!(matches!(
        admin.install_plugin_objects(&manifest, context()).await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        stranger.install_plugin_objects(&manifest, context()).await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        stranger.resume_plugin_objects(PLUGIN, context()).await,
        Err(ObjectError::PermissionDenied)
    ));
    // The installer holds no member permissions.
    assert!(matches!(
        installer
            .create_object(object("sneaky", "Sneaky"), context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));

    // --- Install: created suspended, in service once resumed ----------------
    let installed = installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    assert_eq!(installed.created_objects, vec![MILESTONE, TICKET]);
    assert_eq!(installed.created_fields.len(), 4);
    assert!(matches!(
        admin.describe(TICKET).await,
        Err(ObjectError::NotFound)
    ));
    assert!(!listed(&admin).await.iter().any(|name| name == TICKET));
    let mut resumed = installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    resumed.sort();
    assert_eq!(resumed, vec![MILESTONE, TICKET]);
    let (ticket, flags) = admin.describe(TICKET).await.expect("describe ticket");
    assert_eq!(ticket.origin, Origin::Plugin);
    assert_eq!(ticket.owner_plugin.as_deref(), Some(PLUGIN));
    assert_eq!(ticket.key_prefix, "tkt");
    assert!(ticket.track_history);
    assert_eq!(ticket.sharing_model, SharingModel::ReadWrite);
    assert!(flags.read, "records.modify_all reads every object");
    assert_eq!(
        ticket
            .fields
            .iter()
            .map(|field| (field.api_name.as_str(), field.origin))
            .collect::<Vec<_>>(),
        vec![
            ("example.tracker:status", Origin::Plugin),
            ("example.tracker:summary", Origin::Plugin),
            ("example.tracker:milestone", Origin::Plugin),
        ]
    );
    let lookup = ticket.field("example.tracker:milestone").expect("lookup");
    assert_eq!(lookup.field_type, FieldType::Lookup);
    assert_eq!(lookup.reference_object.as_deref(), Some(MILESTONE));
    // Role permissions start as for a new workspace object: no role has any.
    assert!(admin
        .object_permissions(TICKET)
        .await
        .expect("permissions")
        .is_empty());
    let (milestone, _) = admin.describe(MILESTONE).await.expect("milestone");
    assert_eq!(milestone.name_label, "Milestone name");

    // --- Re-installing changes nothing ---------------------------------------
    assert!(installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("re-install")
        .is_empty());
    assert!(installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume again")
        .is_empty());
    assert_eq!(
        admin.describe(TICKET).await.expect("same").0.version,
        ticket.version
    );

    // --- Records ---------------------------------------------------------------
    let launch = admin
        .create_record(MILESTONE, &body(json!({"name": "Launch"})), context())
        .await
        .expect("milestone record");
    assert!(launch.public_id.starts_with("tkm"));
    let apollo = admin
        .create_record(
            TICKET,
            &body(json!({
                "name": "Apollo",
                "example.tracker:summary": "Ship the apollo release",
                "example.tracker:milestone": launch.public_id,
            })),
            context(),
        )
        .await
        .expect("ticket record");
    assert_eq!(
        apollo.values.get("example.tracker:status"),
        Some(&json!("open"))
    );

    // The query language reads dotted plugin namespaces.
    let rows = query(
        &admin,
        "SELECT name, example.tracker:summary, example.tracker:milestone.name \
         FROM example.tracker:ticket WHERE example.tracker:status = 'open'",
    )
    .await
    .expect("query a plugin object");
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(
        rows.rows[0].get("example.tracker:milestone.name"),
        Some(&json!("Launch"))
    );

    // --- Administrators extend plugin objects ------------------------------
    let priority = admin
        .create_field(TICKET, field("priority", FieldType::Number), context())
        .await
        .expect("a custom field on a plugin object");
    assert_eq!(priority.api_name, "custom:priority");
    assert_eq!(priority.origin, Origin::Workspace);
    let mut relabel = update_field(priority.version);
    relabel.label = Some("Priority score".into());
    admin
        .update_field(TICKET, "custom:priority", relabel, context())
        .await
        .expect("their own fields stay theirs to change");
    admin
        .create_validation_rule(
            TICKET,
            CreateValidationRule {
                name: "summary_required".into(),
                description: None,
                condition: "ISBLANK(example.tracker:summary)".into(),
                message: "Add a summary".into(),
                error_field: Some("example.tracker:summary".into()),
                active: Some(true),
            },
            context(),
        )
        .await
        .expect("a validation rule on a plugin object");
    let refused = admin
        .create_record(TICKET, &body(json!({"name": "No summary"})), context())
        .await
        .expect_err("the rule applies");
    assert!(
        codes(&refused).contains(&"records.validation_rule"),
        "{refused:?}"
    );

    // --- ... but cannot change what the plugin owns -----------------------
    let summary = ticket.field("example.tracker:summary").expect("summary");
    let mut change = update_field(summary.version);
    change.label = Some("Mine now".into());
    assert_eq!(
        conflict(
            admin
                .update_field(TICKET, "example.tracker:summary", change, context())
                .await
        ),
        PLUGIN_OWNED
    );
    assert_eq!(
        conflict(
            admin
                .delete_field(
                    TICKET,
                    "example.tracker:summary",
                    summary.version,
                    context()
                )
                .await
        ),
        PLUGIN_OWNED
    );
    let (ticket, _) = admin.describe(TICKET).await.expect("ticket");
    let mut relabel_object = update_object(ticket.version);
    relabel_object.label = Some("Issue".into());
    assert_eq!(
        conflict(admin.update_object(TICKET, relabel_object, context()).await),
        PLUGIN_OWNED
    );
    assert_eq!(
        conflict(admin.delete_object(TICKET, ticket.version, context()).await),
        PLUGIN_OWNED
    );
    // Sharing is the workspace's to choose.
    let mut private = update_object(ticket.version);
    private.sharing_model = Some(SharingModel::Private);
    let ticket = admin
        .update_object(TICKET, private, context())
        .await
        .expect("sharing settings of a plugin object");
    assert_eq!(ticket.sharing_model, SharingModel::Private);
    // Only installation creates objects in a plugin's namespace.
    let named = admin
        .create_object(object("example.tracker:ticket2", "Ticket"), context())
        .await
        .expect_err("a plugin namespace through the object API");
    assert!(codes(&named).contains(&"objects.name_invalid"));
    let mut named_field = field("x", FieldType::Text);
    named_field.name = "example.tracker:x".into();
    let named = admin
        .create_field(TICKET, named_field, context())
        .await
        .expect_err("a plugin field through the object API");
    assert!(codes(&named).contains(&"objects.name_invalid"));

    // --- Upgrade: new fields, new labels and options, nothing dropped -----
    let mut upgrade = fixture();
    upgrade.version = "0.2.0".parse().expect("version");
    upgrade.objects[1].label = "Work item".into();
    upgrade.objects[1].plural_label = "Work items".into();
    let mut estimate = declared_field("estimate", ObjectFieldType::Number);
    estimate.label = "Estimate".into();
    upgrade.objects[1].fields.push(estimate);
    ticket_field(&mut upgrade, "status").options = vec![
        ObjectFieldOption {
            value: "open".into(),
            label: "Open".into(),
            default: true,
        },
        ObjectFieldOption {
            value: "closed".into(),
            label: "Closed".into(),
            default: false,
        },
    ];
    // The summary field is no longer declared: it stays, with its values.
    upgrade.objects[1]
        .fields
        .retain(|field| field.name != "summary");
    let upgraded = installer
        .install_plugin_objects(&upgrade, context())
        .await
        .expect("upgrade");
    assert_eq!(upgraded.updated_objects, vec![TICKET]);
    assert_eq!(
        upgraded.created_fields,
        vec!["example.tracker:ticket/example.tracker:estimate"]
    );
    assert_eq!(
        upgraded.updated_fields,
        vec!["example.tracker:ticket/example.tracker:status"]
    );
    let (ticket, _) = admin.describe(TICKET).await.expect("upgraded ticket");
    assert_eq!(ticket.label, "Work item");
    assert_eq!(
        ticket.sharing_model,
        SharingModel::Private,
        "kept the workspace's choice"
    );
    assert!(
        ticket.field("example.tracker:summary").is_some(),
        "dropped fields stay"
    );
    assert!(
        ticket.field("custom:priority").is_some(),
        "custom fields stay"
    );
    let status = ticket.field("example.tracker:status").expect("status");
    assert_eq!(
        status
            .options
            .iter()
            .map(|option| (option.value.as_str(), option.active))
            .collect::<Vec<_>>(),
        vec![("open", true), ("closed", true), ("done", false)],
        "options the plugin dropped stay, inactive"
    );
    let reread = admin
        .get_record(TICKET, &apollo.public_id, false)
        .await
        .expect("record after the upgrade");
    assert_eq!(
        reread.values.get("example.tracker:summary"),
        Some(&json!("Ship the apollo release"))
    );
    assert!(installer
        .install_plugin_objects(&upgrade, context())
        .await
        .expect("upgrade again")
        .is_empty());

    // --- Refused upgrades change nothing ------------------------------------
    let mut retyped = upgrade.clone();
    retyped.objects[1].label = "Changed again".into();
    ticket_field(&mut retyped, "estimate").field_type = ObjectFieldType::Text;
    assert_eq!(
        conflict(installer.install_plugin_objects(&retyped, context()).await),
        PLUGIN_FIELD_TYPE_CHANGED
    );
    assert_eq!(
        admin.describe(TICKET).await.expect("ticket").0.label,
        "Work item",
        "a refused upgrade is rolled back whole"
    );
    let mut moved = upgrade.clone();
    moved.objects[1].key_prefix = "tkx".into();
    assert_eq!(
        conflict(installer.install_plugin_objects(&moved, context()).await),
        PLUGIN_KEY_PREFIX_CHANGED
    );
    let mut other = fixture();
    other.id = "example.other".into();
    other.objects.truncate(1);
    other.objects[0].key_prefix = "tkt".into();
    assert_eq!(
        conflict(stranger.install_plugin_objects(&other, context()).await),
        PLUGIN_KEY_PREFIX_TAKEN
    );

    // --- Other objects point at plugin objects -----------------------------
    admin
        .create_object(object("project", "Project"), context())
        .await
        .expect("project");
    let mut ticket_link = field("ticket", FieldType::Lookup);
    ticket_link.reference_object = Some(TICKET.into());
    admin
        .create_field("custom:project", ticket_link, context())
        .await
        .expect("a lookup to a plugin object");
    let mut about = field("about", FieldType::RelatedTo);
    about.reference_objects = Some(vec![MILESTONE.into(), "custom:project".into()]);
    admin
        .create_field("custom:project", about, context())
        .await
        .expect("a related-to field allowing a plugin object");
    let project = admin
        .create_record(
            "custom:project",
            &body(json!({
                "name": "Moonshot",
                "custom:ticket": apollo.public_id,
                "custom:about": launch.public_id,
            })),
            context(),
        )
        .await
        .expect("project record");
    let names = admin
        .related_names("custom:project", std::slice::from_ref(&project))
        .await
        .expect("names");
    assert_eq!(
        names.get(&apollo.public_id).map(String::as_str),
        Some("Apollo")
    );

    // --- Disabling hides the plugin's objects everywhere --------------------
    let mut suspended = installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend");
    suspended.sort();
    assert_eq!(suspended, vec![MILESTONE, TICKET]);
    assert!(matches!(
        admin.describe(TICKET).await,
        Err(ObjectError::NotFound)
    ));
    let visible = listed(&admin).await;
    assert!(!visible.iter().any(|name| name.starts_with(PLUGIN)));
    assert!(visible.iter().any(|name| name == "custom:project"));
    assert!(matches!(
        admin.get_record(TICKET, &apollo.public_id, false).await,
        Err(ObjectError::NotFound)
    ));
    assert!(matches!(
        admin.list_records(TICKET, ListRecords::default()).await,
        Err(ObjectError::NotFound)
    ));
    assert!(matches!(
        admin
            .create_record(TICKET, &body(json!({"name": "Hidden"})), context())
            .await,
        Err(ObjectError::NotFound)
    ));
    assert!(query(&admin, "SELECT name FROM example.tracker:ticket")
        .await
        .is_err());
    let found = admin
        .search(SearchRequest {
            q: "apollo".into(),
            object: None,
            limit: None,
        })
        .await
        .expect("search");
    assert!(!found.iter().any(|group| group.object == TICKET));
    // Records pointing at them still read, list, query and save; their
    // values are kept but no longer followed.
    let still = admin
        .get_record("custom:project", &project.public_id, false)
        .await
        .expect("a record pointing at a suspended object");
    assert_eq!(
        still.values.get("custom:ticket"),
        Some(&Value::String(apollo.public_id.clone()))
    );
    assert_eq!(
        still.values.get("custom:about"),
        Some(&Value::String(launch.public_id.clone()))
    );
    let names = admin
        .related_names("custom:project", std::slice::from_ref(&still))
        .await
        .expect("names");
    assert!(names.is_empty(), "suspended targets are not followable");
    admin
        .list_records("custom:project", ListRecords::default())
        .await
        .expect("list projects");
    query(&admin, "SELECT name, custom:ticket FROM custom:project")
        .await
        .expect("query projects");
    admin
        .related_lists("custom:project", &project.public_id)
        .await
        .expect("related lists of a project");
    let renamed = admin
        .update_record(
            "custom:project",
            &project.public_id,
            still.version,
            &body(json!({"name": "Moonshot II"})),
            context(),
        )
        .await
        .expect("saving a record that points at a suspended object");
    assert_eq!(
        renamed.values.get("custom:ticket"),
        Some(&Value::String(apollo.public_id.clone()))
    );
    let pointing = admin
        .create_record(
            "custom:project",
            &body(json!({"name": "Another", "custom:ticket": apollo.public_id})),
            context(),
        )
        .await
        .expect_err("a new value cannot follow a suspended object");
    assert!(codes(&pointing).contains(&"records.reference_not_found"));
    // Nor can new definitions start depending on them.
    let mut late = field("late", FieldType::Lookup);
    late.reference_object = Some(TICKET.into());
    let late = admin
        .create_field("custom:project", late, context())
        .await
        .expect_err("a lookup to a suspended object");
    assert!(codes(&late).contains(&"objects.reference_object_unknown"));

    // --- Enabling again brings everything back unchanged -------------------
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    let back = admin
        .get_record(TICKET, &apollo.public_id, false)
        .await
        .expect("the ticket is back");
    assert_eq!(back.version, reread.version);
    assert_eq!(back.values, reread.values);
    let (ticket, _) = admin.describe(TICKET).await.expect("ticket");
    assert!(ticket.field("custom:priority").is_some());
    assert_eq!(
        admin
            .related_names("custom:project", std::slice::from_ref(&renamed))
            .await
            .expect("names")
            .get(&apollo.public_id)
            .map(String::as_str),
        Some("Apollo")
    );

    // --- Removal keeps records for export; reinstalling brings them back ---
    installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend for removal");
    let mut tx = store.begin(tenant).await.expect("tx");
    for record in [&apollo, &launch] {
        assert!(
            tx.record_by_id(record.id)
                .await
                .expect("read")
                .is_some_and(|kept| kept.deleted_at.is_none()),
            "records are kept"
        );
    }
    tx.commit().await.expect("commit");
    assert!(installer
        .install_plugin_objects(&upgrade, context())
        .await
        .expect("reinstall")
        .is_empty());
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume after reinstall");
    admin
        .get_record(MILESTONE, &launch.public_id, false)
        .await
        .expect("records are back after reinstalling");

    Report {
        record_ids: vec![apollo.id, launch.id],
    }
}

fn update_field(version: i64) -> UpdateField {
    UpdateField {
        expected_version: version,
        label: None,
        description: None,
        required: None,
        indexed: None,
        searchable: None,
        track_history: None,
        max_length: None,
        options: None,
        default_value: None,
        position: None,
        formula: None,
        lookup_filter: None,
        reference_objects: None,
    }
}

fn update_object(version: i64) -> UpdateObject {
    UpdateObject {
        expected_version: version,
        label: None,
        plural_label: None,
        description: None,
        name_label: None,
        sharing_model: None,
        grant_hierarchy: None,
        track_history: None,
        searchable: None,
        in_navigation: None,
    }
}
