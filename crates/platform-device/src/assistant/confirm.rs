//! Confirmation tokens for two-step writes (ADR 0034 §5).
//!
//! A token is 16 bytes that name it and 32 secret bytes, both from the
//! operating system's CSPRNG, in unpadded URL-safe base64. Tokens live in
//! memory only, in this store, and never reach the replica, the push or a
//! log. A token is bound to the exact prepared change (workspace, member,
//! assistant, operation, object, record, values), the record's version and
//! the metadata version; it is taken out of the store by the first commit
//! that presents its secret, and expires [`CONFIRMATION_TTL`] after it was
//! issued. Secrets are compared in constant time.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// How long a prepared change waits for its commit (decided in review).
pub const CONFIRMATION_TTL: Duration = Duration::from_secs(2 * 60);

/// Prepared changes waiting at once; the oldest goes first beyond it.
const PENDING_MAX: usize = 32;

const ID_LEN: usize = 16;
const SECRET_LEN: usize = 32;

/// The time the store measures expiry by. The shell uses the monotonic
/// clock ([`SystemClock`]); a test harness may move it.
pub trait AssistantClock: Send + Sync {
    fn now(&self) -> Instant;
}

/// The device's monotonic clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl AssistantClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Which write a prepared change makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreparedWrite {
    Create,
    Update,
    /// A quick action on `object` (the action's own object).
    RunAction {
        action_object: String,
        action: String,
        /// A create action makes a child record; an update action changes
        /// the record itself.
        creates: bool,
    },
}

/// Everything a token is bound to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prepared {
    pub workspace: Uuid,
    pub member: Uuid,
    pub assistant: platform_objects::Assistant,
    pub write: PreparedWrite,
    /// The object the write changes.
    pub object: String,
    /// The record an update changes, or the record a create action points
    /// its new record at.
    pub record: Option<String>,
    /// That record's version when the change was prepared.
    pub record_version: Option<i64>,
    pub metadata_version: i64,
    /// The exact values the commit sends.
    pub values: Map<String, Value>,
}

struct Pending {
    /// Issue order, so the oldest goes first when the store is full.
    order: u64,
    secret: [u8; SECRET_LEN],
    expires: Instant,
    prepared: Prepared,
}

/// Why a token was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TokenRefused {
    /// Unknown, malformed, already used, or the wrong secret.
    Invalid,
    /// Known but past its two minutes; it is gone now.
    Expired,
}

/// The store of prepared changes.
#[derive(Clone)]
pub(crate) struct Confirmations {
    pending: Arc<Mutex<HashMap<[u8; ID_LEN], Pending>>>,
    random: Arc<SystemRandom>,
    clock: Arc<dyn AssistantClock>,
    issued: Arc<std::sync::atomic::AtomicU64>,
}

impl Confirmations {
    pub fn new(clock: Arc<dyn AssistantClock>) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            random: Arc::new(SystemRandom::new()),
            clock,
            issued: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// A token for `prepared`, or `None` when the CSPRNG fails, in which
    /// case nothing can be confirmed.
    pub fn issue(&self, prepared: Prepared) -> Option<String> {
        let mut id = [0u8; ID_LEN];
        let mut secret = [0u8; SECRET_LEN];
        self.random.fill(&mut id).ok()?;
        self.random.fill(&mut secret).ok()?;
        let now = self.clock.now();
        let mut pending = self.pending.lock().ok()?;
        pending.retain(|_, entry| entry.expires > now);
        while pending.len() >= PENDING_MAX {
            let oldest = pending
                .iter()
                .min_by_key(|(_, entry)| entry.order)
                .map(|(key, _)| *key)?;
            pending.remove(&oldest);
        }
        pending.insert(
            id,
            Pending {
                order: self
                    .issued
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                secret,
                expires: now + CONFIRMATION_TTL,
                prepared,
            },
        );
        let mut bytes = Vec::with_capacity(ID_LEN + SECRET_LEN);
        bytes.extend_from_slice(&id);
        bytes.extend_from_slice(&secret);
        Some(URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Takes the prepared change a token names: once, and only before it
    /// expires.
    pub fn take(&self, token: &str) -> Result<Prepared, TokenRefused> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token.as_bytes())
            .map_err(|_| TokenRefused::Invalid)?;
        if bytes.len() != ID_LEN + SECRET_LEN {
            return Err(TokenRefused::Invalid);
        }
        let (id, secret) = bytes.split_at(ID_LEN);
        let id: [u8; ID_LEN] = id.try_into().map_err(|_| TokenRefused::Invalid)?;
        let now = self.clock.now();
        let mut pending = self.pending.lock().map_err(|_| TokenRefused::Invalid)?;
        let matches = pending
            .get(&id)
            .is_some_and(|entry| bool::from(entry.secret.ct_eq(secret)));
        if !matches {
            return Err(TokenRefused::Invalid);
        }
        let entry = pending.remove(&id).ok_or(TokenRefused::Invalid)?;
        if entry.expires <= now {
            return Err(TokenRefused::Expired);
        }
        Ok(entry.prepared)
    }

    /// Forgets every prepared change, as when the workspace closes.
    pub fn clear(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Moved(Instant, AtomicU64);

    impl AssistantClock for Moved {
        fn now(&self) -> Instant {
            self.0 + Duration::from_secs(self.1.load(Ordering::SeqCst))
        }
    }

    fn prepared() -> Prepared {
        Prepared {
            workspace: Uuid::now_v7(),
            member: Uuid::now_v7(),
            assistant: platform_objects::Assistant::Siri,
            write: PreparedWrite::Create,
            object: "custom:account".into(),
            record: None,
            record_version: None,
            metadata_version: 3,
            values: Map::new(),
        }
    }

    #[test]
    fn a_token_is_single_use_and_expires() {
        let clock = Arc::new(Moved(Instant::now(), AtomicU64::new(0)));
        let store = Confirmations::new(clock.clone());
        let token = store.issue(prepared()).expect("token");
        assert_eq!(
            URL_SAFE_NO_PAD.decode(&token).map(|bytes| bytes.len()),
            Ok(48)
        );
        let other = store.issue(prepared()).expect("token");
        assert_ne!(token, other);
        assert_eq!(
            store.take(&token).map(|p| p.object),
            Ok("custom:account".into())
        );
        assert_eq!(store.take(&token), Err(TokenRefused::Invalid));
        // The same id with another secret is refused and leaves it in place.
        let mut forged = URL_SAFE_NO_PAD.decode(&other).expect("bytes");
        forged[47] ^= 1;
        assert_eq!(
            store.take(&URL_SAFE_NO_PAD.encode(&forged)),
            Err(TokenRefused::Invalid)
        );
        for garbage in ["", "not base64!", "AAAA"] {
            assert_eq!(store.take(garbage), Err(TokenRefused::Invalid));
        }
        clock.1.store(121, Ordering::SeqCst);
        assert_eq!(store.take(&other), Err(TokenRefused::Expired));
        assert_eq!(store.take(&other), Err(TokenRefused::Invalid));
    }

    #[test]
    fn the_store_is_bounded_and_cleared() {
        let store = Confirmations::new(Arc::new(SystemClock));
        let first = store.issue(prepared()).expect("token");
        let tokens: Vec<String> = (0..PENDING_MAX)
            .map(|_| store.issue(prepared()).expect("token"))
            .collect();
        assert_eq!(store.take(&first), Err(TokenRefused::Invalid));
        store.clear();
        assert_eq!(store.take(&tokens[0]), Err(TokenRefused::Invalid));
    }
}
