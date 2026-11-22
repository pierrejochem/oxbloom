//! Reading the session cookie from a request and writing `Set-Cookie`.

use std::sync::Arc;
use std::time::Duration;

use http::HeaderValue;
use oxbloom_web::{Request, Scheme};

use super::settings::{SameSite, SecureMode, SessionSettings};

/// The `Path` and the `Secure` decision of the session cookie for one request. The session
/// filter puts it into the request extensions, so the other cookies of the crate, such as the
/// remember-me cookie, carry the same `Path` and `Secure` as the session cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CookieScope {
    /// The `Path` of the session cookie.
    pub(crate) path: Arc<str>,
    /// Whether the session cookie of this request is `Secure`.
    pub(crate) secure: bool,
}

impl CookieScope {
    /// The scope of `req`: the one the session filter stored, or without a session filter the
    /// defaults of [`SessionSettings`] (`Path=/`, `Secure` when the request came over HTTPS).
    pub(crate) fn of(req: &Request) -> Self {
        req.extensions().get::<Self>().cloned().unwrap_or_else(|| {
            let defaults = SessionSettings::default();
            Self {
                path: Arc::from(defaults.path),
                secure: is_secure(defaults.secure, req),
            }
        })
    }
}

/// Whether a cookie set in the answer to `req` is `Secure` under `mode`: `Auto` follows
/// [`Request::scheme`], `Always` and `Never` override.
pub(crate) fn is_secure(mode: SecureMode, req: &Request) -> bool {
    match mode {
        SecureMode::Always => true,
        SecureMode::Never => false,
        SecureMode::Auto => req.scheme() == Scheme::Https,
    }
}

/// A `Cookie` header longer than this counts as no cookie.
const MAX_COOKIE_HEADER_BYTES: usize = 8 * 1024;

/// RFC 6265 `cookie-octet`: no control character, space, double quote, comma, semicolon or
/// backslash.
fn is_cookie_octet(b: u8) -> bool {
    matches!(b, 0x21 | 0x23..=0x2B | 0x2D..=0x3A | 0x3C..=0x5B | 0x5D..=0x7E)
}

/// Returns every value of the cookie `name` in header order. A header over 8 KiB is skipped
/// and a value with a byte outside the cookie octets is dropped.
pub(crate) fn parse_cookie_header(values: &[&HeaderValue], name: &str) -> Vec<String> {
    let mut found = Vec::new();
    for header in values {
        if header.len() > MAX_COOKIE_HEADER_BYTES {
            continue;
        }
        let Ok(text) = header.to_str() else { continue };
        for pair in text.split(';') {
            let pair = pair.trim_matches(|c| c == ' ' || c == '\t');
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            if key == name && value.bytes().all(is_cookie_octet) {
                found.push(value.to_owned());
            }
        }
    }
    found
}

