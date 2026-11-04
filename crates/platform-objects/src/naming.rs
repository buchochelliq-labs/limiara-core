//! API names and key prefixes (ADR 0015 section 3).
//!
//! API names are `namespace:name`. The namespace is `custom` for anything a
//! workspace administrator defines and a plugin id for anything a plugin
//! defines; kernel standard fields have none. A dot never appears in the
//! name part, so the query language can use dots for relationship traversal.

use crate::{ObjectError, Origin};

/// The namespace of everything a workspace administrator defines. It can
/// never be a plugin id.
pub const CUSTOM_NAMESPACE: &str = "custom";

const NAME_MAX: usize = 63;
const NAMESPACE_MAX: usize = 100;
const PREFIX_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Whether `name` is a valid unqualified name: lowercase letter, then
/// lowercase letters, digits or underscores, at most 63 characters, without a
/// trailing or doubled underscore.
pub fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        && !name.ends_with('_')
        && !name.contains("__")
}

/// Whether `namespace` is `custom` or a dotted plugin id.
pub fn valid_namespace(namespace: &str) -> bool {
    namespace == CUSTOM_NAMESPACE
        || ((3..=NAMESPACE_MAX).contains(&namespace.len())
            && namespace.split('.').count() >= 2
            && namespace.split('.').all(|token| {
                !token.is_empty()
                    && token.as_bytes()[0].is_ascii_lowercase()
                    && token.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    })
            }))
}

/// A qualified API name split into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiName<'a> {
    pub namespace: &'a str,
    pub name: &'a str,
}

impl<'a> ApiName<'a> {
    pub fn parse(value: &'a str) -> Option<Self> {
        let (namespace, name) = value.split_once(':')?;
        (valid_namespace(namespace) && valid_name(name)).then_some(Self { namespace, name })
    }

    /// The origin a namespace implies.
    pub fn origin(&self) -> Origin {
        if self.namespace == CUSTOM_NAMESPACE {
            Origin::Workspace
        } else {
            Origin::Plugin
        }
    }
}

/// The API name a workspace administrator's short name becomes.
pub fn custom_api_name(field: &str, name: &str) -> Result<String, ObjectError> {
    let name = name.trim();
    if !valid_name(name) {
        return Err(ObjectError::invalid(
            field,
            "objects.name_invalid",
            "use a lowercase letter followed by lowercase letters, digits or single underscores, at most 63 characters",
        ));
    }
    Ok(format!("{CUSTOM_NAMESPACE}:{name}"))
}

/// The first digit-led key prefix not yet used in the workspace. Workspace
/// objects always get a digit-first prefix; plugin objects register
/// letter-first ones, so the ranges never collide.
pub fn next_workspace_prefix<'a>(used: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let used: std::collections::BTreeSet<&str> = used.into_iter().collect();
    for first in b'0'..=b'9' {
        for second in PREFIX_ALPHABET {
            for third in PREFIX_ALPHABET {
                let candidate = String::from_utf8(vec![first, *second, *third]).ok()?;
                if !used.contains(candidate.as_str()) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Whether a key prefix belongs to a workspace object.
pub fn is_workspace_prefix(prefix: &str) -> bool {
    prefix.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_names_carry_their_namespace() {
        let custom = ApiName::parse("custom:project").expect("custom");
        assert_eq!(custom.namespace, "custom");
        assert_eq!(custom.origin(), Origin::Workspace);
        let plugin = ApiName::parse("example.loyalty:tier").expect("plugin");
        assert_eq!(plugin.origin(), Origin::Plugin);
        for invalid in [
            "project",
            "custom:",
            "custom:Project",
            "custom:pro.ject",
            "custom:pro__ject",
            "custom:project_",
            "loyalty:tier",
            "custom:1st",
        ] {
            assert!(ApiName::parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn workspace_prefixes_start_with_a_digit_and_skip_used_ones() {
        assert_eq!(next_workspace_prefix([]).as_deref(), Some("000"));
        assert_eq!(
            next_workspace_prefix(["000", "001"]).as_deref(),
            Some("002")
        );
        assert!(is_workspace_prefix("0a1"));
        assert!(!is_workspace_prefix("per"));
    }

    #[test]
    fn short_names_become_custom_api_names() {
        assert_eq!(
            custom_api_name("name", " project ").unwrap(),
            "custom:project"
        );
        assert!(custom_api_name("name", "Project").is_err());
        assert!(custom_api_name("name", "custom:project").is_err());
    }
}
