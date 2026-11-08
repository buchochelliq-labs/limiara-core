//! The desktop's keychain for the app's secrets (ADR 0024, "Encryption at
//! rest"; ADR 0029): the signed-in session, with its rotating refresh
//! token, and each workspace file's database key. Nothing secret is written
//! to the app's directory.
//!
//! - macOS and iOS: the Keychain.
//! - Windows: the Credential Manager.
//! - Linux: the kernel's keyring (keyutils), which keeps the secrets until
//!   the member logs out. The next launch after that finds no session and
//!   signs in again, starting a fresh replica. A Secret Service store that
//!   survives logging out is a follow-up (docs/roadmap).
//!
//! - Android: no keychain yet. The Keystore adapter is a follow-up; until
//!   it lands, [`Keychain`] on Android keeps nothing: there is no saved
//!   session to resume, and signing in refuses to save one rather than
//!   write it to the app's files. The demonstration workspace keeps its
//!   keys in memory and works.

use async_trait::async_trait;
use platform_device::app::{SessionVault, StoredSession, VaultError};
use platform_device::{DatabaseKey, DatabaseKeys, KeyError};
use uuid::Uuid;

/// The keychain service every entry is filed under.
pub const SERVICE: &str = "app.limiara.shell";
const SESSION: &str = "session";

fn key_entry(tenant_id: Uuid) -> String {
    format!("workspace-key-{tenant_id}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let mut bytes = [0_u8; 32];
    if text.len() != 64 {
        return None;
    }
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(bytes)
}

/// The platform keychain under one service name.
#[derive(Debug, Clone)]
pub struct Keychain {
    #[cfg_attr(target_os = "android", allow(dead_code))]
    service: String,
}

impl Default for Keychain {
    fn default() -> Self {
        Self::new(SERVICE)
    }
}

impl Keychain {
    /// `service` separates apps, and tests from the app.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

#[cfg(not(target_os = "android"))]
impl Keychain {
    /// Runs a keychain call off the async runtime: the platform APIs block.
    async fn call<T: Send + 'static>(
        &self,
        user: String,
        work: impl FnOnce(keyring::Entry) -> keyring::Result<T> + Send + 'static,
    ) -> Result<Option<T>, String> {
        let service = self.service.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            keyring::Entry::new(&service, &user).and_then(work)
        })
        .await
        .map_err(|error| error.to_string())?;
        match outcome {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    async fn read(&self, user: String) -> Result<Option<String>, String> {
        self.call(user, |entry| entry.get_password()).await
    }

    async fn write(&self, user: String, secret: String) -> Result<(), String> {
        self.call(user, move |entry| entry.set_password(&secret))
            .await
            .map(|_| ())
    }

    async fn delete(&self, user: String) -> Result<(), String> {
        self.call(user, |entry| entry.delete_credential())
            .await
            .map(|_| ())
    }
}

/// Android until the Keystore adapter lands: nothing is stored, and nothing
/// secret reaches the app's files.
#[cfg(target_os = "android")]
impl Keychain {
    const UNAVAILABLE: &'static str = "this device has no keychain for Limiara yet";

    async fn read(&self, _user: String) -> Result<Option<String>, String> {
        Ok(None)
    }

    async fn write(&self, _user: String, _secret: String) -> Result<(), String> {
        Err(Self::UNAVAILABLE.into())
    }

    async fn delete(&self, _user: String) -> Result<(), String> {
        Ok(())
    }
}

#[async_trait]
impl SessionVault for Keychain {
    async fn load(&self) -> Result<Option<StoredSession>, VaultError> {
        let Some(text) = self.read(SESSION.into()).await.map_err(VaultError)? else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|_| VaultError("the stored session is unreadable".into()))
    }

    async fn save(&self, session: &StoredSession) -> Result<(), VaultError> {
        let text = serde_json::to_string(session).map_err(|error| VaultError(error.to_string()))?;
        self.write(SESSION.into(), text).await.map_err(VaultError)
    }

    async fn delete(&self) -> Result<(), VaultError> {
        Keychain::delete(self, SESSION.into())
            .await
            .map_err(VaultError)
    }
}

#[async_trait]
impl DatabaseKeys for Keychain {
    async fn key(&self, tenant_id: Uuid) -> Result<DatabaseKey, KeyError> {
        let user = key_entry(tenant_id);
        if let Some(text) = self
            .read(user.clone())
            .await
            .map_err(|_| KeyError::Unavailable)?
        {
            return unhex(&text)
                .map(DatabaseKey::from_bytes)
                .ok_or(KeyError::Unavailable);
        }
        let key = DatabaseKey::generate();
        self.write(user, hex(key.as_bytes()))
            .await
            .map_err(|_| KeyError::Unavailable)?;
        Ok(key)
    }

    async fn destroy(&self, tenant_id: Uuid) -> Result<(), KeyError> {
        Keychain::delete(self, key_entry(tenant_id))
            .await
            .map_err(|_| KeyError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_as_hex() {
        let key = DatabaseKey::generate();
        assert_eq!(unhex(&hex(key.as_bytes())), Some(*key.as_bytes()));
        assert_eq!(unhex("00"), None);
        assert_eq!(unhex(&"zz".repeat(32)), None);
    }
}
