//! Archiving records on every backend (ADR 0035 G1): archive and unarchive
//! are edits that need edit access, move the version, leave a lifecycle
//! entry in history and an `archived_at` update in the change log, and
//! take a record out of default lists, queries, global search, lookup
//! pickers and related lists while it stays readable by id.

use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::service::{ListRecords, ALREADY_ARCHIVED, ARCHIVE_DELETED, NOT_ARCHIVED};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// What the store's own test checks in its outbox and audit log.
pub struct ArchiveReport {
    /// Public IDs archived, once each, in order, all through the web app.
    pub archived: Vec<String>,
    /// Public IDs unarchived, once each, in order.
    pub unarchived: Vec<String>,
}

async fn names(service: &ObjectService, status: RecordStatus) -> Vec<String> {
    service
        .list_records(
            "custom:case",
            ListRecords {
                status,
                ..ListRecords::default()
            },
        )
        .await
        .expect("list")
        .records
        .into_iter()
        .map(|record| record.name)
        .collect()
}

fn ids(result: &QueryResult) -> Vec<String> {
    result
        .rows
        .iter()
        .filter_map(|row| row.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

fn query(q: &str, include_archived: bool) -> QueryRequest {
    QueryRequest {
        q: q.to_owned(),
        include_archived,
        ..QueryRequest::default()
    }
}

async fn kinds(service: &ObjectService, public_id: &str) -> Vec<(HistoryKind, Value)> {
    service
        .record_history("custom:case", public_id, None, None)
        .await
        .expect("history")
        .0
        .entries
        .into_iter()
        .map(|entry| {
            (
                entry.kind,
                serde_json::to_value(entry.source).expect("source"),
            )
        })
        .collect()
}

async fn search(service: &ObjectService, q: &str) -> Vec<String> {
    service
        .search(SearchRequest {
            q: q.to_owned(),
            object: Some("custom:case".into()),
            limit: None,
        })
        .await
        .expect("search")
        .into_iter()
        .flat_map(|group| group.records.into_iter().map(|hit| hit.name))
        .collect()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> ArchiveReport {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let web = || context().with_source(MutationSource::web());
    let mut report = ArchiveReport {
        archived: Vec::new(),
        unarchived: Vec::new(),
    };

    let mut case = object("case", "Case");
    case.track_history = Some(true);
    case.sharing_model = Some(SharingModel::Private);
    admin.create_object(case, context()).await.expect("case");
    let mut note = field("note", FieldType::Text);
    note.searchable = true;
    admin
        .create_field("custom:case", note, context())
        .await
        .expect("note");
    admin
        .create_object(object("task", "Task"), context())
        .await
        .expect("task");
    let mut regarding = field("case", FieldType::Lookup);
    regarding.reference_object = Some("custom:case".into());
    admin
        .create_field("custom:task", regarding, context())
        .await
        .expect("lookup");

    let create = |name: &str| {
        let admin = admin.clone();
        let values = body(json!({"name": name, "custom:note": format!("{name} notes")}));
        async move {
            admin
                .create_record("custom:case", &values, context())
                .await
                .expect("case record")
        }
    };
    let alpha = create("Alpha").await;
    let beta = create("Beta").await;
    let gamma = create("Gamma").await;
    let task = |name: &str, case: &str| {
        let admin = admin.clone();
        let values = body(json!({"name": name, "custom:case": case}));
        async move {
            admin
                .create_record("custom:task", &values, context())
                .await
                .expect("task record")
        }
    };
    let on_alpha = task("On alpha", &alpha.public_id).await;
    let hidden_task = task("Archived task", &beta.public_id).await;
    task("Open task", &beta.public_id).await;

    // --- Archiving --------------------------------------------------------
    assert!(matches!(
        admin
            .archive_record("custom:case", &alpha.public_id, alpha.version + 1, web())
            .await,
        Err(ObjectError::VersionConflict)
    ));
    let archived = admin
        .archive_record("custom:case", &alpha.public_id, alpha.version, web())
        .await
        .expect("archive");
    report.archived.push(alpha.public_id.clone());
    assert_eq!(
        archived.version,
        alpha.version + 1,
        "an edit moves the version"
    );
    assert_eq!(archived.archived_at, Some(archived.updated_at));
    assert!(archived.updated_at > alpha.updated_at);
    assert_eq!(archived.touched_at, archived.updated_at);
    assert_eq!(archived.values, alpha.values, "no value changes");
    assert!(matches!(
        admin
            .archive_record("custom:case", &alpha.public_id, archived.version, web())
            .await,
        Err(ObjectError::Conflict(code)) if code == ALREADY_ARCHIVED
    ));
    assert_eq!(ALREADY_ARCHIVED, "records.already_archived");
    assert!(matches!(
        admin
            .unarchive_record("custom:case", &beta.public_id, beta.version, web())
            .await,
        Err(ObjectError::Conflict(code)) if code == NOT_ARCHIVED
    ));
    assert_eq!(NOT_ARCHIVED, "records.not_archived");
    // A record the caller missed being archived says so before its version.
    assert!(matches!(
        admin
            .archive_record("custom:case", &alpha.public_id, alpha.version, web())
            .await,
        Err(ObjectError::Conflict("records.already_archived"))
    ));

    // Readable by id, hidden from default lists.
    let read = admin
        .get_record("custom:case", &alpha.public_id, false)
        .await
        .expect("readable by id");
    // Stores keep microseconds.
    assert_eq!(
        read.archived_at.map(|at| at.timestamp_micros()),
        archived.archived_at.map(|at| at.timestamp_micros())
    );
    assert_eq!(names(&admin, RecordStatus::Active).await, ["Beta", "Gamma"]);
    assert_eq!(names(&admin, RecordStatus::Archived).await, ["Alpha"]);
    assert_eq!(
        names(&admin, RecordStatus::All).await,
        ["Alpha", "Beta", "Gamma"]
    );
    assert!(names(&admin, RecordStatus::Deleted).await.is_empty());
    assert_eq!(
        names(&admin, RecordStatus::Any).await,
        ["Alpha", "Beta", "Gamma"],
        "sync clients see archived records with their archived_at"
    );

    // --- The query language ----------------------------------------------
    let all = "SELECT id, name, archived_at FROM custom:case ORDER BY name";
    let default = admin.query(query(all, false)).await.expect("query");
    assert_eq!(
        ids(&default),
        [beta.public_id.clone(), gamma.public_id.clone()]
    );
    assert!(default
        .columns
        .iter()
        .any(|column| column.key == "archived_at" && column.column_type == "date_time"));
    let with = admin.query(query(all, true)).await.expect("query");
    assert_eq!(
        ids(&with),
        [
            alpha.public_id.clone(),
            beta.public_id.clone(),
            gamma.public_id.clone()
        ]
    );
    assert!(with.rows[0]["archived_at"].is_string());
    assert!(with.rows[1]["archived_at"].is_null());
    let only = admin
        .query(query(
            "SELECT id FROM custom:case WHERE archived_at != null",
            true,
        ))
        .await
        .expect("archived only");
    assert_eq!(ids(&only), [alpha.public_id.as_str()]);
    let recent = admin
        .query(query(
            "SELECT id FROM custom:case WHERE archived_at != null ORDER BY archived_at DESC, name",
            true,
        ))
        .await
        .expect("by date");
    assert_eq!(ids(&recent), [alpha.public_id.as_str()]);
    let deleted_too = admin
        .query(QueryRequest {
            q: "SELECT id FROM custom:case".into(),
            include_deleted: true,
            ..QueryRequest::default()
        })
        .await
        .expect("include deleted");
    assert_eq!(
        ids(&deleted_too).len(),
        3,
        "the recycle bin view is everything"
    );
    // A cursor belongs to its flags.
    let paged = admin
        .query(QueryRequest {
            q: "SELECT id FROM custom:case ORDER BY name".into(),
            page_size: Some(1),
            include_archived: true,
            ..QueryRequest::default()
        })
        .await
        .expect("page");
    let cursor = paged.next_cursor.clone().expect("cursor");
    assert!(admin
        .query(QueryRequest {
            q: "SELECT id FROM custom:case ORDER BY name".into(),
            cursor: Some(cursor),
            page_size: Some(1),
            ..QueryRequest::default()
        })
        .await
        .is_err());

    // --- Search, pickers, related lists and lookups -----------------------
    assert!(search(&admin, "alpha").await.is_empty(), "search hides it");
    assert_eq!(search(&admin, "beta").await, ["Beta"]);
    let picker = admin
        .lookup_candidates("custom:task", "custom:case", None, None)
        .await
        .expect("candidates");
    assert_eq!(
        ids(&picker),
        [beta.public_id.clone(), gamma.public_id.clone()]
    );
    // An existing lookup to an archived record keeps its value and name,
    // and paths through it still read the target.
    let kept = admin
        .get_record("custom:task", &on_alpha.public_id, false)
        .await
        .expect("task");
    assert_eq!(kept.values["custom:case"], json!(alpha.public_id));
    let through = admin
        .query(query(
            "SELECT id, custom:case.name FROM custom:task WHERE custom:case.name = 'Alpha'",
            false,
        ))
        .await
        .expect("through the lookup");
    assert_eq!(ids(&through), [on_alpha.public_id.as_str()]);
    assert_eq!(through.rows[0]["custom:case.name"], json!("Alpha"));
    // A member may still point a lookup at it by id.
    admin
        .update_record(
            "custom:task",
            &hidden_task.public_id,
            hidden_task.version,
            &body(json!({"custom:case": alpha.public_id})),
            context(),
        )
        .await
        .expect("a lookup may name an archived record");
    let hidden_task = admin
        .update_record(
            "custom:task",
            &hidden_task.public_id,
            hidden_task.version + 1,
            &body(json!({"custom:case": beta.public_id})),
            context(),
        )
        .await
        .expect("back to beta");
    admin
        .archive_record(
            "custom:task",
            &hidden_task.public_id,
            hidden_task.version,
            web(),
        )
        .await
        .expect("archive a task");
    let related = admin
        .related_lists("custom:case", &beta.public_id)
        .await
        .expect("related");
    let tasks = related
        .iter()
        .find(|list| list.relationship.object == "custom:task")
        .expect("tasks list");
    assert_eq!(
        tasks
            .records
            .rows
            .iter()
            .map(|row| row["name"].clone())
            .collect::<Vec<_>>(),
        [json!("Open task")],
        "related lists leave archived records out"
    );
    // The related lists of an archived record still open.
    admin
        .related_lists("custom:case", &alpha.public_id)
        .await
        .expect("an archived record's related lists");

    // --- Archiving is an edit ---------------------------------------------
    let role = Uuid::now_v7();
    let reader_role = Uuid::now_v7();
    admin
        .set_object_permission(
            "custom:case",
            role,
            AccessFlags {
                read: true,
                create: true,
                edit: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("member access");
    admin
        .set_object_permission(
            "custom:case",
            reader_role,
            AccessFlags {
                read: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("reader access");
    let ana_id = Uuid::now_v7();
    let ben_id = Uuid::now_v7();
    let ana = service(&store, caller(tenant, ana_id, &[], &[role]));
    let ben = service(&store, caller(tenant, ben_id, &[], &[role]));
    let reader = service(&store, caller(tenant, Uuid::now_v7(), &[], &[reader_role]));
    let anas = ana
        .create_record("custom:case", &body(json!({"name": "Ana case"})), context())
        .await
        .expect("ana's case");
    assert!(
        matches!(
            ben.archive_record("custom:case", &anas.public_id, anas.version, web())
                .await,
            Err(ObjectError::NotFound)
        ),
        "a private record another member cannot see is missing"
    );
    assert!(
        matches!(
            reader
                .archive_record("custom:case", &anas.public_id, anas.version, web())
                .await,
            Err(ObjectError::PermissionDenied)
        ),
        "archiving needs the object's edit permission"
    );
    ana.share_record(
        "custom:case",
        &anas.public_id,
        GranteeKind::User,
        ben_id,
        ShareAccess::Read,
        context(),
    )
    .await
    .expect("share read");
    assert!(
        matches!(
            ben.archive_record("custom:case", &anas.public_id, anas.version, web())
                .await,
            Err(ObjectError::PermissionDenied)
        ),
        "a read share does not archive"
    );
    ana.share_record(
        "custom:case",
        &anas.public_id,
        GranteeKind::User,
        ben_id,
        ShareAccess::Edit,
        context(),
    )
    .await
    .expect("share edit");
    let anas = ben
        .archive_record("custom:case", &anas.public_id, anas.version, web())
        .await
        .expect("an edit share archives");
    report.archived.push(anas.public_id.clone());
    assert!(matches!(
        reader
            .unarchive_record("custom:case", &anas.public_id, anas.version, web())
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    let anas = ana
        .unarchive_record("custom:case", &anas.public_id, anas.version, web())
        .await
        .expect("the owner unarchives");
    report.unarchived.push(anas.public_id.clone());
    assert!(anas.archived_at.is_none());

    // --- The recycle bin --------------------------------------------------
    let gamma = admin
        .archive_record("custom:case", &gamma.public_id, gamma.version, web())
        .await
        .expect("archive gamma");
    report.archived.push(gamma.public_id.clone());
    let gamma = admin
        .delete_record("custom:case", &gamma.public_id, gamma.version, context())
        .await
        .expect("an archived record can be deleted");
    assert!(gamma.archived_at.is_some(), "it keeps its archive state");
    assert_eq!(names(&admin, RecordStatus::Deleted).await, ["Gamma"]);
    assert_eq!(names(&admin, RecordStatus::Archived).await, ["Alpha"]);
    for result in [
        admin
            .archive_record("custom:case", &gamma.public_id, gamma.version, web())
            .await,
        admin
            .unarchive_record("custom:case", &gamma.public_id, gamma.version, web())
            .await,
    ] {
        assert!(matches!(
            result,
            Err(ObjectError::Conflict(code)) if code == ARCHIVE_DELETED
        ));
    }
    assert_eq!(ARCHIVE_DELETED, "records.deleted");
    let gamma = admin
        .undelete_record("custom:case", &gamma.public_id, gamma.version, context())
        .await
        .expect("undelete");
    assert!(gamma.archived_at.is_some(), "it comes back archived");
    assert_eq!(
        names(&admin, RecordStatus::Archived).await,
        ["Alpha", "Gamma"]
    );

    // --- Unarchiving ------------------------------------------------------
    let alpha = admin
        .unarchive_record("custom:case", &alpha.public_id, archived.version, web())
        .await
        .expect("unarchive");
    report.unarchived.push(alpha.public_id.clone());
    assert_eq!(alpha.version, archived.version + 1);
    assert!(alpha.archived_at.is_none());
    assert_eq!(
        names(&admin, RecordStatus::Active).await,
        ["Alpha", "Ana case", "Beta"]
    );
    assert_eq!(search(&admin, "alpha").await, ["Alpha"]);

    // --- History and the change log ----------------------------------------
    let web_source = serde_json::to_value(MutationSource::web()).expect("source");
    assert_eq!(
        kinds(&admin, &alpha.public_id).await,
        [
            (HistoryKind::Unarchived, web_source.clone()),
            (HistoryKind::Archived, web_source),
            (HistoryKind::Created, Value::Null),
        ],
        "lifecycle entries, newest first"
    );
    let changes = admin
        .list_changes(None, Some(200), Some("custom:case"))
        .await
        .expect("changes")
        .changes;
    let alpha_changes: Vec<(ChangeKind, Vec<String>)> = changes
        .iter()
        .filter(|(_, change)| change.public_id == alpha.public_id)
        .map(|(_, change)| (change.kind, change.fields.clone()))
        .collect();
    assert_eq!(
        alpha_changes,
        [
            (ChangeKind::Created, alpha_changes[0].1.clone()),
            (ChangeKind::Updated, vec!["archived_at".to_owned()]),
            (ChangeKind::Updated, vec!["archived_at".to_owned()]),
        ]
    );
    report
}

/// A replica takes the server's archive state with every record it applies
/// (ADR 0024 §3): lists, queries and search on the device follow it.
pub async fn replica(server_store: Arc<dyn ObjectStore>, device_store: Arc<dyn ObjectStore>) {
    let user = Uuid::now_v7();
    let tenant = Uuid::now_v7();
    let member = caller(
        tenant,
        user,
        &[OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION],
        &[],
    );
    let server = service(&server_store, member.clone());
    let device = service(&device_store, member).with_replica();
    for engine in [&server, &device] {
        engine
            .create_object(object("case", "Case"), context())
            .await
            .expect("case");
    }
    let record = server
        .create_record("custom:case", &body(json!({"name": "Synced"})), context())
        .await
        .expect("record");
    let apply = |record: Record| {
        let device = device.clone();
        async move {
            device
                .apply_remote("custom:case", &super::push::remote(&record), None)
                .await
                .expect("apply")
        }
    };
    assert!(matches!(apply(record.clone()).await, Applied::Inserted(_)));
    assert_eq!(names(&device, RecordStatus::Active).await, ["Synced"]);

    let archived = server
        .archive_record("custom:case", &record.public_id, record.version, context())
        .await
        .expect("archive on the server");
    let taken = match apply(archived.clone()).await {
        Applied::Updated(taken) => taken,
        other => panic!("applied {other:?}"),
    };
    assert_eq!(
        taken.archived_at.map(|at| at.timestamp_micros()),
        archived.archived_at.map(|at| at.timestamp_micros())
    );
    assert!(names(&device, RecordStatus::Active).await.is_empty());
    assert_eq!(names(&device, RecordStatus::Archived).await, ["Synced"]);
    assert!(device
        .query(query("SELECT id FROM custom:case", false))
        .await
        .expect("device query")
        .rows
        .is_empty());
    assert!(search(&device, "synced").await.is_empty());
    assert!(
        matches!(
            device
                .unarchive_record("custom:case", &record.public_id, taken.version, context())
                .await,
            Err(ObjectError::PermissionDenied)
        ),
        "archive state is the server's to change"
    );
    assert!(
        device.push_queue().await.expect("queue").is_empty(),
        "the server's archive is not the member's to push"
    );

    let unarchived = server
        .unarchive_record(
            "custom:case",
            &record.public_id,
            archived.version,
            context(),
        )
        .await
        .expect("unarchive on the server");
    apply(unarchived).await;
    assert_eq!(names(&device, RecordStatus::Active).await, ["Synced"]);
    assert!(device
        .get_record("custom:case", &record.public_id, false)
        .await
        .expect("device read")
        .archived_at
        .is_none());
}
