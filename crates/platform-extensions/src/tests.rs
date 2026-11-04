use super::*;
use serde_json::json;

const PERSON: &str = "platform.contacts.person";
const ORGANISATION: &str = "platform.contacts.organisation";
const ACCOUNT: &str = "example.billing.account";

fn host(id: &str, version: &str, resources: &[&str]) -> Manifest {
    let resources = resources
        .iter()
        .map(|resource| {
            format!(
                "[[ui.resources]]\nid = \"{resource}\"\npath = \"ui/resources/{}.json\"\n",
                resource.rsplit('.').next().unwrap_or("resource")
            )
        })
        .collect::<String>();
    Manifest::parse_toml(&format!(
        r#"
manifest_version = 1
id = "{id}"
name = "{id}"
version = "{version}"
description = "Host"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

{resources}
"#
    ))
    .expect("valid host manifest")
}

fn loyalty() -> Manifest {
    Manifest::parse_toml(
        r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty tiers"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[dependencies."example.billing"]
version = "^1.0.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.read"
description = "Read loyalty fields"
[[permissions]]
id = "loyalty.write"
description = "Change loyalty fields"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Loyalty tier"
field_type = "choice"
options = [{ value = "gold", label = "Gold" }, { value = "silver", label = "Silver" }]
read_permission = "loyalty.read"
write_permission = "loyalty.write"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "points"
label = "Points"
field_type = "number"
read_permission = "loyalty.read"
write_permission = "loyalty.write"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "nickname"
label = "Nickname"
field_type = "text"
max_length = 5
read_permission = "loyalty.read"
write_permission = "loyalty.write"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "vip"
label = "VIP"
field_type = "boolean"
read_permission = "loyalty.read"
write_permission = "loyalty.write"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.organisation"
key = "tier"
label = "Account tier"
field_type = "text"
read_permission = "loyalty.read"
write_permission = "loyalty.write"

[[extensions.fields]]
host = "example.billing"
target = "example.billing.account"
key = "member_since"
label = "Member since"
field_type = "date"
read_permission = "loyalty.read"
write_permission = "loyalty.write"
"#,
    )
    .expect("valid extension manifest")
}

fn contacts() -> Manifest {
    host("platform.contacts", "0.1.0", &[PERSON, ORGANISATION])
}

fn billing() -> Manifest {
    host("example.billing", "1.2.0", &[ACCOUNT])
}

fn catalog() -> ExtensionCatalog {
    let manifests = [contacts(), billing(), loyalty()];
    ExtensionCatalog::from_manifests(&manifests)
}

fn grants(permissions: &'static [&'static str]) -> impl Fn(&str, &str) -> bool {
    move |plugin: &str, permission: &str| {
        plugin == "example.loyalty" && permissions.contains(&permission)
    }
}

fn values(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect()
}

#[test]
fn one_plugin_extends_resources_of_two_hosts() {
    let catalog = catalog();
    assert!(catalog.paused().is_empty());
    assert_eq!(catalog.fields_for(PERSON).count(), 4);
    assert_eq!(catalog.fields_for(ORGANISATION).count(), 1);
    assert_eq!(catalog.fields_for(ACCOUNT).count(), 1);
    let ids = catalog
        .fields_for(ACCOUNT)
        .map(ActiveExtensionField::id)
        .collect::<Vec<_>>();
    assert_eq!(ids, ["example.loyalty.member_since"]);
}

#[test]
fn fields_pause_when_a_host_is_inactive_incompatible_or_drops_the_target() {
    let manifests = [contacts(), loyalty()];
    let catalog = ExtensionCatalog::from_manifests(&manifests);
    assert_eq!(catalog.fields_for(ACCOUNT).count(), 0);
    assert_eq!(catalog.paused().len(), 1);
    assert_eq!(catalog.paused()[0].reason, PauseReason::HostInactive);

    let manifests = [
        contacts(),
        host("example.billing", "2.0.0", &[ACCOUNT]),
        loyalty(),
    ];
    let catalog = ExtensionCatalog::from_manifests(&manifests);
    assert_eq!(catalog.paused()[0].reason, PauseReason::HostIncompatible);
    assert_eq!(
        catalog.fields_for(PERSON).count(),
        4,
        "other hosts stay active"
    );

    let manifests = [
        host("platform.contacts", "0.1.5", &[PERSON]),
        billing(),
        loyalty(),
    ];
    let catalog = ExtensionCatalog::from_manifests(&manifests);
    assert_eq!(catalog.paused().len(), 1);
    assert_eq!(catalog.paused()[0].target, ORGANISATION);
    assert_eq!(catalog.paused()[0].reason, PauseReason::TargetMissing);
}

#[test]
fn inactive_extension_plugins_contribute_nothing() {
    let manifests = [contacts(), billing()];
    let catalog = ExtensionCatalog::from_manifests(&manifests);
    assert!(catalog.active().is_empty());
    assert!(catalog.paused().is_empty());
}

