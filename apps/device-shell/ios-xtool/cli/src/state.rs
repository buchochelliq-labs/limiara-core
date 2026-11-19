//! What the tool remembers between runs, in
//! `~/.config/limiara-sideload/state.toml` (`0600`). It holds no secrets:
//! no password, no two-factor code, no key contents, no token. The key
//! file's *location* is kept so `doctor` can check its permissions.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{io_reason, Error, Result};
use crate::perms;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Where the repository is cloned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<PathBuf>,
    /// The branch checked out there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The Apple ID used in password mode (an account name, not a secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apple_id: Option<String>,
    /// `key` or `password`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_mode: Option<String>,
    /// Where the API key file was put.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<PathBuf>,
    /// Steps finished, by id.
    #[serde(default)]
    pub completed: Vec<String>,
}

impl State {
    pub fn load(path: &Path) -> Result<State> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|_| Error::State {
                path: path.to_path_buf(),
                reason: "it is not valid; delete it to start afresh".into(),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(Error::State {
                path: path.to_path_buf(),
                reason: io_reason(&e),
            }),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self).map_err(|_| Error::State {
            path: path.to_path_buf(),
            reason: "it could not be encoded".into(),
        })?;
        if let Some(dir) = path.parent() {
            perms::ensure_private_dir(dir)?;
        }
        perms::write_private_file(path, text.as_bytes())
    }

    pub fn mark_done(&mut self, step: &str) {
        if !self.completed.iter().any(|s| s == step) {
            self.completed.push(step.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn round_trip_is_0600() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg/state.toml");
        let mut state = State {
            repo: Some("/home/ada/limiara-core".into()),
            branch: Some("main".into()),
            apple_id: Some("me@example.com".into()),
            login_mode: Some("password".into()),
            key_file: None,
            completed: vec![],
        };
        state.mark_done("apt");
        state.mark_done("apt");
        state.save(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(State::load(&path).unwrap(), state);
        assert_eq!(state.completed, vec!["apt".to_string()]);
    }

    #[test]
    fn holds_no_secret_fields() {
        let state = State {
            repo: Some("/r".into()),
            branch: Some("b".into()),
            apple_id: Some("a@b.c".into()),
            login_mode: Some("key".into()),
            key_file: Some("/k/AuthKey_X.p8".into()),
            completed: vec!["x".into()],
        };
        let value: toml::Value = toml::from_str(&toml::to_string(&state).unwrap()).unwrap();
        let keys: Vec<&String> = value.as_table().unwrap().keys().collect();
        let allowed = [
            "repo",
            "branch",
            "apple_id",
            "login_mode",
            "key_file",
            "completed",
        ];
        for key in &keys {
            assert!(allowed.contains(&key.as_str()), "unexpected field {key}");
            for secret in ["password", "code", "pem", "token", "secret", "private"] {
                assert!(!key.contains(secret), "{key}");
            }
        }
        // And a state file with a smuggled-in field is refused, not kept.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        std::fs::write(&path, "password = \"hunter2\"\n").unwrap();
        let err = State::load(&path).unwrap_err();
        assert!(!err.to_string().contains("hunter2"));
    }

    #[test]
    fn missing_file_is_a_fresh_state() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            State::load(&dir.path().join("none.toml")).unwrap(),
            State::default()
        );
    }
}
