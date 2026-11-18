//! The assistant bridge's errors (ADR 0034 §8 and §10): stable codes in the
//! `assistant.*` namespace and fixed sentences written to be spoken.
//!
//! A message is filled only with metadata (object and field labels, and a
//! validation rule's own message, which the member would see in the form).
//! It never carries record values, ids, problem codes or engine detail.
//! The command bridge's problem codes are mapped onto these in one table,
//! [`from_problem`].

use serde::{Serialize, Serializer};
use std::fmt;

/// Every code the bridge answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssistantCode {
    /// Signed out, never signed in, wiped, or a sign-out or wipe is in
    /// progress.
    SignedOut,
    /// Past the offline lease (`sync.lease_expired`).
    Locked,
    /// The key or storage is unavailable (`storage.key_unavailable`).
    Unavailable,
    /// The native plugin reports the device is locked.
    DeviceLocked,
    /// Assistant access is off, for the workspace or on this device.
    Disabled,
    /// The record does not exist or the member cannot read it: the same
    /// answer for both.
    NotFound,
    /// The policy does not expose the object, or the member cannot read it.
    ObjectNotExposed,
    /// The policy does not let assistants write the object.
    NotWritable,
    /// The policy does not offer the quick action.
    ActionNotOffered,
    /// The member may not do this.
    NotPermitted,
    /// The request is not valid; [`AssistantError::fields`] names the
    /// failing fields.
    Invalid,
    /// The confirmation token is used, expired, unknown or for another
    /// change.
    ConfirmationInvalid,
    /// The record or the metadata moved since the change was prepared.
    ConfirmationStale,
    /// The server's assistant policy refused a pushed write (§4).
    NotAllowed,
    /// Anything else.
    Failed,
}

impl AssistantCode {
    pub const ALL: [Self; 15] = [
        Self::SignedOut,
        Self::Locked,
        Self::Unavailable,
        Self::DeviceLocked,
        Self::Disabled,
        Self::NotFound,
        Self::ObjectNotExposed,
        Self::NotWritable,
        Self::ActionNotOffered,
        Self::NotPermitted,
        Self::Invalid,
        Self::ConfirmationInvalid,
        Self::ConfirmationStale,
        Self::NotAllowed,
        Self::Failed,
    ];

    /// The stable code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SignedOut => "assistant.signed_out",
            Self::Locked => "assistant.locked",
            Self::Unavailable => "assistant.unavailable",
            Self::DeviceLocked => "assistant.device_locked",
            Self::Disabled => "assistant.disabled",
            Self::NotFound => "assistant.not_found",
            Self::ObjectNotExposed => "assistant.object_not_exposed",
            Self::NotWritable => "assistant.not_writable",
            Self::ActionNotOffered => "assistant.action_not_offered",
            Self::NotPermitted => "assistant.not_permitted",
            Self::Invalid => "assistant.invalid",
            Self::ConfirmationInvalid => "assistant.confirmation_invalid",
            Self::ConfirmationStale => "assistant.confirmation_stale",
            Self::NotAllowed => "assistant.not_allowed",
            Self::Failed => "assistant.failed",
        }
    }

    /// The fixed sentence for the code, when it names no object.
    pub fn message(self) -> &'static str {
        match self {
            Self::SignedOut => "Sign in to Limiara on this device first.",
            Self::Locked => "Open Limiara while online so it can check your access.",
            Self::Unavailable => "Limiara can't open this workspace right now.",
            Self::DeviceLocked => "Unlock your device first.",
            Self::Disabled => "Assistant access is off for this workspace.",
            Self::NotFound => "I couldn't find that record in Limiara.",
            Self::ObjectNotExposed => "Assistants can't reach those records in Limiara.",
            Self::NotWritable => "Assistants can't change those records in Limiara.",
            Self::ActionNotOffered => "That action isn't available to assistants.",
            Self::NotPermitted => "You don't have access to do that in Limiara.",
            Self::Invalid => "Some details need to change before I can do that.",
            Self::ConfirmationInvalid => {
                "That confirmation is no longer valid. Ask again to make the change."
            }
            Self::ConfirmationStale => {
                "Something changed since you confirmed. Ask again to make the change."
            }
            Self::NotAllowed => "This workspace no longer lets assistants make that change.",
            Self::Failed => "Limiara couldn't do that right now.",
        }
    }
}

impl fmt::Display for AssistantCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for AssistantCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A field `assistant.invalid` names, by label, with its message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldProblem {
    /// The field's API name (metadata), so the plugin can ask for it again.
    pub field: String,
    pub label: String,
    pub message: String,
}

/// An error answer: a stable code, a sentence to speak or show, and for
/// `assistant.invalid` the failing fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct AssistantError {
    pub code: AssistantCode,
    pub message: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldProblem>,
}

impl AssistantError {
    pub fn new(code: AssistantCode) -> Self {
        Self {
            code,
            message: code.message().to_owned(),
            fields: Vec::new(),
        }
    }

    /// The code's sentence about one object, by its plural label.
    pub(crate) fn about(code: AssistantCode, plural_label: &str) -> Self {
        let message = match code {
            AssistantCode::ObjectNotExposed => {
                format!("Assistants can't reach {plural_label} in Limiara.")
            }
            AssistantCode::NotWritable => {
                format!("Assistants can't change {plural_label} in Limiara.")
            }
            other => other.message().to_owned(),
        };
        Self {
            code,
            message,
            fields: Vec::new(),
        }
    }

