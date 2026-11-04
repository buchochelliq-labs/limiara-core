//! The demonstration workspace a signed-out member can open from the
//! sign-in screen: an encrypted replica on the device with one object and a
//! few records, so the shell's navigation, lists and record pages work with
//! no server. Its key is held in memory only, so it starts afresh each time.

use platform_device::{device_databases, Bridge, DeviceHost, Member, MemoryKeys, Workspace};
use platform_objects::store::sqlite::SqliteObjectStore;
use platform_objects::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use uuid::Uuid;

/// The demonstration workspace's fixed id, so each launch replaces the last.
pub const WORKSPACE: Uuid = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001);
const MEMBER: Uuid = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0002);
const ROLE: Uuid = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0003);

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

fn values(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn text_field(name: &str, label: &str) -> CreateField {
    CreateField {
        name: name.into(),
        label: label.into(),
        field_type: FieldType::Text,
        description: None,
        required: false,
        unique: false,
        indexed: false,
        searchable: true,
        track_history: false,
        max_length: None,
        scale: None,
        options: Vec::new(),
        default_value: None,
        reference_object: None,
        delete_behaviour: None,
        formula: None,
        formula_type: None,
        rollup: None,
        lookup_filter: None,
    }
}

/// Opens the demonstration workspace under `directory`, replacing any
/// left by an earlier launch.
pub async fn open(directory: &Path) -> Result<Bridge, ObjectError> {
    let files = device_databases(directory, Arc::new(MemoryKeys::new()));
    // A file from an earlier launch was written under a key that ended
    // with that process.
    files
        .wipe(WORKSPACE)
        .await
        .map_err(|_| ObjectError::Conflict("storage.key_unavailable"))?;
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(files));

    // What a sync would have brought down: definitions, the member's
    // object permissions and their records.
    let setup = ObjectService::new(
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
    setup
        .create_object(
            CreateObject {
                name: "account".into(),
                label: "Account".into(),
                plural_label: "Accounts".into(),
                description: Some("Companies you work with.".into()),
                name_label: None,
                sharing_model: Some(SharingModel::Private),
                grant_hierarchy: None,
                track_history: None,
                searchable: Some(true),
                in_navigation: Some(true),
            },
            context(),
        )
        .await?;
    setup
        .create_field("custom:account", text_field("city", "City"), context())
        .await?;
    setup
        .create_field("custom:account", text_field("notes", "Notes"), context())
        .await?;
    setup
        .set_object_permission(
            "custom:account",
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

    let engine = ObjectService::new(
        store,
        ObjectCaller::new(WORKSPACE, MEMBER, BTreeSet::new(), vec![ROLE]),
    )
    .with_replica()
    .with_client_ids();
    for (name, city) in [
        ("Analytical Engines Ltd", "London"),
        ("Difference Works", "Manchester"),
        ("Jacquard Looms", "Lyon"),
    ] {
        engine
            .create_record(
                "custom:account",
                &values(json!({"name": name, "custom:city": city})),
                context(),
            )
            .await?;
    }
    Ok(Bridge::new(DeviceHost::new(
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
    )))
}
