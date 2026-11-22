//! Parsing of the `application/x-www-form-urlencoded` login body.

use oxbloom_config::Secret;

/// The largest login body, in bytes.
const MAX_BODY_BYTES: usize = 4096;

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Decodes one urlencoded component: `+` is a space, `%XX` is a byte, and a malformed escape
/// stays literal. `None` when the result is not UTF-8.
fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let pair = bytes
                    .get(i + 1)
                    .copied()
                    .and_then(hex)
                    .zip(bytes.get(i + 2).copied().and_then(hex));
                if let Some((hi, lo)) = pair {
                    out.push(hi * 16 + lo);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Splits an `application/x-www-form-urlencoded` body into decoded `(name, value)` pairs, in
/// order. A body over 4096 bytes, invalid UTF-8 or a pair that does not decode to UTF-8 gives
/// `None`.
pub(crate) fn urlencoded_pairs(body: &[u8]) -> Option<Vec<(String, String)>> {
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    body.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(key)?, decode(value)?))
        })
        .collect()
}

/// A login body read by [`parse_login_form`].
pub(crate) struct LoginForm {
    pub(crate) username: String,
    pub(crate) password: Secret<String>,
    /// Whether the body asks for a remember-me cookie.
    pub(crate) remember_me: bool,
}

/// Whether a value of the remember-me field asks for the cookie: `on`, `true`, `yes` or `1`,
/// without regard to ASCII case. Any other value, the empty one included, does not.
fn asks_to_remember(value: &str) -> bool {
    ["on", "true", "yes", "1"]
        .iter()
        .any(|yes| value.eq_ignore_ascii_case(yes))
}

/// Reads the username, the password and, when `remember_me_field` is given, whether that field
/// asks for a remember-me cookie, from a login body. The first occurrence of a duplicated field
/// wins. The remember-me field is optional. A body over 4096 bytes, invalid UTF-8, a missing
/// username or password field or an empty username gives `None`.
pub(crate) fn parse_login_form(
    body: &[u8],
    username_field: &str,
    password_field: &str,
    remember_me_field: Option<&str>,
) -> Option<LoginForm> {
    let mut username: Option<String> = None;
    let mut password: Option<String> = None;
    let mut remember_me: Option<bool> = None;
    for (key, value) in urlencoded_pairs(body)? {
        if key == username_field {
            username.get_or_insert(value);
        } else if key == password_field {
            password.get_or_insert(value);
        } else if Some(key.as_str()) == remember_me_field {
            remember_me.get_or_insert_with(|| asks_to_remember(&value));
        }
    }
    let username = username.filter(|u| !u.is_empty())?;
    Some(LoginForm {
        username,
        password: Secret::new(password?),
        remember_me: remember_me.unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &[u8]) -> Option<(String, String)> {
        parse_login_form(body, "username", "password", None)
            .map(|f| (f.username, f.password.expose().clone()))
    }

    fn remember(body: &[u8]) -> Option<bool> {
        parse_login_form(body, "username", "password", Some("remember-me")).map(|f| f.remember_me)
    }

    #[test]
    fn normal_body() {
        assert_eq!(
            parse(b"username=alice&password=s3cret"),
            Some(("alice".into(), "s3cret".into()))
        );
    }

    #[test]
    fn plus_and_percent_20_are_spaces() {
        assert_eq!(
            parse(b"username=a+b&password=c%20d"),
            Some(("a b".into(), "c d".into()))
        );
    }

    #[test]
    fn first_duplicate_wins() {
        assert_eq!(
            parse(b"username=a&username=b&password=x&password=y"),
            Some(("a".into(), "x".into()))
        );
    }

    #[test]
    fn missing_password_or_empty_username_fails() {
        assert_eq!(parse(b"username=a"), None);
        assert_eq!(parse(b"password=a"), None);
        assert_eq!(parse(b"username=&password=a"), None);
    }

    #[test]
    fn empty_password_is_allowed() {
        assert_eq!(
            parse(b"username=a&password="),
            Some(("a".into(), String::new()))
        );
    }

    #[test]
    fn invalid_utf8_fails() {
        assert_eq!(parse(b"username=a&password=%FF"), None);
        assert_eq!(parse(b"username=a&password=\xff"), None);
    }

    #[test]
    fn oversize_body_fails() {
        let mut body = b"username=a&password=".to_vec();
        body.resize(4096, b'x');
        assert!(parse(&body).is_some());
        body.push(b'x');
        assert_eq!(body.len(), 4097);
        assert_eq!(parse(&body), None);
    }

    #[test]
    fn encoded_equals_and_ampersand_stay_in_the_value() {
        assert_eq!(
            parse(b"username=a%26b&password=x%3Dy%26z"),
            Some(("a&b".into(), "x=y&z".into()))
        );
    }

    #[test]
    fn configured_field_names_are_used() {
        let got = parse_login_form(b"u=a&p=b&username=no", "u", "p", None).unwrap();
        assert_eq!(
            (got.username.as_str(), got.password.expose().as_str()),
            ("a", "b")
        );
    }

    #[test]
    fn the_remember_me_field_accepts_four_values_in_any_ascii_case() {
        for yes in ["on", "true", "yes", "1", "ON", "True", "yEs"] {
            let body = format!("username=a&password=b&remember-me={yes}");
            assert_eq!(remember(body.as_bytes()), Some(true), "{yes}");
        }
        for no in ["", "0", "off", "no", "false", "onn", " on", "y"] {
            let body = format!("username=a&password=b&remember-me={no}");
            assert_eq!(remember(body.as_bytes()), Some(false), "{no:?}");
        }
        assert_eq!(remember(b"username=a&password=b&remember-me"), Some(false));
    }

    #[test]
    fn the_remember_me_field_is_optional_and_its_first_value_wins() {
        assert_eq!(remember(b"username=a&password=b"), Some(false));
        assert_eq!(
            remember(b"remember-me=on&username=a&remember-me=off&password=b"),
            Some(true)
        );
        assert_eq!(
            remember(b"username=a&password=b&remember-me=off&remember-me=on"),
            Some(false)
        );
        // Without a field name nothing asks for the cookie, and the field never replaces a
        // missing username or password.
        let got = parse_login_form(
            b"username=a&password=b&remember-me=on",
            "username",
            "password",
            None,
        );
        assert!(!got.unwrap().remember_me);
        assert_eq!(remember(b"username=a&remember-me=on"), None);
    }

    #[test]
    fn the_remember_me_field_counts_toward_the_size_cap() {
        let mut body = b"username=a&password=b&remember-me=on&pad=".to_vec();
        body.resize(4096, b'x');
        assert_eq!(remember(&body), Some(true));
        body.push(b'x');
        assert_eq!(remember(&body), None);
    }
}
