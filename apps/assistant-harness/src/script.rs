//! The script format, `assistant-harness.v1`: a session written as JSON.
//!
//! ```json
//! {
//!   "format": "assistant-harness.v1",
//!   "name": "reads",
//!   "description": "What one script checks.",
//!   "steps": [
//!     {"call": {"operation": {"name": "get", "object": "custom:account", "record": "${acme}"}},
//!      "expect": {"kind": "record", "equals": {"/result/name": "Acme"}},
//!      "save": {"version": "/result/version"}},
//!     {"world": {"action": "sign_out"}},
//!     {"call": {"operation": {"name": "describe"}},
//!      "expect": {"error": "assistant.signed_out",
//!                 "message": "Sign in to Limiara on this device first."}}
//!   ]
//! }
//! ```
//!
//! Every step does one thing (`call`, `world` or `index`) and yields one
//! JSON document: the bridge's answer, or what the world action or index
//! step reports. `expect` asserts on that document with JSON Pointers
//! (RFC 6901), `save` keeps values from it for later steps as `${name}`,
//! and `save_document` keeps the whole document for a later `same_as`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The version of this format every script names.
pub const FORMAT: &str = "assistant-harness.v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub format: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// What the step is for, shown when it fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<Call>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<WorldAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<IndexAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<Expect>,
    /// Variables to keep, by JSON Pointer into the step's document.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub save: BTreeMap<String, String>,
    /// Keeps the whole document under this name, for `same_as`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save_document: Option<String>,
}

/// One request through `call_json`, as a native plugin sends it. The
/// harness fills `contract` (`assistant.v1`), `assistant` (the platform's)
/// and `device_unlocked` (`true`) unless the step gives them.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_unlocked: Option<bool>,
    /// Bytes sent as they are, instead of a request built from the fields
    /// above: for requests that are not even JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// The request breaks the contract on purpose. The harness checks that
    /// the contract file refuses it too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub malformed: bool,
}

/// Something that happens to the device, the server or the clock between
/// calls.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorldAction {
    /// One sync: the push queue goes up, then the server's changes come
    /// down. Reports `{"failed": [{object, record, code}], "pulled": n}`.
    Sync,
    /// The push queue as it would go up: `{"operations": [...]}`, each
    /// with its assistant marker.
    PushQueue,
    /// Sign-out starts: the bridge closes, the feed clears the workspace.
    BeginSignOut,
    /// Sign-out (finishing one begun): the replica's files go.
    SignOut,
    /// The server wiped the device: like sign-out, with reason `wiped`.
    Wipe,
    /// Signs in again (a fresh replica after sign-out or a wipe), or
    /// reopens the replica after `storage_unavailable`.
    SignIn,
    /// The bridge opens a workspace whose key cannot be had.
    StorageUnavailable,
    /// The member's settings on this device; a missing one keeps its value.
    DeviceSettings {
        #[serde(default)]
        assistant_access: Option<bool>,
        #[serde(default)]
        indexing: Option<bool>,
    },
    /// The workspace switch, on the server.
    SetSwitch { enabled: bool },
    /// An object's assistant policy on the server, merged into the current
    /// one: `exposed`, `writable`, `indexable`, `excluded_fields`,
    /// `offered_actions`.
    SetPolicy {
        object: String,
        change: Map<String, Value>,
    },
    /// Renames an object on the server, which moves the metadata version.
    Relabel { object: String, label: String },
    /// A colleague changes a record on the server.
    ServerUpdate {
        object: String,
        record: String,
        values: Map<String, Value>,
    },
    /// A record as the server holds it: `{"found": true, "values": {...}}`.
    ServerRecord { object: String, record: String },
    /// The sources the server's history records for a record:
    /// `{"sources": [...]}`.
    ServerHistory { object: String, record: String },
    /// The sources the replica's history records: `{"sources": [...]}`.
    DeviceHistory { object: String, record: String },
    /// The member does something in the web shell, through the local
    /// command bridge: `{"status": n, "body": ...}`.
    Web {
        method: String,
        path: String,
        #[serde(default)]
        body: Option<Value>,
    },
    /// The offline lease length the server gives the device.
    SetOfflineLimit { seconds: u64 },
    /// Moves the injectable clock: confirmation tokens and the lease's
    /// trusted time.
    AdvanceClock { seconds: u64 },
    /// The controller's lease check, which publishes a lock or an unlock.
    CheckLease,
}

/// The index stream (ADR 0034 §7).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndexAction {
    /// Takes the plugin's index stream, as a plugin does at launch.
    Subscribe,
    /// Every update ready now, without waiting:
    /// `{"updates": [...], "refreshes": n}`, where `updates` leaves out the
    /// `refresh` updates, which `refreshes` counts.
    Drain,
    /// The launch-time sweep for the workspaces the OS index holds.
    Sweep { indexed: Vec<String> },
}

/// Assertions on a step's document.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// `/result/kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `/error/code`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `/error/message`: the sentence spoken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Pointer to the exact value there.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub equals: BTreeMap<String, Value>,
    /// Pointer to a value the one there matches: objects need only the
    /// keys given, arrays match element by element, the rest are equal.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub matches: BTreeMap<String, Value>,
    /// Pointer to an array, and a value one of its elements matches.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub includes: BTreeMap<String, Value>,
    /// Pointer to an array, and a value none of its elements matches.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub excludes: BTreeMap<String, Value>,
    /// Pointer to an array or a string, and its length.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub length: BTreeMap<String, usize>,
    /// Pointers that must be present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub present: Vec<String>,
    /// Pointers that must be absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub absent: Vec<String>,
    /// Text that appears nowhere in the document.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub never_contains: Vec<String>,
    /// The `kind` of each index update in `/updates`, in order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<String>>,
    /// The document equals one an earlier step kept with `save_document`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub same_as: Option<String>,
}

impl Script {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let script: Script = serde_json::from_str(&text)
            .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
        anyhow::ensure!(
            script.format == FORMAT,
            "{}: format {} is not {FORMAT}",
            path.display(),
            script.format
        );
        for (index, step) in script.steps.iter().enumerate() {
            let actions = usize::from(step.call.is_some())
                + usize::from(step.world.is_some())
                + usize::from(step.index.is_some());
            anyhow::ensure!(
                actions == 1,
                "{}: step {} must have exactly one of call, world and index",
                path.display(),
                index + 1
            );
        }
        Ok(script)
    }
}

/// The scripts at `path`: one file, or every `*.json` in a directory, by
/// name. The contract file and exports are not scripts.
pub fn discover(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_file() {
        return Ok(vec![path.to_owned()]);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".schema.json"))
        })
        .collect();
    found.sort();
    Ok(found)
}

/// The directory of the conformance suite, from this crate.
pub fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/assistant-harness")
}
