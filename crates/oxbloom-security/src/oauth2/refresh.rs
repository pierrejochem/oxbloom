//! The locks that serialize the refreshes of a user's tokens, one per session and
//! registration.
//!
//! Two requests of one session run with their own copy of the session, loaded from the store
//! when each request started, so the second request does not see what the first one stored.
//! The lock therefore also remembers the last refresh made under it: the refresh token it spent
//! and the tokens it got. A caller that waited and still holds the spent refresh token takes
//! those tokens instead of spending the refresh token a second time, which a provider that
//! rotates refresh tokens would refuse with `invalid_grant`.
//!
//! An entry exists only while a call holds it or waits for it: the last one to let go removes
//! it, so the map is bounded by the number of calls in flight, whatever sessions the clients
//! present.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use super::cache::Waiting;
use super::stored::StoredTokens;

/// A refresh made under a lock: the refresh token it spent and the tokens it stored.
pub(crate) struct Refreshed {
    pub(crate) spent: String,
    pub(crate) tokens: StoredTokens,
}

/// The lock of one session and registration, with the last refresh made under it.
#[derive(Default)]
struct RefreshLock {
    last: tokio::sync::Mutex<Option<Refreshed>>,
}

type Key = (String, String);

/// The refresh locks of a client.
#[derive(Default)]
pub(crate) struct RefreshLocks {
    locks: Mutex<HashMap<Key, Weak<RefreshLock>>>,
    waiting: AtomicUsize,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl RefreshLocks {
    /// The lock entry of `session_id` and `registration`, created when no call holds it.
    pub(crate) fn entry(&self, session_id: String, registration: &str) -> Entry<'_> {
        let key = (session_id, registration.to_owned());
        let mut locks = lock(&self.locks);
        let shared = match locks.get(&key).and_then(Weak::upgrade) {
            Some(shared) => shared,
            None => {
                let shared = Arc::new(RefreshLock::default());
                locks.insert(key.clone(), Arc::downgrade(&shared));
                shared
            }
        };
        Entry {
            locks: self,
            key,
            shared,
        }
    }

    /// The number of entries: the calls in flight that hold or wait for a lock. For tests.
    pub(crate) fn len(&self) -> usize {
        lock(&self.locks).len()
    }

    /// The number of calls waiting for a lock. For tests.
    pub(crate) fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Acquire)
    }
}

impl fmt::Debug for RefreshLocks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefreshLocks").finish_non_exhaustive()
    }
}

/// A call's hold on a lock entry. Dropping it removes the entry when no other call holds it.
pub(crate) struct Entry<'a> {
    locks: &'a RefreshLocks,
    key: Key,
    shared: Arc<RefreshLock>,
}

impl Entry<'_> {
    /// Waits for the lock and returns the last refresh made under it.
    pub(crate) async fn lock(&self) -> tokio::sync::MutexGuard<'_, Option<Refreshed>> {
        let _waiting = Waiting::new(&self.locks.waiting);
        self.shared.last.lock().await
    }
}

impl Drop for Entry<'_> {
    fn drop(&mut self) {
        let mut locks = lock(&self.locks.locks);
        // Every clone of `shared` is made with the map locked, so with the map locked a count
        // of one means that no other call holds this entry or can get it.
        if Arc::strong_count(&self.shared) == 1 {
            locks.remove(&self.key);
        }
    }
}
