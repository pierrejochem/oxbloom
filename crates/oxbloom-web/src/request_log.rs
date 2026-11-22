use std::fmt::Write as _;
use std::time::Instant;

use http::HeaderMap;
use tracing::Level;

use crate::{Filter, Next, Request, Response, order};

/// Header names whose values are always logged as `<redacted>`, even when allowlisted.
///
/// Stored normalized (see [`normalize`]): `authorization`, `proxy-authorization`, `cookie`,
/// `set-cookie`, `x-api-key`, `x-auth-token`, `x-csrf-token` and `x-xsrf-token`, matched with any
/// `-`/`_` separator and case. A custom CSRF header name (`security.csrf.header-name`) is not on
/// this list and must not be allowlisted.
const DENY_LIST: [&str; 8] = [
    "authorization",
    "proxyauthorization",
    "cookie",
    "setcookie",
    "xapikey",
    "xauthtoken",
    "xcsrftoken",
    "xxsrftoken",
];

/// Lowercases `name` and drops `-` and `_`, so `X_Api-Key` and `x-api-key` compare equal.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '-' && *c != '_')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

const REDACTED: &str = "<redacted>";

/// A [`Filter`] that logs one event per request after the response is produced.
///
/// The event has target `oxbloom::web::request` and the fields `method`, `path`, `status`,
/// `latency_ms` and `headers`. `headers` holds `name="value"` pairs, separated by `, `, for the
/// allowlisted headers that the request carries. The query string is never logged, also not
/// when a hand-built request leaves it in the path (the path is cut at the first `?`).
///
/// A header value prints as `<redacted>` when it is flagged sensitive or when its name is
/// `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`,
/// `x-auth-token`, `x-csrf-token` or `x-xsrf-token` (in any case, with `-` or `_` as separator), even if that name is on the
/// allowlist. Control characters in the path and in
/// header values are escaped, so an event never spans more than one line.
///
/// The filter takes the slot [`order::REQUEST_LOG`] (-400), the outermost one: it wraps error
/// handling, the body limit and the security chain, so their answers are logged too.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{App, Filter, HttpMethod, Request, RequestLogFilter, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let log = RequestLogFilter::new(tracing::Level::INFO, vec!["user-agent".into()]);
/// assert_eq!(log.order(), oxbloom_web::order::REQUEST_LOG);
/// let app = App::new()
///     .filter(log)
///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
/// let res = app.handle(Request::new(HttpMethod::Get, "/ping")).await;
/// assert_eq!(res.body(), "pong");
/// # Ok(())
/// # }
/// ```
pub struct RequestLogFilter {
    level: Level,
    headers: Vec<String>,
}

impl RequestLogFilter {
    /// Creates a filter that logs at `level` and includes the request headers named in
    /// `headers` (matched case-insensitively).
    ///
    /// Only the exact built-in credential names (`authorization`, `proxy-authorization`,
    /// `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`, `x-csrf-token`, `x-xsrf-token`, in
    /// any case and with `-` or `_`) are redacted by name. A name that merely contains a
    /// credential word, such as `x-my-token` or a custom CSRF header name, is logged in full if
    /// allowlisted: keeping secrets off the allowlist is the operator's responsibility. Values flagged sensitive are always redacted.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, HttpMethod, Request, RequestLogFilter};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// // `authorization` is allowlisted here, but it is still logged as `<redacted>`.
    /// let filter = RequestLogFilter::new(
    ///     tracing::Level::DEBUG,
    ///     vec!["Accept".into(), "Authorization".into()],
    /// );
    /// let res = App::new().filter(filter).handle(Request::new(HttpMethod::Get, "/x")).await;
    /// assert_eq!(res.status(), 404);
    /// # }
    /// ```
    pub fn new(level: Level, headers: Vec<String>) -> Self {
        let mut names: Vec<String> = Vec::with_capacity(headers.len());
        for name in headers {
            let name = name.to_ascii_lowercase();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        Self {
            level,
            headers: names,
        }
    }

    fn render_headers(&self, map: &HeaderMap) -> String {
        let mut out = String::new();
        for name in &self.headers {
            let denied = DENY_LIST.contains(&normalize(name).as_str());
            for value in map.get_all(name.as_str()) {
                if !out.is_empty() {
                    out.push_str(", ");
                }
                out.push_str(&escape(name));
                out.push('=');
                if denied || value.is_sensitive() {
                    out.push_str(REDACTED);
                } else {
                    out.push('"');
                    out.push_str(&escape(&String::from_utf8_lossy(value.as_bytes())));
                    out.push('"');
                }
            }
        }
        out
    }
}

/// Escapes control characters (and quotes and backslashes) so a value stays on one line.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let _ = write!(out, "{}", s.escape_debug());
    out
}

