//! Contacts' engine objects on every backend (ADR 0035 slice 2). The real
//! Contacts manifest declares Person and Organisation dormant: installing
//! and enabling the plugin leaves them out of service and empty, and fields
//! other plugins add to Contacts' resources (ADR 0012) become theirs. Once
//! woken, as a workspace's cutover will (slice 5), they hold records under
//! Contacts' own public id prefixes, with the person's name composed from
//! its parts.

use super::{body, caller, context, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use plugin_contract::Manifest;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const PLUGIN: &str = "platform.contacts";
const PERSON: &str = "platform.contacts:person";
const ORGANISATION: &str = "platform.contacts:organisation";
const TIER: &str = "example.loyalty:tier";

pub fn contacts() -> Manifest {
    Manifest::parse_toml(include_str!("../../../../plugins/contacts/plugin.toml"))
        .expect("the Contacts manifest parses")
}

/// A plugin that adds a field to people, as ADR 0012 extensions do today.
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
version = ">=0.1.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.tier.read"
description = "Read loyalty tiers"
[[permissions]]
id = "loyalty.tier.write"
description = "Change loyalty tiers"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Loyalty tier"
field_type = "choice"
options = [{ value = "gold", label = "Gold" }, { value = "silver", label = "Silver" }]
read_permission = "loyalty.tier.read"
write_permission = "loyalty.tier.write"
"#,
    )
    .expect("the extending manifest parses")
}

