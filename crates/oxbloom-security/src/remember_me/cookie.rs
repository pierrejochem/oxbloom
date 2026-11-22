//! The remember-me cookie: issuing it after a password login, clearing it at logout and reading
//! it from a request.
//!
//! The cookie takes its `Path` and its `Secure` decision from the session cookie of the same
//! request (see [`CookieScope`]), is always `HttpOnly` and `SameSite=Lax`, and its `Max-Age` is
//! the lifetime. The attribute text comes from the session cookie writer, so both cookies are
//! written by the same code. Every `Set-Cookie` value is marked sensitive, so it never shows in
//! `Debug` output.

use std::time::SystemTime;

use http::{HeaderValue, header};
use oxbloom_web::Request;

use super::settings::RememberMe;
use super::token::TokenCodec;
use crate::session::{CookieScope, build_set_cookie, parse_cookie_header};
use crate::{SameSite, SessionSettings, UserDetails};

/// The settings the session cookie writer needs for the remember-me cookie: its name, the
/// `Path` of the session cookie and `SameSite=Lax`.
fn cookie_settings(settings: &RememberMe, scope: &CookieScope) -> SessionSettings {
    SessionSettings {
        cookie_name: settings.cookie_name.clone(),
        path: scope.path.to_string(),
        same_site: SameSite::Lax,
        ..SessionSettings::default()
    }
}

/// The `Set-Cookie` that remembers `user` from `now` for the lifetime, or `None` when the token
/// would be longer than 1 KiB or the expiry does not fit a `SystemTime`.
pub(crate) fn issue_cookie(
    settings: &RememberMe,
    codec: &TokenCodec,
    user: &UserDetails,
    now: SystemTime,
    scope: &CookieScope,
) -> Option<HeaderValue> {
    let expires = now.checked_add(settings.lifetime)?;
    let token = codec.encode(user.username(), user.password(), expires)?;
    let mut value = build_set_cookie(
        &cookie_settings(settings, scope),
        &token,
        Some(settings.lifetime),
        scope.secure,
    );
    value.set_sensitive(true);
    Some(value)
}

/// The `Set-Cookie` that removes the cookie: an empty value with `Max-Age=0` and the same `Path`
/// and attributes as the issued cookie.
pub(crate) fn clear_cookie(settings: &RememberMe, scope: &CookieScope) -> HeaderValue {
    let mut value = build_set_cookie(
        &cookie_settings(settings, scope),
        "",
        Some(std::time::Duration::ZERO),
        scope.secure,
    );
    value.set_sensitive(true);
    value
}

/// The first value of the cookie `name` in the `Cookie` headers of `req`. A header over 8 KiB
/// counts as no cookie and a value with a byte outside the cookie octets is skipped.
#[allow(dead_code, reason = "used by the auto login of the chain")]
pub(crate) fn read_cookie(req: &Request, name: &str) -> Option<String> {
    let values: Vec<&HeaderValue> = req.headers.get_all(header::COOKIE).iter().collect();
    parse_cookie_header(&values, name).into_iter().next()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use oxbloom_config::Secret;
    use oxbloom_core::HttpMethod;

    use super::*;
    use crate::InMemoryUserDetailsService;

    fn settings() -> RememberMe {
        RememberMe::new()
            .secret(Secret::new("a-remember-me-secret-of-32-bytes".to_owned()))
            .user_details(Arc::new(InMemoryUserDetailsService::new(vec![]).unwrap()))
            .lifetime(Duration::from_secs(3600))
    }

    fn scope(path: &str, secure: bool) -> CookieScope {
        CookieScope {
            path: Arc::from(path),
            secure,
        }
    }

    #[test]
    fn an_issued_cookie_carries_a_token_the_codec_accepts() {
        let settings = settings();
        let codec = TokenCodec::new(&settings).unwrap();
        let user = UserDetails::new("alice", "$argon2id$hash", []);
        let now = SystemTime::now();
        let value = issue_cookie(&settings, &codec, &user, now, &scope("/", false)).unwrap();
        assert!(value.is_sensitive());
        let text = value.to_str().unwrap();
        let rest = text.strip_prefix("OXBLOOM_REMEMBER_ME=").unwrap();
        let (token, attributes) = rest.split_once("; ").unwrap();
        assert_eq!(attributes, "Path=/; Max-Age=3600; HttpOnly; SameSite=Lax");
        let parsed = codec.parse(token).unwrap();
        assert_eq!(parsed.username(), "alice");
        assert!(codec.verify(&parsed, "$argon2id$hash"));
        assert!(!codec.verify(&parsed, "another hash"));
        let expires = parsed
            .expires()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        let wanted = (now + Duration::from_secs(3600))
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap();
        assert_eq!(expires.as_secs(), wanted.as_secs());
    }

    #[test]
    fn issuing_gives_up_on_a_long_token_or_an_expiry_past_system_time() {
        let settings = settings();
        let codec = TokenCodec::new(&settings).unwrap();
        let long = UserDetails::new("l".repeat(800), "h", []);
        let now = SystemTime::now();
        assert!(issue_cookie(&settings, &codec, &long, now, &scope("/", false)).is_none());
        let user = UserDetails::new("alice", "h", []);
        // The last whole second a `SystemTime` holds on this platform.
        let mut edge = SystemTime::UNIX_EPOCH;
        let mut step = u64::MAX;
        while step > 0 {
            match edge.checked_add(Duration::from_secs(step)) {
                Some(later) => edge = later,
                None => step /= 2,
            }
        }
        assert!(issue_cookie(&settings, &codec, &user, edge, &scope("/", false)).is_none());
    }

    #[test]
    fn the_clearing_cookie_keeps_path_and_secure() {
        let settings = settings().cookie_name("R");
        let value = clear_cookie(&settings, &scope("/app", true));
        assert!(value.is_sensitive());
        assert_eq!(
            value.to_str().unwrap(),
            "R=; Path=/app; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
        );
    }

    #[test]
    fn read_cookie_takes_the_first_value_of_the_name() {
        let mut req = Request::new(HttpMethod::Get, "/");
        assert_eq!(read_cookie(&req, "R"), None);
        req.headers
            .append(header::COOKIE, HeaderValue::from_static("a=1; R=first"));
        req.headers
            .append(header::COOKIE, HeaderValue::from_static("R=second"));
        assert_eq!(read_cookie(&req, "R").as_deref(), Some("first"));
        assert_eq!(read_cookie(&req, "S"), None);
    }
}
