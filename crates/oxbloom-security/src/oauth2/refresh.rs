//! The serialization of the refreshes of users' tokens and the record of recent rotations.
//!
//! Both are keyed by the registration and the SHA-256 digest of the refresh token being
//! spent, never by the session id: with the cookie session backend every request has a new
//! session id, and with a server store each request works on its own copy of the session, so a
//! session id says nothing about which refresh token a request holds. The refresh token itself
//! is never a key.
//!
//! - A lock per (registration, digest) serializes the refreshes of one refresh token, so two
//!   calls holding the same token send it once. An entry exists only while a call or a refresh
//!   task holds it or waits for it; the last one to let go removes it, so the lock map is
//!   bounded by the calls in flight.
//! - The rotation record remembers, for each successful refresh, the tokens it produced, under
//!   the digest of the refresh token it spent. A call that holds a spent refresh token (a
//!   concurrent request, or a stale session copy written back after the refresh) adopts the
//!   recorded tokens instead of spending the token again. Records are created only from
//!   successful refreshes, so a client cannot grow the map; it keeps at most 256 records
//!   (least recently used out first) for at most 10 minutes on the client's clock.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime};

use aws_lc_rs::digest::{SHA256, digest};

use super::cache::Waiting;
use super::client::TokenError;
use super::stored::StoredTokens;

/// The most rotation records kept.
pub(crate) const RECORD_LIMIT: usize = 256;
/// How long a rotation record is kept.
pub(crate) const RECORD_LIFETIME: Duration = Duration::from_secs(600);

/// The registration and the SHA-256 digest of a refresh token.
type Key = (String, [u8; 32]);

fn key(registration: &str, refresh_token: &str) -> Key {
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(digest(&SHA256, refresh_token.as_bytes()).as_ref());
    (registration.to_owned(), bytes)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The failure of the last refresh under a lock, for the calls that waited for it; `None`
/// after a success.
pub(crate) type LastFailure = Option<TokenError>;

/// The lock of one refresh token. `attempts` counts the finished refreshes, so a call that
/// waited can tell whether a refresh ran while it waited.
#[derive(Default)]
struct RefreshLock {
    attempts: AtomicU64,
    last: Arc<tokio::sync::Mutex<LastFailure>>,
}

/// A rotation: the tokens a refresh produced, when, and when the record was last used.
struct Record {
    tokens: StoredTokens,
    created: SystemTime,
    used: u64,
}

#[derive(Default)]
struct Records {
    map: HashMap<Key, Record>,
    tick: u64,
}

impl Records {
    fn purge(&mut self, now: SystemTime) {
        self.map.retain(|_, record| {
            record
                .created
                .checked_add(RECORD_LIFETIME)
                .is_some_and(|end| now < end)
        });
    }
}

/// The refresh locks and the rotation record of a client.
#[derive(Default)]
pub(crate) struct Rotations {
    locks: Mutex<HashMap<Key, Weak<RefreshLock>>>,
    records: Mutex<Records>,
    waiting: AtomicUsize,
}

impl Rotations {
    /// The lock entry of `refresh_token` of `registration`, created when no one holds it.
    pub(crate) fn entry(self: &Arc<Self>, registration: &str, refresh_token: &str) -> Entry {
        let key = key(registration, refresh_token);
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
            rotations: self.clone(),
            key,
            shared,
        }
    }

    /// The tokens that the refresh of `refresh_token` produced, when that refresh is recorded
    /// and the record is younger than 10 minutes.
    pub(crate) fn record(
        &self,
        registration: &str,
        refresh_token: &str,
        now: SystemTime,
    ) -> Option<StoredTokens> {
        let mut records = lock(&self.records);
        records.purge(now);
        records.tick += 1;
        let tick = records.tick;
        let record = records.map.get_mut(&key(registration, refresh_token))?;
        record.used = tick;
        Some(record.tokens.clone())
    }

    /// Records that spending `spent` produced `tokens`, dropping the least recently used
    /// record when 256 are kept.
    pub(crate) fn remember(
        &self,
        registration: &str,
        spent: &str,
        tokens: StoredTokens,
        now: SystemTime,
    ) {
        let key = key(registration, spent);
        let mut records = lock(&self.records);
        records.purge(now);
        if records.map.len() >= RECORD_LIMIT && !records.map.contains_key(&key) {
            let oldest = records
                .map
                .iter()
                .min_by_key(|(_, record)| record.used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                records.map.remove(&oldest);
            }
        }
        records.tick += 1;
        let used = records.tick;
        records.map.insert(
            key,
            Record {
                tokens,
                created: now,
                used,
            },
        );
    }

    /// Drops the record of `refresh_token`.
    pub(crate) fn forget(&self, registration: &str, refresh_token: &str) {
        lock(&self.records)
            .map
            .remove(&key(registration, refresh_token));
    }

    /// The number of live records. For tests.
    pub(crate) fn records(&self, now: SystemTime) -> usize {
        let mut records = lock(&self.records);
        records.purge(now);
        records.map.len()
    }

    /// The number of lock entries. For tests.
    pub(crate) fn locks(&self) -> usize {
        lock(&self.locks).len()
    }

    /// The number of calls waiting for a lock. For tests.
    pub(crate) fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Acquire)
    }
}

impl fmt::Debug for Rotations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rotations").finish_non_exhaustive()
    }
}

/// A hold on a lock entry. Dropping the last hold removes the entry. It is owned, so it can
/// move into the task that runs a refresh.
pub(crate) struct Entry {
    rotations: Arc<Rotations>,
    key: Key,
    shared: Arc<RefreshLock>,
}

impl Entry {
    /// The number of refreshes finished under this lock.
    pub(crate) fn attempts(&self) -> u64 {
        self.shared.attempts.load(Ordering::Acquire)
    }

    /// Waits for the lock; the guard holds the failure of the last refresh.
    pub(crate) async fn lock(&self) -> tokio::sync::OwnedMutexGuard<LastFailure> {
        let _waiting = Waiting::new(&self.rotations.waiting);
        self.shared.last.clone().lock_owned().await
    }

    /// Counts a finished refresh; called with the lock held.
    pub(crate) fn finished(&self) {
        self.shared.attempts.fetch_add(1, Ordering::AcqRel);
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        let mut locks = lock(&self.rotations.locks);
        // Every clone of `shared` is made with the map locked, so with the map locked a count
        // of one means that no other call holds this entry or can get it.
        if Arc::strong_count(&self.shared) == 1 {
            locks.remove(&self.key);
        }
    }
}
