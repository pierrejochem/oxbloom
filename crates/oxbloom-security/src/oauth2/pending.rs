//! The record of a login that has been started and not yet completed.
//!
//! Each pending login lives under its own reserved session key, so two browser tabs do not
//! overwrite each other. An index key lists the states, and keeps at most [`MAX_PENDING`].

use std::fmt;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::session::{Session, SessionError};

/// The prefix of the per-state keys.
const PENDING_PREFIX: &str = "oxbloom.security.oauth2.pending.";
/// The key of the list of states, oldest first.
const INDEX_KEY: &str = "oxbloom.security.oauth2.pending-index";
/// How many pending logins one session keeps.
pub(crate) const MAX_PENDING: usize = 5;

/// A started login: what the callback needs to finish it.
///
/// `Debug` does not show the state, the nonce or the verifier.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pending {
    /// The registration id the login was started for.
    pub(crate) registration: String,
    /// The `state` parameter sent to the provider.
    pub(crate) state: String,
    /// The `nonce` sent to an OpenID Connect provider.
    pub(crate) nonce: Option<String>,
    /// The PKCE code verifier.
    pub(crate) verifier: String,
    /// The request to return to after the login, when one was saved.
    pub(crate) saved_request: Option<String>,
    /// When the login was started.
    pub(crate) created_at: SystemTime,
}

impl fmt::Debug for Pending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pending")
            .field("registration", &self.registration)
            .field("state", &"<redacted>")
            .field("nonce", &self.nonce.as_ref().map(|_| "<redacted>"))
            .field("verifier", &"<redacted>")
            .field("saved_request", &self.saved_request)
            .field("created_at", &self.created_at)
            .finish()
    }
}

impl Pending {
    /// Stores the record and adds its state to the index. A sixth state drops the oldest.
    ///
    /// # Errors
    ///
    /// [`SessionError::Serialization`] when the record or the index cannot be written.
    pub(crate) fn store(&self, session: &Session) -> Result<(), SessionError> {
        let mut index = read_index(session);
        index.retain(|state| *state != self.state);
        index.push(self.state.clone());
        while index.len() > MAX_PENDING {
            let dropped = index.remove(0);
            session.remove_reserved(&key_for(&dropped));
        }
        session.set_reserved(&key_for(&self.state), self)?;
        session.set_reserved(INDEX_KEY, &index)
    }

    /// Removes the record for `state` and returns it when it is still valid: it is removed
    /// whether or not it has expired, so it can be used once. A record older than `max_age`
    /// is invalid.
    pub(crate) fn take(
        session: &Session,
        state: &str,
        now: SystemTime,
        max_age: Duration,
    ) -> Option<Pending> {
        let key = key_for(state);
        let found = session.get::<Pending>(&key).ok().flatten();
        session.remove_reserved(&key);
        let mut index = read_index(session);
        if index.iter().any(|entry| entry == state) {
            index.retain(|entry| entry != state);
            if index.is_empty() {
                session.remove_reserved(INDEX_KEY);
            } else {
                let _ = session.set_reserved(INDEX_KEY, &index);
            }
        }
        let pending = found?;
        // A start time in the future (a clock step) counts as age zero.
        let age = now
            .duration_since(pending.created_at)
            .unwrap_or(Duration::ZERO);
        (age <= max_age).then_some(pending)
    }
}

fn key_for(state: &str) -> String {
    format!("{PENDING_PREFIX}{state}")
}

