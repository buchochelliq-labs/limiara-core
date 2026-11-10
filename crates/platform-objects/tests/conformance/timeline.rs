//! The Activity timeline on every backend (ADR 0033 §4): the Activities
//! plugin's tasks and events listed on the records they point at, in an
//! open group (open tasks and upcoming events, soonest first) and a past
//! group (newest first, paged), read as the viewer.

use super::related_actions::activities;
use super::{body, caller, codes, context, object, service, ADMIN_PERMISSIONS};
use chrono::{Duration, Utc};
use platform_objects::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;
use uuid::Uuid;

const PLUGIN: &str = "platform.activities";
const TASK: &str = "platform.activities:task";
const EVENT: &str = "platform.activities:event";
const RELATED_TO: &str = "platform.activities:related_to";
const WHO: &str = "platform.activities:who";
const STATUS: &str = "platform.activities:status";
const DUE: &str = "platform.activities:due_date";
const STARTS: &str = "platform.activities:starts_at";

fn names(items: &[TimelineItem]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

fn day(offset: i64) -> String {
    (Utc::now().date_naive() + Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

fn at(offset: i64, time: &str) -> String {
    format!("{}T{time}Z", day(offset))
}

async fn allow(admin: &ObjectService, object: &str, field: &str, objects: &[&str]) {
    let version = admin
        .describe(object)
        .await
        .expect("describe")
        .0
        .field(field)
        .expect("field")
        .version;
    admin
        .update_field(
            object,
            field,
            serde_json::from_value(
                json!({"expected_version": version, "reference_objects": objects}),
            )
            .expect("change"),
            context(),
        )
        .await
        .expect("allowlist");
}

async fn create(member: &ObjectService, object: &str, values: Value) -> Record {
    member
        .create_record(object, &body(values), context())
        .await
        .expect("record")
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, Uuid::now_v7(), PLUGIN),
    );
    let role = Uuid::now_v7();
    let ana = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));
    let ben = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));

    installer
        .install_plugin_objects(&activities(), context())
        .await
        .expect("install");
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    for (name, label) in [
        ("account", "Account"),
        ("contact", "Contact"),
        ("deal", "Deal"),
    ] {
        admin
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    for api in [
        TASK,
        EVENT,
        "custom:account",
        "custom:contact",
        "custom:deal",
    ] {
        admin
            .set_object_permission(
                api,
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
            .expect("access");
    }
    for activity in [TASK, EVENT] {
        allow(&admin, activity, RELATED_TO, &["custom:account"]).await;
        allow(&admin, activity, WHO, &["custom:contact"]).await;
    }
    let acme = create(&ana, "custom:account", json!({"name": "Acme"})).await;
    let pat = create(&ana, "custom:contact", json!({"name": "Pat"})).await;

    // --- Nothing yet: two sources, no items, and Log a call offered -------
    let empty = ana
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("timeline");
    assert_eq!(empty.sources, 2);
    assert!(empty.open.is_empty() && empty.past.is_empty());
    assert_eq!(empty.next_cursor, None);
    assert_eq!(empty.actions, ["log_a_call"]);
    let contact = ana
        .timeline("custom:contact", &pat.public_id, None)
        .await
        .expect("contact timeline");
    assert_eq!(contact.sources, 2, "reached through Who");
    assert!(contact.actions.is_empty(), "Who has no related action");

    // --- Both groups, in order --------------------------------------------
    let task = |name: &str, extra: Value| {
        let mut values = json!({"name": name, RELATED_TO: acme.public_id});
        if let (Some(values), Some(extra)) = (values.as_object_mut(), extra.as_object()) {
            values.extend(extra.clone());
        }
        values
    };
    create(&ana, TASK, task("Prepare proposal", json!({DUE: day(2)}))).await;
    create(&ana, TASK, task("Send agenda", json!({DUE: day(1)}))).await;
    create(&ana, TASK, task("Follow up", json!({}))).await;
    create(&ana, TASK, task("Overdue review", json!({DUE: day(-2)}))).await;
    create(
        &ana,
        TASK,
        task("Signed NDA", json!({DUE: day(-1), STATUS: "completed"})),
    )
    .await;
    create(
        &ana,
        TASK,
        task(
            "Lunch with Pat",
            json!({DUE: day(-3), STATUS: "completed", WHO: pat.public_id}),
        ),
    )
    .await;
    create(
        &ana,
        EVENT,
        json!({"name": "Kick-off", STARTS: at(1, "10:00:00"), RELATED_TO: acme.public_id}),
    )
    .await;
    create(
        &ana,
        EVENT,
        json!({"name": "Intro meeting", STARTS: at(-1, "09:00:00"), RELATED_TO: acme.public_id}),
    )
    .await;
    create(
        &ana,
        EVENT,
        json!({"name": "Today's standup", STARTS: at(0, "00:00:01"), RELATED_TO: acme.public_id}),
    )
    .await;
    // A logged call has no date: it sorts by when it was created.
    let call = create(
        &ana,
        TASK,
        task(
            "Called Acme",
            json!({STATUS: "completed", "platform.activities:type": "call"}),
        ),
    )
    .await;
    // Another member's private task is theirs alone.
    create(&ben, TASK, task("Ben's private task", json!({DUE: day(1)}))).await;

    let timeline = ana
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("timeline");
    assert_eq!(
        names(&timeline.open),
        [
            "Overdue review",
            "Today's standup",
            "Send agenda",
            "Kick-off",
            "Prepare proposal",
            "Follow up"
        ],
        "open tasks and upcoming events (today counts), soonest first, then undated"
    );
    assert!(!timeline.open_more);
    assert!(timeline.open.iter().all(|item| item.open));
    assert_eq!(
        names(&timeline.past),
        [
            "Called Acme",
            "Intro meeting",
            "Signed NDA",
            "Lunch with Pat"
        ],
        "newest first, a dated record from midnight"
    );
    assert!(timeline.past.iter().all(|item| !item.open));
    assert_eq!(timeline.next_cursor, None);
    let called = &timeline.past[0];
    assert_eq!(called.id, call.public_id);
    assert_eq!(called.object, TASK);
    assert_eq!(called.object_label, "Task");
    assert_eq!(called.date, None);
    assert_eq!(called.date_field, DUE);
    assert_eq!(called.date_type, FieldType::Date);
    assert_eq!(called.through, RELATED_TO);
    assert_eq!(
        called
            .details
            .iter()
            .map(|detail| (detail.label.as_str(), detail.value.as_str()))
            .collect::<Vec<_>>(),
        [
            ("Status", "Completed"),
            ("Priority", "Normal"),
            ("Type", "Call")
        ],
        "picklists as their labels"
    );
    let kick_off = &timeline.open[3];
    assert_eq!(kick_off.object, EVENT);
    assert_eq!(kick_off.date_type, FieldType::DateTime);
    assert_eq!(kick_off.date_field, STARTS);
    assert!(kick_off.details.is_empty());
    assert_eq!(timeline.open[2].date.as_deref(), Some(day(1).as_str()));

    // --- Visibility: the viewer's sharing applies ---------------------------
    let everything = admin
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("an administrator's timeline");
    assert!(names(&everything.open).contains(&"Ben's private task"));
    let bens = ben
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("Ben's timeline");
    assert_eq!(names(&bens.open), ["Ben's private task"]);
    assert!(bens.past.is_empty());
    // A record the viewer cannot read is not found.
    let hidden = ben
        .list_records(TASK, Default::default())
        .await
        .expect("Ben's tasks")
        .records
        .remove(0);
    assert!(matches!(
        ana.timeline(TASK, &hidden.public_id, None).await,
        Err(ObjectError::NotFound)
    ));
    assert!(matches!(
        ana.timeline("custom:account", "0a0000000000000ZZ", None)
            .await,
        Err(ObjectError::NotFound)
    ));

    // --- Who vs Related to ---------------------------------------------------
    let pats = ana
        .timeline("custom:contact", &pat.public_id, None)
        .await
        .expect("Pat's timeline");
    assert!(pats.open.is_empty());
    assert_eq!(names(&pats.past), ["Lunch with Pat"]);
    assert_eq!(pats.past[0].through, WHO);
    let lunch = timeline
        .past
        .iter()
        .find(|item| item.name == "Lunch with Pat")
        .expect("lunch");
    assert_eq!(lunch.through, RELATED_TO);

    // --- No sources: an empty timeline, not an error --------------------------
    let deal = create(&ana, "custom:deal", json!({"name": "Big deal"})).await;
    let none = ana
        .timeline("custom:deal", &deal.public_id, None)
        .await
        .expect("no sources");
    assert_eq!(none.sources, 0);
    assert!(none.open.is_empty() && none.past.is_empty() && none.actions.is_empty());

    // --- Paging the past group -------------------------------------------------
    let globex = create(&ana, "custom:account", json!({"name": "Globex"})).await;
    let mut expected = BTreeSet::new();
    for index in 0..22 {
        let name = format!("Call {index:02}");
        create(
            &ana,
            TASK,
            json!({"name": name, RELATED_TO: globex.public_id, STATUS: "completed"}),
        )
        .await;
        expected.insert(name);
    }
    for (name, offset) in [("Old meeting", -30), ("Older meeting", -40)] {
        create(
            &ana,
            EVENT,
            json!({"name": name, STARTS: at(offset, "12:00:00"), RELATED_TO: globex.public_id}),
        )
        .await;
        expected.insert(name.to_owned());
    }
    let first = ana
        .timeline("custom:account", &globex.public_id, None)
        .await
        .expect("first page");
    assert_eq!(first.past.len(), 20);
    assert_eq!(first.past[0].name, "Call 21", "newest first");
    let cursor = first.next_cursor.clone().expect("more");
    let second = ana
        .timeline("custom:account", &globex.public_id, Some(&cursor))
        .await
        .expect("second page");
    assert!(
        second.open.is_empty(),
        "open comes with the first page only"
    );
    assert_eq!(second.sources, 2);
    assert_eq!(
        names(&second.past),
        ["Call 01", "Call 00", "Old meeting", "Older meeting"]
    );
    assert_eq!(second.next_cursor, None);
    let seen: BTreeSet<String> = first
        .past
        .iter()
        .chain(&second.past)
        .map(|item| item.name.clone())
        .collect();
    assert_eq!(seen, expected, "every record once");
    for refused in [
        ana.timeline("custom:account", &globex.public_id, Some("not a cursor"))
            .await,
        // A cursor belongs to its record's timeline.
        ana.timeline("custom:account", &acme.public_id, Some(&cursor))
            .await,
    ] {
        assert_eq!(
            codes(&refused.expect_err("a foreign cursor")),
            ["timeline.cursor_invalid"]
        );
    }

    // --- A value kept after its object left the allowlist still shows -----
    allow(&admin, TASK, RELATED_TO, &[]).await;
    let kept = ana
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("timeline after the allowlist changed");
    assert!(names(&kept.past).contains(&"Called Acme"));
    assert!(names(&kept.open).contains(&"Send agenda"));
    assert!(kept.actions.is_empty(), "Log a call left with the account");
    allow(&admin, TASK, RELATED_TO, &["custom:account"]).await;

    // --- A suspended plugin contributes nothing ----------------------------
    installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend");
    let suspended = ana
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("timeline while disabled");
    assert_eq!(suspended.sources, 0);
    assert!(suspended.open.is_empty() && suspended.past.is_empty());
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    let back = ana
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("timeline again");
    assert_eq!(names(&back.past), names(&timeline.past));

    // --- An upgrade that drops a declaration drops the source ---------------
    let mut upgrade = activities();
    upgrade.version = "0.2.0".parse().expect("version");
    upgrade.objects[1].timeline = None;
    installer
        .install_plugin_objects(&upgrade, context())
        .await
        .expect("upgrade");
    let tasks_only = ana
        .timeline("custom:account", &acme.public_id, None)
        .await
        .expect("timeline after the upgrade");
    assert_eq!(tasks_only.sources, 1);
    assert!(tasks_only.open.iter().all(|item| item.object == TASK));
    assert!(!names(&tasks_only.past).contains(&"Intro meeting"));
}