macro_rules! request_event {
    ($level:expr, $($fields:tt)*) => {
        match $level {
            Level::TRACE => tracing::event!(target: "oxbloom::web::request", Level::TRACE, $($fields)*),
            Level::DEBUG => tracing::event!(target: "oxbloom::web::request", Level::DEBUG, $($fields)*),
            Level::INFO => tracing::event!(target: "oxbloom::web::request", Level::INFO, $($fields)*),
            Level::WARN => tracing::event!(target: "oxbloom::web::request", Level::WARN, $($fields)*),
            Level::ERROR => tracing::event!(target: "oxbloom::web::request", Level::ERROR, $($fields)*),
        }
    };
}

#[async_trait::async_trait]
impl Filter for RequestLogFilter {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let start = Instant::now();
        let method = escape(&req.method.to_string());
        // A path built by hand may still carry its query; it is never logged.
        let path = escape(req.path.split('?').next().unwrap_or_default());
        let headers = self.render_headers(&req.headers);
        let res = next.run(req).await;
        let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let status = res.status().as_u16();
        request_event!(
            self.level,
            method = %method,
            path = %path,
            status,
            latency_ms,
            headers = %headers,
            "request"
        );
        res
    }

    fn order(&self) -> i32 {
        order::REQUEST_LOG
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use http::{HeaderValue, StatusCode};
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;
    use crate::{App, HttpMethod, Route};

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);

    impl Buf {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl io::Write for Buf {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buf {
        type Writer = Buf;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn app(filter: RequestLogFilter) -> App {
        App::new()
            .filter(filter)
            .route(Route::new(HttpMethod::Get, "/a", |_| async { "body-a" }))
            .unwrap()
    }

    /// Runs `req` through `app` with a subscriber at `max` capturing into a buffer.
    async fn capture(app: &App, req: Request, max: Level) -> (Response, String) {
        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_max_level(max)
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let res = app.handle(req).await;
        (res, buf.text())
    }

    fn filter(headers: &[&str]) -> RequestLogFilter {
        RequestLogFilter::new(Level::INFO, headers.iter().map(|h| h.to_string()).collect())
    }

    fn get(path: &str) -> Request {
        Request::new(HttpMethod::Get, path)
    }

    fn with_header(mut req: Request, name: &'static str, value: &'static str) -> Request {
        req.headers.append(name, HeaderValue::from_static(value));
        req
    }

    #[test]
    fn request_log_declares_the_outermost_order() {
        let filter = RequestLogFilter::new(Level::INFO, Vec::new());
        assert_eq!(filter.order(), crate::order::REQUEST_LOG);
    }

    #[tokio::test]
    async fn logs_method_path_status_latency() {
        let (_, out) = capture(&app(filter(&[])), get("/a"), Level::TRACE).await;
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(out.contains("oxbloom::web::request"), "{out}");
        assert!(out.contains("INFO"), "{out}");
        assert!(out.contains("method=GET"), "{out}");
        assert!(out.contains("path=/a"), "{out}");
        assert!(out.contains("status=200"), "{out}");
        assert!(out.contains("latency_ms="), "{out}");
    }

    #[tokio::test]
    async fn status_of_unmatched_route_is_logged() {
        let (_, out) = capture(&app(filter(&[])), get("/missing"), Level::TRACE).await;
        assert!(out.contains("status=404"), "{out}");
    }

    #[tokio::test]
    async fn query_string_is_not_logged() {
        let mut req = get("/a");
        req.query = Some("token=abc".into());
        let (_, out) = capture(&app(filter(&[])), req, Level::TRACE).await;
        assert!(out.contains("path=/a"), "{out}");
        assert!(!out.contains("abc"), "{out}");
        assert!(!out.contains("token"), "{out}");
    }

    #[tokio::test]
    async fn query_left_in_the_path_is_not_logged() {
        let (_, out) = capture(&app(filter(&[])), get("/a?token=x"), Level::TRACE).await;
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(out.contains("path=/a "), "{out}");
        assert!(!out.contains("token"), "{out}");
    }

    #[tokio::test]
    async fn allowlisted_header_is_logged() {
        let req = with_header(get("/a"), "user-agent", "probe/1.0");
        let (_, out) = capture(&app(filter(&["User-Agent"])), req, Level::TRACE).await;
        assert!(out.contains("user-agent=\"probe/1.0\""), "{out}");
    }

    #[tokio::test]
    async fn absent_allowlisted_header_is_omitted() {
        let (_, out) = capture(&app(filter(&["user-agent"])), get("/a"), Level::TRACE).await;
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(!out.contains("user-agent"), "{out}");
    }

    #[tokio::test]
    async fn every_value_of_a_repeated_header_is_logged() {
        let req = with_header(get("/a"), "accept", "text/plain");
        let req = with_header(req, "accept", "text/html");
        let (_, out) = capture(&app(filter(&["accept"])), req, Level::TRACE).await;
        assert!(out.contains("accept=\"text/plain\""), "{out}");
        assert!(out.contains("accept=\"text/html\""), "{out}");
    }

    #[tokio::test]
    async fn non_allowlisted_header_is_not_logged() {
        let req = with_header(get("/a"), "x-trace", "secret-trace");
        let (_, out) = capture(&app(filter(&["user-agent"])), req, Level::TRACE).await;
        assert!(!out.contains("x-trace"), "{out}");
        assert!(!out.contains("secret-trace"), "{out}");
    }

    #[tokio::test]
    async fn authorization_is_redacted_even_if_allowlisted() {
        let req = with_header(get("/a"), "authorization", "Bearer s3cr3t");
        let req = with_header(req, "proxy-authorization", "Basic cHJveHk=");
        let req = with_header(req, "cookie", "session=c00kie");
        let f = filter(&["Authorization", "PROXY-AUTHORIZATION", "cookie"]);
        let (_, out) = capture(&app(f), req, Level::TRACE).await;
        assert!(out.contains("authorization=<redacted>"), "{out}");
        assert!(out.contains("proxy-authorization=<redacted>"), "{out}");
        assert!(out.contains("cookie=<redacted>"), "{out}");
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(!out.contains("cHJveHk"), "{out}");
        assert!(!out.contains("c00kie"), "{out}");
    }

    #[tokio::test]
    async fn custom_credential_names_redacted_in_any_case() {
        let req = with_header(get("/a"), "x-api-key", "key-123");
        let req = with_header(req, "x-auth-token", "tok-456");
        let req = with_header(req, "set-cookie", "sid-789");
        let f = filter(&["X-Api-Key", "x-AUTH-token", "Set-Cookie"]);
        let (_, out) = capture(&app(f), req, Level::TRACE).await;
        assert!(out.contains("x-api-key=<redacted>"), "{out}");
        assert!(out.contains("x-auth-token=<redacted>"), "{out}");
        assert!(out.contains("set-cookie=<redacted>"), "{out}");
        for secret in ["key-123", "tok-456", "sid-789"] {
            assert!(!out.contains(secret), "{out}");
        }
    }

    #[tokio::test]
    async fn sensitive_flagged_value_is_redacted() {
        let mut req = get("/a");
        let mut value = HeaderValue::from_static("hidden-value");
        value.set_sensitive(true);
        req.headers.insert("x-custom", value);
        let (_, out) = capture(&app(filter(&["x-custom"])), req, Level::TRACE).await;
        assert!(out.contains("x-custom=<redacted>"), "{out}");
        assert!(!out.contains("hidden-value"), "{out}");
    }

    #[tokio::test]
    async fn control_characters_do_not_split_the_line() {
        let mut req = get("/a\nINJECTED line");
        req.headers
            .insert("x-note", HeaderValue::from_bytes(b"one\ttwo").unwrap());
        let (_, out) = capture(&app(filter(&["x-note"])), req, Level::TRACE).await;
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(out.contains(r"path=/a\nINJECTED line"), "{out}");
        assert!(out.contains(r#"x-note="one\ttwo""#), "{out}");
    }

    #[tokio::test]
    async fn non_utf8_header_value_does_not_panic() {
        let mut req = get("/a");
        req.headers
            .insert("x-raw", HeaderValue::from_bytes(b"ok\xff\xfe").unwrap());
        let (res, out) = capture(&app(filter(&["x-raw"])), req, Level::TRACE).await;
        assert_eq!(res.status(), 200);
        assert!(out.contains("x-raw=\"ok"), "{out}");
    }

    #[tokio::test]
    async fn level_is_configurable() {
        let f = RequestLogFilter::new(Level::DEBUG, Vec::new());
        let (_, out) = capture(&app(f), get("/a"), Level::INFO).await;
        assert_eq!(out, "");

        let f = RequestLogFilter::new(Level::WARN, Vec::new());
        let (_, out) = capture(&app(f), get("/a"), Level::INFO).await;
        assert!(out.contains("WARN"), "{out}");
    }

    #[tokio::test]
    async fn every_level_emits() {
        for level in [
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR,
        ] {
            let f = RequestLogFilter::new(level, Vec::new());
            let (_, out) = capture(&app(f), get("/a"), Level::TRACE).await;
            assert!(out.contains(level.as_str()), "{level}: {out}");
        }
    }

    #[tokio::test]
    async fn underscore_spellings_of_credential_names_are_redacted() {
        let req = with_header(get("/a"), "x_api_key", "secret-1");
        let req = with_header(req, "x_auth_token", "secret-2");
        let req = with_header(req, "proxy_authorization", "secret-3");
        let req = with_header(req, "set_cookie", "secret-4");
        let f = filter(&[
            "X_Api_Key",
            "x_auth_token",
            "proxy_authorization",
            "Set_Cookie",
        ]);
        let (_, out) = capture(&app(f), req, Level::TRACE).await;
        assert!(out.contains("x_api_key=<redacted>"), "{out}");
        assert!(out.contains("x_auth_token=<redacted>"), "{out}");
        assert!(out.contains("proxy_authorization=<redacted>"), "{out}");
        assert!(out.contains("set_cookie=<redacted>"), "{out}");
        assert!(!out.contains("secret"), "{out}");
    }

    #[tokio::test]
    async fn names_merely_containing_a_credential_word_are_not_redacted() {
        let req = with_header(get("/a"), "x-my-token", "visible");
        let (_, out) = capture(&app(filter(&["x-my-token"])), req, Level::TRACE).await;
        assert!(out.contains(r#"x-my-token="visible""#), "{out}");
    }

    #[tokio::test]
    async fn csrf_token_headers_are_redacted_even_if_allowlisted() {
        let mut req = with_header(get("/a"), "x-csrf-token", "csrf-secret");
        req.headers
            .append("x-xsrf-token", HeaderValue::from_static("xsrf-secret"));
        let f = filter(&["X-CSRF-TOKEN", "X-XSRF-Token"]);
        let (_, out) = capture(&app(f), req, Level::TRACE).await;
        assert!(out.contains("x-csrf-token=<redacted>"), "{out}");
        assert!(out.contains("x-xsrf-token=<redacted>"), "{out}");
        assert!(!out.contains("secret"), "{out}");
    }

    #[tokio::test]
    async fn header_values_are_quoted_so_they_cannot_forge_pairs() {
        let req = with_header(get("/a"), "user-agent", "x, authorization=Bearer abc");
        let (_, out) = capture(&app(filter(&["user-agent"])), req, Level::TRACE).await;
        assert!(
            out.contains(r#"headers=user-agent="x, authorization=Bearer abc""#),
            "{out}"
        );
    }

    #[tokio::test]
    async fn quotes_in_values_are_escaped() {
        let req = with_header(get("/a"), "user-agent", "a\" b");
        let (_, out) = capture(&app(filter(&["user-agent"])), req, Level::TRACE).await;
        assert!(out.contains(r#"user-agent="a\" b""#), "{out}");
    }

    #[tokio::test]
    async fn response_is_passed_through_unchanged() {
        let (res, _) = capture(&app(filter(&[])), get("/a"), Level::TRACE).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.body(), "body-a");

        let (res, _) = capture(&app(filter(&[])), get("/nope"), Level::TRACE).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    struct Boom;

    #[async_trait::async_trait]
    impl Filter for Boom {
        async fn filter(&self, req: Request, next: Next<'_>) -> Response {
            if req.path == "/a" {
                panic!("filter boom");
            }
            next.run(req).await
        }
    }

    fn request_events(out: &str) -> usize {
        out.lines()
            .filter(|l| l.contains("oxbloom::web::request"))
            .count()
    }

    #[tokio::test]
    async fn which_panics_lose_the_request_event() {
        // Between the request log and the error filter: the outer catch answers 500, but the
        // panic unwinds through the request log, so there is no request event.
        let between = app(filter(&[])).filter_ordered(order::REQUEST_LOG + 50, Boom);
        let (res, out) = capture(&between, get("/a"), Level::TRACE).await;
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(out.contains("request panicked: filter boom"), "{out}");
        assert_eq!(request_events(&out), 0, "{out}");

        // After the error filter: the error filter answers 500 and the request log records it.
        let after = app(filter(&[])).filter_ordered(order::ERROR_HANDLING + 50, Boom);
        let (res, out) = capture(&after, get("/a"), Level::TRACE).await;
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(request_events(&out), 1, "{out}");
        assert!(out.contains("status=500"), "{out}");
    }
}
