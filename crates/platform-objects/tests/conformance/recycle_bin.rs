//! The recycle bin on every backend (ADR 0035 G2, G3): the purge removes
//! records 15 days after deletion, in idempotent batches, never a record
//! undeleted meanwhile, with its history, change-log rows and index; and
//! undeleting a record puts back the lookups its deletion cleared where
//! the referencing record did not write that field since.

use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::recycle_bin::{PURGE_BATCH, PURGE_BUDGET};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// What the store's own test checks in its tables, outbox and audit log.
pub struct RecycleBinReport {
    /// Internal ids of records the purge removed.
    pub purged: Vec<Uuid>,
    /// Their public ids, for the events.
    pub purged_public: Vec<String>,
    /// Purge transactions that removed records.
    pub batches: u32,
    /// A record undeleted before the purge reached it, still there.
    pub survivor: Uuid,
}

/// A moment the purge treats as `days` days after now.
fn after_days(days: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() + chrono::TimeDelta::days(days)
}

async fn purge(store: &Arc<dyn ObjectStore>, tenant: Uuid, days: i64) -> PurgeOutcome {
    purge_recycle_bin(store.clone(), tenant, after_days(days), PURGE_BUDGET)
        .await
        .expect("purge")
}

fn value<'a>(record: &'a Record, field: &str) -> Option<&'a Value> {
    record.values.get(field)
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> RecycleBinReport {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let mut note = object("note", "Note");
    note.track_history = Some(true);
    note.searchable = Some(true);
    admin.create_object(note, context()).await.expect("note");
    let mut text = field("text", FieldType::Text);
    text.searchable = true;
    text.track_history = true;
    admin
        .create_field("custom:note", text, context())
        .await
        .expect("text");
    let create = |name: String| {
        let admin = admin.clone();
        async move {
            admin
                .create_record(
                    "custom:note",
                    &body(json!({"name": name, "custom:text": "purgeable words"})),
                    context(),
                )
                .await
                .expect("note record")
        }
    };
    let delete = |record: Record| {
        let admin = admin.clone();
        async move {
            admin
                .delete_record("custom:note", &record.public_id, record.version, context())
                .await
                .expect("delete")
        }
    };

    // Nothing is due: a pass changes nothing.
    let idle = purge(&store, tenant, 0).await;
    assert_eq!(idle.purged, 0);
    assert!(idle.complete);

    // --- The window boundary -----------------------------------------------
    let kept = create("Kept".into()).await;
    let doomed = create("Doomed".into()).await;
    let edited = admin
        .update_record(
            "custom:note",
            &doomed.public_id,
            doomed.version,
            &body(json!({"custom:text": "changed words"})),
            context(),
        )
        .await
        .expect("edit");
    let doomed = delete(edited).await;
    // An archived record in the bin goes on its deletion date too.
    let archived = create("Archived".into()).await;
    let archived = admin
        .archive_record(
            "custom:note",
            &archived.public_id,
            archived.version,
            context(),
        )
        .await
        .expect("archive");
    let archived = delete(archived).await;
    // One day short of the window: still in the bin.
    let early = purge(&store, tenant, 14).await;
    assert_eq!(early.purged, 0, "{early:?}");
    assert!(admin
        .get_record("custom:note", &doomed.public_id, true)
        .await
        .is_ok());
    // An hour short of it: still there.
    let short = purge_recycle_bin(
        store.clone(),
        tenant,
        doomed.deleted_at.expect("deleted") + RECYCLE_BIN_RETENTION - chrono::TimeDelta::hours(1),
        PURGE_BUDGET,
    )
    .await
    .expect("purge");
    assert_eq!(short.purged, 0);
    assert!(admin
        .record_history("custom:note", &doomed.public_id, None, None)
        .await
        .is_ok());

    // Undeleted before the purge reaches it: never purged. The purge
    // listed it as due first, as a pass running alongside would have.
    let survivor = delete(create("Survivor".into()).await).await;
    let mut tx = store.begin(tenant).await.expect("tx");
    let due = tx
        .purge_candidates(after_days(16) - RECYCLE_BIN_RETENTION, 1000)
        .await
        .expect("candidates");
    tx.commit().await.expect("commit");
    assert!(due.contains(&survivor.id));
    let survivor = admin
        .undelete_record(
            "custom:note",
            &survivor.public_id,
            survivor.version,
            context(),
        )
        .await
        .expect("undelete");
    let mut tx = store.begin(tenant).await.expect("tx");
    assert!(!tx
        .claim_purge(survivor.id, Some(after_days(16) - RECYCLE_BIN_RETENTION))
        .await
        .expect("claim"));
    tx.commit().await.expect("commit");

    // Past the window: the deleted records go, live ones stay.
    let past = purge(&store, tenant, 16).await;
    assert_eq!(past.purged, 2, "{past:?}");
    assert!(past.complete);
    for gone in [&doomed, &archived] {
        assert!(matches!(
            admin.get_record("custom:note", &gone.public_id, true).await,
            Err(ObjectError::NotFound)
        ));
        assert!(matches!(
            admin
                .record_history("custom:note", &gone.public_id, None, None)
                .await,
            Err(ObjectError::NotFound)
        ));
    }
    for live in [&kept, &survivor] {
        admin
            .get_record("custom:note", &live.public_id, false)
            .await
            .expect("live record stays");
    }
    // The change log forgets them; live records keep their entries.
    let changes = admin
        .list_changes(None, Some(200), None)
        .await
        .expect("changes");
    assert!(changes
        .changes
        .iter()
        .all(|(_, change)| change.record_id != doomed.id && change.record_id != archived.id));
    assert!(changes
        .changes
        .iter()
        .any(|(_, change)| change.record_id == kept.id));
    assert!(changes.purges >= 1);
    // Search no longer finds them, even with the bin in view.
    let mut tx = store.begin(tenant).await.expect("tx");
    assert!(tx.record_by_id(doomed.id).await.expect("read").is_none());
    assert!(tx
        .referencing_all(doomed.id)
        .await
        .expect("refs")
        .is_empty());
    tx.commit().await.expect("commit");
    let mut purged = vec![doomed.id, archived.id];
    let mut purged_public = vec![doomed.public_id.clone(), archived.public_id.clone()];

    // Idempotent: a second pass finds nothing.
    let again = purge(&store, tenant, 16).await;
    assert_eq!(again.purged, 0);
    assert!(again.complete);

    // --- Batches -------------------------------------------------------------
    let total = PURGE_BATCH as usize + 5;
    for n in 0..total {
        let record = delete(create(format!("Bulk {n}")).await).await;
        purged.push(record.id);
        purged_public.push(record.public_id);
    }
    let bulk = purge(&store, tenant, 16).await;
    assert_eq!(bulk.purged, total as u64, "{bulk:?}");
    assert_eq!(bulk.batches, 2, "{bulk:?}");
    assert!(bulk.complete);
    // A pass with no time left stops after one batch and the next goes on.
    for n in 0..(PURGE_BATCH as usize + 1) {
        let record = delete(create(format!("Later {n}")).await).await;
        purged.push(record.id);
        purged_public.push(record.public_id);
    }
    let cut = purge_recycle_bin(
        store.clone(),
        tenant,
        after_days(16),
        std::time::Duration::ZERO,
    )
    .await
    .expect("purge");
    assert_eq!(cut.purged, u64::from(PURGE_BATCH));
    assert!(!cut.complete);
    let rest = purge(&store, tenant, 16).await;
    assert_eq!(rest.purged, 1);
    assert!(rest.complete);
    let batches = past.batches + bulk.batches + cut.batches + rest.batches;

    lookups(&store, tenant, &admin).await;
    relationships(&store, tenant, &admin).await;

    RecycleBinReport {
        purged,
        purged_public,
        batches,
        survivor: survivor.id,
    }
}

