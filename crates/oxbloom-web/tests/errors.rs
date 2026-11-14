//! Error pipeline: framework answers, handler errors and panics rendered as problem+json.

use std::cell::RefCell;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use http::{HeaderValue, StatusCode, header};
use oxbloom_web::{
    App, ErrorAttributes, ErrorRenderer, Filter, HttpError, HttpMethod, IntoResponse, Next,
    ProblemRenderer, Request, RequestInfo, Response, Route, order,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing_subscriber::fmt::MakeWriter;

const PROBLEM: &str = "application/problem+json";

fn get(path: &str) -> Request {
    Request::new(HttpMethod::Get, path)
}

fn json(res: &Response) -> serde_json::Value {
    serde_json::from_slice(res.body()).expect("valid json")
}

fn content_type(res: &Response) -> &str {
    res.headers()[header::CONTENT_TYPE].to_str().unwrap()
}

fn route(path: &str, f: fn() -> Response) -> Route {
    Route::new(HttpMethod::Get, path, move |_| async move { f() })
}

fn panicking_route() -> Route {
    init_tracing();
    Route::new(HttpMethod::Get, "/boom", |_| async {
        if true {
            panic!("secret is hunter2");
        }
        "unreachable"
    })
}

#[tokio::test]
async fn unknown_path_is_problem_json_404() {
    let res = App::new().handle(get("/nope")).await;
    assert_eq!(res.status(), 404);
    assert_eq!(content_type(&res), PROBLEM);
    let v = json(&res);
    assert_eq!(v["status"], 404);
    assert_eq!(v["title"], "Not Found");
    assert_eq!(v["instance"], "/nope");
}

#[tokio::test]
async fn instance_has_no_query_when_path_contains_a_question_mark() {
    let res = App::new().handle(get("/a?token=x")).await;
    assert_eq!(res.status(), 404);
    assert_eq!(json(&res)["instance"], "/a");
    assert!(!String::from_utf8_lossy(res.body()).contains("token"));
}

#[tokio::test]
async fn wrong_method_is_problem_json_405_with_allow() {
    let app = App::new()
        .route(route("/a", || "a".into_response()))
        .unwrap();
    let res = app.handle(Request::new(HttpMethod::Delete, "/a")).await;
    assert_eq!(res.status(), 405);
    assert_eq!(content_type(&res), PROBLEM);
    assert_eq!(res.headers()[header::ALLOW], "GET, HEAD, OPTIONS");
    assert_eq!(json(&res)["status"], 405);
}

#[tokio::test]
async fn handler_http_error_is_rendered_with_detail_and_extensions() {
    let app = App::new()
        .route(route("/e", || {
            HttpError::new(StatusCode::CONFLICT)
                .detail("already there")
                .with("id", 7)
                .into_response()
        }))
        .unwrap();
    let res = app.handle(get("/e")).await;
    assert_eq!(res.status(), 409);
    assert_eq!(content_type(&res), PROBLEM);
    let v = json(&res);
    assert_eq!(v["detail"], "already there");
    assert_eq!(v["id"], 7);
    assert_eq!(v["instance"], "/e");
}

#[tokio::test]
async fn handler_result_err_is_rendered() {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/r", |_| async {
            let r: Result<&'static str, HttpError> =
                Err(HttpError::new(StatusCode::BAD_REQUEST).title("Bad input"));
            r
        }))
        .unwrap();
    let res = app.handle(get("/r")).await;
    assert_eq!(res.status(), 400);
    assert_eq!(json(&res)["title"], "Bad input");
}

#[tokio::test]
async fn status_tuple_over_http_error_renders_the_response_status() {
    let app = App::new()
        .route(route("/t", || {
            (StatusCode::GONE, HttpError::new(StatusCode::BAD_REQUEST)).into_response()
        }))
        .unwrap();
    let res = app.handle(get("/t")).await;
    assert_eq!(res.status(), 410);
    let v = json(&res);
    assert_eq!(v["status"], 410);
    assert_eq!(v["title"], "Gone");
}

