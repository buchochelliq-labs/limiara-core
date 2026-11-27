//! The demonstration workspace a signed-out member can open from the
//! sign-in screen: an encrypted replica on the device, laid out as a sync
//! would leave it, so the shell works with no server. Its key is held in
//! memory only, so it starts afresh each time.
//!
//! It shows what a workspace can hold:
//!
//! - the Sales plugin's leads and opportunities, from its own manifest, on
//!   their paths and boards, with probability following the stage;
//! - the Activities plugin's tasks and events, on the Activity timeline of
//!   the accounts, contacts, leads and opportunities they are about, with
//!   Log a call on each;
//! - two workspace objects: accounts, with many field types, tags, a
//!   formula, tracked history and two quick actions, and contacts, with a
//!   validation rule;
//! - lookups and the related lists they give: an account's contacts and
//!   opportunities.
//!
//! Dates are set from today, so the pipeline and the timeline always look
//! current.

use chrono::{Duration, NaiveDate, Utc};
use platform_device::{device_databases, DeviceHost, Member, MemoryKeys, Workspace};
use platform_objects::store::sqlite::SqliteObjectStore;
use platform_objects::*;
use plugin_contract::Manifest;
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

/// The demonstration workspace's fixed id, so each launch replaces the last.
pub const WORKSPACE: Uuid = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001);
const MEMBER: Uuid = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0002);
const ROLE: Uuid = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0003);

const SALES: &str = "platform.sales";
const ACTIVITIES: &str = "platform.activities";
const ACCOUNT: &str = "custom:account";
const CONTACT: &str = "custom:contact";
const LEAD: &str = "platform.sales:lead";
const OPPORTUNITY: &str = "platform.sales:opportunity";
const TASK: &str = "platform.activities:task";
const EVENT: &str = "platform.activities:event";

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

