//! Change sources on every backend (#233): history entries and audit
//! entries say what a change came through (the web app, an API client, a
//! device's push, an automation or a bulk job), and entries written without
//! one read as unknown.

use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::bulk::{claim_bulk_job, process_bulk_job, BulkOutcome};
use platform_objects::composite::{CompositeOutcome, CompositeRequest};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// What the store's own test checks in its audit log: each record's public
/// ID and the source its creation's audit entry should carry, `Null` for
/// one made without a source.
pub struct SourcesReport {
    pub created: Vec<(String, Value)>,
}

async fn history(service: &ObjectService, record: &str) -> Vec<(Option<String>, Value)> {
    service
        .record_history("custom:visit", record, None, None)
        .await
        .expect("history")
        .0
        .entries
        .into_iter()
        .map(|entry| {
            (
                entry.field,
                serde_json::to_value(entry.source).expect("source"),
            )
        })
        .collect()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> SourcesReport {
    let admin_caller = caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]);
    let admin = service(&store, admin_caller.clone());
    let mut visit = object("visit", "Visit");
    visit.track_history = Some(true);
    admin.create_object(visit, context()).await.expect("visit");
    let mut note = field("note", FieldType::Text);
    note.track_history = true;
    admin
        .create_field("custom:visit", note, context())
        .await
        .expect("note");

    let client = Uuid::now_v7();
    let device = Uuid::now_v7();
    let web = MutationSource::web();
    let api = MutationSource::api_client(client);
    let pushed = MutationSource::device(Some(device));
    let mut created = Vec::new();

    // The web app and an API client, as their HTTP hosts verify them.
    for (name, source) in [("By browser", web), ("By client", api)] {
        let record = admin
            .create_record(
                "custom:visit",
                &body(json!({"name": name})),
                context().with_source(source),
            )
            .await
            .expect("create");
        assert_eq!(
            history(&admin, &record.public_id).await,
            [(None, serde_json::to_value(source).expect("source"))]
        );
        created.push((
            record.public_id,
            serde_json::to_value(source).expect("json"),
        ));
    }

    // A device's push arrives as a composite request with its token; the
    // server names the device it verified.
    let outcomes = admin
        .composite(
            &serde_json::from_value::<CompositeRequest>(json!({"operations": [
                {"method": "create", "reference": "visit", "object": "custom:visit",
                 "values": {"name": "By device"}},
            ]}))
            .expect("request"),
            context().with_source(pushed),
        )
        .await
        .expect("composite");
    let CompositeOutcome::Done { record, .. } = &outcomes[0].1 else {
        panic!("pushed");
    };
    assert_eq!(
        history(&admin, &record.public_id).await,
        [(
            None,
            json!({"kind": "device", "detail": device.to_string()})
        )]
    );
    created.push((
        record.public_id.clone(),
        json!({"kind": "device", "detail": device.to_string()}),
    ));

    // An automation's change names the automation, whoever saved the
    // record. A before-save automation's values are part of the member's
    // own save and carry its source; an after-save one saves on its own.
    admin
        .create_automation(
            "custom:visit",
            serde_json::from_value(json!({
                "name": "follow_up", "trigger": "update", "timing": "after_save",
                "actions": [{"type": "create_record", "object": "custom:visit", "values": [
                    {"field": "name", "formula": "'Follow-up'"}]}]}))
            .expect("automation"),
            context(),
        )
        .await
        .expect("automation");
    let first = created[0].0.clone();
    admin
        .update_record(
            "custom:visit",
            &first,
            1,
            &body(json!({"custom:note": "Checked in"})),
            context().with_source(web),
        )
        .await
        .expect("update");
    let entries = history(&admin, &first).await;
    assert_eq!(
        entries[0],
        (Some("custom:note".into()), json!({"kind": "web"}))
    );
    let follow_up = admin
        .list_records("custom:visit", Default::default())
        .await
        .expect("list")
        .records
        .into_iter()
        .find(|record| record.name == "Follow-up")
        .expect("follow-up");
    let automated = json!({"kind": "automation", "detail": "follow_up"});
    assert_eq!(
        history(&admin, &follow_up.public_id).await,
        [(None, automated.clone())]
    );
    created.push((follow_up.public_id, automated));

    // A bulk job's rows name the job.
    let job = admin
        .submit_bulk_job(
            "custom:visit",
            BulkOperation::Insert,
            &[json!({"name": "By import"})],
            context().with_source(api),
        )
        .await
        .expect("submit");
    let claimed = claim_bulk_job(&store, tenant, job.id, chrono::Utc::now())
        .await
        .expect("claim")
        .expect("claimed");
    assert_eq!(
        process_bulk_job(store.clone(), tenant, claimed, Some(admin_caller))
            .await
            .expect("batch"),
        BulkOutcome::Completed
    );
    let imported = admin
        .list_records("custom:visit", Default::default())
        .await
        .expect("list")
        .records
        .into_iter()
        .find(|record| record.name == "By import")
        .expect("imported");
    let import = json!({"kind": "import", "detail": job.id.to_string()});
    assert_eq!(
        history(&admin, &imported.public_id).await,
        [(None, import.clone())]
    );
    created.push((imported.public_id, import));

    // A change made without a source reads as unknown, as entries written
    // before sources were recorded do.
    let unknown = admin
        .create_record("custom:visit", &body(json!({"name": "Unknown"})), context())
        .await
        .expect("create");
    assert_eq!(
        history(&admin, &unknown.public_id).await,
        [(None, Value::Null)]
    );
    created.push((unknown.public_id, Value::Null));

    SourcesReport { created }
}

/// A device's replica (SQLite only): its own changes record `device`
/// without being told, and the bridge names the device when it knows it.
pub async fn replica(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let device = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    )
    .with_replica()
    .with_client_ids();
    let mut visit = object("visit", "Visit");
    visit.track_history = Some(true);
    device.create_object(visit, context()).await.expect("visit");
    let local = device
        .create_record("custom:visit", &body(json!({"name": "Offline"})), context())
        .await
        .expect("local");
    assert_eq!(
        history(&device, &local.public_id).await,
        [(None, json!({"kind": "device"}))]
    );
    let id = Uuid::now_v7();
    let named = device
        .create_record(
            "custom:visit",
            &body(json!({"name": "Named"})),
            context().with_source(MutationSource::device(Some(id))),
        )
        .await
        .expect("named");
    assert_eq!(
        history(&device, &named.public_id).await,
        [(None, json!({"kind": "device", "detail": id.to_string()}))]
    );
}