    pub(crate) fn invalid(fields: Vec<FieldProblem>) -> Self {
        Self {
            fields,
            ..Self::new(AssistantCode::Invalid)
        }
    }

    /// `assistant.invalid` for one field.
    pub(crate) fn field(field: &str, label: &str, message: String) -> Self {
        Self::invalid(vec![FieldProblem {
            field: field.to_owned(),
            label: label.to_owned(),
            message,
        }])
    }
}

/// One issue of a `validation.failed` problem.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub(crate) struct Issue {
    #[serde(default)]
    pub field: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub detail: String,
}

/// A problem the command bridge answered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Problem {
    pub status: u16,
    pub code: String,
    pub errors: Vec<Issue>,
}

/// The spoken message for one failing field. Only a validation rule's own
/// message, which an administrator wrote and the member would see in the
/// form, is passed on; everything else is a fixed sentence about the label.
pub(crate) fn issue_message(issue: &Issue, label: &str) -> String {
    match issue.code.as_str() {
        "records.validation_rule" if !issue.detail.trim().is_empty() => issue.detail.clone(),
        "records.value_required" | "records.name_required" => format!("{label} is required."),
        "records.picklist_value" => format!("Choose one of the values {label} offers."),
        "records.reference_not_found"
        | "records.reference_invalid"
        | "records.lookup_filter_mismatch" => {
            format!("Choose a record {label} can point at.")
        }
        "records.value_too_long" => format!("{label} is too long."),
        "records.field_read_only" | "records.field_not_writable" => {
            format!("{label} can't be changed.")
        }
        "records.field_unknown" | "query.field_unknown" => "That field isn't available.".into(),
        "query.field_not_indexed" | "records.filter_not_indexed" => {
            format!("Assistants can't filter on {label}.")
        }
        _ => format!("{label} isn't valid."),
    }
}

/// The table from the command bridge's problem codes to the assistant's
/// (§10). `label` names a field by its API name for `assistant.invalid`.
pub(crate) fn from_problem(problem: &Problem, label: &dyn Fn(&str) -> String) -> AssistantError {
    let code = match (problem.status, problem.code.as_str()) {
        (_, "sync.lease_expired") => AssistantCode::Locked,
        (_, "storage.key_unavailable" | "platform.unavailable") => AssistantCode::Unavailable,
        (_, "workspace.not_found") => AssistantCode::SignedOut,
        (_, "assistant.not_allowed") => AssistantCode::NotAllowed,
        (_, "records.version_conflict") => AssistantCode::ConfirmationStale,
        (_, "auth.permission_denied" | "sync.server_only" | "records.field_not_writable") => {
            AssistantCode::NotPermitted
        }
        (_, "records.duplicate_value") => {
            return AssistantError {
                message: "Another record already has that value.".into(),
                ..AssistantError::new(AssistantCode::Invalid)
            }
        }
        (_, "validation.failed") => {
            let fields = problem
                .errors
                .iter()
                .map(|issue| {
                    let label = label(&issue.field);
                    FieldProblem {
                        field: issue.field.clone(),
                        message: issue_message(issue, &label),
                        label,
                    }
                })
                .collect();
            return AssistantError::invalid(fields);
        }
        (404, _) => AssistantCode::NotFound,
        (403, _) => AssistantCode::NotPermitted,
        _ => AssistantCode::Failed,
    };
    AssistantError::new(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_and_messages_carry_no_codes() {
        for code in AssistantCode::ALL {
            assert!(code.as_str().starts_with("assistant."));
            assert!(!code.message().contains("assistant."));
            assert!(code.message().ends_with('.'));
            assert_eq!(
                serde_json::to_value(code).expect("json"),
                serde_json::json!(code.as_str())
            );
        }
    }

    #[test]
    fn problems_map_through_one_table() {
        let label = |field: &str| match field {
            "custom:notes" => "Notes".to_owned(),
            other => other.to_owned(),
        };
        let problem = |status, code: &str| Problem {
            status,
            code: code.into(),
            errors: Vec::new(),
        };
        for (status, code, expected) in [
            (409, "sync.lease_expired", AssistantCode::Locked),
            (409, "storage.key_unavailable", AssistantCode::Unavailable),
            (503, "platform.unavailable", AssistantCode::Unavailable),
            (404, "resource.unavailable", AssistantCode::NotFound),
            (403, "auth.permission_denied", AssistantCode::NotPermitted),
            (
                403,
                "records.field_not_writable",
                AssistantCode::NotPermitted,
            ),
            (403, "assistant.not_allowed", AssistantCode::NotAllowed),
            (
                409,
                "records.version_conflict",
                AssistantCode::ConfirmationStale,
            ),
            (500, "anything.else", AssistantCode::Failed),
        ] {
            assert_eq!(from_problem(&problem(status, code), &label).code, expected);
        }
        let invalid = from_problem(
            &Problem {
                status: 400,
                code: "validation.failed".into(),
                errors: vec![
                    Issue {
                        field: "custom:notes".into(),
                        code: "records.validation_rule".into(),
                        detail: "Notes must mention the region.".into(),
                    },
                    Issue {
                        field: "custom:notes".into(),
                        code: "records.value_type".into(),
                        detail: "expected text, got 12 (secret detail)".into(),
                    },
                ],
            },
            &label,
        );
        assert_eq!(invalid.code, AssistantCode::Invalid);
        assert_eq!(invalid.fields[0].message, "Notes must mention the region.");
        // Engine detail never reaches the member.
        assert_eq!(invalid.fields[1].message, "Notes isn't valid.");
        assert_eq!(invalid.fields[1].label, "Notes");
    }
}
