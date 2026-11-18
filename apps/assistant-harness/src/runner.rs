//! Runs scripts: each step against the world, every request and answer
//! checked against the contract file, every assertion checked, and what
//! the suite exercised counted.

use crate::coverage::Coverage;
use crate::schema::Contract;
use crate::script::{Call, Expect, IndexAction, Script, Step, WorldAction};
use crate::world::World;
use platform_device::assistant::index::{IndexStream, IndexUpdate};
use platform_device::assistant::CONTRACT;
use platform_device::feed::ClearReason;
use platform_objects::Assistant;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

/// The other sentences `assistant.invalid` may speak, about the request
/// as a whole rather than one field.
pub const INVALID_SENTENCES: [&str; 3] = [
    "Another record already has that value.",
    "Use at most 5 conditions.",
    "Nothing would change.",
];

/// The format of an exported transcript.
pub const TRANSCRIPT_FORMAT: &str = "assistant-harness.transcript.v1";

/// One step as it ran: what was sent and what came back.
#[derive(Debug, Clone, Serialize)]
pub struct StepRecord {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The step as the script wrote it, with variables resolved.
    pub step: Value,
    /// For a call, the exact bytes sent through `call_json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// The step's document: the bridge's answer, or what the world or
    /// index step reported.
    pub document: Value,
}

/// A script as it ran, for native plugins' own tests to replay.
#[derive(Debug, Clone, Serialize)]
pub struct Transcript {
    pub format: &'static str,
    pub contract: &'static str,
    pub script: String,
    pub description: String,
    pub assistant: &'static str,
    pub variables: BTreeMap<String, Value>,
    pub steps: Vec<StepRecord>,
}

/// A failed step, with where and why.
#[derive(Debug)]
pub struct StepFailure {
    pub step: usize,
    pub note: Option<String>,
    pub reason: String,
    pub document: Option<Value>,
}

impl std::fmt::Display for StepFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "step {}", self.step)?;
        if let Some(note) = &self.note {
            write!(formatter, " ({note})")?;
        }
        write!(formatter, ": {}", self.reason)?;
        if let Some(document) = &self.document {
            write!(formatter, "\n  document: {document}")?;
        }
        Ok(())
    }
}

/// A live session: the world, the variables, the documents kept and the
/// index stream, stepped one step at a time.
pub struct Session<'a> {
    pub world: World,
    contract: &'a Contract,
    coverage: &'a mut Coverage,
    variables: BTreeMap<String, Value>,
    documents: BTreeMap<String, Value>,
    stream: Option<IndexStream>,
    /// Which write each confirmation token was issued for.
    tokens: BTreeMap<String, String>,
    /// Record ids and hidden values no message may carry.
    secrets: Vec<String>,
}

