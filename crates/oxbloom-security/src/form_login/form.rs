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

/// Reads the username and the password from a login body. The first occurrence of a duplicated
/// field wins. A body over 4096 bytes, invalid UTF-8, a missing field or an empty username gives
/// `None`.
pub(crate) fn parse_login_form(
    body: &[u8],
    username_field: &str,
    password_field: &str,
) -> Option<(String, Secret<String>)> {
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    let body = std::str::from_utf8(body).ok()?;
    let mut username: Option<String> = None;
    let mut password: Option<String> = None;
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode(key)?;
        let value = decode(value)?;
        if key == username_field {
            username.get_or_insert(value);
        } else if key == password_field {
            password.get_or_insert(value);
        }
    }
    let username = username.filter(|u| !u.is_empty())?;
    Some((username, Secret::new(password?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &[u8]) -> Option<(String, String)> {
        parse_login_form(body, "username", "password").map(|(u, p)| (u, p.expose().clone()))
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
        let got = parse_login_form(b"u=a&p=b&username=no", "u", "p").unwrap();
        assert_eq!((got.0.as_str(), got.1.expose().as_str()), ("a", "b"));
    }
}
