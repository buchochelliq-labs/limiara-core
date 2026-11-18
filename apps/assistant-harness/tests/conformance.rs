//! The reference harness's own checks: the conformance suite passes for
//! both assistants and reaches everything the contract lists, and the
//! contract file stays in step with the Rust types it describes.

use assistant_harness::runner::{matches, substitute};
use assistant_harness::schema::{listed, Contract};
use chrono::Utc;
use platform_device::assistant::errors::{AssistantCode, AssistantError, FieldProblem};
use platform_device::assistant::index::{IndexEntry, IndexSweep, IndexUpdate, IndexedField};
use platform_device::assistant::*;
use platform_objects::{Assistant, FieldType, QuickActionKind};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

fn contract() -> Contract {
    Contract::bundled().expect("the contract file parses")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_conformance_suite_passes_for_both_assistants_and_covers_the_contract() {
    let report = assistant_harness::run(&[], &[Assistant::Siri, Assistant::Gemini], true)
        .await
        .expect("the suite runs");
    for (name, failure) in &report.failed {
        eprintln!("FAILED {name}\n  {failure}");
    }
    for missing in &report.missing {
        eprintln!("NOT COVERED {missing}");
    }
    assert!(report.ok(), "{} failed", report.failed.len());
    assert!(report.passed.len() >= 20, "{:?}", report.passed);
}

#[test]
fn every_script_loads() {
    let scripts = assistant_harness::script::discover(&assistant_harness::script::suite_dir())
        .expect("suite");
    assert!(scripts.len() >= 10);
    for path in scripts {
        assistant_harness::script::Script::load(&path).expect("script");
    }
}

#[test]
fn the_error_codes_and_their_sentences_match_the_bridge() {
    let contract = contract();
    let codes = listed(&contract, "ErrorCode", None);
    let expected: Vec<&str> = AssistantCode::ALL
        .iter()
        .map(|code| code.as_str())
        .collect();
    assert_eq!(codes, expected);
    let options = contract
        .definition("ErrorCode")
        .and_then(|schema| schema["oneOf"].as_array().cloned())
        .expect("options");
    for (option, code) in options.iter().zip(AssistantCode::ALL) {
        assert_eq!(option["description"], json!(code.message()), "{code}");
    }
}

#[test]
fn the_enumerations_match_the_bridge() {
    let contract = contract();
    let field_types: Vec<String> = FieldType::ALL
        .iter()
        .map(|kind| {
            serde_json::to_value(kind)
                .expect("json")
                .as_str()
                .expect("text")
                .to_owned()
        })
        .collect();
    assert_eq!(listed(&contract, "FieldType", None), field_types);
    assert_eq!(listed(&contract, "Assistant", None), ["siri", "gemini"]);
    assert_eq!(
        listed(&contract, "QuickActionKind", None),
        ["update", "create"]
    );
}

fn values() -> Map<String, Value> {
    json!({"custom:notes": "x"})
        .as_object()
        .cloned()
        .expect("values")
}

/// One of each operation. The match has no wildcard, so a new operation
/// does not compile until it has a sample here, and the contract file a
/// schema for it.
fn operations() -> Vec<Operation> {
    let all = vec![
        Operation::Describe,
        Operation::Search {
            text: "acme".into(),
            object: Some("custom:account".into()),
        },
        Operation::Get {
            object: "custom:account".into(),
            record: "r".into(),
        },
        Operation::List {
            object: "custom:account".into(),
            conditions: conditions(),
            sort: Some(Sort {
                field: "name".into(),
                descending: true,
            }),
            cursor: Some("c".into()),
        },
        Operation::Timeline {
            object: "custom:account".into(),
            record: "r".into(),
            cursor: None,
        },
        Operation::Actions {
            object: "custom:account".into(),
            record: None,
        },
        Operation::RunAction {
            object: "custom:account".into(),
            action: "note".into(),
            record: "r".into(),
            values: values(),
        },
        Operation::Create {
            object: "custom:account".into(),
            values: values(),
        },
        Operation::Update {
            object: "custom:account".into(),
            record: "r".into(),
            values: values(),
        },
        Operation::Commit { token: "t".into() },
        Operation::OnScreen,
    ];
    for operation in &all {
        match operation {
            Operation::Describe
            | Operation::Search { .. }
            | Operation::Get { .. }
            | Operation::List { .. }
            | Operation::Timeline { .. }
            | Operation::Actions { .. }
            | Operation::RunAction { .. }
            | Operation::Create { .. }
            | Operation::Update { .. }
            | Operation::Commit { .. }
            | Operation::OnScreen => {}
        }
    }
    all
}

/// Every condition test, with the same exhaustive match. Five at most go
/// in one list, so the rest are checked on their own.
fn conditions() -> Vec<Condition> {
    vec![
        Condition::Equals {
            field: "f".into(),
            value: json!(1),
        },
        Condition::Contains {
            field: "f".into(),
            value: "x".into(),
        },
        Condition::Between {
            field: "f".into(),
            from: json!(1),
            to: json!(2),
        },
        Condition::IsEmpty { field: "f".into() },
    ]
}

fn every_condition() -> Vec<Condition> {
    let all = vec![
        Condition::Equals {
            field: "f".into(),
            value: json!("x"),
        },
        Condition::NotEquals {
            field: "f".into(),
            value: Value::Null,
        },
        Condition::Contains {
            field: "f".into(),
            value: "x".into(),
        },
        Condition::Before {
            field: "f".into(),
            value: json!("2026-01-01"),
        },
        Condition::After {
            field: "f".into(),
            value: json!(3),
        },
        Condition::Between {
            field: "f".into(),
            from: json!(1),
            to: json!(2),
        },
        Condition::IsEmpty { field: "f".into() },
    ];
    for condition in &all {
        match condition {
            Condition::Equals { .. }
            | Condition::NotEquals { .. }
            | Condition::Contains { .. }
            | Condition::Before { .. }
            | Condition::After { .. }
            | Condition::Between { .. }
            | Condition::IsEmpty { .. } => {}
        }
    }
    all
}

#[test]
fn every_request_the_types_make_is_valid_and_round_trips() {
    let contract = contract();
    let mut names = Vec::new();
    for operation in operations() {
        let request = AssistantRequest::new(Assistant::Gemini, true, operation);
        let value = serde_json::to_value(&request).expect("json");
        contract
            .check("AssistantRequest", &value)
            .unwrap_or_else(|error| panic!("{value}: {error}"));
        let back: AssistantRequest = serde_json::from_value(value.clone()).expect("parses");
        assert_eq!(back, request);
        names.push(
            value["operation"]["name"]
                .as_str()
                .expect("name")
                .to_owned(),
        );
    }
    assert_eq!(listed(&contract, "Operation", Some("name")), names);
    for condition in every_condition() {
        let value = serde_json::to_value(&condition).expect("json");
        contract
            .check("Condition", &value)
            .unwrap_or_else(|error| panic!("{value}: {error}"));
        let back: Condition = serde_json::from_value(value).expect("parses");
        assert_eq!(back, condition);
    }
    let mut tests: Vec<String> = every_condition()
        .iter()
        .map(|condition| {
            serde_json::to_value(condition).expect("json")["test"]
                .as_str()
                .expect("test")
                .to_owned()
        })
        .collect();
    let mut listed_tests = listed(&contract, "Condition", Some("test"));
    tests.sort();
    listed_tests.sort();
    assert_eq!(listed_tests, tests);
    // Minimal requests, with every optional field left out, parse in Rust
    // and pass the contract file.
    for minimal in [
        json!({"name": "search", "text": "x"}),
        json!({"name": "list", "object": "o"}),
        json!({"name": "timeline", "object": "o", "record": "r"}),
        json!({"name": "actions", "object": "o"}),
        json!({"name": "run_action", "object": "o", "action": "a", "record": "r"}),
    ] {
        let request = json!({"contract": "assistant.v1", "assistant": "siri",
                             "device_unlocked": true, "operation": minimal});
        contract
            .check("AssistantRequest", &request)
            .unwrap_or_else(|error| panic!("{request}: {error}"));
        serde_json::from_value::<AssistantRequest>(request).expect("parses");
    }
}

#[test]
fn what_the_contract_file_refuses_the_bridge_does_not_parse() {
    let contract = contract();
    for bad in [
        json!({"contract": "assistant.v1", "assistant": "siri", "device_unlocked": true,
               "operation": {"name": "delete", "object": "o", "record": "r"}}),
        json!({"contract": "assistant.v1", "assistant": "alexa", "device_unlocked": true,
               "operation": {"name": "describe"}}),
        json!({"contract": "assistant.v1", "assistant": "siri", "device_unlocked": true,
               "operation": {"name": "get", "object": "o", "record": "r", "extra": 1}}),
        json!({"contract": "assistant.v1", "assistant": "siri",
               "operation": {"name": "describe"}}),
        json!({"contract": "assistant.v1", "assistant": "siri", "device_unlocked": true,
               "operation": {"name": "list", "object": "o",
                             "conditions": [{"test": "like", "field": "f", "value": "%"}]}}),
        json!({"contract": "assistant.v1", "assistant": "siri", "device_unlocked": true,
               "operation": {"name": "commit", "token": 1}}),
    ] {
        assert!(contract.check("AssistantRequest", &bad).is_err(), "{bad}");
        assert!(
            serde_json::from_value::<AssistantRequest>(bad.clone()).is_err(),
            "{bad}"
        );
    }
    // The contract file is the stricter rule plugins code against: serde
    // lets an operation without fields (describe, on_screen) carry extra
    // keys, which the file refuses.
    let lenient = json!({"contract": "assistant.v1", "assistant": "siri", "device_unlocked": true,
                         "operation": {"name": "describe", "extra": 1}});
    assert!(contract.check("AssistantRequest", &lenient).is_err());
}

fn field_value() -> FieldValue {
    FieldValue {
        field: "custom:amount".into(),
        label: "Amount".into(),
        field_type: FieldType::Currency,
        value: json!(12.5),
        display: "12.50".into(),
    }
}

fn record_ref() -> RecordRef {
    RecordRef {
        object: "custom:account".into(),
        object_label: "Account".into(),
        id: "r".into(),
        name: "Acme".into(),
    }
}

fn action() -> DescribedAction {
    DescribedAction {
        name: "note".into(),
        label: "Add a note".into(),
        kind: QuickActionKind::Update,
        object: "custom:account".into(),
        target_object: "custom:account".into(),
        fields: vec!["custom:notes".into()],
    }
}

/// One of each result, with the same exhaustive match.
fn results() -> Vec<AssistantResult> {
    let entry = TimelineEntry {
        object: "custom:task".into(),
        object_label: "Task".into(),
        id: "t".into(),
        name: "Call".into(),
        date: None,
        open: true,
    };
    let all = vec![
        AssistantResult::Description(Description {
            metadata_version: 3,
            objects: vec![DescribedObject {
                api_name: "custom:account".into(),
                label: "Account".into(),
                plural_label: "Accounts".into(),
                name_label: "Name".into(),
                can_create: true,
                can_edit: false,
                writable: true,
                timeline: false,
                fields: vec![
                    DescribedField {
                        api_name: "custom:stage".into(),
                        label: "Stage".into(),
                        field_type: FieldType::Picklist,
                        required: false,
                        read_only: false,
                        options: vec![DescribedOption {
                            value: "open".into(),
                            label: "Open".into(),
                        }],
                        reference_object: None,
                    },
                    DescribedField {
                        api_name: "custom:parent".into(),
                        label: "Parent".into(),
                        field_type: FieldType::Lookup,
                        required: true,
                        read_only: true,
                        options: Vec::new(),
                        reference_object: Some("custom:account".into()),
                    },
                ],
                actions: vec![action()],
                metadata_version: 3,
            }],
        }),
        AssistantResult::SearchResults(SearchResults {
            results: vec![record_ref()],
        }),
        AssistantResult::Record(RecordView {
            object: "custom:account".into(),
            object_label: "Account".into(),
            id: "r".into(),
            name: "Acme".into(),
            version: 2,
            fields: vec![field_value()],
            metadata_version: 3,
        }),
        AssistantResult::Records(RecordPage {
            object: "custom:account".into(),
            records: vec![record_ref()],
            next_cursor: Some("next".into()),
        }),
        AssistantResult::Timeline(TimelineView {
            open: vec![entry.clone()],
            past: vec![TimelineEntry {
                date: Some("2026-11-18".into()),
                open: false,
                ..entry
            }],
            next_cursor: None,
        }),
        AssistantResult::Actions(ActionList {
            actions: vec![action()],
        }),
        AssistantResult::Confirmation(Confirmation {
            token: "t".into(),
            expires_at: Utc::now(),
            write: WriteKind::RunAction,
            object: "custom:account".into(),
            object_label: "Account".into(),
            record_name: Some("Acme".into()),
            action_label: Some("Add a note".into()),
            changes: vec![field_value()],
        }),
        AssistantResult::Committed(Committed {
            object: "custom:account".into(),
            id: "r".into(),
            name: "Acme".into(),
            version: 3,
        }),
        AssistantResult::OnScreen(OnScreen {
            workspace: Uuid::now_v7(),
            object: None,
            record: None,
        }),
    ];
    for result in &all {
        match result {
            AssistantResult::Description(_)
            | AssistantResult::SearchResults(_)
            | AssistantResult::Record(_)
            | AssistantResult::Records(_)
            | AssistantResult::Timeline(_)
            | AssistantResult::Actions(_)
            | AssistantResult::Confirmation(_)
            | AssistantResult::Committed(_)
            | AssistantResult::OnScreen(_) => {}
        }
    }
    all
}

#[test]
fn every_answer_the_types_make_is_valid() {
    let contract = contract();
    let mut kinds = Vec::new();
    for result in results() {
        let value = serde_json::to_value(AssistantAnswer::ok(result)).expect("json");
        contract
            .check("AssistantAnswer", &value)
            .unwrap_or_else(|error| panic!("{value}: {error}"));
        kinds.push(value["result"]["kind"].as_str().expect("kind").to_owned());
    }
    assert_eq!(listed(&contract, "AssistantResult", Some("kind")), kinds);
    for write in [WriteKind::Create, WriteKind::Update, WriteKind::RunAction] {
        let value = serde_json::to_value(write).expect("json");
        assert!(contract
            .definition("Confirmation")
            .and_then(|schema| schema.pointer("/properties/write/enum"))
            .and_then(Value::as_array)
            .is_some_and(|writes| writes.contains(&value)));
    }
    for code in AssistantCode::ALL {
        let mut error = AssistantError::new(code);
        let value = serde_json::to_value(AssistantAnswer::error(error.clone())).expect("json");
        contract
            .check("AssistantAnswer", &value)
            .unwrap_or_else(|failure| panic!("{value}: {failure}"));
        error.fields.push(FieldProblem {
            field: "name".into(),
            label: "Name".into(),
            message: "Name is required.".into(),
        });
        let value = serde_json::to_value(AssistantAnswer::error(error)).expect("json");
        contract
            .check("AssistantAnswer", &value)
            .unwrap_or_else(|failure| panic!("{value}: {failure}"));
    }
    // A result that leaves out a field, or adds one, is refused.
    let mut answer = serde_json::to_value(AssistantAnswer::ok(results().remove(7))).expect("json");
    answer["result"]["extra"] = json!(1);
    assert!(contract.check("AssistantAnswer", &answer).is_err());
    let mut answer = serde_json::to_value(AssistantAnswer::ok(results().remove(7))).expect("json");
    answer["result"]
        .as_object_mut()
        .expect("object")
        .remove("version");
    assert!(contract.check("AssistantAnswer", &answer).is_err());
}

#[test]
fn every_index_update_and_the_sweep_are_valid() {
    let contract = contract();
    let workspace = Uuid::now_v7();
    let all = vec![
        IndexUpdate::Upsert {
            entry: IndexEntry {
                workspace,
                object: "custom:account".into(),
                object_label: "Account".into(),
                record: "r".into(),
                name: None,
                fields: vec![IndexedField {
                    field: "custom:amount".into(),
                    label: "Amount".into(),
                    display: "1.00".into(),
                }],
                expires_at: Utc::now(),
            },
        },
        IndexUpdate::Remove {
            workspace,
            object: "custom:account".into(),
            record: "r".into(),
        },
        IndexUpdate::RemoveObject {
            workspace,
            object: "custom:account".into(),
        },
        IndexUpdate::RemoveWorkspace { workspace },
        IndexUpdate::Rebuild { workspace },
        IndexUpdate::Refresh {
            workspace,
            expires_at: Utc::now(),
        },
    ];
    for update in &all {
        match update {
            IndexUpdate::Upsert { .. }
            | IndexUpdate::Remove { .. }
            | IndexUpdate::RemoveObject { .. }
            | IndexUpdate::RemoveWorkspace { .. }
            | IndexUpdate::Rebuild { .. }
            | IndexUpdate::Refresh { .. } => {}
        }
        let value = serde_json::to_value(update).expect("json");
        contract
            .check("IndexUpdate", &value)
            .unwrap_or_else(|error| panic!("{value}: {error}"));
    }
    let sweep = serde_json::to_value(IndexSweep {
        remove_workspaces: vec![workspace],
        remove_expired_before: Utc::now(),
    })
    .expect("json");
    contract.check("IndexSweep", &sweep).expect("sweep");
}

#[test]
fn variables_and_matching_behave_as_documented() {
    let variables = BTreeMap::from([
        ("id".to_owned(), json!("a-1")),
        ("version".to_owned(), json!(3)),
    ]);
    let value = substitute(
        &json!({"/records/${id}": "${version}", "path": "/r/${id}?v=${version}"}),
        &variables,
    )
    .expect("substitute");
    assert_eq!(value, json!({"/records/a-1": 3, "path": "/r/a-1?v=3"}));
    assert!(substitute(&json!("${missing}"), &variables).is_err());
    assert!(matches(
        &json!({"a": 1, "b": [1, {"c": 2, "d": 3}]}),
        &json!({"b": [1, {"c": 2}]})
    ));
    assert!(!matches(&json!({"b": [1, 2]}), &json!({"b": [1]})));
    assert!(!matches(&json!({"a": 1}), &json!({"a": 1.0})));
}
