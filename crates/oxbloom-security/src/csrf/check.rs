//! The CSRF check: which requests are checked and whether the submitted token matches.
//!
//! The check only reads. It never creates a session or a token, and a failure changes nothing.
//! The chain decides when a request counts as authenticated by the session; this module decides
//! which methods are checked and compares the tokens.

use http::header::HeaderName;
use oxbloom_core::HttpMethod;
use oxbloom_web::Request;

use super::store::peek_token;
use super::{CsrfConfig, submitted_token};
use crate::RequestSessionExt;

/// Whether a request with this method changes state and so needs a token: `POST`, `PUT`,
/// `PATCH` or `DELETE`.
pub(crate) fn is_checked_method(method: &HttpMethod) -> bool {
    matches!(
        method,
        HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch | HttpMethod::Delete
    )
}

/// Whether the request passes the token check: a session that the client sent exists, it holds
/// a token, and the submitted token matches it in constant time.
pub(crate) fn token_check_passes(req: &Request, cfg: &CsrfConfig) -> bool {
    let Some(session) = req.session().filter(|s| !s.is_new()) else {
        return false;
    };
    let Some(stored) = peek_token(&session) else {
        return false;
    };
    submitted_token(req, cfg).is_some_and(|submitted| stored.matches(&submitted))
}

/// Whether a request that must be protected passes: `true` when the check is off (`cfg` is
/// `None`) or the method is safe, otherwise the result of [`token_check_passes`].
pub(crate) fn request_passes(req: &Request, cfg: Option<&CsrfConfig>) -> bool {
    match cfg {
        Some(cfg) if is_checked_method(&req.method) => token_check_passes(req, cfg),
        _ => true,
    }
}

/// Marks the token header sensitive, so `Debug` output of the request shows `Sensitive`
/// instead of the token. Marking a value sensitive does not change its bytes.
pub(crate) fn hide_token_header(req: &mut Request, cfg: &CsrfConfig) {
    let Ok(name) = HeaderName::from_bytes(cfg.header_name.as_bytes()) else {
        return;
    };
    if let http::header::Entry::Occupied(mut entry) = req.headers.entry(name) {
        entry.iter_mut().for_each(|v| v.set_sensitive(true));
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::Session;
    use crate::SessionData;
    use crate::csrf::store::{CSRF_TOKEN_KEY, ensure_token};
    use crate::session::handle::SessionState;

    /// A session the client sent: loaded from the store, not new.
    fn loaded() -> Session {
        Session::from_state(SessionState::loaded(
            "id".to_owned(),
            SessionData::new(SystemTime::UNIX_EPOCH),
            SystemTime::UNIX_EPOCH,
        ))
    }

    fn post(session: Option<&Session>, header: Option<&str>) -> Request {
        let mut req = Request::new(HttpMethod::Post, "/x");
        if let Some(s) = session {
            req.extensions_mut().insert(s.clone());
        }
        if let Some(h) = header {
            req.headers.insert("x-csrf-token", h.parse().unwrap());
        }
        req
    }

    #[test]
    fn only_unsafe_methods_are_checked() {
        for m in [
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            assert!(is_checked_method(&m), "{m:?}");
        }
        for m in [
            HttpMethod::Get,
            HttpMethod::Head,
            HttpMethod::Options,
            HttpMethod::Trace,
        ] {
            assert!(!is_checked_method(&m), "{m:?}");
        }
    }

    #[test]
    fn the_check_needs_a_sent_session_a_stored_token_and_a_match() {
        let cfg = CsrfConfig::default();
        let s = loaded();
        assert!(!token_check_passes(&post(Some(&s), Some("x")), &cfg));
        let token = ensure_token(&s);
        assert!(token_check_passes(
            &post(Some(&s), Some(token.as_str())),
            &cfg
        ));
        assert!(!token_check_passes(&post(Some(&s), None), &cfg));
        assert!(!token_check_passes(&post(Some(&s), Some("x")), &cfg));
        assert!(!token_check_passes(&post(None, Some(token.as_str())), &cfg));

        // A session minted in this request never passes.
        let fresh = Session::from_state(SessionState::fresh(SystemTime::UNIX_EPOCH));
        let token = ensure_token(&fresh);
        assert!(!token_check_passes(
            &post(Some(&fresh), Some(token.as_str())),
            &cfg
        ));
    }

    #[test]
    fn the_check_writes_nothing() {
        let cfg = CsrfConfig::default();
        let s = loaded();
        assert!(!token_check_passes(&post(Some(&s), Some("x")), &cfg));
        assert!(!s.take_state().dirty);

        // A corrupt stored value stays where it is: the check only reads.
        let s = loaded();
        s.set(CSRF_TOKEN_KEY, "corrupt").unwrap();
        assert!(!token_check_passes(&post(Some(&s), Some("corrupt")), &cfg));
        assert_eq!(
            s.get::<String>(CSRF_TOKEN_KEY).unwrap().as_deref(),
            Some("corrupt")
        );
    }

    #[test]
    fn request_passes_skips_safe_methods_and_a_disabled_check() {
        let cfg = CsrfConfig::default();
        let s = loaded();
        let mut get = post(Some(&s), None);
        get.method = HttpMethod::Get;
        assert!(request_passes(&get, Some(&cfg)));
        assert!(request_passes(&post(Some(&s), None), None));
        assert!(!request_passes(&post(Some(&s), None), Some(&cfg)));
    }

    #[test]
    fn the_token_header_is_marked_sensitive() {
        let cfg = CsrfConfig::default();
        let mut req = post(None, Some("secret-value"));
        hide_token_header(&mut req, &cfg);
        assert!(req.headers["x-csrf-token"].is_sensitive());
        assert!(!format!("{req:?}").contains("secret-value"));
        assert_eq!(req.headers["x-csrf-token"], "secret-value");
    }
}
