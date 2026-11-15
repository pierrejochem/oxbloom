//! The choice between the server-side and the stateless session backend, and how each loads
//! and commits a session.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::cookie_backend::CookieSessionBackend;
use super::error::SessionError;
use super::handle::{SessionState, id_fingerprint};
use super::settings::SessionSettings;
use super::store::SessionStore;

/// How many values of the session cookie a request may offer before the rest are ignored, so
/// that a request cannot make the filter run many store lookups or decryptions.
const MAX_CANDIDATES: usize = 8;

/// An untouched session is written back only when its last touch is older than this, so a
/// read does not cause a store write on every request.
const TOUCH_INTERVAL: Duration = Duration::from_secs(60);

/// The length of a server-minted session id: 32 bytes in base64url without padding.
const ID_LEN: usize = 43;

/// Where the sessions of the [`SessionFilter`](crate::SessionFilter) live.
///
/// - `Server`: the data stays in a [`SessionStore`] and the cookie carries only a random id
///   that the server minted. An id the client sends that the store does not know is never
///   adopted.
/// - `Cookie`: the data travels in the cookie itself, sealed by a [`CookieSessionBackend`].
///   Such a session has no stored id, so [`Session::id`](crate::Session::id) is a fresh random
///   value on each request and is only stable within one request.
///
/// The settings are validated by [`SessionFilter::new`](crate::SessionFilter::new), which every
/// backend passes through before it serves a request;
/// [`InMemorySessionStore::new`](crate::InMemorySessionStore::new) does not validate on its own.
///
/// `Debug` prints only the kind of backend, never a key or a session.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_core::SystemClock;
/// use oxbloom_security::{InMemorySessionStore, SessionBackend, SessionSettings};
///
/// let settings = SessionSettings::default();
/// let store = InMemorySessionStore::new(&settings, Arc::new(SystemClock));
/// let backend = SessionBackend::Server(Arc::new(store));
/// assert_eq!(format!("{backend:?}"), "SessionBackend::Server(..)");
/// ```
#[derive(Clone)]
pub enum SessionBackend {
    /// Sessions kept in a store, keyed by a random id in the cookie.
    Server(Arc<dyn SessionStore>),
    /// Sessions kept in the cookie, encrypted and authenticated.
    Cookie(Arc<CookieSessionBackend>),
}

/// What the filter does with the session cookie after the request.
#[derive(PartialEq, Eq)]
pub(crate) enum CookieAction {
    /// Leave the cookie alone.
    None,
    /// Set the cookie to `value` for `max_age`.
    Set { value: String, max_age: Duration },
    /// Expire the cookie.
    Clear,
}

/// Whether `touched` is more than [`TOUCH_INTERVAL`] before `now`. A touch in the future, after
/// the clock moved back, is not due.
fn touch_due(touched: SystemTime, now: SystemTime) -> bool {
    now.duration_since(touched)
        .is_ok_and(|age| age > TOUCH_INTERVAL)
}

/// The cookie lifetime: the idle timeout, cut short by the absolute timeout.
fn max_age(created: SystemTime, now: SystemTime, settings: &SessionSettings) -> Duration {
    let remaining = created
        .checked_add(settings.absolute_timeout)
        .map_or(settings.absolute_timeout, |deadline| {
            deadline.duration_since(now).unwrap_or(Duration::ZERO)
        });
    settings.idle_timeout.min(remaining)
}

impl SessionBackend {
    /// Loads the session the cookie values name, trying them in order.
    ///
    /// The server backend uses the first value the store knows and otherwise starts a new
    /// session with a freshly minted id; the cookie backend uses the first value that opens and
    /// otherwise starts a new empty session.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be read.
    pub(crate) async fn load(
        &self,
        cookie_values: &[String],
        now: SystemTime,
    ) -> Result<SessionState, SessionError> {
        match self {
            Self::Server(store) => {
                // Only a value of the shape the server mints can be a stored id.
                let candidates = cookie_values.iter().filter(|id| id.len() == ID_LEN);
                for id in candidates.take(MAX_CANDIDATES) {
                    if let Some(data) = store.load(id).await? {
                        return Ok(SessionState::loaded(id.clone(), data, now));
                    }
                }
            }
            Self::Cookie(backend) => {
                for value in cookie_values.iter().take(MAX_CANDIDATES) {
                    if let Some(data) = backend.open(value) {
                        let fresh = SessionState::fresh(now);
                        return Ok(SessionState::loaded(fresh.id, data, now));
                    }
                }
            }
        }
        Ok(SessionState::fresh(now))
    }

    /// Writes the session back after the request and tells the filter what to do with the
    /// cookie. The rules are those documented on [`SessionState`].
    ///
    /// A session loaded from the store is written with the conditional
    /// [`SessionStore::update`] and moved with [`SessionStore::rotate`]; only a session minted
    /// in this request is written with [`SessionStore::save`]. When the stored session is gone
    /// by commit time, because a concurrent request invalidated it or it expired, nothing is
    /// stored and the cookie is cleared, so a slow request can never bring a logged-out session
    /// back. A session whose absolute deadline has passed is cleared without a write too.
    ///
    /// # Errors
    ///
    /// [`SessionError::Store`] when the store cannot be written, and the errors of
    /// [`CookieSessionBackend::seal`], such as [`SessionError::TooLarge`].
    pub(crate) async fn commit(
        &self,
        mut state: SessionState,
        now: SystemTime,
        settings: &SessionSettings,
    ) -> Result<CookieAction, SessionError> {
        if state.invalidated {
            if let (Self::Server(store), Some(stored)) = (self, state.stored_id()) {
                store.delete(stored).await?;
                tracing::debug!(
                    target: "oxbloom::security",
                    session = %id_fingerprint(stored),
                    "the session was invalidated"
                );
            }
            return Ok(CookieAction::Clear);
        }
        if state.is_empty_untouched() {
            return Ok(CookieAction::None);
        }
        let write = state.dirty
            || state.previous_id.is_some()
            || (state.is_new && !state.data.is_empty())
            || touch_due(state.data.touched(), now);
        if !write {
            return Ok(CookieAction::None);
        }
        let max_age = max_age(state.data.created(), now, settings);
        if max_age < Duration::from_secs(1) {
            // The absolute deadline has passed: the session is over, so nothing is written.
            return Ok(CookieAction::Clear);
        }
        // Sealing and saving never update the touch time themselves.
        state.data.touch(now);
        let value = match self {
            Self::Server(store) => {
                if !Self::write_server(&**store, &state, settings.idle_timeout).await? {
                    return Ok(CookieAction::Clear);
                }
                state.id
            }
            Self::Cookie(backend) => backend.seal(&state.data)?,
        };
        Ok(CookieAction::Set { value, max_age })
    }

