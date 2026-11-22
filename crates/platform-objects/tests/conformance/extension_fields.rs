//! Fields one plugin adds to another plugin's object, on every backend
//! (ADR 0035 G6). A host object declares the record resource it serves;
//! ADR 0012 `[[extensions.fields]]` for that resource become fields of the
//! object in the extending plugin's namespace, owned by it, with its read
//! and write permissions. They follow the extending plugin: out of service,
//! values kept, while it is not enabled, and back when it is. Either install
//! order works.

use super::{body, caller, context, service, ADMIN_PERMISSIONS};
use platform_objects::service::PLUGIN_OWNED;
use platform_objects::*;
use plugin_contract::Manifest;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const HOST: &str = "example.tracker";
const EXTENDER: &str = "example.loyalty";
const TICKET: &str = "example.tracker:ticket";
const TIER: &str = "example.loyalty:tier";
const NOTES: &str = "example.loyalty:notes";
const SUMMARY: &str = "example.tracker:summary";
const READ: &str = "loyalty.tier.read";
const WRITE: &str = "loyalty.tier.write";

fn host() -> Manifest {
    let mut manifest = super::plugin_objects::fixture();
    let ticket = manifest
        .objects
        .iter_mut()
        .find(|object| object.name == "ticket")
        .expect("ticket");
    ticket.resource = Some("example.tracker.ticket".into());
    manifest
}

fn extender(tier_label: &str) -> Manifest {
    Manifest::parse_toml(&format!(
        r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty tiers"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."example.tracker"]
version = ">=0.1.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.tier.read"
description = "Read loyalty tiers"
[[permissions]]
id = "loyalty.tier.write"
description = "Change loyalty tiers"

[[extensions.fields]]
host = "example.tracker"
target = "example.tracker.ticket"
key = "tier"
label = "{tier_label}"
field_type = "choice"
options = [{{ value = "gold", label = "Gold" }}, {{ value = "silver", label = "Silver" }}]
read_permission = "loyalty.tier.read"
write_permission = "loyalty.tier.write"

[[extensions.fields]]
host = "example.tracker"
target = "example.tracker.ticket"
key = "notes"
label = "Loyalty notes"
field_type = "text"
max_length = 1000
read_permission = "loyalty.tier.read"
write_permission = "loyalty.tier.write"
"#
    ))
    .expect("the extending manifest parses")
}

async fn install(store: &Arc<dyn ObjectStore>, tenant: Uuid, member: Uuid, manifest: &Manifest) {
    service(
        store,
        ObjectCaller::plugin_installer(tenant, member, manifest.id.clone()),
    )
    .install_plugin_objects(manifest, context())
    .await
    .expect("install");
}

async fn set_enabled(
    store: &Arc<dyn ObjectStore>,
    tenant: Uuid,
    member: Uuid,
    plugin: &str,
    on: bool,
) {
    let installer = service(
        store,
        ObjectCaller::plugin_installer(tenant, member, plugin),
    );
    if on {
        installer
            .resume_plugin_objects(plugin, context())
            .await
            .expect("resume");
    } else {
        installer
            .suspend_plugin_objects(plugin, context())
            .await
            .expect("suspend");
    }
}

async fn field(admin: &ObjectService, api_name: &str) -> Option<FieldDefinition> {
    admin
        .describe(TICKET)
        .await
        .expect("ticket")
        .0
        .field(api_name)
        .cloned()
}