fn read_index(session: &Session) -> Vec<String> {
    session
        .get::<Vec<String>>(INDEX_KEY)
        .ok()
        .flatten()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::handle::SessionState;

    fn session() -> Session {
        Session::from_state(SessionState::fresh(SystemTime::UNIX_EPOCH))
    }

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn pending(state: &str) -> Pending {
        Pending {
            registration: "idp".into(),
            state: state.into(),
            nonce: Some("n".into()),
            verifier: "v".into(),
            saved_request: Some("/orders?x=1".into()),
            created_at: t0(),
        }
    }

    const TEN: Duration = Duration::from_secs(600);

    #[test]
    fn pending_roundtrips_and_is_removed_by_take() {
        let s = session();
        pending("a").store(&s).unwrap();
        assert_eq!(Pending::take(&s, "a", t0(), TEN), Some(pending("a")));
        assert_eq!(Pending::take(&s, "a", t0(), TEN), None);
        assert!(read_index(&s).is_empty());
    }

    #[test]
    fn take_with_the_wrong_state_returns_none_and_keeps_others() {
        let s = session();
        pending("a").store(&s).unwrap();
        assert_eq!(Pending::take(&s, "b", t0(), TEN), None);
        assert_eq!(Pending::take(&s, "a", t0(), TEN), Some(pending("a")));
    }

    #[test]
    fn an_expired_pending_is_none_and_removed() {
        let s = session();
        pending("a").store(&s).unwrap();
        let late = t0() + TEN + Duration::from_secs(1);
        assert_eq!(Pending::take(&s, "a", late, TEN), None);
        assert_eq!(
            s.get::<Pending>(&format!("{PENDING_PREFIX}a")).unwrap(),
            None
        );
        assert_eq!(Pending::take(&s, "a", t0(), TEN), None);
    }

    #[test]
    fn a_pending_exactly_at_max_age_is_still_valid() {
        let s = session();
        pending("a").store(&s).unwrap();
        assert!(Pending::take(&s, "a", t0() + TEN, TEN).is_some());
    }

    #[test]
    fn two_pending_logins_do_not_collide() {
        let s = session();
        pending("a").store(&s).unwrap();
        pending("b").store(&s).unwrap();
        assert_eq!(Pending::take(&s, "b", t0(), TEN), Some(pending("b")));
        assert_eq!(Pending::take(&s, "a", t0(), TEN), Some(pending("a")));
    }

    #[test]
    fn the_sixth_pending_login_drops_the_oldest() {
        let s = session();
        for i in 1..=6 {
            pending(&format!("s{i}")).store(&s).unwrap();
        }
        assert_eq!(Pending::take(&s, "s1", t0(), TEN), None);
        assert_eq!(
            s.get::<Pending>(&format!("{PENDING_PREFIX}s1")).unwrap(),
            None
        );
        for i in 2..=6 {
            assert!(Pending::take(&s, &format!("s{i}"), t0(), TEN).is_some());
        }
    }

    #[test]
    fn storing_the_same_state_twice_keeps_one_index_entry() {
        let s = session();
        pending("a").store(&s).unwrap();
        pending("a").store(&s).unwrap();
        assert_eq!(
            s.get::<Vec<String>>(INDEX_KEY).unwrap().unwrap(),
            vec!["a".to_owned()]
        );
    }

    #[test]
    fn take_on_malformed_json_under_the_key_is_none_and_cleans_up() {
        let s = session();
        pending("a").store(&s).unwrap();
        s.set_reserved(&key_for("a"), &serde_json::json!({"registration": 7}))
            .unwrap();
        assert_eq!(Pending::take(&s, "a", t0(), TEN), None);
        assert_eq!(s.get::<serde_json::Value>(&key_for("a")).unwrap(), None);
        assert!(read_index(&s).is_empty());
        assert_eq!(s.get::<Vec<String>>(INDEX_KEY).unwrap(), None);
    }

    #[test]
    fn take_on_a_stale_record_outside_the_index_removes_it_and_keeps_the_index() {
        let s = session();
        pending("a").store(&s).unwrap();
        // A record left behind without an index entry, as after a failed index write.
        s.set_reserved(&key_for("stale"), &pending("stale"))
            .unwrap();
        assert_eq!(
            Pending::take(&s, "stale", t0(), TEN),
            Some(pending("stale"))
        );
        assert_eq!(s.get::<Pending>(&key_for("stale")).unwrap(), None);
        assert_eq!(read_index(&s), vec!["a".to_owned()]);
        assert_eq!(Pending::take(&s, "stale", t0(), TEN), None);
    }

    #[test]
    fn the_index_holds_exactly_five_entries_after_the_sixth_state() {
        let s = session();
        for i in 1..=6 {
            pending(&format!("s{i}")).store(&s).unwrap();
        }
        let index = read_index(&s);
        assert_eq!(index.len(), MAX_PENDING);
        assert_eq!(index, vec!["s2", "s3", "s4", "s5", "s6"]);
    }

    #[test]
    fn a_now_earlier_than_created_at_counts_as_age_zero() {
        let s = session();
        pending("a").store(&s).unwrap();
        let earlier = t0() - Duration::from_secs(3600);
        assert_eq!(Pending::take(&s, "a", earlier, TEN), Some(pending("a")));
    }

    #[test]
    fn debug_output_with_no_nonce_shows_none_and_hides_the_rest() {
        let mut p = pending("the-state");
        p.verifier = "the-verifier".into();
        p.nonce = None;
        let text = format!("{p:?}");
        assert!(text.contains("nonce: None"), "{text}");
        for secret in ["the-state", "the-verifier"] {
            assert!(!text.contains(secret), "{text}");
        }
    }

    #[test]
    fn debug_output_hides_verifier_state_and_nonce() {
        let mut p = pending("the-state");
        p.verifier = "the-verifier".into();
        p.nonce = Some("the-nonce".into());
        let text = format!("{p:?}");
        for secret in ["the-state", "the-verifier", "the-nonce"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}