#[tokio::test]
async fn bodiless_500_from_a_handler_is_rendered_from_status() {
    let app = App::new()
        .route(route("/s", || {
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }))
        .unwrap();
    let res = app.handle(get("/s")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(content_type(&res), PROBLEM);
    assert_eq!(json(&res)["title"], "Internal Server Error");
}

#[tokio::test]
async fn handler_own_error_body_is_not_rewritten() {
    let app = App::new()
        .route(route("/own", || {
            (StatusCode::BAD_REQUEST, "plain").into_response()
        }))
        .unwrap();
    let res = app.handle(get("/own")).await;
    assert_eq!(res.status(), 400);
    assert!(content_type(&res).starts_with("text/plain"));
    assert_eq!(res.body(), "plain");
}

#[tokio::test]
async fn head_request_to_own_error_body_keeps_its_content_type() {
    let app = App::new()
        .route(route("/own", || {
            (StatusCode::BAD_REQUEST, "plain").into_response()
        }))
        .unwrap();
    let res = app.handle(Request::new(HttpMethod::Head, "/own")).await;
    assert_eq!(res.status(), 400);
    assert!(content_type(&res).starts_with("text/plain"));
    assert!(res.body().is_empty());
}

#[tokio::test]
async fn head_request_error_has_no_body() {
    let app = App::new()
        .route(route("/e", || {
            let mut res = HttpError::new(StatusCode::FORBIDDEN).into_response();
            res.headers_mut()
                .insert("x-keep", HeaderValue::from_static("1"));
            res
        }))
        .unwrap();
    let res = app.handle(Request::new(HttpMethod::Head, "/e")).await;
    assert_eq!(res.status(), 403);
    assert_eq!(content_type(&res), PROBLEM);
    assert_eq!(res.headers()["x-keep"], "1");
    assert!(res.body().is_empty());
    assert!(!res.headers().contains_key(header::CONTENT_LENGTH));
}

#[tokio::test]
async fn rendered_answer_keeps_other_headers_but_not_content_length() {
    let app = App::new()
        .route(route("/w", || {
            let mut res = HttpError::new(StatusCode::UNAUTHORIZED).into_response();
            res.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            res.headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
            res.headers_mut()
                .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
            res
        }))
        .unwrap();
    let res = app.handle(get("/w")).await;
    assert_eq!(res.status(), 401);
    assert_eq!(res.headers()[header::WWW_AUTHENTICATE], "Bearer");
    assert_eq!(content_type(&res), PROBLEM);
    assert!(!res.headers().contains_key(header::CONTENT_LENGTH));
}

#[tokio::test]
async fn success_response_is_passed_through() {
    let app = App::new()
        .route(route("/ok", || "fine".into_response()))
        .unwrap();
    let res = app.handle(get("/ok")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "fine");
}

fn assert_no_secret(res: &Response) {
    assert!(!String::from_utf8_lossy(res.body()).contains("hunter2"));
    for (_, v) in res.headers() {
        assert!(!String::from_utf8_lossy(v.as_bytes()).contains("hunter2"));
    }
}

#[tokio::test]
async fn panic_in_handler_is_500_and_hides_message_by_default() {
    let app = App::new().route(panicking_route()).unwrap();
    let res = app.handle(get("/boom")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(content_type(&res), PROBLEM);
    assert!(json(&res).get("detail").is_none());
    assert_no_secret(&res);
}

#[tokio::test]
async fn panic_in_handler_with_include_message_shows_it() {
    let app = App::new()
        .error_renderer(ProblemRenderer::new(true))
        .route(panicking_route())
        .unwrap();
    let res = app.handle(get("/boom")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(json(&res)["detail"], "secret is hunter2");
}

#[tokio::test]
async fn panic_with_a_non_string_payload_says_panic() {
    init_tracing();
    let app = App::new()
        .error_renderer(ProblemRenderer::new(true))
        .route(Route::new(HttpMethod::Get, "/n", |_| async {
            if true {
                std::panic::panic_any(42_u8);
            }
            "unreachable"
        }))
        .unwrap();
    let res = app.handle(get("/n")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(json(&res)["detail"], "panic");
}

struct PanicFilter;

fn panic_filter() -> PanicFilter {
    init_tracing();
    PanicFilter
}

#[async_trait::async_trait]
impl Filter for PanicFilter {
    async fn filter(&self, _req: Request, _next: Next<'_>) -> Response {
        panic!("filter secret hunter2");
    }
}

#[tokio::test]
async fn panic_in_a_filter_inside_error_handling_is_500() {
    let app = App::new().filter(panic_filter());
    let res = app.handle(get("/x")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(content_type(&res), PROBLEM);
    assert_no_secret(&res);
}

#[tokio::test]
async fn panic_in_a_filter_outside_error_handling_is_500_through_handle() {
    let app = App::new().filter_ordered(order::REQUEST_LOG, panic_filter());
    let res = app.handle(get("/x")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(content_type(&res), PROBLEM);
    assert_no_secret(&res);
}

async fn send(addr: std::net::SocketAddr, request: &[u8]) -> String {
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(request).await.unwrap();
    let mut reply = String::new();
    conn.read_to_string(&mut reply).await.unwrap();
    reply
}

#[tokio::test]
async fn panic_in_a_filter_outside_error_handling_is_still_500_over_serve() {
    let app = App::new().filter_ordered(order::REQUEST_LOG, panic_filter());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(app.serve(listener, async {
        let _ = stopped.await;
    }));

    let req = b"GET /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
    for _ in 0..2 {
        let reply = send(addr, req).await;
        assert!(reply.starts_with("HTTP/1.1 500"), "{reply}");
        assert!(reply.contains(PROBLEM), "{reply}");
        assert!(!reply.contains("hunter2"), "{reply}");
    }

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

struct Teapot;

impl ErrorRenderer for Teapot {
    fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response {
        let mut res = format!("custom {} {}", attrs.status.as_u16(), info.path).into_response();
        *res.status_mut() = attrs.status;
        res
    }
}

#[tokio::test]
async fn custom_error_renderer_replaces_the_default() {
    let app = App::new()
        .error_renderer(Teapot)
        .route(panicking_route())
        .unwrap();
    let res = app.handle(get("/nope")).await;
    assert_eq!(res.status(), 404);
    assert_eq!(res.body(), "custom 404 /nope");
    let res = app.handle(get("/boom")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(res.body(), "custom 500 /boom");
}

#[tokio::test]
async fn custom_error_renderer_is_used_by_the_outer_catch() {
    let app = App::new()
        .filter_ordered(order::REQUEST_LOG, panic_filter())
        .error_renderer(Teapot);
    let res = app.handle(get("/x")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(res.body(), "custom 500 /x");
}

#[tokio::test]
async fn error_renderer_keeps_filter_order() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    struct Outer(Arc<Mutex<Vec<u16>>>);
    #[async_trait::async_trait]
    impl Filter for Outer {
        async fn filter(&self, req: Request, next: Next<'_>) -> Response {
            let res = next.run(req).await;
            let rendered = res.body().starts_with(b"custom");
            self.0.lock().unwrap().push(u16::from(rendered));
            res
        }
    }
    let app = App::new()
        .filter_ordered(order::REQUEST_LOG, Outer(seen.clone()))
        .error_renderer(Teapot);
    app.handle(get("/x")).await;
    assert_eq!(*seen.lock().unwrap(), [1]);
}

thread_local! {
    static CAPTURE: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

/// Writes into the buffer of the current thread's [`capture`], or nowhere.
struct ThreadBuf;

impl io::Write for ThreadBuf {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        CAPTURE.with(|c| {
            if let Some(buf) = &*c.borrow() {
                buf.lock().unwrap().extend_from_slice(data);
            }
        });
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for ThreadBuf {
    type Writer = ThreadBuf;

    fn make_writer(&'a self) -> Self::Writer {
        ThreadBuf
    }
}

/// One global subscriber for this test binary, installed before any panic is handled. Thread-
/// scoped subscribers race with the callsite interest cache when tests run in parallel.
fn init_tracing() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadBuf)
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
    });
}

/// Runs `app.handle(req)` and returns the response with the log lines it produced.
async fn capture(app: &App, req: Request) -> (Response, String) {
    init_tracing();
    let buf = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.with(|c| *c.borrow_mut() = Some(buf.clone()));
    let res = app.handle(req).await;
    CAPTURE.with(|c| *c.borrow_mut() = None);
    let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    (res, out)
}

#[tokio::test]
async fn panic_is_logged_at_error_on_oxbloom_web() {
    let app = App::new().route(panicking_route()).unwrap();
    let (res, out) = capture(&app, get("/boom")).await;
    assert_eq!(res.status(), 500);
    let line = out
        .lines()
        .find(|l| l.contains("oxbloom::web"))
        .unwrap_or_else(|| panic!("no oxbloom::web line in {out}"));
    assert!(line.contains("ERROR"), "{line}");
    assert!(line.contains("secret is hunter2"), "{line}");
}

#[tokio::test]
async fn status_413_is_rendered_as_problem_json() {
    let app = App::new().max_body_bytes(8);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(app.serve(listener, async {
        let _ = stopped.await;
    }));
    let reply = send(
        addr,
        b"POST /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 9\r\n\r\n123456789",
    )
    .await;
    assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
    assert!(reply.contains(PROBLEM), "{reply}");
    assert!(reply.contains("\"status\":413"), "{reply}");
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

struct SeenStatus(Arc<Mutex<Vec<u16>>>);

#[async_trait::async_trait]
impl Filter for SeenStatus {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let res = next.run(req).await;
        // Only reached when `next.run` returned normally instead of unwinding.
        self.0.lock().unwrap().push(res.status().as_u16());
        res
    }
}

#[tokio::test]
async fn filter_outside_error_handling_sees_500_for_a_handler_panic() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = App::new()
        .filter_ordered(order::REQUEST_LOG, SeenStatus(seen.clone()))
        .route(panicking_route())
        .unwrap();
    let (res, out) = capture(&app, get("/boom")).await;
    assert_eq!(res.status(), 500);
    assert_eq!(*seen.lock().unwrap(), [500]);
    assert_eq!(out.matches("request panicked").count(), 1, "{out}");
}

fn error_with_own_body() -> Response {
    let mut res = HttpError::new(StatusCode::BAD_REQUEST).into_response();
    *res.body_mut() = "own body".into();
    res.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    res
}

#[tokio::test]
async fn http_error_response_with_a_body_is_not_rewritten() {
    let app = App::new()
        .route(route("/own", error_with_own_body))
        .unwrap();
    let res = app.handle(get("/own")).await;
    assert_eq!(res.status(), 400);
    assert_eq!(content_type(&res), "text/plain");
    assert_eq!(res.body(), "own body");
}

#[tokio::test]
async fn head_to_http_error_response_with_a_body_is_not_rewritten() {
    let app = App::new()
        .route(route("/own", error_with_own_body))
        .unwrap();
    let res = app.handle(Request::new(HttpMethod::Head, "/own")).await;
    assert_eq!(res.status(), 400);
    assert_eq!(content_type(&res), "text/plain");
    assert!(res.body().is_empty());
}

#[tokio::test]
async fn rendered_answer_drops_headers_describing_the_old_body() {
    let app = App::new()
        .route(route("/gz", || {
            let mut res = HttpError::new(StatusCode::BAD_REQUEST).into_response();
            for (name, value) in [
                (header::CONTENT_ENCODING, "gzip"),
                (header::CONTENT_RANGE, "bytes 0-1/2"),
                (header::TRANSFER_ENCODING, "chunked"),
            ] {
                res.headers_mut()
                    .insert(name, HeaderValue::from_static(value));
            }
            res
        }))
        .unwrap();
    let res = app.handle(get("/gz")).await;
    assert_eq!(content_type(&res), PROBLEM);
    for name in [
        header::CONTENT_ENCODING,
        header::CONTENT_RANGE,
        header::TRANSFER_ENCODING,
    ] {
        assert!(!res.headers().contains_key(&name), "{name}");
    }
}

/// A renderer that sets `content-length`, to show HEAD answers drop it.
struct WithLength;

impl ErrorRenderer for WithLength {
    fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response {
        let mut res = ProblemRenderer::new(false).render(attrs, info);
        let len = res.body().len().to_string();
        res.headers_mut()
            .insert(header::CONTENT_LENGTH, len.parse().unwrap());
        res
    }
}

#[tokio::test]
async fn head_request_to_a_panicking_handler_has_no_body_and_no_content_length() {
    for outer in [false, true] {
        let mut app = App::new().error_renderer(WithLength);
        app = if outer {
            app.filter_ordered(order::REQUEST_LOG, panic_filter())
        } else {
            app.route(panicking_route()).unwrap()
        };
        let res = app.handle(Request::new(HttpMethod::Head, "/boom")).await;
        assert_eq!(res.status(), 500, "outer={outer}");
        assert_eq!(content_type(&res), PROBLEM);
        assert!(res.body().is_empty(), "outer={outer}");
        assert!(
            !res.headers().contains_key(header::CONTENT_LENGTH),
            "outer={outer}"
        );
    }
}
