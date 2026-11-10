//! Where a change came from (#233, ADR 0017 "Change source"): the web
//! app, an API client, a synced device, an automation, a quick action, a
//! device assistant, a bulk job or the platform itself.
//!
//! A source is provenance only. It is recorded with audit entries and field
//! history and never changes what a caller may do. Each entry point sets
//! it from what the server verified: the kind of session or token that
//! authenticated the request, the automation or bulk job it resolved. No
//! request field or header names a source, so a client cannot claim to be
//! an automation, an assistant or another client.
//!
//! The detail, when there is one, is an id or an API name, never free text,
//! so it carries no personal data. It is short and plain enough to keep
//! [`MutationSource`] `Copy`, like the context it travels in.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use uuid::Uuid;

/// The longest detail: an API name (63 characters) or a UUID (36).
pub const DETAIL_MAX: usize = 64;

/// What made a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// The web app, through a browser session.
    Web,
    /// An API client (ADR 0023), with its id.
    ApiClient,
    /// A synced device (ADR 0024, ADR 0029), with its id when known.
    Device,
    /// A record-triggered automation (ADR 0022), with its API name.
    Automation,
    /// A quick action (ADR 0032), with its API name. Only an entry point
    /// that resolved the action itself sets it; none does yet.
    QuickAction,
    /// A device's local assistant bridge (#232), with the assistant's name.
    /// It only ever comes from the device app, never from an HTTP request.
    Assistant,
    /// A bulk job (ADR 0023 §2), with the job's id.
    Import,
    /// The platform itself, such as a plugin's installation.
    System,
}

impl SourceKind {
    pub const ALL: [Self; 8] = [
        Self::Web,
        Self::ApiClient,
        Self::Device,
        Self::Automation,
        Self::QuickAction,
        Self::Assistant,
        Self::Import,
        Self::System,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::ApiClient => "api_client",
            Self::Device => "device",
            Self::Automation => "automation",
            Self::QuickAction => "quick_action",
            Self::Assistant => "assistant",
            Self::Import => "import",
            Self::System => "system",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// The assistants a device's assistant bridge may name (#232).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Assistant {
    Siri,
    Gemini,
}

impl Assistant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Siri => "siri",
            Self::Gemini => "gemini",
        }
    }
}

/// A source's detail: an id or API name of at most [`DETAIL_MAX`] ASCII
/// letters, digits, `_`, `-`, `.` and `:`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceDetail {
    len: u8,
    bytes: [u8; DETAIL_MAX],
}

impl SourceDetail {
    /// `None` when `value` is empty, too long or not plain.
    pub fn new(value: &str) -> Option<Self> {
        let plain = |byte: u8| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte);
        if value.is_empty() || value.len() > DETAIL_MAX || !value.bytes().all(plain) {
            return None;
        }
        let mut bytes = [0; DETAIL_MAX];
        bytes[..value.len()].copy_from_slice(value.as_bytes());
        Some(Self {
            len: u8::try_from(value.len()).ok()?,
            bytes,
        })
    }

    fn id(id: Uuid) -> Self {
        let mut buffer = Uuid::encode_buffer();
        let text = id.hyphenated().encode_lower(&mut buffer);
        let mut bytes = [0; DETAIL_MAX];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        // A hyphenated UUID is always 36 plain characters.
        Self { len: 36, bytes }
    }

    pub fn as_str(&self) -> &str {
        // Only plain ASCII is ever stored, so this never falls back.
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }
}

impl fmt::Debug for SourceDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl Serialize for SourceDetail {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SourceDetail {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(&text).ok_or_else(|| de::Error::custom("invalid source detail"))
    }
}

/// Where a change came from, as `{"kind": "device", "detail": "<id>"}`;
/// a source without a detail is `{"kind": "web"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MutationSource {
    pub kind: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<SourceDetail>,
}

impl MutationSource {
    fn bare(kind: SourceKind) -> Self {
        Self { kind, detail: None }
    }

