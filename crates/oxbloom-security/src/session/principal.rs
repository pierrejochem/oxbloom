//! The per-user view of sessions: the [`PrincipalSessions`] trait that a store implements and
//! the [`SessionRegistry`] that applications use.

use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;

use super::error::SessionError;

/// The reserved session key that holds the username of the logged-in principal, as a JSON
/// string. Form login writes it at every successful login.
pub(crate) const PRINCIPAL_KEY: &str = "oxbloom.security.principal";

/// What is known about one live session of a user: its id, when it was created and when it
/// was last touched.
///
/// The id is a credential, so `Debug` prints neither the id nor the times.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::{Duration, SystemTime};
/// use oxbloom_core::SystemClock;
/// use oxbloom_security::{InMemorySessionStore, SessionData, SessionSettings, SessionStore};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let settings = SessionSettings::default();
/// let store = Arc::new(InMemorySessionStore::new(&settings, Arc::new(SystemClock)));
/// let mut data = SessionData::new(SystemTime::now());
/// data.values_mut().insert("oxbloom.security.principal".to_owned(), serde_json::json!("alice"));
/// store.save("an-id", &data, Duration::from_secs(60)).await.unwrap();
///
/// let index = store.principal_sessions().unwrap();
/// let sessions = index.sessions_of("alice").await.unwrap();
/// assert_eq!(sessions[0].id(), "an-id");
/// assert_eq!(sessions[0].created(), data.created());
/// assert_eq!(sessions[0].touched(), data.touched());
/// assert_eq!(format!("{:?}", sessions[0]), "SessionSummary { .. }");
/// # });
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct SessionSummary {
    id: String,
    created: SystemTime,
    touched: SystemTime,
}

impl SessionSummary {
    pub(crate) fn new(id: String, created: SystemTime, touched: SystemTime) -> Self {
        Self {
            id,
            created,
            touched,
        }
    }

    /// The session id. Treat it as a secret.
    ///
    /// # Examples
    ///
    /// ```
    /// # use oxbloom_security::SessionSummary;
    /// fn show(summary: &SessionSummary) -> usize {
    ///     summary.id().len()
    /// }
    /// ```
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// When the session was created.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::SystemTime;
    /// # use oxbloom_security::SessionSummary;
    /// fn created(summary: &SessionSummary) -> SystemTime {
    ///     summary.created()
    /// }
    /// ```
    #[must_use]
    pub fn created(&self) -> SystemTime {
        self.created
    }

    /// When the session was last touched.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::SystemTime;
    /// # use oxbloom_security::SessionSummary;
    /// fn touched(summary: &SessionSummary) -> SystemTime {
    ///     summary.touched()
    /// }
    /// ```
    #[must_use]
    pub fn touched(&self) -> SystemTime {
        self.touched
    }
}

impl fmt::Debug for SessionSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionSummary { .. }")
    }
}

/// A store that can find, delete and limit the sessions of one user.
///
/// A session belongs to a user when its value under the reserved key
/// `oxbloom.security.principal` is the username as a JSON string, which form login writes.
/// Sessions without that key belong to nobody and are never listed or deleted here. Only live
/// sessions count. [`InMemorySessionStore`](crate::InMemorySessionStore) implements the trait
/// by scanning its shards, one lock at a time; reach it with
/// [`SessionStore::principal_sessions`](crate::SessionStore::principal_sessions).
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::{Duration, SystemTime};
/// use oxbloom_core::SystemClock;
/// use oxbloom_security::{InMemorySessionStore, SessionData, SessionSettings, SessionStore};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let store = Arc::new(InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock)));
/// for id in ["a", "b", "c"] {
///     let mut data = SessionData::new(SystemTime::now());
///     data.values_mut().insert("oxbloom.security.principal".to_owned(), serde_json::json!("alice"));
///     store.save(id, &data, Duration::from_secs(60)).await.unwrap();
/// }
/// let index = store.principal_sessions().unwrap();
/// assert_eq!(index.sessions_of("alice").await.unwrap().len(), 3);
/// assert_eq!(index.evict_oldest("alice", 1, Some("a")).await.unwrap(), 1);
/// assert_eq!(index.delete_of("alice", None).await.unwrap(), 2);
/// # });
/// ```
#[async_trait]
pub trait PrincipalSessions: Send + Sync {
    /// The live sessions of `principal`, the one touched longest ago first.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be read.
    async fn sessions_of(&self, principal: &str) -> Result<Vec<SessionSummary>, SessionError>;

