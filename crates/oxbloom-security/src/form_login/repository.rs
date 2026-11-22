//! Keeps the `Authentication` and the saved request in the HTTP session.
//!
//! Every helper acts on a [`Session`] it is given and never creates one, so an anonymous request
//! that has no session stays without one. No username, session id or saved path appears in an
//! error text or in `Debug` output.

use std::time::SystemTime;

use super::redirect::path_of;
use crate::{Authentication, Session, SessionError, is_safe_redirect_target};

/// The session key that holds the stored authentication and the login time.
pub(crate) const AUTHENTICATION_KEY: &str = "oxbloom.security.authentication";
/// The session key that holds the path and query to return to after a login.
pub(crate) const SAVED_REQUEST_KEY: &str = "oxbloom.security.saved-request";

/// The version of the stored authentication value.
const VERSION: u64 = 1;

/// Reads the stored authentication. A value that does not deserialize, or has an unknown
/// version, is removed from the session and gives `None`.
pub(crate) fn load_authentication(session: &Session) -> Option<Authentication> {
    let stored = match session.get::<serde_json::Value>(AUTHENTICATION_KEY) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(_) => {
            session.remove(AUTHENTICATION_KEY);
            return None;
        }
    };
    let parsed = (stored.get("v").and_then(serde_json::Value::as_u64) == Some(VERSION))
        .then(|| stored.get("login").and_then(serde_json::Value::as_u64))
        .flatten()
        .and_then(|_| stored.get("authentication").cloned())
        .and_then(|value| serde_json::from_value::<Authentication>(value).ok());
    if parsed.is_none() {
        session.remove(AUTHENTICATION_KEY);
    }
    parsed
}

/// Stores the authentication and the login time in the session.
///
/// # Errors
///
/// [`SessionError::Serialization`] when the authentication cannot be turned into JSON.
pub(crate) fn store_authentication(
    session: &Session,
    authentication: &Authentication,
    now: SystemTime,
) -> Result<(), SessionError> {
    let login = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let authentication =
        serde_json::to_value(authentication).map_err(|_| SessionError::Serialization)?;
    session.set(
        AUTHENTICATION_KEY,
        &serde_json::json!({ "v": VERSION, "login": login, "authentication": authentication }),
    )
}

/// Saves `path_and_query` as the request to return to, when it passes the redirect rule. A saved
/// request is replaced only when the new path differs from the stored one.
pub(crate) fn save_request(session: &Session, path_and_query: &str) {
    if !is_safe_redirect_target(path_and_query) {
        return;
    }
    if let Ok(Some(existing)) = session.get::<String>(SAVED_REQUEST_KEY)
        && path_of(&existing) == path_of(path_and_query)
    {
        return;
    }
    // A target that is a plain string always serializes; a failure leaves the session unchanged.
    let _ = session.set(SAVED_REQUEST_KEY, path_and_query);
}

