//! The same-origin check for the state-changing form login requests.

use http::header::{HOST, ORIGIN, REFERER};
use oxbloom_web::{Request, Scheme};

/// A scheme, a lower-cased host and a port.
#[derive(PartialEq, Eq)]
struct Origin {
    scheme: Scheme,
    host: String,
    port: u16,
}

fn default_port(scheme: Scheme) -> u16 {
    match scheme {
        Scheme::Http => 80,
        Scheme::Https => 443,
    }
}

/// Splits `host[:port]` and lower-cases the host. The port defaults from `scheme`.
fn parse_authority(authority: &str, scheme: Scheme) -> Option<(String, u16)> {
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after {
            "" => None,
            _ => Some(after.strip_prefix(':')?),
        };
        (format!("[{host}]"), port)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p)),
            None => (authority.to_owned(), None),
        }
    };
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let port = match port {
        None => default_port(scheme),
        Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok()?,
        Some(_) => return None,
    };
    Some((host.to_ascii_lowercase(), port))
}

/// Parses `scheme://authority[/path...]` (an `Origin` or a `Referer`). `null` and anything that
/// is not `http` or `https` give `None`.
fn parse_url_origin(value: &str) -> Option<Origin> {
    let (scheme, rest) = value.split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "http" => Scheme::Http,
        "https" => Scheme::Https,
        _ => return None,
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, port) = parse_authority(&rest[..end], scheme)?;
    Some(Origin { scheme, host, port })
}

/// The one value of a header; `None` when it is absent, repeated or not UTF-8.
fn single(req: &Request, name: http::header::HeaderName) -> Result<Option<&str>, ()> {
    let mut values = req.headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (None, _) => Ok(None),
        (Some(v), None) => v.to_str().map(Some).map_err(|_| ()),
        _ => Err(()),
    }
}

/// The origin the request was sent to: its own scheme and the host the server trusts (the
/// forwarded host when a trusted proxy supplied one, otherwise the `Host` header).
fn expected(req: &Request) -> Option<Origin> {
    let scheme = req.scheme();
    let host = match req.host() {
        Some(h) => h,
        None => single(req, HOST).ok()??,
    };
    let (host, port) = parse_authority(host, scheme)?;
    Some(Origin { scheme, host, port })
}

/// Whether a state-changing request comes from the same origin.
///
/// The origin of `Origin` (scheme, host and port) must equal the request's own scheme, `Host` and
/// port; default ports 80 and 443 equal a missing port. With no `Origin`, a `Referer` is parsed
/// the same way. `Origin: null`, a repeated header or a malformed value is rejected, and a
/// request with neither header is allowed.
pub(crate) fn same_origin(req: &Request) -> bool {
    let Ok(origin) = single(req, ORIGIN) else {
        return false;
    };
    let claimed = match origin {
        Some(o) => o,
        None => match single(req, REFERER) {
            Ok(Some(r)) => r,
            Ok(None) => return true,
            Err(()) => return false,
        },
    };
    match (parse_url_origin(claimed), expected(req)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxbloom_web::{ClientInfo, HttpMethod};

    fn req(host: &str, headers: &[(&str, &str)]) -> Request {
        let mut r = Request::new(HttpMethod::Post, "/login");
        r.headers.insert("host", host.parse().unwrap());
        for (k, v) in headers {
            r.headers.append(
                http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        r
    }

    #[test]
    fn matching_origin_passes() {
        assert!(same_origin(&req(
            "example.com",
            &[("origin", "http://example.com")]
        )));
        assert!(same_origin(&req(
            "Example.com:8080",
            &[("origin", "http://example.COM:8080")]
        )));
    }

    #[test]
    fn different_scheme_host_or_port_fails() {
        assert!(!same_origin(&req(
            "example.com",
            &[("origin", "https://example.com")]
        )));
        assert!(!same_origin(&req(
            "example.com",
            &[("origin", "http://evil.com")]
        )));
        assert!(!same_origin(&req(
            "example.com:8080",
            &[("origin", "http://example.com")]
        )));
        assert!(!same_origin(&req(
            "example.com",
            &[("origin", "http://example.com:8080")]
        )));
    }

    #[test]
    fn null_origin_fails() {
        assert!(!same_origin(&req("example.com", &[("origin", "null")])));
    }

    #[test]
    fn default_ports_equal_a_missing_port() {
        assert!(same_origin(&req(
            "example.com:80",
            &[("origin", "http://example.com")]
        )));
        assert!(same_origin(&req(
            "example.com",
            &[("origin", "http://example.com:80")]
        )));
        let mut r = req("example.com", &[("origin", "https://example.com:443")]);
        r.extensions_mut().insert(ClientInfo {
            addr: "192.0.2.1:0".parse().unwrap(),
            scheme: Scheme::Https,
            host: None,
        });
        assert!(same_origin(&r));
    }

    #[test]
    fn referer_is_the_fallback() {
        assert!(same_origin(&req(
            "example.com",
            &[("referer", "http://example.com/login?x=1")]
        )));
        assert!(!same_origin(&req(
            "example.com",
            &[("referer", "http://example.com.evil.com/login")]
        )));
        assert!(!same_origin(&req("example.com", &[("referer", "garbage")])));
    }

    #[test]
    fn origin_wins_over_referer() {
        let r = req(
            "example.com",
            &[
                ("origin", "http://evil.com"),
                ("referer", "http://example.com/"),
            ],
        );
        assert!(!same_origin(&r));
    }

    #[test]
    fn neither_header_is_allowed() {
        assert!(same_origin(&req("example.com", &[])));
    }

    #[test]
    fn two_origin_headers_fail() {
        let r = req(
            "example.com",
            &[
                ("origin", "http://example.com"),
                ("origin", "http://example.com"),
            ],
        );
        assert!(!same_origin(&r));
    }

    #[test]
    fn userinfo_and_missing_host_fail() {
        assert!(!same_origin(&req(
            "example.com",
            &[("origin", "http://a@example.com")]
        )));
        let mut r = Request::new(HttpMethod::Post, "/login");
        r.headers
            .insert("origin", "http://example.com".parse().unwrap());
        assert!(!same_origin(&r));
    }

    #[test]
    fn forwarded_https_comes_from_the_client_info() {
        let mut r = req("internal:8080", &[("origin", "https://example.com")]);
        assert!(!same_origin(&r));
        r.extensions_mut().insert(ClientInfo {
            addr: "192.0.2.1:0".parse().unwrap(),
            scheme: Scheme::Https,
            host: Some("example.com".into()),
        });
        assert!(same_origin(&r));
        // The plain-http origin of the same host no longer matches.
        let mut r2 = req("internal:8080", &[("origin", "http://example.com")]);
        r2.extensions_mut().insert(ClientInfo {
            addr: "192.0.2.1:0".parse().unwrap(),
            scheme: Scheme::Https,
            host: Some("example.com".into()),
        });
        assert!(!same_origin(&r2));
    }

    #[test]
    fn ipv6_hosts_compare() {
        assert!(same_origin(&req(
            "[::1]:8080",
            &[("origin", "http://[::1]:8080")]
        )));
        assert!(!same_origin(&req(
            "[::1]:8080",
            &[("origin", "http://[::1]")]
        )));
    }
}