/// G3: undeleting a record restores the lookups its deletion cleared.
async fn lookups(store: &Arc<dyn ObjectStore>, tenant: Uuid, admin: &ObjectService) {
    let mut org = object("org", "Org");
    org.track_history = Some(true);
    admin.create_object(org, context()).await.expect("org");
    let mut person = object("person", "Person");
    person.track_history = Some(true);
    admin
        .create_object(person, context())
        .await
        .expect("person");
    for name in ["employer", "previous"] {
        let mut lookup = field(name, FieldType::Lookup);
        lookup.reference_object = Some("custom:org".into());
        lookup.track_history = true;
        admin
            .create_field("custom:person", lookup, context())
            .await
            .expect("lookup");
    }
    let org = |name: &str| {
        let admin = admin.clone();
        let values = body(json!({"name": name}));
        async move {
            admin
                .create_record("custom:org", &values, context())
                .await
                .expect("org record")
        }
    };
    let acme = org("Acme").await;
    let other = org("Other").await;
    let person = |name: &str, employer: &str, previous: Option<&str>| {
        let admin = admin.clone();
        let mut values = json!({"name": name, "custom:employer": employer});
        if let Some(previous) = previous {
            values["custom:previous"] = json!(previous);
        }
        let values = body(values);
        async move {
            admin
                .create_record("custom:person", &values, context())
                .await
                .expect("person record")
        }
    };
    let read = |id: &str| {
        let admin = admin.clone();
        let id = id.to_owned();
        async move {
            admin
                .get_record("custom:person", &id, true)
                .await
                .expect("person")
        }
    };
    // Two fields on one record, and several records.
    let ada = person("Ada", &acme.public_id, Some(&acme.public_id)).await;
    let bob = person("Bob", &acme.public_id, None).await;
    let cy = person("Cy", &acme.public_id, None).await;
    let dee = person("Dee", &acme.public_id, None).await;
    let eve = person("Eve", &acme.public_id, None).await;

    let acme = admin
        .delete_record("custom:org", &acme.public_id, acme.version, context())
        .await
        .expect("delete acme");
    // The clear stays as it was: a new version, an update, no value.
    let ada_cleared = read(&ada.public_id).await;
    assert!(ada_cleared.version > ada.version);
    assert_eq!(value(&ada_cleared, "custom:employer"), None);
    assert_eq!(value(&ada_cleared, "custom:previous"), None);

    // Bob picks another employer; Cy picks one and drops it again.
    let bob = read(&bob.public_id).await;
    admin
        .update_record(
            "custom:person",
            &bob.public_id,
            bob.version,
            &body(json!({"custom:employer": other.public_id})),
            context(),
        )
        .await
        .expect("bob moves");
    let cy = read(&cy.public_id).await;
    let cy = admin
        .update_record(
            "custom:person",
            &cy.public_id,
            cy.version,
            &body(json!({"custom:employer": other.public_id})),
            context(),
        )
        .await
        .expect("cy moves");
    admin
        .update_record(
            "custom:person",
            &cy.public_id,
            cy.version,
            &body(json!({"custom:employer": null})),
            context(),
        )
        .await
        .expect("cy leaves");
    // Dee is in the bin; Eve went there and came back, unchanged.
    let dee = read(&dee.public_id).await;
    admin
        .delete_record("custom:person", &dee.public_id, dee.version, context())
        .await
        .expect("delete dee");
    let eve = read(&eve.public_id).await;
    let eve = admin
        .delete_record("custom:person", &eve.public_id, eve.version, context())
        .await
        .expect("delete eve");
    admin
        .undelete_record("custom:person", &eve.public_id, eve.version, context())
        .await
        .expect("undelete eve");

    let web = context().with_source(MutationSource::web());
    let (restored, report) = admin
        .undelete_record_with_lookups("custom:org", &acme.public_id, acme.version, web)
        .await
        .expect("undelete acme");
    assert!(restored.deleted_at.is_none());
    assert_eq!(report.restored, 3, "{report:?}");
    assert_eq!(report.skipped_total(), 3, "{report:?}");
    assert_eq!(report.skipped.get(&SkipReason::Changed), Some(&2));
    assert_eq!(report.skipped.get(&SkipReason::Deleted), Some(&1));

    let ada_now = read(&ada.public_id).await;
    assert_eq!(
        value(&ada_now, "custom:employer"),
        Some(&json!(acme.public_id))
    );
    assert_eq!(
        value(&ada_now, "custom:previous"),
        Some(&json!(acme.public_id))
    );
    assert_eq!(
        ada_now.version,
        ada_cleared.version + 2,
        "one edit per field"
    );
    assert_eq!(
        value(&read(&eve.public_id).await, "custom:employer"),
        Some(&json!(acme.public_id))
    );
    assert_eq!(
        value(&read(&bob.public_id).await, "custom:employer"),
        Some(&json!(other.public_id))
    );
    assert_eq!(value(&read(&cy.public_id).await, "custom:employer"), None);
    // Each restore is history with the system's source.
    let (history, _) = admin
        .record_history("custom:person", &ada.public_id, None, None)
        .await
        .expect("history");
    let restores: Vec<&HistoryEntry> = history
        .entries
        .iter()
        .filter(|entry| entry.new_value == Some(json!(acme.public_id)) && entry.old_value.is_none())
        .collect();
    assert_eq!(restores.len(), 2, "{:?}", history.entries);
    assert!(restores.iter().all(|entry| entry
        .source
        .is_some_and(|source| source.kind == SourceKind::System)));
    // And a change-log update naming the field.
    let changes = admin
        .list_changes(None, Some(200), Some("custom:person"))
        .await
        .expect("changes");
    let last = changes
        .changes
        .iter()
        .rev()
        .find(|(_, change)| change.record_id == ada.id)
        .expect("ada change");
    assert_eq!(last.1.kind, ChangeKind::Updated);
    assert_eq!(last.1.fields.len(), 1);

    // What was kept went with the undelete: Dee's undelete restores
    // nothing, and a second delete and undelete starts afresh.
    let dee_bin = read(&dee.public_id).await;
    admin
        .undelete_record("custom:person", &dee.public_id, dee_bin.version, context())
        .await
        .expect("undelete dee");
    assert_eq!(value(&read(&dee.public_id).await, "custom:employer"), None);
    let mut tx = store.begin(tenant).await.expect("tx");
    assert!(tx.cleared_lookups(acme.id).await.expect("kept").is_empty());
    tx.commit().await.expect("commit");

    // Purging a record forgets what its deletion kept.
    let acme = admin
        .get_record("custom:org", &acme.public_id, false)
        .await
        .expect("acme");
    let acme = admin
        .delete_record("custom:org", &acme.public_id, acme.version, context())
        .await
        .expect("delete acme again");
    let mut tx = store.begin(tenant).await.expect("tx");
    // Ada's two fields and Eve's; Dee lost hers while in the bin.
    assert_eq!(tx.cleared_lookups(acme.id).await.expect("kept").len(), 3);
    tx.commit().await.expect("commit");
    let outcome = purge(store, tenant, 16).await;
    assert!(outcome.purged >= 1);
    let mut tx = store.begin(tenant).await.expect("tx");
    assert!(tx.cleared_lookups(acme.id).await.expect("kept").is_empty());
    tx.commit().await.expect("commit");
}

