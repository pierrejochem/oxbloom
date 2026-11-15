//! The session data and the trait that stores it.

use std::fmt;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use serde_json::{Map, Value};

use super::error::SessionError;

/// The contents of one session: its values, its creation time and its last-touch time.
///
/// The values are JSON, so every backend serializes them the same way. `Debug` prints neither
/// the keys nor the values.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use oxbloom_security::SessionData;
///
/// let start = SystemTime::UNIX_EPOCH;
/// let mut data = SessionData::new(start);
/// assert!(data.is_empty());
///
/// data.values_mut().insert("user".to_owned(), serde_json::json!("alice"));
/// data.touch(start + Duration::from_secs(5));
/// assert!(!data.is_empty());
/// assert_eq!(data.created(), start);
/// assert_eq!(data.touched(), start + Duration::from_secs(5));
/// assert_eq!(format!("{data:?}"), "SessionData { .. }");
/// ```
#[derive(Clone, PartialEq)]
pub struct SessionData {
    values: Map<String, Value>,
    created: SystemTime,
    touched: SystemTime,
}

impl SessionData {
    /// Creates empty session data, created and touched at `now`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::SessionData;
    ///
    /// let now = SystemTime::now();
    /// let data = SessionData::new(now);
    /// assert!(data.is_empty());
    /// assert_eq!(data.created(), now);
    /// assert_eq!(data.touched(), now);
    /// ```
    #[must_use]
    pub fn new(now: SystemTime) -> Self {
        Self::from_parts(Map::new(), now, now)
    }

    /// Rebuilds session data from its parts, for example when a store reads it back.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use oxbloom_security::SessionData;
    ///
    /// let created = SystemTime::UNIX_EPOCH;
    /// let touched = created + Duration::from_secs(60);
    /// let mut values = serde_json::Map::new();
    /// values.insert("n".to_owned(), serde_json::json!(1));
    ///
    /// let data = SessionData::from_parts(values, created, touched);
    /// assert_eq!(data.values().get("n"), Some(&serde_json::json!(1)));
    /// assert_eq!(data.touched(), touched);
    /// ```
    #[must_use]
    pub fn from_parts(
        values: Map<String, Value>,
        created: SystemTime,
        touched: SystemTime,
    ) -> Self {
        Self {
            values,
            created,
            touched,
        }
    }

    /// The values of the session.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::SessionData;
    ///
    /// assert!(SessionData::new(SystemTime::now()).values().is_empty());
    /// ```
    #[must_use]
    pub fn values(&self) -> &Map<String, Value> {
        &self.values
    }

    /// The values of the session, for changing them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::SessionData;
    ///
    /// let mut data = SessionData::new(SystemTime::now());
    /// data.values_mut().insert("theme".to_owned(), serde_json::json!("dark"));
    /// assert_eq!(data.values().len(), 1);
    /// ```
    pub fn values_mut(&mut self) -> &mut Map<String, Value> {
        &mut self.values
    }

    /// When the session was created. The absolute timeout counts from here.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::SessionData;
    ///
    /// let now = SystemTime::now();
    /// assert_eq!(SessionData::new(now).created(), now);
    /// ```
    #[must_use]
    pub fn created(&self) -> SystemTime {
        self.created
    }

    /// When the session was last touched, that is last written back by a request.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::SessionData;
    ///
    /// let now = SystemTime::now();
    /// assert_eq!(SessionData::new(now).touched(), now);
    /// ```
    #[must_use]
    pub fn touched(&self) -> SystemTime {
        self.touched
    }

    /// Records a touch at `now`. The creation time does not change.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use oxbloom_security::SessionData;
    ///
    /// let start = SystemTime::UNIX_EPOCH;
    /// let mut data = SessionData::new(start);
    /// data.touch(start + Duration::from_secs(1));
    /// assert_eq!(data.created(), start);
    /// assert_eq!(data.touched(), start + Duration::from_secs(1));
    /// ```
    pub fn touch(&mut self, now: SystemTime) {
        self.touched = now;
    }

    /// Whether the session holds no values.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::SessionData;
    ///
    /// assert!(SessionData::new(SystemTime::now()).is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl fmt::Debug for SessionData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionData { .. }")
    }
}

/// Where server-side sessions live, keyed by their id.
///
/// A store treats an expired session as absent. An implementation must never put an id or a
/// value in an error or a log line; a log line may carry the fingerprint of an id at most.
/// Redis or SQL stores plug in by implementing this trait.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use std::sync::Mutex;
/// use std::time::{Duration, SystemTime};
/// use async_trait::async_trait;
/// use oxbloom_security::{SessionData, SessionError, SessionStore};
///
/// /// A store without expiry, to show the shape of the trait.
/// #[derive(Default)]
/// struct MapStore(Mutex<HashMap<String, SessionData>>);
///
/// #[async_trait]
/// impl SessionStore for MapStore {
///     async fn load(&self, id: &str) -> Result<Option<SessionData>, SessionError> {
///         Ok(self.0.lock().unwrap().get(id).cloned())
///     }
///     async fn save(&self, id: &str, data: &SessionData, _ttl: Duration) -> Result<(), SessionError> {
///         self.0.lock().unwrap().insert(id.to_owned(), data.clone());
///         Ok(())
///     }
///     async fn delete(&self, id: &str) -> Result<(), SessionError> {
///         self.0.lock().unwrap().remove(id);
///         Ok(())
///     }
///     async fn rotate(&self, old: &str, new: &str) -> Result<(), SessionError> {
///         let mut map = self.0.lock().unwrap();
///         if let Some(data) = map.remove(old) {
///             map.insert(new.to_owned(), data);
///         }
///         Ok(())
///     }
/// }
///
/// # tokio_test_block_on(async {
/// let store = MapStore::default();
/// store.save("id", &SessionData::new(SystemTime::now()), Duration::from_secs(60)).await?;
/// store.rotate("id", "id2").await?;
/// assert!(store.load("id").await?.is_none());
/// assert!(store.load("id2").await?.is_some());
/// store.maintain().await;
/// # Ok::<(), SessionError>(())
/// # }).unwrap();
/// # fn tokio_test_block_on<F: std::future::Future>(f: F) -> F::Output {
/// #     tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
/// # }
/// ```
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Loads the session stored under `id`, or `None` when it is unknown or expired.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be read.
    async fn load(&self, id: &str) -> Result<Option<SessionData>, SessionError>;

    /// Stores `data` under `id`, replacing what was there. The session expires when it is
    /// not saved again within `ttl`, and in any case at its absolute timeout.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written.
    async fn save(&self, id: &str, data: &SessionData, ttl: Duration) -> Result<(), SessionError>;

    /// Removes the session stored under `id`. An unknown id is not an error.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written.
    async fn delete(&self, id: &str) -> Result<(), SessionError>;

    /// Moves the session from `old` to `new` and removes `old`, keeping its creation time.
    /// An unknown `old` is not an error and stores nothing.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written.
    async fn rotate(&self, old: &str, new: &str) -> Result<(), SessionError>;

    /// Periodic housekeeping, such as removing expired sessions. The session filter calls it
    /// at most once a minute. The default does nothing.
    async fn maintain(&self) {}
}
