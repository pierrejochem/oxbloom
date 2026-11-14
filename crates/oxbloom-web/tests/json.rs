//! JSON bodies: the `Json` response type and `Request::json`.
#![cfg(feature = "json")]

use std::cell::RefCell;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use http::{HeaderValue, StatusCode, header};
use oxbloom_web::{
    App, ErrorAttributes, ErrorValue, Filter, HttpError, HttpMethod, IntoResponse, Json, JsonBody,
    Next, Request, RequestLogFilter, Response, Route, order,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing_subscriber::fmt::MakeWriter;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Point {
    x: i32,
    y: i32,
}

fn post(content_type: Option<&str>, body: &str) -> Request {
    let mut req = Request::new(HttpMethod::Post, "/p");
    if let Some(ct) = content_type {
        req.headers
            .insert(header::CONTENT_TYPE, HeaderValue::from_str(ct).unwrap());
    }
    req.body = body.to_owned().into();
    req
}

fn attrs(err: HttpError) -> ErrorAttributes {
    err.into_response()
        .extensions()
        .get::<ErrorAttributes>()
        .unwrap()
        .clone()
}

#[test]
fn json_response_has_status_headers_and_body() {
    let res = Json(Point { x: 1, y: 2 }).into_response();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(res.body(), r#"{"x":1,"y":2}"#);
    assert!(res.extensions().get::<JsonBody>().is_some());
}

#[test]
fn status_tuple_keeps_the_json_body() {
    let res = (StatusCode::CREATED, Json(Point { x: 1, y: 2 })).into_response();
    assert_eq!(res.status(), 201);
    assert_eq!(res.body(), r#"{"x":1,"y":2}"#);
    assert!(res.extensions().get::<JsonBody>().is_some());
}

struct Failing;

impl Serialize for Failing {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("secret-text"))
    }
}

#[test]
fn serialization_failure_is_500_without_the_error_text() {
    init_tracing();
    let buf = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.with(|c| *c.borrow_mut() = Some(buf.clone()));
    let res = Json(Failing).into_response();
    CAPTURE.with(|c| *c.borrow_mut() = None);
    let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();

    assert_eq!(res.status(), 500);
    assert!(res.body().is_empty());
    assert!(res.extensions().get::<JsonBody>().is_none());
    let attrs = res.extensions().get::<ErrorAttributes>().unwrap();
    assert!(attrs.detail.is_none());
    assert!(!format!("{attrs:?}").contains("secret-text"));
    let line = out
        .lines()
        .find(|l| l.contains("oxbloom::web"))
        .unwrap_or_else(|| panic!("no oxbloom::web line in {out}"));
    assert!(line.contains("ERROR"), "{line}");
    assert!(line.contains("secret-text"), "{line}");
}

#[test]
fn request_json_parses_a_valid_body() {
    let p: Point = post(Some("application/json"), r#"{"x":3,"y":4}"#)
        .json()
        .unwrap();
    assert_eq!(p, Point { x: 3, y: 4 });
}

#[test]
fn request_json_content_type_table() {
    let accepted = [
        "application/json",
        "application/json; charset=utf-8",
        "application/json;charset=\"utf-8\"",
        "APPLICATION/JSON",
        "application/vnd.api+json",
        "application/json;",
    ];
    for ct in accepted {
        let r: Result<Point, _> = post(Some(ct), r#"{"x":1,"y":2}"#).json();
        assert!(r.is_ok(), "{ct} should parse");
    }
    for ct in [
        Some("text/plain"),
        Some("application/jsonx"),
        Some("application/json-seq"),
        Some("application/+json"),
        None,
    ] {
        let r: Result<Point, _> = post(ct, r#"{"x":1,"y":2}"#).json();
        assert_eq!(attrs(r.unwrap_err()).status, 415, "{ct:?} should be 415");
    }
}

#[test]
fn request_json_415_names_the_accepted_type() {
    let err = post(Some("text/plain"), "{}").json::<Point>().unwrap_err();
    let a = attrs(err);
    assert_eq!(a.status, 415);
    assert_eq!(
        a.extensions,
        vec![(
            "accepted".into(),
            ErrorValue::Str("application/json".into())
        )]
    );
}

#[test]
fn request_json_400_cases() {
    let cases = [
        ("", "eof"),
        (r#"{"x":"#, "eof"),
        ("not json", "syntax"),
        (r#"{"x":"hunter2","y":2}"#, "data"),
    ];
    for (body, class) in cases {
        let a = attrs(
            post(Some("application/json"), body)
                .json::<Point>()
                .unwrap_err(),
        );
        assert_eq!(a.status, 400, "{body}");
        let detail = a.detail.unwrap();
        assert!(detail.contains(class), "{detail} should name {class}");
        assert!(detail.contains("line 1"), "{detail}");
        for leak in ["not json", "\"hunter2\"", "hunter2"] {
            assert!(!detail.contains(leak), "{detail} leaks {leak}");
        }
    }
}

#[tokio::test]
async fn request_json_works_with_the_question_mark_operator() {
    let app = App::new()
        .route(Route::new(HttpMethod::Post, "/p", |req| async move {
            let p: Point = req.json()?;
            Ok::<_, HttpError>(Json(p))
        }))
        .unwrap();
    let ok = app
        .handle(post(Some("application/json"), r#"{"x":1,"y":2}"#))
        .await;
    assert_eq!(ok.status(), 200);
    assert_eq!(ok.body(), r#"{"x":1,"y":2}"#);
    let bad = app.handle(post(Some("application/json"), "nope")).await;
    assert_eq!(bad.status(), 400);
    assert_eq!(
        bad.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[test]
fn negotiation_constant_sits_between_error_handling_and_body_limit() {
    const {
        assert!(
            order::ERROR_HANDLING < order::NEGOTIATION && order::NEGOTIATION < order::BODY_LIMIT
        )
    };
}

fn json_app() -> App {
    App::new()
        .route(Route::new(HttpMethod::Get, "/j", |_| async {
            Json(Point { x: 1, y: 2 })
        }))
        .unwrap()
}

fn request_with_accept(method: HttpMethod, path: &str, accept: &str) -> Request {
    let mut req = Request::new(method, path);
    req.headers
        .insert(header::ACCEPT, HeaderValue::from_str(accept).unwrap());
    req
}

#[tokio::test]
async fn unacceptable_accept_gives_406_problem_json() {
    let res = json_app()
        .handle(request_with_accept(HttpMethod::Get, "/j", "text/html"))
        .await;
    assert_eq!(res.status(), 406);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let body: serde_json::Value = serde_json::from_slice(res.body()).unwrap();
    assert_eq!(body["status"], 406);
    assert_eq!(body["supported"], "application/json");
}

#[tokio::test]
async fn acceptable_accept_passes() {
    for accept in [
        "application/json",
        "*/*",
        "text/html, application/json;q=0.5",
    ] {
        let res = json_app()
            .handle(request_with_accept(HttpMethod::Get, "/j", accept))
            .await;
        assert_eq!(res.status(), 200, "{accept}");
        assert_eq!(res.body(), r#"{"x":1,"y":2}"#);
    }
    let res = json_app().handle(Request::new(HttpMethod::Get, "/j")).await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn head_request_with_unacceptable_accept_is_406() {
    let res = json_app()
        .handle(request_with_accept(HttpMethod::Head, "/j", "text/html"))
        .await;
    assert_eq!(res.status(), 406);
    assert!(res.body().is_empty());
}

#[tokio::test]
async fn error_answers_are_not_turned_into_406() {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/e", |_| async {
            HttpError::new(StatusCode::NOT_FOUND)
        }))
        .unwrap();
    let res = app
        .handle(request_with_accept(HttpMethod::Get, "/e", "text/html"))
        .await;
    assert_eq!(res.status(), 404);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[tokio::test]
async fn non_json_responses_are_never_406() {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/h", |_| async { "hello" }))
        .unwrap();
    let res = app
        .handle(request_with_accept(HttpMethod::Get, "/h", "image/png"))
        .await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "hello");
}

struct Reject;

#[async_trait::async_trait]
impl Filter for Reject {
    async fn filter(&self, _req: Request, _next: Next<'_>) -> Response {
        HttpError::new(StatusCode::UNAUTHORIZED).into_response()
    }

    fn order(&self) -> i32 {
        order::SECURITY
    }
}

#[tokio::test]
async fn security_style_rejection_is_not_406() {
    let app = json_app().filter(Reject);
    let res = app
        .handle(request_with_accept(HttpMethod::Get, "/j", "text/html"))
        .await;
    assert_eq!(res.status(), 401);
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[tokio::test]
async fn oversized_body_is_413_before_json_runs() {
    let app = App::new()
        .max_body_bytes(10)
        .route(Route::new(HttpMethod::Post, "/p", |req| async move {
            let p: Point = req.json()?;
            Ok::<_, HttpError>(Json(p))
        }))
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(app.serve(listener, async {
        let _ = stopped.await;
    }));
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        b"POST /p HTTP/1.1\r\nHost: t\r\nConnection: close\r\nAccept: text/html\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"x\":1,\"y\"}",
    )
    .await
    .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out))
        .await
        .unwrap()
        .unwrap();
    let reply = String::from_utf8_lossy(&out);
    assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn request_log_sees_the_406() {
    init_tracing();
    let app = json_app().filter(RequestLogFilter::new(tracing::Level::INFO, Vec::new()));
    let buf = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.with(|c| *c.borrow_mut() = Some(buf.clone()));
    let res = app
        .handle(request_with_accept(HttpMethod::Get, "/j", "text/html"))
        .await;
    CAPTURE.with(|c| *c.borrow_mut() = None);
    let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    assert_eq!(res.status(), 406);
    assert!(out.contains("status=406"), "{out}");
}

struct Record(Arc<Mutex<Vec<(&'static str, u16)>>>, &'static str, i32);

#[async_trait::async_trait]
impl Filter for Record {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let res = next.run(req).await;
        self.0.lock().unwrap().push((self.1, res.status().as_u16()));
        res
    }

    fn order(&self) -> i32 {
        self.2
    }
}

#[tokio::test]
async fn filter_ordered_can_run_before_and_after_negotiation() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let app = json_app()
        .filter(Record(seen.clone(), "before", order::NEGOTIATION - 1))
        .filter(Record(seen.clone(), "after", order::NEGOTIATION + 1));
    let res = app
        .handle(request_with_accept(HttpMethod::Get, "/j", "text/html"))
        .await;
    assert_eq!(res.status(), 406);
    assert_eq!(*seen.lock().unwrap(), [("after", 200), ("before", 406)]);
}

thread_local! {
    static CAPTURE: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

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

/// One global subscriber for this test binary; thread-scoped ones race with the callsite cache.
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