/// What else a purge does: records still pointing at the purged one lose
/// the value, and the details of a purged master go with it.
async fn relationships(store: &Arc<dyn ObjectStore>, tenant: Uuid, admin: &ObjectService) {
    admin
        .create_object(object("project", "Project"), context())
        .await
        .expect("project");
    admin
        .create_object(object("milestone", "Milestone"), context())
        .await
        .expect("milestone");
    let mut master = field("project", FieldType::MasterDetail);
    master.reference_object = Some("custom:project".into());
    admin
        .create_field("custom:milestone", master, context())
        .await
        .expect("master-detail");
    admin
        .create_object(object("ticket", "Ticket"), context())
        .await
        .expect("ticket");
    let mut about = field("project", FieldType::Lookup);
    about.reference_object = Some("custom:project".into());
    admin
        .create_field("custom:ticket", about, context())
        .await
        .expect("lookup");

    let project = admin
        .create_record(
            "custom:project",
            &body(json!({"name": "Apollo"})),
            context(),
        )
        .await
        .expect("project record");
    let milestone = admin
        .create_record(
            "custom:milestone",
            &body(json!({"name": "Launch", "custom:project": project.public_id})),
            context(),
        )
        .await
        .expect("milestone record");
    let ticket = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Bug", "custom:project": project.public_id})),
            context(),
        )
        .await
        .expect("ticket record");
    // The ticket goes to the bin first, so the project's deletion does not
    // clear it; then the project, with its milestone.
    let ticket = admin
        .delete_record(
            "custom:ticket",
            &ticket.public_id,
            ticket.version,
            context(),
        )
        .await
        .expect("delete ticket");
    admin
        .delete_record(
            "custom:project",
            &project.public_id,
            project.version,
            context(),
        )
        .await
        .expect("delete project");
    let outcome = purge(store, tenant, 16).await;
    assert!(outcome.purged >= 3, "{outcome:?}");
    for (object, id) in [
        ("custom:project", &project.public_id),
        ("custom:milestone", &milestone.public_id),
        ("custom:ticket", &ticket.public_id),
    ] {
        assert!(
            matches!(
                admin.get_record(object, id, true).await,
                Err(ObjectError::NotFound)
            ),
            "{object} {id}"
        );
    }

    // A live record left pointing at a deleted one (undeleted while its
    // target stayed in the bin) loses the value when the target is purged.
    let project = admin
        .create_record(
            "custom:project",
            &body(json!({"name": "Gemini"})),
            context(),
        )
        .await
        .expect("project record");
    let ticket = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Task", "custom:project": project.public_id})),
            context(),
        )
        .await
        .expect("ticket record");
    let ticket = admin
        .delete_record(
            "custom:ticket",
            &ticket.public_id,
            ticket.version,
            context(),
        )
        .await
        .expect("delete ticket");
    admin
        .delete_record(
            "custom:project",
            &project.public_id,
            project.version,
            context(),
        )
        .await
        .expect("delete project");
    let ticket = admin
        .undelete_record(
            "custom:ticket",
            &ticket.public_id,
            ticket.version,
            context(),
        )
        .await
        .expect("undelete ticket");
    assert_eq!(
        value(&ticket, "custom:project"),
        Some(&json!(project.public_id))
    );
    purge(store, tenant, 16).await;
    let ticket_now = admin
        .get_record("custom:ticket", &ticket.public_id, false)
        .await
        .expect("ticket stays");
    assert_eq!(value(&ticket_now, "custom:project"), None);
    assert!(ticket_now.version > ticket.version);
    assert_eq!(ticket_now.updated_by, None, "the platform made the change");
}
