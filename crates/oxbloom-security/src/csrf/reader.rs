//! Reading the token a request submitted.

use http::header::{CONTENT_TYPE, HeaderName};
use oxbloom_web::Request;

use super::CsrfConfig;
use crate::form_login::urlencoded_pairs;

/// The media type of an HTML form post.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// Reads the token a request submitted: the configured header first and, when there is no such
/// header and the body is urlencoded, the configured form field in the buffered body (at most
/// 4096 bytes, the first value wins). The query string is never read. `None` when no value is
/// found, the value is empty or the body was too large.
pub(crate) fn submitted_token(req: &Request, cfg: &CsrfConfig) -> Option<String> {
    let name = HeaderName::from_bytes(cfg.header_name.as_bytes()).ok()?;
    let value = if let Some(v) = req.headers.get(&name) {
        v.to_str().ok()?.to_owned()
    } else {
        let is_form = req
            .headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|m| m.trim().eq_ignore_ascii_case(FORM_CONTENT_TYPE));
        if !is_form || req.body_too_large() {
            return None;
        }
        urlencoded_pairs(&req.body)?
            .into_iter()
            .find(|(k, _)| *k == cfg.field_name)?
            .1
    };
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxbloom_core::HttpMethod;

    fn req(header: &[&str], content_type: Option<&str>, body: &[u8]) -> Request {
        let mut r = Request::new(HttpMethod::Post, "/x");
        for v in header {
            r.headers.append("x-csrf-token", v.parse().unwrap());
        }
        if let Some(ct) = content_type {
            r.headers.insert("content-type", ct.parse().unwrap());
        }
        r.body = body.to_vec().into();
        r
    }

    const FORM: Option<&str> = Some("application/x-www-form-urlencoded");

    fn read(r: &Request) -> Option<String> {
        submitted_token(r, &CsrfConfig::default())
    }

    #[test]
    fn header_wins_over_the_body_field() {
        let r = req(&["from-header"], FORM, b"_csrf=from-body");
        assert_eq!(read(&r).as_deref(), Some("from-header"));
    }

    #[test]
    fn body_field_when_there_is_no_header() {
        let r = req(&[], FORM, b"a=1&_csrf=from-body");
        assert_eq!(read(&r).as_deref(), Some("from-body"));
        let r = req(
            &[],
            Some("application/x-www-form-urlencoded; charset=UTF-8"),
            b"_csrf=v",
        );
        assert_eq!(read(&r).as_deref(), Some("v"));
    }

    #[test]
    fn first_of_two_headers_and_first_of_two_fields() {
        let r = req(&["one", "two"], None, b"");
        assert_eq!(read(&r).as_deref(), Some("one"));
        let r = req(&[], FORM, b"_csrf=one&_csrf=two");
        assert_eq!(read(&r).as_deref(), Some("one"));
    }

    #[test]
    fn a_token_in_the_query_is_ignored() {
        let mut r = req(&[], FORM, b"");
        r.query = Some("_csrf=q&X-CSRF-TOKEN=q".to_owned());
        assert_eq!(read(&r), None);
    }

    #[test]
    fn empty_value_is_none() {
        assert_eq!(read(&req(&[""], FORM, b"_csrf=body")), None);
        assert_eq!(read(&req(&[], FORM, b"_csrf=")), None);
    }

    #[test]
    fn oversize_body_is_none() {
        let mut body = b"_csrf=v&pad=".to_vec();
        body.resize(4096, b'x');
        assert_eq!(read(&req(&[], FORM, &body)).as_deref(), Some("v"));
        body.push(b'x');
        assert_eq!(body.len(), 4097);
        assert_eq!(read(&req(&[], FORM, &body)), None);
    }

    #[test]
    fn json_body_without_the_header_is_none() {
        let r = req(&[], Some("application/json"), b"{\"_csrf\":\"v\"}");
        assert_eq!(read(&r), None);
        let r = req(&[], None, b"_csrf=v");
        assert_eq!(read(&r), None);
    }

    #[test]
    fn field_value_is_percent_decoded() {
        let r = req(&[], FORM, b"_csrf=a%2Bb%3D+c");
        assert_eq!(read(&r).as_deref(), Some("a+b= c"));
    }

    #[test]
    fn configured_names_are_used() {
        let cfg = CsrfConfig::default().field_name("t").header_name("X-T");
        let mut r = req(&[], FORM, b"t=v&_csrf=no");
        assert_eq!(submitted_token(&r, &cfg).as_deref(), Some("v"));
        r.headers.insert("x-t", "h".parse().unwrap());
        assert_eq!(submitted_token(&r, &cfg).as_deref(), Some("h"));
    }
}
