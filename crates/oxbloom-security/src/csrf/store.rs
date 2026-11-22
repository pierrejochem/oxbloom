//! Keeps the CSRF token in the HTTP session.
//!
//! Every helper acts on a [`Session`] it is given and never creates one. The token is created
//! lazily by [`ensure_token`], never by the check itself. A stored value that is not a valid
//! token is removed and read as absent. The token never appears in an error text or in `Debug`
//! output.

use super::CsrfToken;
use crate::Session;

/// The session key that holds the CSRF token.
pub(crate) const CSRF_TOKEN_KEY: &str = "oxbloom.security.csrf-token";

/// Reads the stored token. A value that is not a string of the token shape, or that cannot be
/// read, is removed from the session and gives `None`. A missing token leaves the session
/// untouched.
pub(crate) fn stored_token(session: &Session) -> Option<CsrfToken> {
    match session.get::<String>(CSRF_TOKEN_KEY) {
        Ok(None) => None,
        Ok(Some(value)) => {
            let token = CsrfToken::from_stored(value);
            if token.is_none() {
                session.remove(CSRF_TOKEN_KEY);
            }
            token
        }
        Err(_) => {
            session.remove(CSRF_TOKEN_KEY);
            None
        }
    }
}

/// Returns the stored token, or mints a new one and stores it.
pub(crate) fn ensure_token(session: &Session) -> CsrfToken {
    stored_token(session).unwrap_or_else(|| replace_token(session))
}

/// Mints a new token, stores it in place of any old one and returns it.
pub(crate) fn replace_token(session: &Session) -> CsrfToken {
    let token = CsrfToken::generate();
    // A plain string always serializes, so the store cannot fail here.
    let _ = session.set(CSRF_TOKEN_KEY, token.as_str());
    token
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::session::handle::SessionState;

    fn session() -> Session {
        Session::from_state(SessionState::fresh(SystemTime::UNIX_EPOCH))
    }

    fn raw(s: &Session) -> Option<serde_json::Value> {
        s.get(CSRF_TOKEN_KEY).unwrap()
    }

    #[test]
    fn ensure_token_creates_once_and_returns_the_same_value() {
        let s = session();
        assert!(stored_token(&s).is_none());
        let a = ensure_token(&s);
        let b = ensure_token(&s);
        assert_eq!(a.as_str(), b.as_str());
        assert_eq!(stored_token(&s).unwrap().as_str(), a.as_str());
        assert_eq!(raw(&s), Some(serde_json::json!(a.as_str())));
    }

    #[test]
    fn replace_token_changes_the_value() {
        let s = session();
        let a = ensure_token(&s);
        let b = replace_token(&s);
        assert_ne!(a.as_str(), b.as_str());
        assert_eq!(stored_token(&s).unwrap().as_str(), b.as_str());
        let c = replace_token(&session());
        assert_eq!(c.as_str().len(), 43);
    }

    #[test]
    fn a_corrupt_stored_value_is_removed() {
        for bad in [
            "short",
            "",
            &"A".repeat(44),
            &format!("{}+", "A".repeat(42)),
        ] {
            let s = session();
            s.set(CSRF_TOKEN_KEY, bad).unwrap();
            assert!(stored_token(&s).is_none(), "{bad:?}");
            assert!(raw(&s).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_value_of_the_wrong_type_is_dropped() {
        for value in [
            serde_json::json!(7),
            serde_json::json!([1]),
            serde_json::json!({"a": 1}),
            serde_json::json!(null),
            serde_json::json!(true),
        ] {
            let s = session();
            s.set(CSRF_TOKEN_KEY, &value).unwrap();
            assert!(stored_token(&s).is_none(), "{value}");
            assert!(raw(&s).is_none(), "{value}");
            let fresh = ensure_token(&s);
            assert_eq!(fresh.as_str().len(), 43);
        }
    }

    #[test]
    fn reading_a_missing_token_does_not_write_the_session() {
        let s = session();
        assert!(stored_token(&s).is_none());
        assert!(s.take_state().is_empty_untouched());
    }
}