    /// Deletes every session of `principal` except the one with id `except`, and returns how
    /// many were deleted.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written.
    async fn delete_of(&self, principal: &str, except: Option<&str>)
    -> Result<usize, SessionError>;

    /// Deletes the sessions of `principal` touched longest ago, never the one with id `except`,
    /// until at most `keep` other sessions remain, and returns how many were deleted.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written.
    async fn evict_oldest(
        &self,
        principal: &str,
        keep: usize,
        except: Option<&str>,
    ) -> Result<usize, SessionError>;
}

/// Lists and ends the sessions of a user, for "sign out everywhere" and "sign out other
/// devices" features.
///
/// Cloning is cheap. `Debug` prints no session data.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::{Duration, SystemTime};
/// use oxbloom_core::SystemClock;
/// use oxbloom_security::{
///     InMemorySessionStore, SessionData, SessionRegistry, SessionSettings, SessionStore,
/// };
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let store = Arc::new(InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock)));
/// for id in ["phone", "laptop"] {
///     let mut data = SessionData::new(SystemTime::now());
///     data.values_mut().insert("oxbloom.security.principal".to_owned(), serde_json::json!("alice"));
///     store.save(id, &data, Duration::from_secs(60)).await.unwrap();
/// }
/// let registry = SessionRegistry::new(store.principal_sessions().unwrap());
/// assert_eq!(registry.sessions_of("alice").await.unwrap().len(), 2);
/// assert_eq!(registry.invalidate_others("alice", "laptop").await.unwrap(), 1);
/// assert_eq!(registry.invalidate_all("alice").await.unwrap(), 1);
/// assert_eq!(format!("{registry:?}"), "SessionRegistry { .. }");
/// # });
/// ```
#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<dyn PrincipalSessions>,
}

impl SessionRegistry {
    /// Creates a registry over `inner`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionRegistry, SessionSettings, SessionStore};
    ///
    /// let store = Arc::new(InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock)));
    /// let _registry = SessionRegistry::new(store.principal_sessions().unwrap());
    /// ```
    #[must_use]
    pub fn new(inner: Arc<dyn PrincipalSessions>) -> Self {
        Self { inner }
    }

    /// The live sessions of user `name`, the one touched longest ago first.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be read.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionRegistry, SessionSettings, SessionStore};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let store = Arc::new(InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock)));
    /// let registry = SessionRegistry::new(store.principal_sessions().unwrap());
    /// assert!(registry.sessions_of("alice").await.unwrap().is_empty());
    /// # });
    /// ```
    pub async fn sessions_of(&self, name: &str) -> Result<Vec<SessionSummary>, SessionError> {
        self.inner.sessions_of(name).await
    }

    /// Ends every session of user `name` and returns how many there were.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionRegistry, SessionSettings, SessionStore};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let store = Arc::new(InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock)));
    /// let registry = SessionRegistry::new(store.principal_sessions().unwrap());
    /// assert_eq!(registry.invalidate_all("alice").await.unwrap(), 0);
    /// # });
    /// ```
    pub async fn invalidate_all(&self, name: &str) -> Result<usize, SessionError> {
        self.inner.delete_of(name, None).await
    }

    /// Ends every session of user `name` except the one with id `current_id`, and returns how
    /// many were ended.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{InMemorySessionStore, SessionRegistry, SessionSettings, SessionStore};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let store = Arc::new(InMemorySessionStore::new(&SessionSettings::default(), Arc::new(SystemClock)));
    /// let registry = SessionRegistry::new(store.principal_sessions().unwrap());
    /// assert_eq!(registry.invalidate_others("alice", "current").await.unwrap(), 0);
    /// # });
    /// ```
    pub async fn invalidate_others(
        &self,
        name: &str,
        current_id: &str,
    ) -> Result<usize, SessionError> {
        self.inner.delete_of(name, Some(current_id)).await
    }
}

impl fmt::Debug for SessionRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionRegistry { .. }")
    }
}
