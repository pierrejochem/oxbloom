//! The default session store: a sharded map in process memory.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::fmt;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use oxbloom_core::Clock;

use super::error::SessionError;
use super::settings::SessionSettings;
use super::store::{SessionData, SessionStore};

/// The number of independently locked shards.
const SHARDS: usize = 16;

/// The shortest gap between two eviction warnings.
const WARN_INTERVAL: Duration = Duration::from_secs(60);

struct Entry {
    data: SessionData,
    /// The earlier of the idle deadline (save time plus ttl) and the absolute deadline
    /// (creation plus the absolute timeout). A deadline that does not fit in a `SystemTime`,
    /// such as one from `Duration::MAX`, means no limit; `None` means neither limit applies.
    expires: Option<SystemTime>,
}

impl Entry {
    fn is_live(&self, now: SystemTime) -> bool {
        self.expires.is_none_or(|expires| expires > now)
    }
}

/// The earlier of two optional deadlines, where `None` means no deadline.
fn earliest(a: Option<SystemTime>, b: Option<SystemTime>) -> Option<SystemTime> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

type Shard = Mutex<HashMap<String, Entry>>;

/// Sessions kept in process memory, the default [`SessionStore`].
///
/// The map is split into shards, each behind its own lock. A session expires when it is not
/// saved again within the ttl of its last save, and in any case once the absolute timeout of
/// the settings has passed since its creation. Expired sessions are removed when they are
/// accessed and by [`sweep`](Self::sweep). When `max_sessions` sessions are stored, saving a
/// new one evicts the session with the oldest last touch and logs a warning (target
/// `oxbloom::security`, at most once a minute, without any id). Under concurrent saves of new
/// sessions the count may overshoot the cap by the number of racing saves.
///
/// Sessions are lost when the process stops and are not shared between instances. `Debug`
/// shows only the number of sessions and the cap.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::SystemTime;
/// use oxbloom_core::SystemClock;
/// use oxbloom_security::{InMemorySessionStore, SessionData, SessionSettings, SessionStore};
///
/// let settings = SessionSettings::default();
/// let store = InMemorySessionStore::new(&settings, Arc::new(SystemClock));
///
/// tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
///     let mut data = SessionData::new(SystemTime::now());
///     data.values_mut().insert("visits".to_owned(), serde_json::json!(1));
///     store.save("an-id", &data, settings.idle_timeout).await.unwrap();
///     assert_eq!(store.load("an-id").await.unwrap(), Some(data));
/// });
/// assert_eq!(store.len(), 1);
/// ```
pub struct InMemorySessionStore {
    shards: Box<[Shard]>,
    hasher: RandomState,
    count: AtomicUsize,
    max_sessions: usize,
    absolute_timeout: Duration,
    clock: Arc<dyn Clock>,
    last_warning: Mutex<Option<SystemTime>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a shard lock cannot leave a map half-updated in a way that
    // matters here, so a poisoned lock is used as it is.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl InMemorySessionStore {
    /// Creates an empty store with the cap and the absolute timeout of `settings`, reading
    /// the time from `clock`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionSettings};
    ///
    /// let store = InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock));
    /// assert!(store.is_empty());
    /// ```
    #[must_use]
    pub fn new(settings: &SessionSettings, clock: Arc<dyn Clock>) -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Mutex::new(HashMap::new())).collect(),
            hasher: RandomState::new(),
            count: AtomicUsize::new(0),
            max_sessions: settings.max_sessions,
            absolute_timeout: settings.absolute_timeout,
            clock,
            last_warning: Mutex::new(None),
        }
    }

    /// The number of stored sessions, counting expired ones not yet removed.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionSettings};
    ///
    /// let store = InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock));
    /// assert_eq!(store.len(), 0);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Whether the store holds no sessions.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionSettings};
    ///
    /// let store = InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock));
    /// assert!(store.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Removes every expired session. The session filter calls it, through
    /// [`SessionStore::maintain`], at most once a minute.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionSettings};
    ///
    /// let store = InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock));
    /// store.sweep();
    /// assert!(store.is_empty());
    /// ```
    pub fn sweep(&self) {
        let now = self.clock.now();
        for shard in &*self.shards {
            let mut map = lock(shard);
            let before = map.len();
            map.retain(|_, entry| entry.is_live(now));
            self.count.fetch_sub(before - map.len(), Ordering::Relaxed);
        }
    }

    fn shard(&self, id: &str) -> &Shard {
        // A plain hash-map lookup: the id is compared after hashing, and constant-time
        // comparison buys nothing here because the lookup time does not reveal a stored id
        // that the caller does not already hold.
        let index = (self.hasher.hash_one(id) as usize) % self.shards.len();
        &self.shards[index]
    }

    /// Evicts the session with the oldest last touch, dropping expired sessions on the way.
    fn evict_oldest(&self, now: SystemTime) {
        let mut oldest: Option<(usize, String, SystemTime)> = None;
        for (index, shard) in self.shards.iter().enumerate() {
            let mut map = lock(shard);
            let before = map.len();
            map.retain(|_, entry| entry.is_live(now));
            self.count.fetch_sub(before - map.len(), Ordering::Relaxed);
            for (id, entry) in map.iter() {
                let touched = entry.data.touched();
                if oldest.as_ref().is_none_or(|(_, _, t)| touched < *t) {
                    oldest = Some((index, id.clone(), touched));
                }
            }
        }
        if self.len() < self.max_sessions {
            // Expired sessions made room; nothing live is evicted.
            return;
        }
        let Some((index, id, _)) = oldest else { return };
        let removed = {
            let mut map = lock(&self.shards[index]);
            let removed = map.remove(&id).is_some();
            if removed {
                self.count.fetch_sub(1, Ordering::Relaxed);
            }
            removed
        };
        if removed {
            self.warn_evicted(now);
        }
    }

    fn warn_evicted(&self, now: SystemTime) {
        let mut last = lock(&self.last_warning);
        let due =
            last.is_none_or(|at| now.duration_since(at).is_ok_and(|gap| gap >= WARN_INTERVAL));
        if due {
            *last = Some(now);
            tracing::warn!(
                target: "oxbloom::security",
                max_sessions = self.max_sessions,
                "the in-memory session store is full; the least recently touched session was evicted"
            );
        }
    }
}

