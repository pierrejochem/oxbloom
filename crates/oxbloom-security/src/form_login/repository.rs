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

/// The session key that holds the username of the logged-in principal, as a JSON string.
pub(crate) use crate::session::PRINCIPAL_KEY;
/// The session key that holds the seconds since the epoch of the last check of the user.
pub(crate) const REVALIDATED_AT_KEY: &str = "oxbloom.security.revalidated-at";

/// The version of the stored authentication value.
const VERSION: u64 = 1;

/// Reads the stored authentication. A value that does not deserialize, or has an unknown
/// version, is removed from the session and gives `None`.
pub(crate) fn load_authentication(session: &Session) -> Option<Authentication> {
    let stored = match session.get::<serde_json::Value>(AUTHENTICATION_KEY) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(_) => {
            session.remove_reserved(AUTHENTICATION_KEY);
            return None;
        }
    };
    let parsed = (stored.get("v").and_then(serde_json::Value::as_u64) == Some(VERSION))
        .then(|| stored.get("login").and_then(serde_json::Value::as_u64))
        .flatten()
        .and_then(|_| stored.get("authentication").cloned())
        .and_then(|value| serde_json::from_value::<Authentication>(value).ok());
    if parsed.is_none() {
        session.remove_reserved(AUTHENTICATION_KEY);
    }
    parsed
}

/// Stores the username as the principal key and `now` as the time of the last check, for the
/// per-user session index and for revalidation.
///
/// # Errors
///
/// [`SessionError::Serialization`] when a value cannot be turned into JSON.
pub(crate) fn store_principal(
    session: &Session,
    principal: &str,
    now: SystemTime,
) -> Result<(), SessionError> {
    session.set_reserved(PRINCIPAL_KEY, &principal)?;
    store_revalidated_at(session, now)
}

/// Stores `now` as the time of the last check of the user.
///
/// # Errors
///
/// [`SessionError::Serialization`] when the value cannot be turned into JSON.
pub(crate) fn store_revalidated_at(session: &Session, now: SystemTime) -> Result<(), SessionError> {
    session.set_reserved(REVALIDATED_AT_KEY, &epoch_seconds(now))
}

/// The seconds since the epoch of the last check of the user, `None` when the value is missing
/// or is not a number.
pub(crate) fn load_revalidated_at(session: &Session) -> Option<u64> {
    session.get::<u64>(REVALIDATED_AT_KEY).ok().flatten()
}

/// Seconds since the epoch, zero for a time before it.
pub(crate) fn epoch_seconds(now: SystemTime) -> u64 {
    now.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
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
    write_authentication(session, authentication, epoch_seconds(now))
}

/// Replaces the stored authentication and keeps the stored login time. The caller passes the
/// details it wants kept as part of `authentication`.
///
/// # Errors
///
/// [`SessionError::Serialization`] when no valid authentication is stored, so there is no login
/// time to keep, or when the authentication cannot be turned into JSON.
pub(crate) fn refresh_authentication(
    session: &Session,
    authentication: &Authentication,
) -> Result<(), SessionError> {
    let login = session
        .get::<serde_json::Value>(AUTHENTICATION_KEY)
        .ok()
        .flatten()
        .filter(|stored| stored.get("v").and_then(serde_json::Value::as_u64) == Some(VERSION))
        .and_then(|stored| stored.get("login").and_then(serde_json::Value::as_u64))
        .ok_or(SessionError::Serialization)?;
    write_authentication(session, authentication, login)
}

fn write_authentication(
    session: &Session,
    authentication: &Authentication,
    login: u64,
) -> Result<(), SessionError> {
    let authentication =
        serde_json::to_value(authentication).map_err(|_| SessionError::Serialization)?;
    session.set_reserved(
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
    let _ = session.set_reserved(SAVED_REQUEST_KEY, path_and_query);
}

/// Removes and returns the saved request, or `None` when none is stored or the stored value
/// fails the redirect rule again.
pub(crate) fn take_saved_request(session: &Session) -> Option<String> {
    let stored = session.get::<String>(SAVED_REQUEST_KEY);
    if !matches!(stored, Ok(None)) {
        session.remove_reserved(SAVED_REQUEST_KEY);
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
    fn refresh_keeps_the_login_time_and_replaces_the_authentication() {
        let s = session();
        store_authentication(&s, &alice(), now()).unwrap();
        let fresh = Authentication::new("alice", [GrantedAuthority::new("ROLE_ADMIN")])
            .with_details(alice().details().clone());
        refresh_authentication(&s, &fresh).unwrap();
        let stored: serde_json::Value = s.get(AUTHENTICATION_KEY).unwrap().unwrap();
        assert_eq!(stored["login"], 1_800_000_000u64);
        let loaded = load_authentication(&s).unwrap();
        assert!(loaded.has_role("ADMIN") && !loaded.has_role("USER"));
        assert_eq!(loaded.details()["tenant"], "acme");
    }

    #[test]
    fn refresh_without_a_stored_authentication_fails_and_writes_nothing() {
        let s = session();
        assert_eq!(
            refresh_authentication(&s, &alice()),
            Err(SessionError::Serialization)
        );
        assert!(s.take_state().is_empty_untouched());
    }

    #[test]
    fn the_revalidation_time_round_trips_and_a_bad_value_reads_as_missing() {
        let s = session();
        assert_eq!(load_revalidated_at(&s), None);
        store_revalidated_at(&s, now()).unwrap();
        assert_eq!(load_revalidated_at(&s), Some(1_800_000_000));
        s.set_reserved(REVALIDATED_AT_KEY, "soon").unwrap();
        assert_eq!(load_revalidated_at(&s), None);
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
        s.set_reserved(
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
        s.set_reserved(AUTHENTICATION_KEY, &stored).unwrap();
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
            s.set_reserved(AUTHENTICATION_KEY, &value).unwrap();
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
        s.set_reserved(SAVED_REQUEST_KEY, "//evil.com").unwrap();
        assert!(take_saved_request(&s).is_none());
        s.set_reserved(SAVED_REQUEST_KEY, &5).unwrap();
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
