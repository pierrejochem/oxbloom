use std::fmt::Write as _;

use bytes::Bytes;
use http::{HeaderValue, header};

use crate::{ErrorAttributes, ErrorValue, HttpMethod, Response};

/// The request facts an [`ErrorRenderer`] may use: method and path (never the query).
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, RequestInfo};
///
/// let info = RequestInfo { method: HttpMethod::Get, path: "/a".into() };
/// assert_eq!(info.path, "/a");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestInfo {
    /// Request method.
    pub method: HttpMethod,
    /// Request path, without query.
    pub path: String,
}

/// Turns [`ErrorAttributes`] into a fresh error [`Response`].
///
/// The renderer returns a new response; copying headers of the original response is the
/// caller's job.
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::{ErrorAttributes, ErrorRenderer, HttpMethod, RequestInfo, Response};
///
/// struct Plain;
///
/// impl ErrorRenderer for Plain {
///     fn render(&self, attrs: &ErrorAttributes, _info: &RequestInfo) -> Response {
///         let mut res = Response::new("nope".into());
///         *res.status_mut() = attrs.status;
///         res
///     }
/// }
///
/// let info = RequestInfo { method: HttpMethod::Get, path: "/".into() };
/// let res = Plain.render(&ErrorAttributes::from_status(StatusCode::BAD_REQUEST), &info);
/// assert_eq!(res.status(), 400);
/// ```
pub trait ErrorRenderer: Send + Sync {
    /// Renders `attrs` for the request described by `info`.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::{ErrorAttributes, ErrorRenderer, HttpMethod, ProblemRenderer, RequestInfo};
    ///
    /// let info = RequestInfo { method: HttpMethod::Get, path: "/x".into() };
    /// let res = ProblemRenderer::new(false)
    ///     .render(&ErrorAttributes::from_status(StatusCode::NOT_FOUND), &info);
    /// assert_eq!(res.status(), 404);
    /// ```
    fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response;
}

/// A shared renderer, such as an `Arc<dyn ErrorRenderer>` bean, is a renderer too, so it can be
/// passed to [`App::error_renderer`](crate::App::error_renderer) as it is.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_web::{App, ErrorRenderer, HttpMethod, ProblemRenderer, Request};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let shared: Arc<dyn ErrorRenderer> = Arc::new(ProblemRenderer::new(false));
/// let app = App::new().error_renderer(Arc::clone(&shared));
/// let res = app.handle(Request::new(HttpMethod::Get, "/missing")).await;
/// assert_eq!(res.status(), 404);
/// assert_eq!(res.headers()["content-type"], "application/problem+json");
/// # }
/// ```
impl<R: ErrorRenderer + ?Sized> ErrorRenderer for std::sync::Arc<R> {
    fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response {
        (**self).render(attrs, info)
    }
}

/// The default [`ErrorRenderer`]: RFC 9457 `application/problem+json`.
///
/// Members: `type` (default `about:blank`), `title` (the author's, else the status reason
/// phrase), `status`, `detail` (the author's; else the internal message only when
/// `include_internal`), `instance` (the request path) and the extension members. An extension
/// whose key equals a built-in member is dropped.
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::{ErrorAttributes, ErrorRenderer, HttpMethod, ProblemRenderer, RequestInfo};
///
/// let info = RequestInfo { method: HttpMethod::Get, path: "/x".into() };
/// let res = ProblemRenderer::new(false)
///     .render(&ErrorAttributes::from_status(StatusCode::NOT_FOUND), &info);
/// assert_eq!(res.headers()["content-type"], "application/problem+json");
/// assert_eq!(
///     res.body(),
///     r#"{"type":"about:blank","title":"Not Found","status":404,"instance":"/x"}"#
/// );
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ProblemRenderer {
    include_internal: bool,
}

impl ProblemRenderer {
    /// A renderer; `include_internal` shows the internal message as `detail` when the author set
    /// none.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::ProblemRenderer;
    ///
    /// let _ = ProblemRenderer::new(true);
    /// ```
    pub fn new(include_internal: bool) -> Self {
        Self { include_internal }
    }
}

