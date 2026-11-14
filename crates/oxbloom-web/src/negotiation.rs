//! `Accept` handling for JSON answers.

use http::{HeaderMap, StatusCode, header};

use crate::json::JsonBody;
use crate::{Filter, HttpError, IntoResponse, Next, Request, Response, order};

/// Splits `value` at commas that are not inside a double-quoted string, or `None` when a quote
/// is never closed.
fn split_ranges(value: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return None;
    }
    parts.push(&value[start..]);
    Some(parts)
}

/// Parses a qvalue as RFC 9110 writes it: `0` or `1`, then optionally `.` and up to three
/// digits, with `1` followed only by zeros.
fn parse_qvalue(value: &str) -> Option<f32> {
    let (int, frac) = match value.split_once('.') {
        Some((int, frac)) => (int, frac),
        None => (value, ""),
    };
    let digits_ok = frac.len() <= 3 && frac.bytes().all(|b| b.is_ascii_digit());
    let ok = match int {
        "0" => digits_ok,
        "1" => digits_ok && frac.bytes().all(|b| b == b'0'),
        _ => false,
    };
    if !ok {
        return None;
    }
    value.parse().ok()
}

/// Whether `media` (already lower-cased) is `*/*`, `type/*` or `type/subtype` with non-empty
/// parts that contain no `*` other than the whole part.
fn valid_range(media: &str) -> bool {
    let Some((ty, sub)) = media.split_once('/') else {
        return false;
    };
    let plain = |p: &str| !p.is_empty() && !p.contains('*') && !p.contains('/');
    match (ty, sub) {
        ("*", "*") => true,
        (t, "*") => plain(t),
        (t, s) => plain(t) && plain(s),
    }
}

/// The quality of `params` (the text after the first `;` of a range), or `None` when a `q`
/// parameter is not a number in `0..=1`.
fn quality(params: &str) -> Option<f32> {
    let mut q = 1.0;
    let (mut quoted, mut start) = (false, 0);
    let mut pieces = Vec::new();
    for (i, c) in params.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                pieces.push(&params[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    pieces.push(&params[start..]);
    for piece in pieces {
        if let Some((name, value)) = piece.split_once('=')
            && name.trim().eq_ignore_ascii_case("q")
        {
            q = parse_qvalue(value.trim())?;
        }
    }
    Some(q)
}

/// Whether the `Accept` headers allow an `application/json` answer.
///
/// A missing header and a header without any valid range allow it. A header line with a byte
/// that is not visible ASCII, or with an unterminated double quote, is skipped like a missing
/// one. Ranges that are not `*/*`, `type/*` or `type/subtype`, and ranges with an invalid `q`,
/// are skipped. The most specific range that matches decides: `application/json`, then
/// `application/*`, then `*/*`.
pub(crate) fn accepts_json(headers: &HeaderMap) -> bool {
    let mut any_valid = false;
    let mut best: Option<(u8, f32)> = None;
    for value in headers.get_all(header::ACCEPT) {
        let Ok(value) = value.to_str() else { continue };
        let Some(ranges) = split_ranges(value) else {
            continue;
        };
        for range in ranges {
            let (media, params) = range.split_once(';').unwrap_or((range, ""));
            let media = media.trim().to_ascii_lowercase();
            if !valid_range(&media) {
                continue;
            }
            let Some(q) = quality(params) else { continue };
            any_valid = true;
            let specificity = match media.as_str() {
                "application/json" => 3,
                "application/*" => 2,
                "*/*" => 1,
                _ => continue,
            };
            if best.is_none_or(|(s, _)| specificity > s) {
                best = Some((specificity, q));
            }
        }
    }
    !any_valid || best.is_some_and(|(_, q)| q > 0.0)
}

/// Answers 406 when a handler produced a [`JsonBody`] the client's `Accept` header excludes.
///
/// Only responses that carry the marker are touched, so error answers, other body types and
/// responses of filters outside this one pass unchanged. The marker survives the router's body
/// strip for `HEAD`, so `HEAD` gets the same answer as `GET`.
pub(crate) struct JsonNegotiationFilter;

#[async_trait::async_trait]
impl Filter for JsonNegotiationFilter {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let acceptable = accepts_json(&req.headers);
        let res = next.run(req).await;
        if !acceptable && res.extensions().get::<JsonBody>().is_some() {
            return HttpError::new(StatusCode::NOT_ACCEPTABLE)
                .with("supported", "application/json")
                .into_response();
        }
        res
    }

    fn order(&self) -> i32 {
        order::NEGOTIATION
    }
}

#[cfg(test)]
mod tests {
    use http::{HeaderValue, header};

    use super::*;

    fn accepts(values: &[&str]) -> bool {
        let mut headers = HeaderMap::new();
        for v in values {
            headers.append(header::ACCEPT, HeaderValue::from_str(v).unwrap());
        }
        accepts_json(&headers)
    }

    #[test]
    fn accept_matrix_is_decided_by_the_most_specific_range() {
        let table: &[(&str, bool)] = &[
            ("", true),
            ("*/*", true),
            ("application/*", true),
            ("application/json", true),
            ("APPLICATION/JSON", true),
            ("application/json;q=0", false),
            ("application/json ; q=0.0", false),
            ("text/html", false),
            ("text/html, application/json;q=0.5", true),
            ("application/json;q=0, */*;q=0.1", false),
            ("*/*;q=0, application/json", true),
            ("text/html;q=abc", true),
            ("application/json;q=1.5", true),
            ("application/json;q=abc, text/html", false),
            ("application/problem+json", false),
            ("foo", true),
            ("foo, text/html", false),
            ("text/html;x=\"a,b\", application/json", true),
            ("text/html;x=\"a,application/json\"", false),
            ("text/html ,  application/json", true),
            ("text/*", false),
            ("application/*+json", true),
            ("application/*+json, text/html", false),
            ("*/json", true),
            ("/", true),
            ("application/json;q=.5", true),
            ("application/json;q=0.5555", true),
            ("application/json;q=1.000", true),
            ("text/html, application/json;q=+0.5", false),
            ("text/html, application/json;q=1e-1", false),
            ("a/, /b, application/j*", true),
            ("application/json;q=0.", false),
            ("application/json;q=1.", true),
            ("application/json;q=1.5", true),
            ("text/html;x=\"abc, application/json", true),
            ("text/html;x=\"abc, application/json;q=0", true),
        ];
        for (value, expected) in table {
            assert_eq!(accepts(&[value]), *expected, "Accept: {value:?}");
        }
    }

    #[test]
    fn absent_accept_header_is_acceptable() {
        assert!(accepts_json(&HeaderMap::new()));
    }

    #[test]
    fn several_header_lines_are_combined() {
        assert!(accepts(&["text/html", "application/json"]));
        assert!(!accepts(&["text/html", "image/png"]));
    }

    #[test]
    fn header_bytes_that_are_not_visible_ascii_count_as_absent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_bytes(b"text/\xff").unwrap(),
        );
        assert!(accepts_json(&headers));
    }
}
