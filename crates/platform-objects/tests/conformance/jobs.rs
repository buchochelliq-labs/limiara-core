//! Background and scheduled automations on every backend (ADR 0022 §6):
//! jobs queued with the change, run later in their own transaction, moved
//! or cancelled by later changes, and retried with backoff.

use super::{body, caller, context, field, object, option, service, ADMIN_PERMISSIONS};
use platform_objects::automation::{run_job, JobOutcome, JOB_ATTEMPTS_MAX};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn automation(value: Value) -> CreateAutomation {
    serde_json::from_value(value).expect("automation")
}

fn first_code(error: ObjectError) -> &'static str {
    match error {
        ObjectError::Validation(issues) => issues[0].code,
        other => panic!("expected validation issues, got {other:?}"),
    }
}

async fn due(
    store: &Arc<dyn ObjectStore>,
    tenant: Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<Uuid> {
    let mut tx = store.begin(tenant).await.expect("tx");
    let ids = tx.due_jobs(now, 50).await.expect("due jobs");
    tx.commit().await.expect("commit");
    ids
}

async fn jobs(admin: &ObjectService, name: &str) -> Vec<AutomationJob> {
    admin
        .automation_jobs("custom:ticket", name, 50)
        .await
        .expect("jobs")
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    admin
        .create_object(object("ticket", "Ticket"), context())
        .await
        .expect("ticket");
    let mut status = field("status", FieldType::Picklist);
    status.options = vec![option("open", true), option("closed", false)];
    for definition in [
        status,
        field("due", FieldType::Date),
        field("escalated", FieldType::Boolean),
        field("note", FieldType::Text),
    ] {
        admin
            .create_field("custom:ticket", definition, context())
            .await
            .expect("ticket field");
    }

    // Schedules are checked when saved.
    for (input, code) in [
        (
            json!({"name": "bad", "trigger": "create", "timing": "scheduled",
                   "actions": [{"type": "update_record", "values": [
                       {"field": "custom:escalated", "formula": "true"}]}]}),
            "automations.schedule_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "scheduled",
                   "schedule": {"offset_minutes": 60, "from": "custom:note"},
                   "actions": [{"type": "update_record", "values": [
                       {"field": "custom:escalated", "formula": "true"}]}]}),
            "automations.schedule_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "async",
                   "schedule": {"offset_minutes": 60},
                   "actions": [{"type": "update_record", "values": [
                       {"field": "custom:escalated", "formula": "true"}]}]}),
            "automations.schedule_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "delete", "timing": "scheduled",
                   "schedule": {"offset_minutes": 60},
                   "actions": [{"type": "update_record", "values": [
                       {"field": "custom:escalated", "formula": "true"}]}]}),
            "automations.trigger_invalid",
        ),
    ] {
        assert_eq!(
            first_code(
                admin
                    .create_automation("custom:ticket", automation(input), context())
                    .await
                    .expect_err("invalid schedule")
            ),
            code
        );
    }
    for definition in [
        json!({"name": "note_async", "trigger": "create", "timing": "async",
               "condition": "!CONTAINS(name, 'Bad')",
               "actions": [{"type": "update_record", "values": [
                   {"field": "custom:note", "formula": "'queued for ' & name"}]}]}),
        json!({"name": "escalate", "trigger": "create_or_update", "timing": "scheduled",
               "condition": "ISPICKVAL(custom:status, 'open')",
               "schedule": {"offset_minutes": 60, "from": "custom:due"},
               "actions": [{"type": "update_record", "values": [
                   {"field": "custom:escalated", "formula": "true"}]}]}),
        json!({"name": "fail_async", "trigger": "create", "timing": "async",
               "condition": "CONTAINS(name, 'Bad')",
               "actions": [{"type": "update_record", "values": [
                   {"field": "custom:note", "formula": "'fail ' & name"}]}]}),
    ] {
        admin
            .create_automation("custom:ticket", automation(definition), context())
            .await
            .expect("background automation");
    }
    let rule: CreateValidationRule = serde_json::from_value(json!({
        "name": "no_fail", "condition": "BEGINS(custom:note, 'fail')", "message": "No failing notes"}))
    .expect("rule");
    admin
        .create_validation_rule("custom:ticket", rule, context())
        .await
        .expect("rule");

    // The save queues the jobs; nothing runs until they do.
    let today = chrono::Utc::now().date_naive();
    let in_two_days = today + chrono::Days::new(2);
    let ticket = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Printer", "custom:due": in_two_days.to_string()})),
            context(),
        )
        .await
        .expect("ticket");
    assert!(!ticket.values.contains_key("custom:note"));
    let queued = jobs(&admin, "note_async").await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].status, JobStatus::Pending);
    let escalation = jobs(&admin, "escalate").await;
    assert_eq!(escalation.len(), 1);
    let due_at = in_two_days.and_hms_opt(1, 0, 0).expect("time").and_utc();
    assert_eq!(escalation[0].due_at, due_at);

    // Running a due job changes the record as it is now, as the member who
    // made the change, and records the run.
    let now = chrono::Utc::now();
    let ready = due(&store, tenant, now).await;
    assert_eq!(ready, vec![queued[0].id], "only the background job is due");
    assert_eq!(
        run_job(store.clone(), tenant, queued[0].id, now)
            .await
            .expect("run"),
        JobOutcome::Succeeded
    );
    assert_eq!(
        run_job(store.clone(), tenant, queued[0].id, now)
            .await
            .expect("again"),
        JobOutcome::NotClaimed,
        "a finished job is not run twice"
    );
    let read = admin
        .get_record("custom:ticket", &ticket.public_id, false)
        .await
        .expect("ticket");
    assert_eq!(read.values["custom:note"], json!("queued for Printer"));
    assert_eq!(read.updated_by, ticket.created_by);
    let runs = admin
        .automation_runs("custom:ticket", "note_async", None, 10)
        .await
        .expect("runs")
        .0;
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].outcome, runs[0].saves), (RunOutcome::Succeeded, 1));
    assert_eq!(
        jobs(&admin, "note_async").await[0].status,
        JobStatus::Succeeded
    );

    // A scheduled job waits for its time, moves with its date field, and is
    // cancelled when the condition stops holding.
    assert_eq!(
        run_job(store.clone(), tenant, escalation[0].id, now)
            .await
            .expect("early"),
        JobOutcome::NotClaimed
    );
    let in_three_days = today + chrono::Days::new(3);
    let moved = admin
        .update_record(
            "custom:ticket",
            &ticket.public_id,
            read.version,
            &body(json!({"custom:due": in_three_days.to_string()})),
            context(),
        )
        .await
        .expect("move the date");
    let escalation = jobs(&admin, "escalate").await;
    assert_eq!(escalation.len(), 1, "the pending job moved");
    assert_eq!(
        escalation[0].due_at,
        in_three_days.and_hms_opt(1, 0, 0).expect("time").and_utc()
    );
    let later = escalation[0].due_at + chrono::TimeDelta::minutes(1);
    assert_eq!(
        run_job(store.clone(), tenant, escalation[0].id, later)
            .await
            .expect("on time"),
        JobOutcome::Succeeded
    );
    let escalated = admin
        .get_record("custom:ticket", &ticket.public_id, false)
        .await
        .expect("ticket");
    assert_eq!(escalated.values["custom:escalated"], json!(true));
    let other = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Scanner", "custom:due": in_two_days.to_string()})),
            context(),
        )
        .await
        .expect("second ticket");
    admin
        .update_record(
            "custom:ticket",
            &other.public_id,
            other.version,
            &body(json!({"custom:status": "closed"})),
            context(),
        )
        .await
        .expect("close it");
    let cancelled: Vec<JobStatus> = jobs(&admin, "escalate")
        .await
        .into_iter()
        .filter(|job| job.record == other.public_id)
        .map(|job| job.status)
        .collect();
    assert_eq!(cancelled, [JobStatus::Cancelled]);
    let _ = moved;

    // A failing job is retried with backoff and fails for good after its
    // last attempt; an administrator can retry it or cancel it.
    let bad = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Bad ticket"})),
            context(),
        )
        .await
        .expect("bad ticket");
    let failing = jobs(&admin, "fail_async").await;
    let job_id = failing[0].id;
    // Stores keep microseconds.
    let mut at = chrono::SubsecRound::trunc_subsecs(chrono::Utc::now(), 6);
    for attempt in 1..=JOB_ATTEMPTS_MAX {
        let outcome = run_job(store.clone(), tenant, job_id, at)
            .await
            .expect("attempt");
        let job = jobs(&admin, "fail_async").await.remove(0);
        assert_eq!(job.attempts, attempt);
        if attempt < JOB_ATTEMPTS_MAX {
            assert_eq!(outcome, JobOutcome::Retrying);
            assert_eq!(job.status, JobStatus::Pending);
            assert_eq!(job.due_at, at + automation::retry_delay(attempt));
            at = job.due_at;
        } else {
            assert_eq!(outcome, JobOutcome::Failed);
            assert_eq!(job.status, JobStatus::Failed);
            assert!(job
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("No failing notes")));
        }
    }
    let failed_runs = admin
        .automation_runs("custom:ticket", "fail_async", None, 10)
        .await
        .expect("runs")
        .0;
    assert_eq!(failed_runs.len(), JOB_ATTEMPTS_MAX as usize);
    assert!(failed_runs
        .iter()
        .all(|run| run.outcome == RunOutcome::Failed));
    assert!(!admin
        .get_record("custom:ticket", &bad.public_id, false)
        .await
        .expect("bad")
        .values
        .contains_key("custom:note"));
    let retried = admin
        .change_automation_job("custom:ticket", "fail_async", job_id, true, context())
        .await
        .expect("retry");
    assert_eq!((retried.status, retried.attempts), (JobStatus::Pending, 0));
    let cancelled = admin
        .change_automation_job("custom:ticket", "fail_async", job_id, false, context())
        .await
        .expect("cancel");
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(matches!(
        admin
            .change_automation_job("custom:ticket", "fail_async", job_id, true, context())
            .await,
        Err(ObjectError::Conflict("automations.job_state"))
    ));

    // A job whose automation was switched off is cancelled when it comes up.
    admin
        .create_record("custom:ticket", &body(json!({"name": "Mouse"})), context())
        .await
        .expect("third ticket");
    let pending = jobs(&admin, "note_async")
        .await
        .into_iter()
        .find(|job| job.status == JobStatus::Pending)
        .expect("pending job");
    let note_async = admin
        .automations("custom:ticket")
        .await
        .expect("list")
        .into_iter()
        .find(|automation| automation.name == "note_async")
        .expect("note_async");
    let off: UpdateAutomation =
        serde_json::from_value(json!({"expected_version": note_async.version, "active": false}))
            .expect("off");
    admin
        .update_automation("custom:ticket", "note_async", off, context())
        .await
        .expect("switch off");
    assert_eq!(
        run_job(store.clone(), tenant, pending.id, chrono::Utc::now())
            .await
            .expect("run"),
        JobOutcome::Cancelled
    );

    // A job queued by a condition on what changed still runs: the
    // condition held when it was queued and is not checked again.
    admin
        .create_automation(
            "custom:ticket",
            automation(
                json!({"name": "on_status", "trigger": "update", "timing": "async",
                   "condition": "ISCHANGED(custom:status)",
                   "actions": [{"type": "update_record", "values": [
                       {"field": "custom:note", "formula": "'status ' & TEXT(custom:status)"}]}]}),
            ),
            context(),
        )
        .await
        .expect("change-triggered background automation");
    let changing = admin
        .create_record(
            "custom:ticket",
            &body(json!({"name": "Keyboard", "custom:status": "open"})),
            context(),
        )
        .await
        .expect("ticket");
    admin
        .update_record(
            "custom:ticket",
            &changing.public_id,
            changing.version,
            &body(json!({"custom:status": "closed"})),
            context(),
        )
        .await
        .expect("status change");
    let queued = jobs(&admin, "on_status").await;
    assert_eq!(queued.len(), 1);
    assert_eq!(
        run_job(store.clone(), tenant, queued[0].id, chrono::Utc::now())
            .await
            .expect("run"),
        JobOutcome::Succeeded
    );
    let changed = admin
        .get_record("custom:ticket", &changing.public_id, false)
        .await
        .expect("ticket");
    assert_eq!(changed.values["custom:note"], json!("status closed"));

    // A worker whose claim lapsed cannot complete the job once another
    // worker has claimed it again.
    admin
        .create_record("custom:ticket", &body(json!({"name": "Laptop"})), context())
        .await
        .expect("ticket");
    let pending = jobs(&admin, "note_async")
        .await
        .into_iter()
        .find(|job| job.status == JobStatus::Pending)
        .expect("pending job");
    let start = chrono::Utc::now();
    let mut tx = store.begin(tenant).await.expect("tx");
    let first = tx
        .claim_job(pending.id, start, start + chrono::TimeDelta::seconds(1))
        .await
        .expect("claim")
        .expect("claimed");
    let later = start + chrono::TimeDelta::minutes(20);
    let second = tx
        .claim_job(pending.id, later, later + chrono::TimeDelta::minutes(10))
        .await
        .expect("reclaim")
        .expect("reclaimed after the lock lapsed");
    assert_eq!(second.attempts, first.attempts + 1);
    let mut stale = first.clone();
    stale.status = JobStatus::Succeeded;
    assert!(
        !tx.update_job(&stale, JobStatus::Running)
            .await
            .expect("update"),
        "the lapsed claim loses"
    );
    let mut current = second.clone();
    current.status = JobStatus::Succeeded;
    assert!(tx
        .update_job(&current, JobStatus::Running)
        .await
        .expect("update"));
    tx.commit().await.expect("commit");

    // Members cannot see or change jobs.
    let member = service(&store, caller(tenant, Uuid::now_v7(), &[], &[]));
    assert!(matches!(
        member
            .automation_jobs("custom:ticket", "escalate", 10)
            .await,
        Err(ObjectError::PermissionDenied)
    ));
}