    fn named(kind: SourceKind, name: &str) -> Self {
        Self {
            kind,
            detail: SourceDetail::new(name),
        }
    }

    /// A browser session's request.
    pub fn web() -> Self {
        Self::bare(SourceKind::Web)
    }

    /// A request authenticated by this API client's token.
    pub fn api_client(client: Uuid) -> Self {
        Self {
            kind: SourceKind::ApiClient,
            detail: Some(SourceDetail::id(client)),
        }
    }

    /// A synced device: the device's id when the server verified its token,
    /// or the device's own replica, which knows its id once signed in.
    pub fn device(device: Option<Uuid>) -> Self {
        Self {
            kind: SourceKind::Device,
            detail: device.map(SourceDetail::id),
        }
    }

    /// An automation the engine ran, by its API name.
    pub fn automation(name: &str) -> Self {
        Self::named(SourceKind::Automation, name)
    }

    /// A quick action the entry point resolved on the record's object.
    pub fn quick_action(name: &str) -> Self {
        Self::named(SourceKind::QuickAction, name)
    }

    /// A device's local assistant bridge (#232). Never set from a request.
    pub fn assistant(assistant: Assistant) -> Self {
        Self::named(SourceKind::Assistant, assistant.as_str())
    }

    /// A bulk job's row, by the job's id.
    pub fn import(job: Uuid) -> Self {
        Self {
            kind: SourceKind::Import,
            detail: Some(SourceDetail::id(job)),
        }
    }

    /// The platform itself.
    pub fn system() -> Self {
        Self::bare(SourceKind::System)
    }

    /// The source as stored: its kind and detail, or `None` for a stored
    /// kind this build does not know, so old or unknown rows read as
    /// unknown rather than failing.
    pub fn from_parts(kind: Option<&str>, detail: Option<&str>) -> Option<Self> {
        let kind = SourceKind::parse(kind?)?;
        Some(Self {
            kind,
            detail: detail.and_then(SourceDetail::new),
        })
    }

    pub fn detail_str(&self) -> Option<&str> {
        self.detail.as_ref().map(SourceDetail::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sources_serialise_as_a_kind_and_an_optional_detail() {
        let device = Uuid::nil();
        assert_eq!(
            serde_json::to_value(MutationSource::device(Some(device))).unwrap(),
            json!({"kind": "device", "detail": "00000000-0000-0000-0000-000000000000"})
        );
        assert_eq!(
            serde_json::to_value(MutationSource::web()).unwrap(),
            json!({"kind": "web"})
        );
        assert_eq!(
            serde_json::to_value(MutationSource::assistant(Assistant::Siri)).unwrap(),
            json!({"kind": "assistant", "detail": "siri"})
        );
        let back: MutationSource =
            serde_json::from_value(json!({"kind": "automation", "detail": "custom_flag"})).unwrap();
        assert_eq!(back, MutationSource::automation("custom_flag"));
        assert!(serde_json::from_value::<MutationSource>(json!({"kind": "robot"})).is_err());
    }

    #[test]
    fn details_are_plain_and_short() {
        assert!(SourceDetail::new("custom:close_won").is_some());
        assert!(SourceDetail::new("").is_none());
        assert!(SourceDetail::new("Ada Lovelace").is_none());
        assert!(SourceDetail::new(&"a".repeat(DETAIL_MAX + 1)).is_none());
        assert_eq!(
            MutationSource::automation("not plain!").detail,
            None,
            "an unusable name leaves the kind without a detail"
        );
    }

    #[test]
    fn stored_parts_read_back_or_as_unknown() {
        assert_eq!(MutationSource::from_parts(None, None), None);
        assert_eq!(MutationSource::from_parts(Some("robot"), None), None);
        assert_eq!(
            MutationSource::from_parts(Some("import"), Some("x")),
            Some(MutationSource {
                kind: SourceKind::Import,
                detail: SourceDetail::new("x"),
            })
        );
    }
}
