//! The injectable clock. Scripts move it with `advance_clock`; nothing in
//! the harness sleeps.
//!
//! It is both clocks the bridge measures time by: the confirmation store's
//! ([`AssistantClock`], for the two-minute token lifetime) and the
//! replica's trusted monotonic clock ([`TrustedClock`], for the offline
//! lease, ADR 0024 "Trusted elapsed time"). Within one boot the lease runs
//! out once trusted time passes its length, so moving this clock locks the
//! replica, and the next sync, which records contact, opens it again.

use platform_device::assistant::confirm::AssistantClock;
use platform_objects::{TrustedClock, TrustedInstant};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Time since the made-up boot when the harness starts.
const BOOTED_FOR: Duration = Duration::from_secs(1_000);

#[derive(Debug)]
pub struct HarnessClock {
    start: Instant,
    advanced: AtomicU64,
}

impl HarnessClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            advanced: AtomicU64::new(0),
        }
    }

    /// Moves the clock on by `seconds`.
    pub fn advance(&self, seconds: u64) {
        self.advanced.fetch_add(seconds, Ordering::SeqCst);
    }

    fn elapsed(&self) -> Duration {
        Duration::from_secs(self.advanced.load(Ordering::SeqCst))
    }
}

impl Default for HarnessClock {
    fn default() -> Self {
        Self::new()
    }
}

impl AssistantClock for HarnessClock {
    fn now(&self) -> Instant {
        self.start + self.elapsed()
    }
}

impl TrustedClock for HarnessClock {
    fn now(&self) -> Option<TrustedInstant> {
        Some(TrustedInstant {
            boot: "assistant-harness".into(),
            since_boot: BOOTED_FOR + self.elapsed(),
        })
    }
}
