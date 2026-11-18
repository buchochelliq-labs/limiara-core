//! What the suite exercised, against what it must: every operation,
//! condition test, result kind, error code and its sentence, write kind
//! prepared and committed, index update and world action. The lists come
//! from the contract file where it has them, so a new operation or code
//! the suite does not reach fails the run.

use crate::schema::{listed, Contract};
use crate::script::WorldAction;
use platform_device::assistant::errors::AssistantCode;
use serde_json::Value;
use std::collections::BTreeSet;

/// Codes no script can make the bridge answer on purpose:
/// `assistant.failed` is the answer to an engine fault, and
/// `assistant.not_allowed` is the server's answer to a push, which the
/// suite reaches through `sync` instead.
pub const NOT_ANSWERED: [&str; 2] = ["assistant.failed", "assistant.not_allowed"];

/// The world actions the suite must use.
pub const WORLD_ACTIONS: [&str; 14] = [
    "sync",
    "push_queue",
    "begin_sign_out",
    "sign_out",
    "wipe",
    "sign_in",
    "storage_unavailable",
    "device_settings",
    "set_switch",
    "set_policy",
    "relabel",
    "advance_clock",
    "check_lease",
    "server_history",
];

/// The fixed sentence of a code.
pub fn sentence(code: &str) -> Option<&'static str> {
    AssistantCode::ALL
        .into_iter()
        .find(|known| known.as_str() == code)
        .map(AssistantCode::message)
}

#[derive(Debug, Default, Clone)]
pub struct Coverage {
    operations: BTreeSet<String>,
    conditions: BTreeSet<String>,
    kinds: BTreeSet<String>,
    codes: BTreeSet<String>,
    /// Codes whose fixed sentence (not a filled form) was spoken.
    fixed_sentences: BTreeSet<String>,
    pushed_codes: BTreeSet<String>,
    confirmed: BTreeSet<String>,
    committed: BTreeSet<String>,
    index_updates: BTreeSet<String>,
    index_actions: BTreeSet<String>,
    world: BTreeSet<String>,
}

impl Coverage {
    pub(crate) fn call(&mut self, operation: Option<&Value>, answer: &Value) {
        if let Some(operation) = operation {
            if let Some(name) = operation["name"].as_str() {
                self.operations.insert(name.to_owned());
            }
            for condition in operation["conditions"].as_array().into_iter().flatten() {
                if let Some(test) = condition["test"].as_str() {
                    self.conditions.insert(test.to_owned());
                }
            }
        }
        if let Some(kind) = answer.pointer("/result/kind").and_then(Value::as_str) {
            self.kinds.insert(kind.to_owned());
        }
        if let Some(code) = answer.pointer("/error/code").and_then(Value::as_str) {
            self.codes.insert(code.to_owned());
        }
    }

    pub(crate) fn sentence(&mut self, code: &str, message: &str) {
        if sentence(code) == Some(message) {
            self.fixed_sentences.insert(code.to_owned());
        }
    }

    pub(crate) fn pushed_code(&mut self, code: &str) {
        self.pushed_codes.insert(code.to_owned());
    }

    pub(crate) fn confirmed(&mut self, write: &str) {
        self.confirmed.insert(write.to_owned());
    }

    pub(crate) fn committed(&mut self, write: &str) {
        self.committed.insert(write.to_owned());
    }

    pub(crate) fn index_update(&mut self, update: &Value) {
        if let Some(kind) = update["kind"].as_str() {
            self.index_updates.insert(kind.to_owned());
        }
    }

    pub(crate) fn index_action(&mut self, action: &str) {
        self.index_actions.insert(action.to_owned());
    }

    pub(crate) fn world(&mut self, action: &WorldAction) {
        if let Some(name) = serde_json::to_value(action)
            .ok()
            .and_then(|value| value["action"].as_str().map(str::to_owned))
        {
            self.world.insert(name);
        }
    }

    /// Everything the suite was meant to reach and did not.
    pub fn missing(&self, contract: &Contract) -> Vec<String> {
        let mut missing = Vec::new();
        let mut want = |what: &str, required: Vec<String>, seen: &BTreeSet<String>| {
            for item in required {
                if !seen.contains(&item) {
                    missing.push(format!("{what} {item}"));
                }
            }
        };
        want(
            "operation",
            listed(contract, "Operation", Some("name")),
            &self.operations,
        );
        want(
            "condition",
            listed(contract, "Condition", Some("test")),
            &self.conditions,
        );
        want(
            "result kind",
            listed(contract, "AssistantResult", Some("kind")),
            &self.kinds,
        );
        let codes: Vec<String> = listed(contract, "ErrorCode", None);
        want(
            "error code",
            codes
                .iter()
                .filter(|code| !NOT_ANSWERED.contains(&code.as_str()))
                .cloned()
                .collect(),
            &self.codes,
        );
        want(
            "sentence of",
            codes
                .iter()
                .filter(|code| {
                    !NOT_ANSWERED.contains(&code.as_str()) && *code != "assistant.not_writable"
                })
                .cloned()
                .collect(),
            &self.fixed_sentences,
        );
        want(
            "push refusal",
            vec!["assistant.not_allowed".to_owned()],
            &self.pushed_codes,
        );
        let writes: Vec<String> = contract
            .definition("Confirmation")
            .and_then(|schema| schema.pointer("/properties/write/enum"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        want("prepared write", writes.clone(), &self.confirmed);
        want("committed write", writes, &self.committed);
        let mut updates = Vec::new();
        for option in contract
            .definition("IndexUpdate")
            .and_then(|schema| schema.get("oneOf"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(kind) = option
                .pointer("/properties/kind/const")
                .and_then(Value::as_str)
            {
                updates.push(kind.to_owned());
            }
            for kind in option
                .pointer("/properties/kind/enum")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                updates.push(kind.to_owned());
            }
        }
        want("index update", updates, &self.index_updates);
        want(
            "index action",
            ["subscribe", "drain", "sweep"].map(str::to_owned).to_vec(),
            &self.index_actions,
        );
        want(
            "world action",
            WORLD_ACTIONS.map(str::to_owned).to_vec(),
            &self.world,
        );
        missing
    }

    /// A short account of what was reached.
    pub fn summary(&self) -> String {
        format!(
            "{} operations, {} condition tests, {} result kinds, {} error codes, \
             {} write kinds committed, {} index update kinds, {} world actions",
            self.operations.len(),
            self.conditions.len(),
            self.kinds.len(),
            self.codes.len() + self.pushed_codes.len(),
            self.committed.len(),
            self.index_updates.len(),
            self.world.len()
        )
    }
}