impl<'a> Session<'a> {
    pub async fn new(
        platform: Assistant,
        contract: &'a Contract,
        coverage: &'a mut Coverage,
    ) -> anyhow::Result<Session<'a>> {
        let world = World::new(platform).await?;
        let variables = world.variables();
        let mut secrets: Vec<String> = world.records.values().cloned().collect();
        secrets.push("90000".into());
        Ok(Self {
            world,
            contract,
            coverage,
            variables,
            documents: BTreeMap::new(),
            stream: None,
            tokens: BTreeMap::new(),
            secrets,
        })
    }

    pub fn variables(&self) -> &BTreeMap<String, Value> {
        &self.variables
    }

    /// Runs one step and checks its expectations.
    pub async fn step(&mut self, step: &Step) -> Result<StepRecord, String> {
        let resolved = substitute(
            &serde_json::to_value(step).map_err(|error| error.to_string())?,
            &self.variables,
        )?;
        let step: Step = serde_json::from_value(resolved.clone()).map_err(|e| e.to_string())?;
        let (request, document) = if let Some(call) = &step.call {
            let (request, answer) = self.call(call).await?;
            (Some(request), answer)
        } else if let Some(action) = &step.world {
            (
                None,
                self.act(action).await.map_err(|error| error.to_string())?,
            )
        } else if let Some(action) = &step.index {
            (None, self.index(action).await?)
        } else {
            return Err("the step does nothing".into());
        };
        if let Some(expect) = &step.expect {
            check(expect, &document, &self.documents).map_err(|failures| failures.join("\n  "))?;
        }
        for (name, pointer) in &step.save {
            let value = document
                .pointer(pointer)
                .ok_or_else(|| format!("nothing at {pointer} to save as {name}"))?;
            self.variables.insert(name.clone(), value.clone());
        }
        if let Some(name) = &step.save_document {
            self.documents.insert(name.clone(), document.clone());
        }
        Ok(StepRecord {
            note: step.note.clone(),
            step: resolved,
            request,
            document,
        })
    }

    async fn call(&mut self, call: &Call) -> Result<(String, Value), String> {
        let bytes = match &call.raw {
            Some(raw) => raw.clone(),
            None => {
                let operation = call
                    .operation
                    .clone()
                    .ok_or("a call needs an operation or raw bytes")?;
                let request = json!({
                    "contract": call.contract.clone().unwrap_or_else(|| CONTRACT.to_owned()),
                    "assistant": call.assistant.clone().unwrap_or_else(|| {
                        self.world.platform.as_str().to_owned()
                    }),
                    "device_unlocked": call.device_unlocked.unwrap_or(true),
                    "operation": operation,
                });
                serde_json::to_string(&request).map_err(|error| error.to_string())?
            }
        };
        // The contract file accepts exactly the requests the script means
        // to be well formed.
        let parsed: Option<Value> = serde_json::from_str(&bytes).ok();
        let accepted = parsed
            .as_ref()
            .map(|value| self.contract.check("AssistantRequest", value));
        match (&accepted, call.malformed) {
            (Some(Ok(())), true) => {
                return Err(format!(
                    "the contract file accepts a malformed request: {bytes}"
                ))
            }
            (Some(Err(error)), false) => {
                return Err(format!("the contract file refuses the request: {error}"))
            }
            (None, false) => return Err("the request is not JSON; mark it malformed".into()),
            _ => {}
        }
        let answer = self.world.assistant.call_json(bytes.as_bytes()).await;
        let answer: Value = serde_json::from_slice(&answer)
            .map_err(|error| format!("the answer is not JSON: {error}"))?;
        self.contract
            .check("AssistantAnswer", &answer)
            .map_err(|error| {
                format!("the contract file refuses the answer: {error}\n  {answer}")
            })?;
        if call.malformed && answer.pointer("/error/code") != Some(&json!("assistant.invalid")) {
            return Err(format!(
                "a request the contract refuses must answer assistant.invalid: {answer}"
            ));
        }
        if let Some(error) = answer.get("error") {
            self.check_sentence(error)?;
        }
        let operation = parsed
            .as_ref()
            .and_then(|value| value.pointer("/operation"))
            .filter(|_| !call.malformed);
        self.coverage.call(operation, &answer);
        // A confirmation's write, and the commit that makes it.
        if let Some(result) = answer.get("result") {
            match result["kind"].as_str() {
                Some("confirmation") => {
                    if let (Some(token), Some(write)) =
                        (result["token"].as_str(), result["write"].as_str())
                    {
                        self.coverage.confirmed(write);
                        self.tokens.insert(token.to_owned(), write.to_owned());
                    }
                }
                Some("committed") => {
                    let token = operation
                        .and_then(|operation| operation["token"].as_str())
                        .unwrap_or_default();
                    if let Some(write) = self.tokens.get(token) {
                        self.coverage.committed(write);
                    }
                }
                _ => {}
            }
        }
        Ok((bytes, answer))
    }

    /// Every error speaks its code's fixed sentence, or one of the forms
    /// filled with labels only, and never an id, a hidden value or a code.
    fn check_sentence(&mut self, error: &Value) -> Result<(), String> {
        let code = error["code"].as_str().unwrap_or_default();
        let message = error["message"].as_str().unwrap_or_default();
        let fixed = crate::coverage::sentence(code);
        let filled = |prefix: &str| {
            message
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(" in Limiara."))
                .is_some_and(|label| !label.is_empty() && !label.contains(':'))
        };
        let allowed = Some(message) == fixed
            || (code == "assistant.object_not_exposed" && filled("Assistants can't reach "))
            || (code == "assistant.not_writable" && filled("Assistants can't change "))
            || (code == "assistant.invalid" && INVALID_SENTENCES.contains(&message));
        if !allowed {
            return Err(format!("{code} speaks an unknown sentence: {message}"));
        }
        let spoken = error.to_string();
        for forbidden in self
            .secrets
            .iter()
            .map(String::as_str)
            .chain(["assistant."])
        {
            if message.contains(forbidden)
                || error["fields"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|field| {
                        field["message"]
                            .as_str()
                            .unwrap_or_default()
                            .contains(forbidden)
                    })
            {
                return Err(format!("an error carries {forbidden}: {spoken}"));
            }
        }
        self.coverage.sentence(code, message);
        Ok(())
    }

    async fn act(&mut self, action: &WorldAction) -> anyhow::Result<Value> {
        self.coverage.world(action);
        let world = &mut self.world;
        let done = json!({});
        Ok(match action {
            WorldAction::Sync => {
                let report = world.sync().await?;
                for failed in report["failed"].as_array().into_iter().flatten() {
                    if let Some(code) = failed["code"].as_str() {
                        self.coverage.pushed_code(code);
                    }
                }
                report
            }
            WorldAction::PushQueue => world.push_queue().await?,
            WorldAction::BeginSignOut => {
                world.begin_close(ClearReason::SignedOut)?;
                done
            }
            WorldAction::SignOut => {
                world.close(ClearReason::SignedOut).await?;
                done
            }
            WorldAction::Wipe => {
                world.close(ClearReason::Wiped).await?;
                done
            }
            WorldAction::SignIn => {
                world.sign_in().await?;
                done
            }
            WorldAction::StorageUnavailable => {
                world.storage_unavailable()?;
                done
            }
            WorldAction::DeviceSettings {
                assistant_access,
                indexing,
            } => {
                let service = world.services.assistant();
                let mut settings = service.device_settings();
                if let Some(access) = assistant_access {
                    settings.assistant_access = *access;
                }
                if let Some(indexing) = indexing {
                    settings.indexing = *indexing;
                }
                service.set_device_settings(settings);
                serde_json::to_value(service.device_settings())?
            }
            WorldAction::SetSwitch { enabled } => {
                world.set_switch(*enabled).await?;
                done
            }
            WorldAction::SetPolicy { object, change } => {
                world.set_policy(object, change).await?;
                done
            }
            WorldAction::Relabel { object, label } => {
                world.relabel(object, label).await?;
                done
            }
            WorldAction::ServerUpdate {
                object,
                record,
                values,
            } => world.server_update(object, record, values).await?,
            WorldAction::ServerRecord { object, record } => {
                world.server_record(object, record).await?
            }
            WorldAction::ServerHistory { object, record } => {
                world.history(true, object, record).await?
            }
            WorldAction::DeviceHistory { object, record } => {
                world.history(false, object, record).await?
            }
            WorldAction::Web { method, path, body } => {
                world.web(method, path, body.as_ref()).await?
            }
            WorldAction::SetOfflineLimit { seconds } => {
                world.set_offline_limit(*seconds).await?;
                done
            }
            WorldAction::AdvanceClock { seconds } => {
                world.clock.advance(*seconds);
                done
            }
            WorldAction::CheckLease => world.check_lease().await?,
        })
    }

    async fn index(&mut self, action: &IndexAction) -> Result<Value, String> {
        match action {
            IndexAction::Subscribe => {
                self.stream = Some(self.world.assistant.index_stream());
                self.coverage.index_action("subscribe");
                Ok(json!({}))
            }
            IndexAction::Drain => {
                let stream = self
                    .stream
                    .as_mut()
                    .ok_or("subscribe to the index stream first")?;
                let ready = stream.ready().await;
                let mut updates = Vec::new();
                let mut refreshes = 0;
                for update in &ready {
                    let value = serde_json::to_value(update).map_err(|error| error.to_string())?;
                    self.contract
                        .check("IndexUpdate", &value)
                        .map_err(|error| format!("the contract file refuses {value}: {error}"))?;
                    self.coverage.index_update(&value);
                    if matches!(update, IndexUpdate::Refresh { .. }) {
                        refreshes += 1;
                    } else {
                        updates.push(value);
                    }
                }
                self.coverage.index_action("drain");
                Ok(json!({"updates": updates, "refreshes": refreshes}))
            }
            IndexAction::Sweep { indexed } => {
                let indexed: Vec<Uuid> = indexed
                    .iter()
                    .map(|text| Uuid::parse_str(text).map_err(|error| error.to_string()))
                    .collect::<Result<_, _>>()?;
                let sweep = self.world.assistant.sweep(&indexed);
                let value = serde_json::to_value(sweep).map_err(|error| error.to_string())?;
                self.contract
                    .check("IndexSweep", &value)
                    .map_err(|error| format!("the contract file refuses {value}: {error}"))?;
                self.coverage.index_action("sweep");
                Ok(value)
            }
        }
    }
}