fn values(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// An engine input written as the API takes it. The inputs below are
/// fixed, so a refusal here is a mistake in this file.
fn input<T: DeserializeOwned>(value: Value) -> Result<T, ObjectError> {
    serde_json::from_value(value).map_err(|_| ObjectError::Invariant)
}

fn manifest(text: &str) -> Result<Manifest, ObjectError> {
    Manifest::parse_toml(text).map_err(|_| ObjectError::Invariant)
}

fn day(today: NaiveDate, offset: i64) -> String {
    (today + Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

/// Opens the demonstration workspace under `directory`, replacing any
/// left by an earlier launch, as the host the controller's bridge serves.
pub async fn open(directory: &Path) -> Result<DeviceHost, ObjectError> {
    let files = device_databases(directory, Arc::new(MemoryKeys::new()));
    // A file from an earlier launch was written under a key that ended
    // with that process.
    files
        .wipe(WORKSPACE)
        .await
        .map_err(|_| ObjectError::Conflict("storage.key_unavailable"))?;
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(files));

    define(&store).await?;

    let engine = ObjectService::new(
        store,
        ObjectCaller::new(WORKSPACE, MEMBER, BTreeSet::new(), vec![ROLE]),
    )
    .with_replica()
    .with_client_ids();
    fill(&engine).await?;

    Ok(DeviceHost::new(
        engine,
        Member {
            display_name: "Demo member".into(),
            email: None,
            permissions: BTreeSet::new(),
            roles: BTreeSet::from(["Member".to_owned()]),
        },
        Workspace {
            name: "Demonstration".into(),
        },
    ))
}

/// What a sync would have brought down: the plugins' objects, the
/// workspace's own, and the member's object permissions.
async fn define(store: &Arc<dyn ObjectStore>) -> Result<(), ObjectError> {
    let admin = ObjectService::new(
        store.clone(),
        ObjectCaller::new(
            WORKSPACE,
            MEMBER,
            [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            vec![ROLE],
        ),
    );

    // The plugins, from the manifests they ship.
    for (plugin, text) in [
        (SALES, include_str!("../../../../plugins/sales/plugin.toml")),
        (
            ACTIVITIES,
            include_str!("../../../../plugins/activities/plugin.toml"),
        ),
    ] {
        let installer = ObjectService::new(
            store.clone(),
            ObjectCaller::plugin_installer(WORKSPACE, MEMBER, plugin),
        );
        installer
            .install_plugin_objects(&manifest(text)?, context())
            .await?;
        installer.resume_plugin_objects(plugin, context()).await?;
    }

    // Accounts: the companies the workspace works with.
    admin
        .create_object(
            input(json!({
                "name": "account", "label": "Account", "plural_label": "Accounts",
                "description": "Companies you work with.",
                "sharing_model": "private", "track_history": true,
                "searchable": true, "in_navigation": true,
            }))?,
            context(),
        )
        .await?;
    for field in [
        json!({"name": "industry", "label": "Industry", "field_type": "picklist", "indexed": true,
               "track_history": true, "options": [
                   {"value": "engineering", "label": "Engineering"},
                   {"value": "manufacturing", "label": "Manufacturing"},
                   {"value": "textiles", "label": "Textiles"},
                   {"value": "software", "label": "Software"}]}),
        json!({"name": "city", "label": "City", "field_type": "text", "searchable": true}),
        json!({"name": "phone", "label": "Phone", "field_type": "phone", "searchable": true}),
        json!({"name": "website", "label": "Website", "field_type": "url"}),
        json!({"name": "annual_revenue", "label": "Annual revenue", "field_type": "currency",
               "scale": 2, "track_history": true}),
        json!({"name": "customer_since", "label": "Customer since", "field_type": "date"}),
        json!({"name": "key_account", "label": "Key account", "field_type": "boolean",
               "track_history": true}),
        json!({"name": "tags", "label": "Tags", "field_type": "text_list", "indexed": true}),
        json!({"name": "notes", "label": "Notes", "field_type": "long_text"}),
        json!({"name": "segment", "label": "Segment", "field_type": "formula", "formula_type": "text",
               "formula": "IF(custom:annual_revenue >= 1000000, 'Enterprise', 'Growth')"}),
    ] {
        admin
            .create_field(ACCOUNT, input(field)?, context())
            .await?;
    }

    // Contacts: the people at those accounts.
    admin
        .create_object(
            input(json!({
                "name": "contact", "label": "Contact", "plural_label": "Contacts",
                "description": "People at the accounts you work with.",
                "sharing_model": "read_write", "searchable": true, "in_navigation": true,
            }))?,
            context(),
        )
        .await?;
    for field in [
        json!({"name": "account", "label": "Account", "field_type": "lookup",
               "reference_object": ACCOUNT, "delete_behaviour": "clear"}),
        json!({"name": "title", "label": "Title", "field_type": "text"}),
        json!({"name": "email", "label": "Email", "field_type": "email", "searchable": true}),
        json!({"name": "phone", "label": "Phone", "field_type": "phone", "searchable": true}),
        json!({"name": "tags", "label": "Tags", "field_type": "text_list", "indexed": true}),
    ] {
        admin
            .create_field(CONTACT, input(field)?, context())
            .await?;
    }
    admin
        .create_validation_rule(
            CONTACT,
            input(json!({
                "name": "reachable", "description": "A contact can be reached.",
                "condition": "AND(ISBLANK(custom:email), ISBLANK(custom:phone))",
                "message": "Give an email address or a phone number.",
            }))?,
            context(),
        )
        .await?;

    // Opportunities belong to an account, and show their weighted amount.
    for field in [
        json!({"name": "account", "label": "Account", "field_type": "lookup",
               "reference_object": ACCOUNT, "delete_behaviour": "clear"}),
        json!({"name": "weighted_amount", "label": "Weighted amount", "field_type": "formula",
               "formula_type": "currency",
               "formula": "platform.sales:amount * platform.sales:probability / 100"}),
    ] {
        admin
            .create_field(OPPORTUNITY, input(field)?, context())
            .await?;
    }

    // Tasks and events can be about accounts and contacts too, which gives
    // them the Activity timeline and Log a call.
    for object in [TASK, EVENT] {
        let (described, _) = admin.describe(object).await?;
        for (field, extra) in [("related_to", ACCOUNT), ("who", CONTACT)] {
            let name = format!("{ACTIVITIES}:{field}");
            let current = described.field(&name).ok_or(ObjectError::Invariant)?;
            let mut allowed = current.reference_objects.clone();
            allowed.push(extra.to_owned());
            admin
                .update_field(
                    object,
                    &name,
                    input(
                        json!({"expected_version": current.version, "reference_objects": allowed}),
                    )?,
                    context(),
                )
                .await?;
        }
    }

    // Quick actions on an account.
    admin
        .create_quick_action(
            ACCOUNT,
            input(json!({
                "name": "new_contact", "label": "New contact", "kind": "create",
                "target_object": CONTACT, "target_field": "custom:account",
                "fields": ["name", "custom:title", "custom:email", "custom:phone"],
            }))?,
            context(),
        )
        .await?;
    admin
        .create_quick_action(
            ACCOUNT,
            input(json!({
                "name": "mark_key_account", "label": "Mark as key account", "kind": "update",
                "fields": ["custom:key_account", "custom:notes"],
                "presets": {"custom:key_account": true},
            }))?,
            context(),
        )
        .await?;

    // The member works with every record of every object.
    for object in [ACCOUNT, CONTACT, LEAD, OPPORTUNITY, TASK, EVENT] {
        admin
            .set_object_permission(
                object,
                ROLE,
                AccessFlags {
                    read: true,
                    create: true,
                    edit: true,
                    delete: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await?;
    }
    Ok(())
}

/// The member's records, as the replica would hold them.
async fn fill(engine: &ObjectService) -> Result<(), ObjectError> {
    let today = Utc::now().date_naive();
    let create = |object: &'static str, body: Value| async move {
        engine.create_record(object, &values(body), context()).await
    };

    // --- Accounts --------------------------------------------------------
    let analytical = create(
        ACCOUNT,
        json!({
            "name": "Analytical Engines Ltd", "custom:city": "London",
            "custom:industry": "engineering", "custom:annual_revenue": 2_100_000,
            "custom:phone": "+44 20 7946 0018", "custom:website": "https://analytical.example",
            "custom:customer_since": day(today, -540), "custom:key_account": false,
            "custom:tags": ["Key account", "Engines"],
        }),
    )
    .await?;
    let difference = create(
        ACCOUNT,
        json!({
            "name": "Difference Works", "custom:city": "Manchester",
            "custom:industry": "manufacturing", "custom:annual_revenue": 850_000,
            "custom:website": "https://difference.example", "custom:tags": ["Prospect"],
        }),
    )
    .await?;
    let jacquard = create(
        ACCOUNT,
        json!({
            "name": "Jacquard Looms", "custom:city": "Lyon",
            "custom:industry": "textiles", "custom:annual_revenue": 1_300_000,
            "custom:customer_since": day(today, -210), "custom:tags": ["Textiles", "Europe"],
        }),
    )
    .await?;
    let hollerith = create(
        ACCOUNT,
        json!({
            "name": "Hollerith Tabulating", "custom:city": "New York",
            "custom:industry": "software", "custom:annual_revenue": 640_000,
            "custom:tags": ["Prospect", "Americas"],
        }),
    )
    .await?;
    // An edit after the fact, so the account's history shows changes.
    engine
        .update_record(
            ACCOUNT,
            &analytical.public_id,
            analytical.version,
            &values(json!({"custom:annual_revenue": 2_400_000, "custom:key_account": true})),
            context(),
        )
        .await?;

    // --- Contacts --------------------------------------------------------
    let ada = create(
        CONTACT,
        json!({
            "name": "Ada Lovelace", "custom:account": analytical.public_id,
            "custom:title": "Head of analysis", "custom:email": "ada@analytical.example",
            "custom:tags": ["Decision maker"],
        }),
    )
    .await?;
    create(
        CONTACT,
        json!({
            "name": "Charles Babbage", "custom:account": analytical.public_id,
            "custom:title": "Founder", "custom:phone": "+44 20 7946 0019",
        }),
    )
    .await?;
    let joseph = create(
        CONTACT,
        json!({
            "name": "Joseph Marie Jacquard", "custom:account": jacquard.public_id,
            "custom:title": "Managing director", "custom:email": "joseph@jacquard.example",
            "custom:tags": ["Decision maker", "French"],
        }),
    )
    .await?;
    let herman = create(
        CONTACT,
        json!({
            "name": "Herman Hollerith", "custom:account": hollerith.public_id,
            "custom:title": "Chief engineer", "custom:email": "herman@hollerith.example",
        }),
    )
    .await?;

    // --- Leads, along their path ----------------------------------------
    for (name, company, status, source, rating) in [
        ("Grace Hopper", "Compiler Systems", "new", "web", "hot"),
        (
            "Alan Turing",
            "Bletchley Computing",
            "working",
            "referral",
            "hot",
        ),
        ("Konrad Zuse", "Zuse Rechner", "nurturing", "event", "warm"),
        (
            "Tommy Flowers",
            "Colossus Post Office",
            "qualified",
            "partner",
            "warm",
        ),
        (
            "John Napier",
            "Logarithm Tables",
            "unqualified",
            "other",
            "cold",
        ),
    ] {
        create(
            LEAD,
            json!({
                "name": name, "platform.sales:company": company,
                "platform.sales:status": status, "platform.sales:source": source,
                "platform.sales:rating": rating,
            }),
        )
        .await?;
    }

    // --- Opportunities, across the pipeline ------------------------------
    let mut opportunities = Vec::new();
    for (name, account, stage, amount, close_in) in [
        (
            "Analytical Engine upgrade",
            &analytical,
            "negotiation",
            180_000,
            14,
        ),
        (
            "Punched-card service plan",
            &analytical,
            "closed_won",
            42_000,
            -20,
        ),
        ("Loom automation pilot", &jacquard, "proposal", 96_000, 30),
        (
            "Difference Engine No. 2",
            &difference,
            "qualification",
            260_000,
            60,
        ),
        (
            "Census tabulators",
            &hollerith,
            "needs_analysis",
            120_000,
            45,
        ),
        ("Mill retrofit", &difference, "closed_lost", 55_000, -35),
        ("Card reader rollout", &hollerith, "prospecting", 38_000, 90),
    ] {
        let mut body = json!({
            "name": name, "custom:account": account.public_id,
            "platform.sales:stage": stage, "platform.sales:amount": amount,
            "platform.sales:close_date": day(today, close_in),
        });
        if stage == "closed_lost" {
            body["platform.sales:loss_reason"] = json!("price");
        }
        opportunities.push(create(OPPORTUNITY, body).await?);
    }
    // A deal moves along its path, so its history shows the stage change
    // and probability follows it.
    let upgrade = &opportunities[0];
    engine
        .update_record(
            OPPORTUNITY,
            &upgrade.public_id,
            upgrade.version,
            &values(json!({"platform.sales:next_step": "Send the revised order form"})),
            context(),
        )
        .await?;

    // --- Tasks and events, on the timelines ------------------------------
    for (subject, related, who, status, kind, due_in) in [
        (
            "Send the upgrade proposal",
            &opportunities[0].public_id,
            Some(&ada.public_id),
            "open",
            "to_do",
            2,
        ),
        (
            "Call about the loom pilot",
            &opportunities[2].public_id,
            Some(&joseph.public_id),
            "open",
            "call",
            -1,
        ),
        (
            "Introductory call",
            &hollerith.public_id,
            Some(&herman.public_id),
            "completed",
            "call",
            -6,
        ),
        (
            "Review the service renewal",
            &analytical.public_id,
            None,
            "open",
            "to_do",
            9,
        ),
        (
            "Quarterly check-in call",
            &analytical.public_id,
            Some(&ada.public_id),
            "completed",
            "call",
            -4,
        ),
    ] {
        let mut body = json!({
            "name": subject, "platform.activities:related_to": related,
            "platform.activities:status": status, "platform.activities:type": kind,
            "platform.activities:due_date": day(today, due_in),
        });
        if let Some(who) = who {
            body["platform.activities:who"] = json!(who);
        }
        create(TASK, body).await?;
    }
    let at = |days: i64, hour: u32| {
        (today + Duration::days(days))
            .and_hms_opt(hour, 0, 0)
            .map(|time| time.and_utc().to_rfc3339())
            .unwrap_or_default()
    };
    create(EVENT, json!({
        "name": "Engine demonstration", "platform.activities:related_to": opportunities[0].public_id,
        "platform.activities:who": ada.public_id, "platform.activities:location": "London office",
        "platform.activities:starts_at": at(3, 10), "platform.activities:ends_at": at(3, 11),
    })).await?;
    create(EVENT, json!({
        "name": "Quarterly business review", "platform.activities:related_to": analytical.public_id,
        "platform.activities:who": ada.public_id, "platform.activities:location": "Video call",
        "platform.activities:starts_at": at(5, 14), "platform.activities:ends_at": at(5, 15),
    })).await?;
    create(EVENT, json!({
        "name": "Site visit in Lyon", "platform.activities:related_to": jacquard.public_id,
        "platform.activities:who": joseph.public_id, "platform.activities:location": "Lyon works",
        "platform.activities:starts_at": at(8, 9), "platform.activities:ends_at": at(8, 12),
    })).await?;
    Ok(())
}