fn installer(
    store: &Arc<dyn ObjectStore>,
    tenant: Uuid,
    member: Uuid,
    plugin: &str,
) -> ObjectService {
    service(
        store,
        ObjectCaller::plugin_installer(tenant, member, plugin),
    )
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let member = Uuid::now_v7();
    let permissions = [
        ADMIN_PERMISSIONS[0],
        ADMIN_PERMISSIONS[1],
        "loyalty.tier.read",
        "loyalty.tier.write",
    ];
    let admin = service(&store, caller(tenant, member, &permissions, &[]));
    let contacts_installer = installer(&store, tenant, member, PLUGIN);
    let loyalty_installer = installer(&store, tenant, member, "example.loyalty");

    // --- The manifest declares both objects dormant, mapped to resources -----
    let manifest = contacts();
    let declared: Vec<_> = manifest
        .objects
        .iter()
        .map(|object| {
            (
                object.name.as_str(),
                object.key_prefix.as_str(),
                object.dormant,
                object.resource.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        declared,
        [
            ("person", "per", true, Some("platform.contacts.person")),
            (
                "organisation",
                "org",
                true,
                Some("platform.contacts.organisation")
            ),
        ]
    );

    // --- Installing and enabling Contacts leaves them out of service --------
    let installed = contacts_installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    let mut created = installed.created_objects.clone();
    created.sort();
    assert_eq!(created, [ORGANISATION, PERSON]);
    contacts_installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("enable");
    for object in [PERSON, ORGANISATION] {
        assert!(
            matches!(admin.describe(object).await, Err(ObjectError::NotFound)),
            "{object} stays dormant while Contacts is enabled"
        );
        assert!(matches!(
            admin
                .create_record(object, &body(json!({"name": "Early"})), context())
                .await,
            Err(ObjectError::NotFound)
        ));
    }

    // A field another plugin adds to people joins the dormant object.
    loyalty_installer
        .install_plugin_objects(&loyalty(), context())
        .await
        .expect("install the extender");
    loyalty_installer
        .resume_plugin_objects("example.loyalty", context())
        .await
        .expect("enable the extender");

    // Upgrading Contacts again changes nothing: still dormant, nothing new.
    let again = contacts_installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("reinstall");
    assert!(again.created_objects.is_empty(), "{again:?}");
    assert!(matches!(
        admin.describe(PERSON).await,
        Err(ObjectError::NotFound)
    ));

    // --- Woken, as a cutover will, they match ADR 0035 §1 -------------------
    let mut woken = contacts_installer
        .wake_plugin_objects(PLUGIN, &[PERSON, ORGANISATION], context())
        .await
        .expect("wake");
    woken.sort();
    assert_eq!(woken, [ORGANISATION, PERSON]);

    let (person, _) = admin.describe(PERSON).await.expect("person");
    assert_eq!(person.key_prefix, "per");
    assert_eq!(person.sharing_model, SharingModel::ReadWrite);
    assert!(person.track_history && person.searchable && !person.in_navigation);
    assert_eq!(person.resource.as_deref(), Some("platform.contacts.person"));
    assert_eq!(
        person
            .name_from
            .as_ref()
            .map(|from| (from.fields.clone(), from.separator.clone())),
        Some((
            vec![
                "platform.contacts:first_name".to_owned(),
                "platform.contacts:last_name".to_owned()
            ],
            " ".to_owned()
        ))
    );
    let field = |name: &str| person.field(name).unwrap_or_else(|| panic!("{name}"));
    let last = field("platform.contacts:last_name");
    assert!(last.required && last.track_history);
    assert_eq!(last.max_length, Some(120));
    assert_eq!(
        last.governance.data_classification,
        Some(DataClassification::Personal)
    );
    let email = field("platform.contacts:email");
    assert_eq!(email.field_type, FieldType::Email);
    assert!(email.indexed && email.searchable);
    assert_eq!(email.max_length, Some(320));
    let organisation = field("platform.contacts:organisation");
    assert_eq!(organisation.field_type, FieldType::Lookup);
    assert_eq!(organisation.reference_object.as_deref(), Some(ORGANISATION));
    assert_eq!(organisation.delete_behaviour, Some(DeleteBehaviour::Clear));
    assert_eq!(
        field("platform.contacts:tags").field_type,
        FieldType::TextList
    );
    assert_eq!(
        field("platform.contacts:telephone").field_type,
        FieldType::Phone
    );
    // The extender's field, in its own namespace.
    assert_eq!(field(TIER).field_type, FieldType::Picklist);

    let (org, _) = admin.describe(ORGANISATION).await.expect("organisation");
    assert_eq!(org.key_prefix, "org");
    assert_eq!(
        org.resource.as_deref(),
        Some("platform.contacts.organisation")
    );
    assert_eq!(
        org.field("platform.contacts:email")
            .map(|field| field.governance.data_classification),
        Some(Some(DataClassification::Internal))
    );

    // --- Records carry Contacts' prefixes and a composed name ---------------
    let acme = admin
        .create_record(
            ORGANISATION,
            &body(json!({"name": "Acme", "platform.contacts:email": "hello@acme.example"})),
            context(),
        )
        .await
        .expect("organisation");
    assert!(acme.public_id.starts_with("org"), "{}", acme.public_id);
    let ada = admin
        .create_record(
            PERSON,
            &body(json!({
                "platform.contacts:first_name": "Ada",
                "platform.contacts:last_name": "Lovelace",
                "platform.contacts:organisation": acme.public_id,
                "platform.contacts:tags": ["VIP", "vip", "Speaker"],
                TIER: "gold",
            })),
            context(),
        )
        .await
        .expect("person");
    assert!(ada.public_id.starts_with("per"), "{}", ada.public_id);
    assert_eq!(ada.name, "Ada Lovelace");
    assert_eq!(
        ada.values["platform.contacts:tags"],
        json!(["VIP", "Speaker"])
    );
    let plato = admin
        .create_record(
            PERSON,
            &body(json!({"platform.contacts:last_name": "Plato"})),
            context(),
        )
        .await
        .expect("one name");
    assert_eq!(plato.name, "Plato", "the last name alone");
    assert!(
        admin
            .create_record(
                PERSON,
                &body(json!({"platform.contacts:first_name": "Ada"})),
                context()
            )
            .await
            .is_err(),
        "a person needs a last name"
    );
}