/// Runs one script in a world of its own.
pub async fn run_script(
    script: &Script,
    platform: Assistant,
    contract: &Contract,
    coverage: &mut Coverage,
) -> Result<Transcript, StepFailure> {
    let mut session = Session::new(platform, contract, coverage)
        .await
        .map_err(|error| StepFailure {
            step: 0,
            note: Some("the world".into()),
            reason: error.to_string(),
            document: None,
        })?;
    let variables = session.variables().clone();
    let mut steps = Vec::new();
    for (index, step) in script.steps.iter().enumerate() {
        match session.step(step).await {
            Ok(record) => steps.push(record),
            Err(reason) => {
                return Err(StepFailure {
                    step: index + 1,
                    note: step.note.clone(),
                    reason,
                    document: None,
                })
            }
        }
    }
    Ok(Transcript {
        format: TRANSCRIPT_FORMAT,
        contract: CONTRACT,
        script: script.name.clone(),
        description: script.description.clone(),
        assistant: platform.as_str(),
        variables,
        steps,
    })
}

/// Replaces `${name}` in every string (and in object keys) by the
/// variable: a string that is only `${name}` becomes the variable's value
/// whatever its type; elsewhere it is spliced in as text.
pub fn substitute(value: &Value, variables: &BTreeMap<String, Value>) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => {
            if let Some(name) = text
                .strip_prefix("${")
                .and_then(|rest| rest.strip_suffix('}'))
                .filter(|name| !name.contains('}') && !name.contains("${"))
            {
                variables
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("no variable {name}"))?
            } else {
                Value::String(splice(text, variables)?)
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| substitute(item, variables))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                out.insert(splice(key, variables)?, substitute(item, variables)?);
            }
            Value::Object(out)
        }
        other => other.clone(),
    })
}