/// The extending plugin first; `tenant_b` installs the host first.
pub async fn run(store: Arc<dyn ObjectStore>, tenant_a: Uuid, tenant_b: Uuid) {
    let member = Uuid::now_v7();
    let mut permissions = ADMIN_PERMISSIONS.to_vec();
    permissions.extend([READ, WRITE]);
    let admin = service(&store, caller(tenant_a, member, &permissions, &[]));

    // --- The extending plugin before any object serves its target --------
    install(&store, tenant_a, member, &extender("Loyalty tier")).await;
    set_enabled(&store, tenant_a, member, EXTENDER, true).await;

    // --- The host arrives: the fields join the object it serves ----------
    install(&store, tenant_a, member, &host()).await;
    set_enabled(&store, tenant_a, member, HOST, true).await;
    let tier = field(&admin, TIER)
        .await
        .expect("the extension joined the host object");
    assert_eq!(tier.origin, Origin::Plugin);
    assert_eq!(tier.field_type, FieldType::Picklist);
    assert_eq!(
        tier.options
            .iter()
            .map(|option| option.value.as_str())
            .collect::<Vec<_>>(),
        ["gold", "silver"]
    );
    assert_eq!(tier.read_permission.as_deref(), Some(READ));
    assert_eq!(tier.write_permission.as_deref(), Some(WRITE));
    let notes = field(&admin, NOTES).await.expect("notes");
    assert_eq!(notes.field_type, FieldType::LongText, "over 255 characters");
    assert_eq!(notes.max_length, Some(1000));

    // --- Values live on the host's record -----------------------------------
    let ticket = admin
        .create_record(
            TICKET,
            &body(json!({"name": "Printer jam", SUMMARY: "Tray 2", TIER: "gold"})),
            context(),
        )
        .await
        .expect("a ticket with a tier");
    assert_eq!(ticket.values[TIER], json!("gold"));
    // Without the extending plugin's permission the field is hidden.
    let plain = service(&store, caller(tenant_a, member, &ADMIN_PERMISSIONS, &[]));
    let seen = plain
        .get_record(TICKET, &ticket.public_id, false)
        .await
        .expect("readable");
    assert!(!seen.values.contains_key(TIER));

    // --- The extending plugin owns its fields -------------------------------
    let refused = admin
        .update_field(
            TICKET,
            TIER,
            serde_json::from_value(json!({"expected_version": tier.version, "label": "Tier"}))
                .expect("change"),
            context(),
        )
        .await
        .expect_err("administrators cannot change another plugin's field");
    assert!(matches!(refused, ObjectError::Conflict(code) if code == PLUGIN_OWNED));
    // Reinstalling the host leaves them alone.
    install(&store, tenant_a, member, &host()).await;
    assert!(field(&admin, TIER).await.is_some());

    // --- Out of service with the extending plugin, values kept --------------
    set_enabled(&store, tenant_a, member, EXTENDER, false).await;
    assert!(
        field(&admin, TIER).await.is_none(),
        "hidden while suspended"
    );
    let hidden = admin
        .get_record(TICKET, &ticket.public_id, false)
        .await
        .expect("still readable");
    assert!(!hidden.values.contains_key(TIER));
    admin
        .create_record(
            TICKET,
            &body(json!({"name": "Other", TIER: "gold"})),
            context(),
        )
        .await
        .expect_err("a suspended field takes no values");
    // Editing the record meanwhile keeps the hidden value.
    let edited = admin
        .update_record(
            TICKET,
            &ticket.public_id,
            hidden.version,
            &body(json!({SUMMARY: "Tray 3"})),
            context(),
        )
        .await
        .expect("edited");
    // An upgrade while suspended changes the field without bringing it back.
    install(&store, tenant_a, member, &extender("Loyalty level")).await;
    assert!(field(&admin, TIER).await.is_none());
    set_enabled(&store, tenant_a, member, EXTENDER, true).await;
    let back = admin
        .get_record(TICKET, &edited.public_id, false)
        .await
        .expect("back");
    assert_eq!(
        back.values[TIER],
        json!("gold"),
        "the value survived the edit"
    );
    assert_eq!(back.values[SUMMARY], json!("Tray 3"));
    assert_eq!(
        field(&admin, TIER).await.expect("tier").label,
        "Loyalty level",
        "the upgrade applied"
    );

    // --- The other order: the host first, then the extending plugin ---------
    let admin_b = service(&store, caller(tenant_b, member, &permissions, &[]));
    install(&store, tenant_b, member, &host()).await;
    set_enabled(&store, tenant_b, member, HOST, true).await;
    install(&store, tenant_b, member, &extender("Loyalty tier")).await;
    assert!(
        field(&admin_b, TIER).await.is_none(),
        "installed out of service until the plugin is enabled"
    );
    set_enabled(&store, tenant_b, member, EXTENDER, true).await;
    assert!(field(&admin_b, TIER).await.is_some());
}