/// Builds a `Set-Cookie` value. Pass `Some(Duration::ZERO)` and an empty value to clear the
/// cookie. The caller must have validated `settings` and passes a value of cookie octets.
pub(crate) fn build_set_cookie(
    settings: &SessionSettings,
    value: &str,
    max_age: Option<Duration>,
    secure: bool,
) -> HeaderValue {
    let mut text = format!("{}={}; Path={}", settings.cookie_name, value, settings.path);
    if let Some(age) = max_age {
        text.push_str(&format!("; Max-Age={}", age.as_secs()));
    }
    text.push_str("; HttpOnly");
    if secure {
        text.push_str("; Secure");
    }
    text.push_str(match settings.same_site {
        SameSite::Lax => "; SameSite=Lax",
        SameSite::Strict => "; SameSite=Strict",
        SameSite::None => "; SameSite=None",
    });
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn hv(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).unwrap()
    }

    fn parse(headers: &[&str], name: &str) -> Vec<String> {
        let owned: Vec<HeaderValue> = headers.iter().map(|h| hv(h)).collect();
        let refs: Vec<&HeaderValue> = owned.iter().collect();
        parse_cookie_header(&refs, name)
    }

    #[test]
    fn picks_the_named_cookie_among_several() {
        assert_eq!(parse(&["a=1; SID=abc; b=2"], "SID"), ["abc"]);
        assert!(parse(&["a=1; b=2"], "SID").is_empty());
    }

    #[test]
    fn same_name_twice_returns_both_in_order() {
        assert_eq!(parse(&["SID=one; x=y; SID=two"], "SID"), ["one", "two"]);
        assert_eq!(parse(&["SID=one", "SID=two"], "SID"), ["one", "two"]);
    }

    #[test]
    fn tolerates_odd_spacing_around_semicolons() {
        assert_eq!(
            parse(&["  SID=abc  ;x=1;\tSID=def ;"], "SID"),
            ["abc", "def"]
        );
    }

    #[test]
    fn drops_values_with_bad_octets() {
        assert!(parse(&["SID=a b"], "SID").is_empty());
        assert!(parse(&["SID=\"abc\""], "SID").is_empty());
        assert!(parse(&["SID=a\\b"], "SID").is_empty());
        assert!(parse(&["SID=a,b"], "SID").is_empty());
        // A `HeaderValue` cannot hold a control byte, so only high bytes reach the parser.
        let high = HeaderValue::from_bytes(b"SID=a\xffb").unwrap();
        assert!(parse_cookie_header(&[&high], "SID").is_empty());
        assert_eq!(parse(&["SID=bad b; SID=good"], "SID"), ["good"]);
    }

    #[test]
    fn over_length_header_counts_as_no_cookie() {
        let at_limit = format!("SID={}", "a".repeat(MAX_COOKIE_HEADER_BYTES - 4));
        assert_eq!(at_limit.len(), MAX_COOKIE_HEADER_BYTES);
        assert_eq!(parse(&[&at_limit], "SID").len(), 1);
        let over = format!("SID={}", "a".repeat(MAX_COOKIE_HEADER_BYTES - 3));
        assert_eq!(over.len(), 8193);
        assert!(parse(&[&over], "SID").is_empty());
    }

    #[test]
    fn many_small_cookies_parse_quickly() {
        // 10 000 pairs of "cNNNN=v; " stay under no limit of their own, so split them over
        // headers of under 8 KiB each.
        let headers: Vec<String> = (0..10_000)
            .collect::<Vec<u32>>()
            .chunks(500)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|i| format!("c{i}=v"))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .collect();
        let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        let mut all = refs.clone();
        all.push("SID=target");
        let start = Instant::now();
        assert_eq!(parse(&all, "SID"), ["target"]);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    fn text(same_site: SameSite, secure: bool) -> String {
        let settings = SessionSettings::default().same_site(same_site);
        let v = build_set_cookie(&settings, "abc", Some(Duration::from_secs(1800)), secure);
        v.to_str().unwrap().to_owned()
    }

    #[test]
    fn set_cookie_text_for_each_same_site() {
        assert_eq!(
            text(SameSite::Lax, false),
            "OXBLOOM_SESSION=abc; Path=/; Max-Age=1800; HttpOnly; SameSite=Lax"
        );
        assert_eq!(
            text(SameSite::Lax, true),
            "OXBLOOM_SESSION=abc; Path=/; Max-Age=1800; HttpOnly; Secure; SameSite=Lax"
        );
        assert_eq!(
            text(SameSite::Strict, false),
            "OXBLOOM_SESSION=abc; Path=/; Max-Age=1800; HttpOnly; SameSite=Strict"
        );
        assert_eq!(
            text(SameSite::Strict, true),
            "OXBLOOM_SESSION=abc; Path=/; Max-Age=1800; HttpOnly; Secure; SameSite=Strict"
        );
        assert_eq!(
            text(SameSite::None, false),
            "OXBLOOM_SESSION=abc; Path=/; Max-Age=1800; HttpOnly; SameSite=None"
        );
        assert_eq!(
            text(SameSite::None, true),
            "OXBLOOM_SESSION=abc; Path=/; Max-Age=1800; HttpOnly; Secure; SameSite=None"
        );
    }

    #[test]
    fn clearing_form() {
        let settings = SessionSettings::default();
        let v = build_set_cookie(&settings, "", Some(Duration::ZERO), true);
        assert_eq!(
            v.to_str().unwrap(),
            "OXBLOOM_SESSION=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Lax"
        );
    }
}