#[test]
fn descriptors_list_only_readable_fields_and_mark_writability() {
    let catalog = catalog();
    assert!(catalog.descriptors(PERSON, &grants(&[])).is_empty());
    let read_only = catalog.descriptors(PERSON, &grants(&["loyalty.read"]));
    assert_eq!(read_only.len(), 4);
    assert!(read_only.iter().all(|descriptor| !descriptor.writable));
    let nickname = read_only
        .iter()
        .find(|descriptor| descriptor.key == "nickname")
        .expect("nickname descriptor");
    assert_eq!(nickname.max_length, Some(5));
    let writable = catalog.descriptors(PERSON, &grants(&["loyalty.read", "loyalty.write"]));
    assert!(writable.iter().all(|descriptor| descriptor.writable));
    let write_without_read = catalog.descriptors(PERSON, &grants(&["loyalty.write"]));
    assert!(write_without_read.is_empty());
}

#[test]
fn permissions_of_another_plugin_never_grant_access() {
    let catalog = catalog();
    let other_plugin = |plugin: &str, permission: &str| {
        plugin == "platform.contacts" && permission.starts_with("loyalty.")
    };
    assert!(catalog.descriptors(PERSON, &other_plugin).is_empty());
}

#[test]
fn writes_are_typed_bounded_and_permission_checked() {
    let catalog = catalog();
    let writer = grants(&["loyalty.read", "loyalty.write"]);
    let patch = catalog
        .validate_patch(
            PERSON,
            &values(&[
                ("example.loyalty.tier", json!("gold")),
                ("example.loyalty.points", json!(120)),
                ("example.loyalty.nickname", json!("  Ada ")),
                ("example.loyalty.vip", json!(true)),
            ]),
            &writer,
        )
        .expect("valid patch");
    assert_eq!(patch.target(), PERSON);
    let mut stored = ExtensionValues::new();
    patch.apply(&mut stored).expect("patch applies");
    assert_eq!(stored["example.loyalty"]["nickname"], json!("Ada"));
    assert_eq!(stored["example.loyalty"].len(), 4);

    for (id, value, code) in [
        (
            "example.loyalty.tier",
            json!("bronze"),
            "extension.value_option",
        ),
        (
            "example.loyalty.points",
            json!("12"),
            "extension.value_type",
        ),
        (
            "example.loyalty.points",
            json!(1e16),
            "extension.value_out_of_range",
        ),
        (
            "example.loyalty.nickname",
            json!("toolong"),
            "extension.value_too_long",
        ),
        (
            "example.loyalty.nickname",
            json!("a\u{7}"),
            "extension.value_unprintable",
        ),
        ("example.loyalty.vip", json!("yes"), "extension.value_type"),
        (
            "example.loyalty.unknown",
            json!("x"),
            "extension.field_unknown",
        ),
        (
            "platform.contacts.full_name",
            json!("x"),
            "extension.field_unknown",
        ),
        (
            "example.loyalty.member_since",
            json!("2026-01-01"),
            "extension.field_unknown",
        ),
    ] {
        match catalog.validate_patch(PERSON, &values(&[(id, value)]), &writer) {
            Err(ExtensionError::Invalid(issues)) => assert_eq!(issues[0].code, code, "{id}"),
            other => panic!("{id}: expected {code}, got {other:?}"),
        }
    }

    let account_patch = catalog
        .validate_patch(
            ACCOUNT,
            &values(&[("example.loyalty.member_since", json!("2024-02-29"))]),
            &writer,
        )
        .expect("valid date");
    assert_eq!(account_patch.target(), ACCOUNT);
    for invalid in ["2023-02-29", "2024-2-1", "01/02/2024"] {
        assert!(catalog
            .validate_patch(
                ACCOUNT,
                &values(&[("example.loyalty.member_since", json!(invalid))]),
                &writer,
            )
            .is_err());
    }

    assert_eq!(
        catalog.validate_patch(
            PERSON,
            &values(&[("example.loyalty.tier", json!("gold"))]),
            &grants(&["loyalty.read"]),
        ),
        Err(ExtensionError::PermissionDenied)
    );
    assert!(matches!(
        catalog.validate_patch(
            PERSON,
            &values(&[("example.loyalty.tier", json!("gold"))]),
            &grants(&["loyalty.write"]),
        ),
        Err(ExtensionError::Invalid(issues)) if issues[0].code == "extension.field_unknown"
    ));
    assert!(matches!(
        catalog.validate_patch(PERSON, &BTreeMap::new(), &writer),
        Err(ExtensionError::Invalid(issues)) if issues[0].code == "extension.values_empty"
    ));
}

#[test]
fn null_and_empty_text_clear_values_and_other_namespaces_survive() {
    let catalog = catalog();
    let writer = grants(&["loyalty.read", "loyalty.write"]);
    let mut stored: ExtensionValues = serde_json::from_value(json!({
        "example.loyalty": {"tier": "gold", "nickname": "Ada"},
        "example.disabled": {"kept": "value"}
    }))
    .expect("stored values");
    catalog
        .validate_patch(
            PERSON,
            &values(&[
                ("example.loyalty.tier", Value::Null),
                ("example.loyalty.nickname", json!("   ")),
            ]),
            &writer,
        )
        .expect("clearing patch")
        .apply(&mut stored)
        .expect("applies");
    assert!(!stored.contains_key("example.loyalty"));
    assert_eq!(stored["example.disabled"]["kept"], json!("value"));
}

