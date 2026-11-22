//! The per-request session handle, its state, and session ids.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::SystemTime;

use aws_lc_rs::digest::{SHA256, digest};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::error::SessionError;
use super::store::SessionData;

/// The prefix of the session keys Oxbloom reserves for itself.
pub(crate) const RESERVED_PREFIX: &str = "oxbloom.security.";

fn is_reserved(key: &str) -> bool {
    key.starts_with(RESERVED_PREFIX)
}

/// Mints a new session id: 256 random bits from the system random number generator,
/// base64url without padding (43 characters).
///
/// # Panics
///
/// Panics when the system random number generator fails, because no safe id can be made
/// without it.
pub(crate) fn new_session_id() -> String {
    let mut bytes = [0u8; 32];
    aws_lc_rs::rand::fill(&mut bytes).expect("the system random number generator failed");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The first 8 hex characters of the SHA-256 of `id`: the only form of an id a log line may
/// carry.
pub(crate) fn id_fingerprint(id: &str) -> String {
    let hash = digest(&SHA256, id.as_bytes());
    hash.as_ref()[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The state behind a [`Session`] handle, which the session filter reads after the request.
///
/// How the filter reads it:
///
/// - `invalidated`: delete [`stored_id`](Self::stored_id) and expire the cookie.
/// - `rotated`: move the stored data from `previous_id` to `id`, then save if dirty.
/// - `previous_id` set and not `rotated`: the session was invalidated and then started anew;
///   delete `previous_id` and save the new session under `id`.
/// - [`is_empty_untouched`](Self::is_empty_untouched): store nothing and set no cookie.
#[derive(Clone)]
pub(crate) struct SessionState {
    /// The current id. For a new session the store does not know it yet.
    pub(crate) id: String,
    /// The session contents.
    pub(crate) data: SessionData,
    /// Whether the session was minted during this request rather than loaded.
    pub(crate) is_new: bool,
    /// Whether a value was set or removed.
    pub(crate) dirty: bool,
    /// Whether the session was invalidated.
    pub(crate) invalidated: bool,
    /// Whether the id was rotated away from a stored id held in `previous_id`.
    pub(crate) rotated: bool,
    /// The id under which the store holds this session's data, when that is no longer `id`.
    pub(crate) previous_id: Option<String>,
    /// The time of the request, used as the creation time when the session starts anew.
    pub(crate) now: SystemTime,
}

impl SessionState {
    /// The state of a session loaded from the store under `id`.
    pub(crate) fn loaded(id: String, data: SessionData, now: SystemTime) -> Self {
        Self {
            id,
            data,
            is_new: false,
            dirty: false,
            invalidated: false,
            rotated: false,
            previous_id: None,
            now,
        }
    }

    /// The state of a fresh, empty session with a newly minted id.
    pub(crate) fn fresh(now: SystemTime) -> Self {
        Self {
            is_new: true,
            ..Self::loaded(new_session_id(), SessionData::new(now), now)
        }
    }

    /// The id the store currently holds this session under, if any.
    pub(crate) fn stored_id(&self) -> Option<&str> {
        match &self.previous_id {
            Some(previous) => Some(previous),
            None if !self.is_new => Some(&self.id),
            None => None,
        }
    }

    /// Whether this is a new session that was never written to and holds nothing. Such a
    /// session is never stored and sets no cookie.
    pub(crate) fn is_empty_untouched(&self) -> bool {
        self.is_new
            && self.data.is_empty()
            && !self.dirty
            && !self.invalidated
            && !self.rotated
            && self.previous_id.is_none()
    }

    /// Starts a new empty session with a new id after an invalidation, remembering the id
    /// the store still holds so that the filter deletes it.
    fn restart(&mut self) {
        let stored = self.stored_id().map(str::to_owned);
        *self = Self {
            previous_id: stored,
            ..Self::fresh(self.now)
        };
    }
}

impl Default for SessionState {
    /// A dead placeholder, left behind by [`Session::take_state`].
    fn default() -> Self {
        Self::loaded(
            String::new(),
            SessionData::new(SystemTime::UNIX_EPOCH),
            SystemTime::UNIX_EPOCH,
        )
    }
}

/// The session of the current request.
///
/// A cheap handle: clones share the same state. Changes are kept in memory and the session
/// filter writes them back once, at the end of the request. Values are stored as JSON, so
/// any `Serialize` type can be set and read back as any `DeserializeOwned` type of the same
/// shape.
///
/// After [`invalidate`](Self::invalidate) the session is empty and is deleted at the end of
/// the request. A later [`set`](Self::set) in the same request starts a new session with a
/// new id that holds only the new value; the old session is still deleted.
///
/// `Debug` prints `Session { .. }`, never the id or a value.
///
/// A `Session` is created by the [`SessionFilter`](crate::SessionFilter); handlers read it from
/// the request with [`RequestSessionExt::session`](crate::RequestSessionExt::session).
///
/// The filter takes the state out of the handle when the rest of the chain has answered. A
/// clone kept past that point, for example moved into a spawned task, then holds an empty,
/// detached placeholder: what it reads is empty and what it writes is lost silently.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Session, SessionError};
///
/// fn count_visit(session: &Session) -> Result<u32, SessionError> {
///     let visits = session.get::<u32>("visits")?.unwrap_or(0) + 1;
///     session.set("visits", &visits)?;
///     Ok(visits)
/// }
/// ```
#[derive(Clone)]
pub struct Session {
    state: Arc<Mutex<SessionState>>,
}

impl Session {
    /// Wraps a state in a new handle.
    pub(crate) fn from_state(state: SessionState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Takes the state out of the handle, leaving a dead placeholder behind.
    pub(crate) fn take_state(&self) -> SessionState {
        std::mem::take(&mut *self.lock())
    }

    fn lock(&self) -> MutexGuard<'_, SessionState> {
        // Every change below completes before the guard drops, so a poisoned lock holds a
        // consistent state.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current session id. Treat it as a secret: never log it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Session;
    ///
    /// fn same_session(a: &Session, b: &Session) -> bool {
    ///     a.id() == b.id()
    /// }
    /// ```
    #[must_use]
    pub fn id(&self) -> String {
        self.lock().id.clone()
    }

    /// Whether the session was started during this request rather than sent by the client.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Session;
    ///
    /// fn greeting(session: &Session) -> &'static str {
    ///     if session.is_new() { "welcome" } else { "welcome back" }
    /// }
    /// ```
    #[must_use]
    pub fn is_new(&self) -> bool {
        self.lock().is_new
    }

    /// Reads the value under `key`, or `None` when there is none.
    ///
    /// # Errors
    ///
    /// [`SessionError::Serialization`] when the stored value does not fit `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Session, SessionError};
    ///
    /// fn user_name(session: &Session) -> Result<Option<String>, SessionError> {
    ///     session.get::<String>("user")
    /// }
    /// ```
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, SessionError> {
        let value = self.lock().data.values().get(key).cloned();
        value
            .map(|value| serde_json::from_value(value).map_err(|_| SessionError::Serialization))
            .transpose()
    }

    /// Stores `value` under `key` and marks the session for writing back. After
    /// [`invalidate`](Self::invalidate), this starts a new session with a new id.
    ///
    /// Keys that start with `oxbloom.security.` are reserved for Oxbloom's own login, CSRF and
    /// session bookkeeping, so that application code cannot overwrite them by accident.
    ///
    /// # Errors
    ///
    /// [`SessionError::ReservedKey`] when `key` starts with `oxbloom.security.`.
    /// [`SessionError::Serialization`] when `value` cannot be turned into JSON. The session
    /// is unchanged in both cases.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Session, SessionError};
    ///
    /// fn remember_theme(session: &Session, theme: &str) -> Result<(), SessionError> {
    ///     session.set("theme", &theme)
    /// }
    ///
    /// fn try_reserved(session: &Session) {
    ///     let result = session.set("oxbloom.security.authentication", "x");
    ///     assert_eq!(result, Err(SessionError::ReservedKey));
    /// }
    /// ```
    pub fn set<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<(), SessionError> {
        if is_reserved(key) {
            return Err(SessionError::ReservedKey);
        }
        self.set_reserved(key, value)
    }

    /// Stores `value` under `key` without the check for the reserved prefix. For Oxbloom's own
    /// writers and this repository's tests; application code uses [`set`](Self::set).
    ///
    /// # Errors
    ///
    /// [`SessionError::Serialization`] when `value` cannot be turned into JSON.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Session, SessionError};
    ///
    /// fn plant(session: &Session) -> Result<(), SessionError> {
    ///     session.set_reserved("oxbloom.security.x", "value")
    /// }
    /// ```
    #[doc(hidden)]
    pub fn set_reserved<T: Serialize + ?Sized>(
        &self,
        key: &str,
        value: &T,
    ) -> Result<(), SessionError> {
        let value = serde_json::to_value(value).map_err(|_| SessionError::Serialization)?;
        let mut state = self.lock();
        if state.invalidated {
            state.restart();
        }
        state.data.values_mut().insert(key.to_owned(), value);
        state.dirty = true;
        Ok(())
    }

    /// Removes the value under `key`, if any. A key that starts with `oxbloom.security.` is
    /// reserved and is silently ignored.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Session;
    ///
    /// fn forget_cart(session: &Session) {
    ///     session.remove("cart");
    /// }
    /// ```
    pub fn remove(&self, key: &str) {
        if is_reserved(key) {
            return;
        }
        self.remove_reserved(key);
    }

    /// Removes the value under `key` without the check for the reserved prefix. For Oxbloom's
    /// own writers and this repository's tests; application code uses [`remove`](Self::remove).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Session;
    ///
    /// fn drop_marker(session: &Session) {
    ///     session.remove_reserved("oxbloom.security.x");
    /// }
    /// ```
    #[doc(hidden)]
    pub fn remove_reserved(&self, key: &str) {
        let mut state = self.lock();
        if state.data.values_mut().remove(key).is_some() {
            state.dirty = true;
        }
    }

    /// Ends the session: its values are dropped now, the stored session is deleted and the
    /// cookie expired at the end of the request.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Session;
    ///
    /// fn log_out(session: &Session) {
    ///     session.invalidate();
    /// }
    /// ```
    pub fn invalidate(&self) {
        let mut state = self.lock();
        state.data.values_mut().clear();
        state.invalidated = true;
        state.dirty = false;
    }

    /// Moves the session to a new id, keeping its values and its creation time, so that an
    /// id known before a login stops working after it (session fixation). The old id is
    /// deleted at the end of the request. It does nothing on an invalidated session.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Session, SessionError};
    ///
    /// fn on_login(session: &Session, user: &str) -> Result<(), SessionError> {
    ///     session.rotate_id();
    ///     session.set("user", user)
    /// }
    /// ```
    pub fn rotate_id(&self) {
        let mut state = self.lock();
        if state.invalidated {
            return;
        }
        // A session minted in this request, also one restarted after `invalidate`, has nothing
        // stored under its id to move: it only gets a new id. A restarted session keeps its
        // `previous_id`, so the filter still deletes the old session and saves the new one.
        if !state.is_new
            && let Some(stored) = state.stored_id().map(str::to_owned)
        {
            state.previous_id = Some(stored);
            state.rotated = true;
        }
        state.id = new_session_id();
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Session { .. }")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)
    }

    fn loaded_session(values: &[(&str, serde_json::Value)]) -> Session {
        let mut data = SessionData::new(now());
        for (key, value) in values {
            data.values_mut().insert((*key).to_owned(), value.clone());
        }
        Session::from_state(SessionState::loaded(new_session_id(), data, now()))
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Cart {
        items: Vec<String>,
        total: u32,
    }

    #[test]
    fn session_get_set_remove_round_trip() {
        let session = Session::from_state(SessionState::fresh(now()));
        assert!(session.is_new());
        assert_eq!(session.get::<String>("name").unwrap(), None);

        let cart = Cart {
            items: vec!["apple".to_owned()],
            total: 3,
        };
        session.set("name", "alice").unwrap();
        session.set("count", &7u32).unwrap();
        session.set("cart", &cart).unwrap();

        assert_eq!(
            session.get::<String>("name").unwrap().as_deref(),
            Some("alice")
        );
        assert_eq!(session.get::<u32>("count").unwrap(), Some(7));
        assert_eq!(session.get::<Cart>("cart").unwrap(), Some(cart));
        assert_eq!(session.get::<u32>("name"), Err(SessionError::Serialization));

        session.remove("name");
        assert_eq!(session.get::<String>("name").unwrap(), None);

        // Clones share the state.
        let clone = session.clone();
        assert_eq!(clone.get::<u32>("count").unwrap(), Some(7));
        assert_eq!(clone.id(), session.id());

        let state = session.take_state();
        assert!(state.dirty);
        assert!(!state.is_empty_untouched());
        assert_eq!(state.data.values().len(), 2);
    }

    #[test]
    fn remove_of_a_missing_key_does_not_mark_dirty() {
        let session = loaded_session(&[("k", json!(1))]);
        session.remove("other");
        assert!(!session.take_state().dirty);
    }

    #[test]
    fn empty_untouched_is_detectable() {
        let fresh = Session::from_state(SessionState::fresh(now()));
        let _ = fresh.get::<u32>("anything");
        assert!(fresh.take_state().is_empty_untouched());

        let loaded = loaded_session(&[]);
        assert!(
            !loaded.take_state().is_empty_untouched(),
            "a loaded session is not new"
        );
    }

    #[test]
    fn new_ids_are_43_chars_and_unique() {
        let ids: HashSet<String> = (0..1000).map(|_| new_session_id()).collect();
        assert_eq!(ids.len(), 1000);
        for id in &ids {
            assert_eq!(id.len(), 43);
            assert!(
                id.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "not base64url: {id}"
            );
        }
    }

    #[test]
    fn fingerprint_is_8_hex_and_stable() {
        let id = new_session_id();
        let fp = id_fingerprint(&id);
        assert_eq!(fp.len(), 8);
        assert!(
            fp.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(fp, id_fingerprint(&id));
        assert_ne!(fp, id_fingerprint("another id"));
        // SHA-256("abc") starts with ba7816bf.
        assert_eq!(id_fingerprint("abc"), "ba7816bf");
    }

    #[test]
    fn invalidate_then_set_starts_a_new_session() {
        let session = loaded_session(&[("old", json!("value"))]);
        let old_id = session.id();

        session.invalidate();
        assert_eq!(session.get::<String>("old").unwrap(), None);
        session.set("new", &1u32).unwrap();

        assert_ne!(session.id(), old_id);
        assert!(session.is_new());
        assert_eq!(session.get::<String>("old").unwrap(), None);
        assert_eq!(session.get::<u32>("new").unwrap(), Some(1));

        let state = session.take_state();
        assert!(!state.invalidated);
        assert!(!state.rotated);
        assert!(state.dirty);
        assert_eq!(state.previous_id.as_deref(), Some(old_id.as_str()));
        assert_eq!(state.data.values().len(), 1);
        assert_eq!(state.data.created(), now());
    }

    #[test]
    fn invalidate_then_set_on_a_new_session_has_nothing_to_delete() {
        let session = Session::from_state(SessionState::fresh(now()));
        session.invalidate();
        session.set("k", &1u32).unwrap();
        let state = session.take_state();
        assert_eq!(state.previous_id, None);
        assert!(state.dirty);
    }

    #[test]
    fn rotate_then_invalidate_is_invalidated() {
        let session = loaded_session(&[("k", json!(1))]);
        let old_id = session.id();

        session.rotate_id();
        assert_ne!(session.id(), old_id);
        session.invalidate();

        let state = session.take_state();
        assert!(state.invalidated);
        assert!(state.data.is_empty());
        assert_eq!(
            state.stored_id(),
            Some(old_id.as_str()),
            "the stored session is deleted"
        );
    }

    #[test]
    fn rotate_keeps_values_and_the_first_stored_id() {
        let session = loaded_session(&[("k", json!(1))]);
        let old_id = session.id();
        session.rotate_id();
        session.rotate_id();
        assert_eq!(session.get::<u32>("k").unwrap(), Some(1));

        let state = session.take_state();
        assert!(state.rotated);
        assert_ne!(state.id, old_id);
        assert_eq!(state.previous_id.as_deref(), Some(old_id.as_str()));
    }

    #[test]
    fn rotate_on_a_new_session_only_changes_the_id() {
        let session = Session::from_state(SessionState::fresh(now()));
        let first = session.id();
        session.rotate_id();
        assert_ne!(session.id(), first);
        let state = session.take_state();
        assert!(!state.rotated);
        assert_eq!(state.previous_id, None);
    }

    #[test]
    fn rotate_after_a_restart_keeps_the_delete_and_save_path() {
        let session = loaded_session(&[("old", json!("value"))]);
        let old_id = session.id();
        session.invalidate();
        session.set("new", &1u32).unwrap();
        let restarted = session.id();
        session.rotate_id();
        assert_ne!(session.id(), restarted);

        let state = session.take_state();
        assert!(state.is_new);
        assert!(
            !state.rotated,
            "nothing stored under the restarted id to move"
        );
        assert_eq!(state.previous_id.as_deref(), Some(old_id.as_str()));
    }

    #[test]
    fn debug_shows_no_id_or_value() {
        let session = loaded_session(&[("secret-key", json!("secret-value"))]);
        let id = session.id();
        let text = format!("{session:?}");
        assert_eq!(text, "Session { .. }");
        assert!(!text.contains(&id));
        assert!(!text.contains("secret"));
    }
}