/// Removes and returns the saved request, or `None` when none is stored or the stored value
/// fails the redirect rule again.
pub(crate) fn take_saved_request(session: &Session) -> Option<String> {
    let stored = session.get::<String>(SAVED_REQUEST_KEY);
    if !matches!(stored, Ok(None)) {
        session.remove(SAVED_REQUEST_KEY);
    }
    stored
        .ok()
        .flatten()
        .filter(|target| is_safe_redirect_target(target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GrantedAuthority;
    use crate::session::handle::SessionState;
    use std::time::Duration;

    fn session() -> Session {
        Session::from_state(SessionState::fresh(SystemTime::UNIX_EPOCH))
    }

    fn alice() -> Authentication {
        let mut details = serde_json::Map::new();
        details.insert("tenant".into(), serde_json::json!("acme"));
        Authentication::new("alice", [GrantedAuthority::new("ROLE_USER")]).with_details(details)
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    #[test]
    fn authentication_round_trip_keeps_principal_authorities_and_details() {
        let s = session();
        store_authentication(&s, &alice(), now()).unwrap();
        let stored: serde_json::Value = s.get(AUTHENTICATION_KEY).unwrap().unwrap();
        assert_eq!(stored["v"], 1);
        assert_eq!(stored["login"], 1_800_000_000u64);
        let loaded = load_authentication(&s).unwrap();
        assert_eq!(loaded.principal(), "alice");
        assert!(loaded.has_role("USER"));
        assert_eq!(loaded.details()["tenant"], "acme");
    }

    #[test]
    fn an_empty_session_has_no_authentication_and_stays_untouched() {
        let s = session();
        assert!(load_authentication(&s).is_none());
        assert!(take_saved_request(&s).is_none());
        assert!(s.take_state().is_empty_untouched());
    }

    #[test]
    fn a_corrupt_stored_value_is_dropped_and_removed() {
        let s = session();
        s.set(
            AUTHENTICATION_KEY,
            &serde_json::json!({"v": 1, "login": 1, "authentication": {"principal": 5}}),
        )
        .unwrap();
        assert!(load_authentication(&s).is_none());
        assert!(
            s.get::<serde_json::Value>(AUTHENTICATION_KEY)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_unknown_version_is_dropped() {
        let s = session();
        store_authentication(&s, &alice(), now()).unwrap();
        let mut stored: serde_json::Value = s.get(AUTHENTICATION_KEY).unwrap().unwrap();
        stored["v"] = serde_json::json!(2);
        s.set(AUTHENTICATION_KEY, &stored).unwrap();
        assert!(load_authentication(&s).is_none());
        assert!(
            s.get::<serde_json::Value>(AUTHENTICATION_KEY)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_value_of_the_wrong_json_type_is_dropped() {
        for value in [
            serde_json::json!("text"),
            serde_json::json!(7),
            serde_json::json!([1]),
            serde_json::json!(null),
        ] {
            let s = session();
            s.set(AUTHENTICATION_KEY, &value).unwrap();
            assert!(load_authentication(&s).is_none(), "{value}");
            assert!(
                s.get::<serde_json::Value>(AUTHENTICATION_KEY)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn saved_request_round_trip_and_consumed() {
        let s = session();
        save_request(&s, "/orders?page=2");
        assert_eq!(take_saved_request(&s).as_deref(), Some("/orders?page=2"));
        assert!(take_saved_request(&s).is_none());
    }

    #[test]
    fn an_unsafe_saved_request_is_never_stored() {
        for bad in [
            "",
            "a",
            "//x",
            "/\\x",
            "https://x",
            "/a\\b",
            "/a\tb",
            "/a\nb",
            "/a://b",
        ] {
            let s = session();
            save_request(&s, bad);
            assert!(
                s.get::<String>(SAVED_REQUEST_KEY).unwrap().is_none(),
                "{bad:?}"
            );
        }
        let s = session();
        save_request(&s, &format!("/{}", "a".repeat(2048)));
        assert!(s.get::<String>(SAVED_REQUEST_KEY).unwrap().is_none());
    }

    #[test]
    fn a_tampered_saved_request_is_rejected_when_taken() {
        let s = session();
        s.set(SAVED_REQUEST_KEY, "//evil.com").unwrap();
        assert!(take_saved_request(&s).is_none());
        s.set(SAVED_REQUEST_KEY, &5).unwrap();
        assert!(take_saved_request(&s).is_none());
    }

    #[test]
    fn a_second_saved_request_with_the_same_path_does_not_replace() {
        let s = session();
        save_request(&s, "/a?x=1");
        save_request(&s, "/a?x=2");
        assert_eq!(take_saved_request(&s).as_deref(), Some("/a?x=1"));
        save_request(&s, "/a");
        save_request(&s, "/b");
        assert_eq!(take_saved_request(&s).as_deref(), Some("/b"));
    }
}