#[test]
fn stored_values_are_size_bounded() {
    let catalog = catalog();
    let writer = grants(&["loyalty.read", "loyalty.write"]);
    let mut stored: ExtensionValues = BTreeMap::from([(
        "example.other".to_owned(),
        BTreeMap::from([(
            "blob".to_owned(),
            json!("x".repeat(MAX_STORED_EXTENSION_BYTES)),
        )]),
    )]);
    let error = catalog
        .validate_patch(
            PERSON,
            &values(&[("example.loyalty.vip", json!(true))]),
            &writer,
        )
        .expect("valid patch")
        .apply(&mut stored)
        .expect_err("oversized record");
    assert!(matches!(
        error,
        ExtensionError::Invalid(issues) if issues[0].code == "extension.values_too_large"
    ));
}

#[test]
fn projection_returns_only_readable_active_valid_values() {
    let catalog = catalog();
    let stored: ExtensionValues = serde_json::from_value(json!({
        "example.loyalty": {"tier": "platinum", "points": 7, "vip": false, "retired": "x"},
        "example.disabled": {"secret": "hidden"}
    }))
    .expect("stored values");
    let projected = catalog.project(PERSON, &stored, &grants(&["loyalty.read"]));
    assert_eq!(
        projected,
        BTreeMap::from([
            ("example.loyalty.points".to_owned(), json!(7)),
            ("example.loyalty.vip".to_owned(), json!(false)),
        ])
    );
    assert!(catalog.project(PERSON, &stored, &grants(&[])).is_empty());

    let without_loyalty = [contacts(), billing()];
    let inactive = ExtensionCatalog::from_manifests(&without_loyalty);
    assert!(inactive
        .project(PERSON, &stored, &grants(&["loyalty.read"]))
        .is_empty());
}

#[test]
fn filters_are_typed_and_limited_to_readable_fields() {
    let catalog = catalog();
    let reader = grants(&["loyalty.read"]);
    let filter = catalog
        .validate_filter(PERSON, "example.loyalty.tier", "gold", &reader)
        .expect("valid filter");
    assert_eq!(
        filter.containment(),
        json!({"example.loyalty": {"tier": "gold"}})
    );
    assert_eq!(filter.identity(), "example.loyalty.tier=\"gold\"");
    let number = catalog
        .validate_filter(PERSON, "example.loyalty.points", "12", &reader)
        .expect("number filter");
    assert_eq!(
        number.containment(),
        json!({"example.loyalty": {"points": 12.0}})
    );
    let boolean = catalog
        .validate_filter(PERSON, "example.loyalty.vip", "true", &reader)
        .expect("boolean filter");
    assert_eq!(
        boolean.containment(),
        json!({"example.loyalty": {"vip": true}})
    );

    for (id, raw, code) in [
        (
            "example.loyalty.tier",
            "bronze",
            "extension.filter_value_invalid",
        ),
        (
            "example.loyalty.points",
            "many",
            "extension.filter_value_invalid",
        ),
        (
            "example.loyalty.vip",
            "yes",
            "extension.filter_value_invalid",
        ),
        ("example.loyalty.unknown", "x", "extension.filter_unknown"),
    ] {
        assert!(
            matches!(
                catalog.validate_filter(PERSON, id, raw, &reader),
                Err(ExtensionError::Invalid(issues)) if issues[0].code == code
            ),
            "{id}"
        );
    }
    assert!(catalog
        .validate_filter(PERSON, "example.loyalty.tier", "gold", &grants(&[]))
        .is_err());
}

#[test]
fn plan_describes_every_field_storage_permission_and_pause() {
    let active = [contacts()];
    let plan = ExtensionPlan::for_plugin(&loyalty(), &active);
    assert_eq!(plan.plugin_id, "example.loyalty");
    assert_eq!(plan.hosts, ["example.billing", "platform.contacts"]);
    assert_eq!(plan.permissions, ["loyalty.read", "loyalty.write"]);
    assert_eq!(plan.fields.len(), 6);
    assert!(plan
        .fields
        .iter()
        .all(|field| field.storage == "host-record"));
    let paused = plan
        .fields
        .iter()
        .filter_map(|field| field.paused.map(|reason| (field.id.as_str(), reason)))
        .collect::<Vec<_>>();
    assert_eq!(
        paused,
        [("example.loyalty.member_since", PauseReason::HostInactive)]
    );
}

#[test]
fn field_ids_split_at_the_last_dot() {
    let catalog = catalog();
    assert!(catalog.field(PERSON, "example.loyalty.tier").is_some());
    assert!(catalog.field(PERSON, "example.tier").is_none());
    assert!(catalog.field(PERSON, "tier").is_none());
}