fn splice(text: &str, variables: &BTreeMap<String, Value>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("unclosed variable in {text}"))?;
        let name = &after[..end];
        match variables.get(name) {
            Some(Value::String(value)) => out.push_str(value),
            Some(other) => out.push_str(&other.to_string()),
            None => return Err(format!("no variable {name}")),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Whether `actual` matches `expected`: objects need only the keys given,
/// arrays match element by element, everything else is equal.
pub fn matches(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected
            .iter()
            .all(|(key, value)| actual.get(key).is_some_and(|found| matches(found, value))),
        (Value::Array(actual), Value::Array(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(found, value)| matches(found, value))
        }
        _ => actual == expected,
    }
}

/// Every assertion of `expect` on `document`; the failures, if any.
pub fn check(
    expect: &Expect,
    document: &Value,
    documents: &BTreeMap<String, Value>,
) -> Result<(), Vec<String>> {
    let mut failures = Vec::new();
    let at = |pointer: &str| document.pointer(pointer);
    let mut expect_at = |pointer: &str, expected: &Value| match at(pointer) {
        Some(found) if found == expected => {}
        found => failures.push(format!(
            "{pointer}: expected {expected}, got {}",
            found.map_or("nothing".to_owned(), Value::to_string)
        )),
    };
    if let Some(kind) = &expect.kind {
        expect_at("/result/kind", &json!(kind));
    }
    if let Some(code) = &expect.error {
        expect_at("/error/code", &json!(code));
    }
    if let Some(message) = &expect.message {
        expect_at("/error/message", &json!(message));
    }
    for (pointer, expected) in &expect.equals {
        expect_at(pointer, expected);
    }
    for (pointer, expected) in &expect.matches {
        if !at(pointer).is_some_and(|found| matches(found, expected)) {
            failures.push(format!(
                "{pointer}: does not match {expected}: {}",
                at(pointer).map_or("nothing".to_owned(), Value::to_string)
            ));
        }
    }
    for (pointer, expected) in &expect.includes {
        match at(pointer).and_then(Value::as_array) {
            Some(items) if items.iter().any(|item| matches(item, expected)) => {}
            Some(_) => failures.push(format!("{pointer}: no element matches {expected}")),
            None => failures.push(format!("{pointer}: not an array")),
        }
    }
    for (pointer, expected) in &expect.excludes {
        match at(pointer).and_then(Value::as_array) {
            Some(items) if items.iter().any(|item| matches(item, expected)) => {
                failures.push(format!("{pointer}: an element matches {expected}"));
            }
            Some(_) => {}
            None => failures.push(format!("{pointer}: not an array")),
        }
    }
    for (pointer, expected) in &expect.length {
        let length = match at(pointer) {
            Some(Value::Array(items)) => Some(items.len()),
            Some(Value::String(text)) => Some(text.chars().count()),
            _ => None,
        };
        if length != Some(*expected) {
            failures.push(format!("{pointer}: length {length:?}, expected {expected}"));
        }
    }
    for pointer in &expect.present {
        if at(pointer).is_none() {
            failures.push(format!("{pointer}: absent"));
        }
    }
    for pointer in &expect.absent {
        if let Some(found) = at(pointer) {
            failures.push(format!("{pointer}: present ({found})"));
        }
    }
    let text = document.to_string();
    for forbidden in &expect.never_contains {
        if text.contains(forbidden.as_str()) {
            failures.push(format!("the document contains {forbidden}"));
        }
    }
    if let Some(kinds) = &expect.kinds {
        let found: Vec<&str> = document["updates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|update| update["kind"].as_str())
            .collect();
        if found != kinds.iter().map(String::as_str).collect::<Vec<_>>() {
            failures.push(format!("update kinds {found:?}, expected {kinds:?}"));
        }
    }
    if let Some(name) = &expect.same_as {
        match documents.get(name) {
            Some(kept) if kept == document => {}
            Some(kept) => failures.push(format!("differs from {name}: {kept}")),
            None => failures.push(format!("no document {name} was kept")),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        failures.push(format!("document: {document}"));
        Err(failures)
    }
}
