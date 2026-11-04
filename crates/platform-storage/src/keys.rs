//! Keys for encrypted workspace files (ADR 0024, "Encryption at rest").
//!
//! A device keeps one 32-byte key per workspace in the platform keystore
//! (the iOS Keychain or the Android Keystore), behind [`DatabaseKeys`]. The
//! storage layer asks for the key when it opens a workspace's file and
//! never stores, logs or derives it; destroying the key makes the file
//! unreadable even if a copy survives.

use async_trait::async_trait;
use rand::RngCore;
use std::collections::HashMap;
use std::fmt;
use tokio::sync::Mutex;
use uuid::Uuid;
use zeroize::Zeroize;

/// A workspace file's raw 256-bit key. Zeroed when dropped; never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct DatabaseKey([u8; 32]);

impl DatabaseKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// A new random key.
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The key as SQLCipher's raw-key literal, `"x'<64 hex digits>'"`, so no
    /// key derivation runs.
    pub(crate) fn sqlcipher_literal(&self) -> String {
        let mut literal = String::with_capacity(69);
        literal.push_str("\"x'");
        for byte in self.0 {
            literal.push_str(&format!("{byte:02x}"));
        }
        literal.push_str("'\"");
        literal
    }
}

impl Drop for DatabaseKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for DatabaseKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DatabaseKey(..)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// The keystore refused or is locked (for example before the first
    /// unlock after a restart).
    #[error("the keystore is unavailable")]
    Unavailable,
}

/// Where a device keeps its workspace keys.
#[async_trait]
pub trait DatabaseKeys: Send + Sync {
    /// The workspace's key, created and stored on first use.
    async fn key(&self, tenant_id: Uuid) -> Result<DatabaseKey, KeyError>;

    /// Forgets the workspace's key, as part of a wipe.
    async fn destroy(&self, tenant_id: Uuid) -> Result<(), KeyError>;
}

/// Keys held in memory, for tests and desktop development. Data written
/// with them is unreadable once the process ends.
#[derive(Default)]
pub struct MemoryKeys {
    keys: Mutex<HashMap<Uuid, DatabaseKey>>,
}

impl MemoryKeys {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a key is held for the workspace.
    pub async fn holds(&self, tenant_id: Uuid) -> bool {
        self.keys.lock().await.contains_key(&tenant_id)
    }

    /// Replaces a workspace's key, as when a wrong key is supplied.
    pub async fn set(&self, tenant_id: Uuid, key: DatabaseKey) {
        self.keys.lock().await.insert(tenant_id, key);
    }
}

#[async_trait]
impl DatabaseKeys for MemoryKeys {
    async fn key(&self, tenant_id: Uuid) -> Result<DatabaseKey, KeyError> {
        Ok(self
            .keys
            .lock()
            .await
            .entry(tenant_id)
            .or_insert_with(DatabaseKey::generate)
            .clone())
    }

    async fn destroy(&self, tenant_id: Uuid) -> Result<(), KeyError> {
        self.keys.lock().await.remove(&tenant_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_never_prints() {
        let key = DatabaseKey::from_bytes([0xab; 32]);
        assert_eq!(format!("{key:?}"), "DatabaseKey(..)");
        assert_eq!(
            key.sqlcipher_literal(),
            format!("\"x'{}'\"", "ab".repeat(32))
        );
    }

    #[tokio::test]
    async fn memory_keys_keep_and_forget() {
        let keys = MemoryKeys::new();
        let tenant = Uuid::now_v7();
        let first = keys.key(tenant).await.expect("key");
        assert_eq!(keys.key(tenant).await.expect("key"), first);
        keys.destroy(tenant).await.expect("destroy");
        assert!(!keys.holds(tenant).await);
        assert_ne!(keys.key(tenant).await.expect("new key"), first);
    }
}