fn escape_json(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

const BUILT_IN: [&str; 5] = ["type", "title", "status", "detail", "instance"];

impl ErrorRenderer for ProblemRenderer {
    fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response {
        let mut b = String::from("{\"type\":");
        escape_json(&mut b, attrs.type_uri.as_deref().unwrap_or("about:blank"));
        b.push_str(",\"title\":");
        let title = attrs
            .title
            .as_deref()
            .or_else(|| attrs.status.canonical_reason())
            .unwrap_or("");
        escape_json(&mut b, title);
        let _ = write!(b, ",\"status\":{}", attrs.status.as_u16());
        let detail = attrs.detail.as_deref().or(if self.include_internal {
            attrs.internal.as_deref()
        } else {
            None
        });
        if let Some(d) = detail {
            b.push_str(",\"detail\":");
            escape_json(&mut b, d);
        }
        b.push_str(",\"instance\":");
        escape_json(&mut b, &info.path);
        for (k, v) in &attrs.extensions {
            if BUILT_IN.contains(&k.as_str()) {
                continue;
            }
            b.push(',');
            escape_json(&mut b, k);
            b.push(':');
            match v {
                ErrorValue::Str(s) => escape_json(&mut b, s),
                ErrorValue::Int(i) => {
                    let _ = write!(b, "{i}");
                }
                ErrorValue::Bool(t) => b.push_str(if *t { "true" } else { "false" }),
            }
        }
        b.push('}');
        let mut res = Response::new(Bytes::from(b));
        *res.status_mut() = attrs.status;
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        res
    }
}

#[cfg(test)]
mod tests {
    use http::StatusCode;

    use super::*;

    fn info(path: &str) -> RequestInfo {
        RequestInfo {
            method: HttpMethod::Get,
            path: path.into(),
        }
    }

    fn json(res: &Response) -> serde_json::Value {
        serde_json::from_slice(res.body()).expect("valid json")
    }

    #[test]
    fn problem_body_has_required_members() {
        let res = ProblemRenderer::new(false).render(
            &ErrorAttributes::from_status(StatusCode::NOT_FOUND),
            &info("/a/b"),
        );
        assert_eq!(res.status(), 404);
        assert_eq!(res.headers()["content-type"], "application/problem+json");
        let v = json(&res);
        assert_eq!(v["type"], "about:blank");
        assert_eq!(v["title"], "Not Found");
        assert_eq!(v["status"], 404);
        assert_eq!(v["instance"], "/a/b");
        assert!(v.get("detail").is_none());
    }

    #[test]
    fn author_detail_and_extensions_are_rendered() {
        let mut a = ErrorAttributes::from_status(StatusCode::BAD_REQUEST);
        a.detail = Some("x".into());
        a.type_uri = Some("https://e.com/t".into());
        a.extensions = vec![
            ("limit".into(), 5_i64.into()),
            ("ok".into(), true.into()),
            ("who".into(), "me".into()),
        ];
        let v = json(&ProblemRenderer::new(false).render(&a, &info("/")));
        assert_eq!(v["detail"], "x");
        assert_eq!(v["type"], "https://e.com/t");
        assert_eq!(v["limit"], 5);
        assert_eq!(v["ok"], true);
        assert_eq!(v["who"], "me");
    }

    #[test]
    fn title_defaults_to_reason_phrase_and_author_title_wins() {
        let mut a = ErrorAttributes::from_status(StatusCode::CONFLICT);
        let r = ProblemRenderer::new(false);
        assert_eq!(json(&r.render(&a, &info("/")))["title"], "Conflict");
        a.title = Some("Mine".into());
        assert_eq!(json(&r.render(&a, &info("/")))["title"], "Mine");
    }

    #[test]
    fn include_internal_false_hides_internal_and_true_shows_it_as_detail() {
        let mut a = ErrorAttributes::from_status(StatusCode::INTERNAL_SERVER_ERROR);
        a.internal = Some("boom".into());
        let hidden = ProblemRenderer::new(false).render(&a, &info("/"));
        assert!(json(&hidden).get("detail").is_none());
        assert!(!String::from_utf8_lossy(hidden.body()).contains("boom"));
        let shown = ProblemRenderer::new(true).render(&a, &info("/"));
        assert_eq!(json(&shown)["detail"], "boom");
    }

    #[test]
    fn author_detail_is_kept_when_internal_is_set() {
        let mut a = ErrorAttributes::from_status(StatusCode::INTERNAL_SERVER_ERROR);
        a.internal = Some("boom".into());
        a.detail = Some("mine".into());
        for flag in [false, true] {
            let v = json(&ProblemRenderer::new(flag).render(&a, &info("/")));
            assert_eq!(v["detail"], "mine");
        }
    }

    #[test]
    fn json_is_escaped() {
        let nasty = "q\" b\\ n\n r\r t\t c\u{1} \u{1f} é€😀";
        let mut a = ErrorAttributes::from_status(StatusCode::BAD_REQUEST);
        a.title = Some(nasty.into());
        a.detail = Some(nasty.into());
        a.type_uri = Some(nasty.into());
        a.extensions = vec![(nasty.into(), nasty.into())];
        let res = ProblemRenderer::new(false).render(&a, &info(nasty));
        let text = std::str::from_utf8(res.body()).unwrap();
        assert!(text.contains("é€😀"));
        assert!(!text.chars().any(|c| (c as u32) < 0x20));
        let v = json(&res);
        assert_eq!(v["title"], nasty);
        assert_eq!(v["detail"], nasty);
        assert_eq!(v["type"], nasty);
        assert_eq!(v["instance"], nasty);
        assert_eq!(v[nasty], nasty);
    }

    #[test]
    fn colliding_extension_keys_are_dropped() {
        let mut a = ErrorAttributes::from_status(StatusCode::BAD_REQUEST);
        a.detail = Some("real".into());
        a.extensions = ["type", "title", "status", "detail", "instance"]
            .iter()
            .map(|k| ((*k).to_owned(), ErrorValue::Str("evil".into())))
            .chain([("extra".to_owned(), ErrorValue::Int(1))])
            .collect();
        let res = ProblemRenderer::new(false).render(&a, &info("/p"));
        let text = std::str::from_utf8(res.body()).unwrap();
        for k in ["type", "title", "status", "detail", "instance"] {
            assert_eq!(text.matches(&format!("\"{k}\":")).count(), 1, "{k}");
        }
        assert!(!text.contains("evil"));
        let v = json(&res);
        assert_eq!(v["status"], 400);
        assert_eq!(v["detail"], "real");
        assert_eq!(v["extra"], 1);
    }

    #[tokio::test]
    async fn arc_renderer_is_a_renderer() {
        struct Teapot;
        impl ErrorRenderer for Teapot {
            fn render(&self, attrs: &ErrorAttributes, _info: &RequestInfo) -> Response {
                let mut res = Response::new(Bytes::from_static(b"tea"));
                *res.status_mut() = attrs.status;
                res
            }
        }
        let shared: std::sync::Arc<dyn ErrorRenderer> = std::sync::Arc::new(Teapot);
        let res = shared.render(
            &ErrorAttributes::from_status(StatusCode::NOT_FOUND),
            &info("/"),
        );
        assert_eq!(res.body(), "tea");
        let app = crate::App::new().error_renderer(shared);
        let res = app.handle(crate::Request::new(HttpMethod::Get, "/x")).await;
        assert_eq!(res.status(), 404);
        assert_eq!(res.body(), "tea");
    }

    #[test]
    fn instance_never_contains_a_query() {
        let v = json(&ProblemRenderer::new(false).render(
            &ErrorAttributes::from_status(StatusCode::NOT_FOUND),
            &info("/users/7"),
        ));
        assert_eq!(v["instance"], "/users/7");
    }
}