    /// Writes a server session. Returns `false`, having stored nothing, when the session this
    /// request loaded no longer exists in the store.
    async fn write_server(
        store: &dyn SessionStore,
        state: &SessionState,
        ttl: Duration,
    ) -> Result<bool, SessionError> {
        let lost = |id: &str| {
            tracing::debug!(
                target: "oxbloom::security",
                session = %id_fingerprint(id),
                "the session ended during the request; its cookie is cleared"
            );
            Ok(false)
        };
        match (state.previous_id.as_deref(), state.rotated) {
            (Some(previous), true) => {
                // Move first: after a concurrent logout there is nothing to move, and nothing
                // may be stored under the new id.
                if !store.rotate(previous, &state.id).await? {
                    return lost(previous);
                }
                tracing::debug!(
                    target: "oxbloom::security",
                    from = %id_fingerprint(previous),
                    to = %id_fingerprint(&state.id),
                    "the session id was rotated"
                );
                if !store.update(&state.id, &state.data, ttl).await? {
                    return lost(&state.id);
                }
            }
            (Some(previous), false) => {
                // Invalidated, then started anew: the old session goes, the new one was minted
                // in this request.
                store.delete(previous).await?;
                tracing::debug!(
                    target: "oxbloom::security",
                    session = %id_fingerprint(previous),
                    "the session was invalidated"
                );
                store.save(&state.id, &state.data, ttl).await?;
            }
            (None, _) if state.is_new => {
                tracing::debug!(
                    target: "oxbloom::security",
                    session = %id_fingerprint(&state.id),
                    "a session was created"
                );
                store.save(&state.id, &state.data, ttl).await?;
            }
            (None, _) => {
                if !store.update(&state.id, &state.data, ttl).await? {
                    return lost(&state.id);
                }
            }
        }
        Ok(true)
    }

    /// Periodic housekeeping of the server store; nothing for the cookie backend.
    pub(crate) async fn maintain(&self) {
        if let Self::Server(store) = self {
            store.maintain().await;
        }
    }
}

impl fmt::Debug for CookieAction {
    /// Prints the action without the cookie value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("None"),
            Self::Set { max_age, .. } => f
                .debug_struct("Set")
                .field("max_age", max_age)
                .finish_non_exhaustive(),
            Self::Clear => f.write_str("Clear"),
        }
    }
}

impl fmt::Debug for SessionBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Server(_) => "SessionBackend::Server(..)",
            Self::Cookie(_) => "SessionBackend::Cookie(..)",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[tokio::test]
    async fn a_passed_absolute_deadline_clears_without_a_write() {
        use oxbloom_core::Clock;

        use super::super::memory::InMemorySessionStore;
        use super::super::store::SessionData;

        struct Fixed(SystemTime);
        impl Clock for Fixed {
            fn now(&self) -> SystemTime {
                self.0
            }
        }

        let settings = SessionSettings::default()
            .idle_timeout(Duration::from_secs(100))
            .absolute_timeout(Duration::from_secs(1000));
        // Loaded while alive at 990 s, committed at 1001 s, after the absolute deadline.
        let store = Arc::new(InMemorySessionStore::new(
            &settings,
            Arc::new(Fixed(at(990))),
        ));
        let mut data = SessionData::new(at(0));
        data.values_mut()
            .insert("k".to_owned(), serde_json::json!(1));
        store
            .save("id", &data, settings.idle_timeout)
            .await
            .unwrap();
        let mut state = SessionState::loaded("id".to_owned(), data, at(990));
        state.dirty = true;

        let backend = SessionBackend::Server(store.clone());
        let action = backend.commit(state, at(1001), &settings).await.unwrap();
        assert_eq!(action, CookieAction::Clear);
        let stored = store.load("id").await.unwrap().unwrap();
        assert_eq!(stored.touched(), at(0), "nothing was written");
    }

    #[test]
    fn touch_is_due_only_after_more_than_a_minute() {
        assert!(!touch_due(at(100), at(160)));
        assert!(touch_due(at(100), at(161)));
        assert!(!touch_due(at(200), at(100)), "a touch in the future");
    }

    #[test]
    fn max_age_is_cut_by_the_absolute_timeout() {
        let settings = SessionSettings::default()
            .idle_timeout(Duration::from_secs(100))
            .absolute_timeout(Duration::from_secs(1000));
        assert_eq!(max_age(at(0), at(10), &settings), Duration::from_secs(100));
        assert_eq!(max_age(at(0), at(950), &settings), Duration::from_secs(50));
        assert_eq!(max_age(at(0), at(2000), &settings), Duration::ZERO);
    }
}