impl fmt::Debug for InMemorySessionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemorySessionStore")
            .field("sessions", &self.len())
            .field("max_sessions", &self.max_sessions)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn load(&self, id: &str) -> Result<Option<SessionData>, SessionError> {
        let now = self.clock.now();
        let mut map = lock(self.shard(id));
        match map.get(id) {
            Some(entry) if entry.is_live(now) => Ok(Some(entry.data.clone())),
            Some(_) => {
                map.remove(id);
                self.count.fetch_sub(1, Ordering::Relaxed);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn save(&self, id: &str, data: &SessionData, ttl: Duration) -> Result<(), SessionError> {
        let now = self.clock.now();
        // An overflowing deadline means no limit, never an immediate expiry.
        let idle = now.checked_add(ttl);
        let absolute = data.created().checked_add(self.absolute_timeout);
        let entry = Entry {
            data: data.clone(),
            expires: earliest(idle, absolute),
        };

        let exists = lock(self.shard(id)).contains_key(id);
        if !exists && self.len() >= self.max_sessions {
            self.evict_oldest(now);
        }
        let mut map = lock(self.shard(id));
        if map.insert(id.to_owned(), entry).is_none() {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn update(
        &self,
        id: &str,
        data: &SessionData,
        ttl: Duration,
    ) -> Result<bool, SessionError> {
        let now = self.clock.now();
        let idle = now.checked_add(ttl);
        let absolute = data.created().checked_add(self.absolute_timeout);
        // The check and the write happen under one shard lock, so a concurrent delete either
        // runs first (and this writes nothing) or runs after (and removes this write).
        let mut map = lock(self.shard(id));
        match map.get_mut(id) {
            Some(entry) if entry.is_live(now) => {
                entry.data = data.clone();
                entry.expires = earliest(idle, absolute);
                Ok(true)
            }
            Some(_) => {
                map.remove(id);
                self.count.fetch_sub(1, Ordering::Relaxed);
                Ok(false)
            }
            None => Ok(false),
        }
    }

    async fn delete(&self, id: &str) -> Result<(), SessionError> {
        let mut map = lock(self.shard(id));
        if map.remove(id).is_some() {
            self.count.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn rotate(&self, old: &str, new: &str) -> Result<bool, SessionError> {
        let now = self.clock.now();
        // Taking `old` out under its shard lock is the atomic step: after a concurrent delete
        // there is nothing to take, and nothing is created under `new`.
        let entry = {
            let mut map = lock(self.shard(old));
            let Some(entry) = map.remove(old) else {
                return Ok(false);
            };
            self.count.fetch_sub(1, Ordering::Relaxed);
            entry
        };
        if !entry.is_live(now) {
            return Ok(false);
        }
        let mut map = lock(self.shard(new));
        if map.insert(new.to_owned(), entry).is_none() {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(true)
    }

    async fn maintain(&self) {
        self.sweep();
    }
}
