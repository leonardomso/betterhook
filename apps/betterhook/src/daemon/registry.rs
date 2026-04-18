//! Lock registry.
//!
//! One `Semaphore` per lock key, keyed on `LockKey`. Acquires issue a
//! monotonic token so clients can call `Release { token }` explicitly;
//! connection drops also release via RAII since we hold the permit in
//! a per-connection vec.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use crate::lock::protocol::{LockKey, LockStatus};

/// Shared registry handed to every connection handler.
#[derive(Debug, Default, Clone)]
pub struct Registry {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    /// The semaphore for each lock key. First acquire of a key creates
    /// it with the caller's `permits`; subsequent acquires must match.
    slots: HashMap<LockKey, Arc<Semaphore>>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fetch (or lazily create) the semaphore for this lock key.
    pub async fn semaphore(&self, key: &LockKey) -> Result<Arc<Semaphore>, String> {
        let mut inner = self.inner.lock().await;
        if let Some(existing) = inner.slots.get(key) {
            if existing.available_permits() == 0 && key.permits > 1 {
                // A live key with unknown capacity — we trust the first
                // acquire. If a later caller passes a mismatched
                // `permits`, we still return the existing semaphore.
            }
            return Ok(existing.clone());
        }
        if key.permits == 0 {
            return Err("lock permits must be > 0".to_owned());
        }
        let sem = Arc::new(Semaphore::new(key.permits as usize));
        inner.slots.insert(key.clone(), sem.clone());
        Ok(sem)
    }

    /// Return a snapshot of every known lock for `Status` replies.
    pub async fn snapshot(&self) -> Vec<LockStatus> {
        let inner = self.inner.lock().await;
        inner
            .slots
            .iter()
            .map(|(key, sem)| LockStatus {
                key: key.clone(),
                active_permits: u32::try_from(key.permits as usize - sem.available_permits())
                    .unwrap_or(u32::MAX),
                waiters: 0,
            })
            .collect()
    }
}

/// An acquired permit together with its monotonic token, kept alive by
/// the connection until explicit release or drop.
#[derive(Debug)]
pub struct HeldPermit {
    #[allow(dead_code)]
    pub token: u64,
    // Dropping the permit releases the slot in the semaphore.
    #[allow(dead_code)]
    pub permit: OwnedSemaphorePermit,
}
