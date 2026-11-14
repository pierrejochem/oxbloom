//! CORS: the order table, simple requests, preflights, origin rules, validation and the place of
//! the filter in the chain.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http::{HeaderMap, HeaderValue, StatusCode, header};
use oxbloom_web::{
    App, CorsConfig, CorsError, CorsFilter, Filter, HttpMethod, Next, Request, Response, Route,
    order,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const A: &str = "https://a.example";
const B: &str = "https://b.example";

const CORS_HEADERS: [&str; 6] = [
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "access-control-allow-credentials",
    "access-control-expose-headers",
    "access-control-max-age",
];

fn hello() -> Route {
    Route::new(HttpMethod::Get, "/hello", |_| async { "hello" })
}

fn app(config: CorsConfig) -> App {
    App::new().cors(config).unwrap().route(hello()).unwrap()
}

fn only_a() -> CorsConfig {
    CorsConfig::new().allowed_origins([A])
}

fn request(method: HttpMethod, path: &str, headers: &[(&str, &str)]) -> Request {
    let mut req = Request::new(method, path);
    for (name, value) in headers {
        req.headers.append(
            header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    req
}

fn get_from(origin: &str) -> Request {
    request(HttpMethod::Get, "/hello", &[("origin", origin)])
}

fn preflight(origin: &str, method: &str, headers: Option<&str>) -> Request {
    let mut list = vec![
        ("origin", origin),
        ("access-control-request-method", method),
    ];
    if let Some(h) = headers {
        list.push(("access-control-request-headers", h));
    }
    request(HttpMethod::Options, "/hello", &list)
}

fn value<'a>(res: &'a Response, name: &str) -> Option<&'a str> {
    res.headers().get(name).map(|v| v.to_str().unwrap())
}

fn vary(res: &Response) -> Vec<String> {
    res.headers()
        .get_all(header::VARY)
        .iter()
        .flat_map(|v| v.to_str().unwrap().split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect()
}

fn assert_no_cors_headers(headers: &HeaderMap) {
    for name in CORS_HEADERS {
        assert!(!headers.contains_key(name), "unexpected {name}");
    }
}

#[test]
fn order_table_is_complete() {
    let table = [
        order::REQUEST_LOG,
        order::CORS,
        order::ERROR_HANDLING,
        order::NEGOTIATION,
        order::BODY_LIMIT,
        order::COMPRESSION,
        order::SECURITY,
        order::DEFAULT,
    ];
    assert_eq!(table, [-400, -350, -300, -250, -200, -150, -100, 0]);
    assert!(table.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn the_filter_declares_the_cors_slot() {
    let filter = CorsFilter::new(only_a()).unwrap();
    assert_eq!(filter.order(), order::CORS);
}

#[tokio::test]
async fn request_without_origin_passes_unchanged_apart_from_vary_origin() {
    let plain = App::new().route(hello()).unwrap();
    let with_cors = app(only_a());
    let req = || Request::new(HttpMethod::Get, "/hello");
    let mut expected = plain.handle(req()).await;
    expected
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("Origin"));
    let res = with_cors.handle(req()).await;
    assert_eq!(res.status(), expected.status());
    assert_eq!(res.headers(), expected.headers());
    assert_eq!(res.body(), expected.body());
    assert_eq!(vary(&res), ["origin"]);
}

#[tokio::test]
async fn star_origins_leave_a_request_without_origin_unchanged() {
    let plain = App::new().route(hello()).unwrap();
    let star = app(CorsConfig::new().allowed_origins(["*"]));
    let req = || Request::new(HttpMethod::Get, "/hello");
    let expected = plain.handle(req()).await;
    let res = star.handle(req()).await;
    assert_eq!(res.headers(), expected.headers());
    assert!(vary(&res).is_empty());
}

#[tokio::test]
async fn vary_origin_is_not_duplicated_on_a_passthrough() {
    for existing in ["Origin", "origin, Accept-Encoding", "*"] {
        let route = Route::new(HttpMethod::Get, "/v", move |_| async move {
            let mut res = Response::new("v".into());
            res.headers_mut()
                .insert(header::VARY, HeaderValue::from_static(existing));
            res
        });
        let app = App::new().cors(only_a()).unwrap().route(route).unwrap();
        for req in [
            request(HttpMethod::Get, "/v", &[]),
            request(HttpMethod::Get, "/v", &[("origin", B)]),
        ] {
            let res = app.handle(req).await;
            assert_eq!(res.headers().get_all(header::VARY).iter().count(), 1);
            assert_eq!(res.headers()[header::VARY], existing);
        }
    }
}

#[tokio::test]
async fn disallowed_origin_gets_no_cors_headers_and_is_not_blocked() {
    let res = app(only_a()).handle(get_from(B)).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "hello");
    assert_no_cors_headers(res.headers());
    assert_eq!(vary(&res), ["origin"]);
}

#[tokio::test]
async fn simple_request_from_allowed_origin_gets_allow_origin_and_vary() {
    let res = app(only_a()).handle(get_from(A)).await;
    assert_eq!(res.status(), 200);
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
    assert_eq!(vary(&res), ["origin"]);
    assert_eq!(value(&res, "access-control-allow-credentials"), None);
    assert_eq!(value(&res, "access-control-expose-headers"), None);
    // Preflight-only headers are not sent on an actual request.
    assert_eq!(value(&res, "access-control-allow-methods"), None);
    assert_eq!(value(&res, "access-control-max-age"), None);

    let config = only_a()
        .exposed_headers(["X-Total", "ETag"])
        .allow_credentials(true);
    let res = app(config).handle(get_from(A)).await;
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
    assert_eq!(
        value(&res, "access-control-expose-headers"),
        Some("x-total, etag")
    );
    assert_eq!(
        value(&res, "access-control-allow-credentials"),
        Some("true")
    );
}

#[tokio::test]
async fn vary_is_appended_to_an_existing_value() {
    let route = Route::new(HttpMethod::Get, "/v", |_| async {
        let mut res = Response::new("v".into());
        res.headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
        res
    });
    let app = App::new().cors(only_a()).unwrap().route(route).unwrap();
    let res = app
        .handle(request(HttpMethod::Get, "/v", &[("origin", A)]))
        .await;
    assert_eq!(vary(&res), ["accept-encoding", "origin"]);
}

#[tokio::test]
async fn allow_origin_echoes_the_origin_and_is_star_only_for_star_without_credentials() {
    let listed = app(CorsConfig::new().allowed_origins([A, B]));
    let res = listed.handle(get_from(B)).await;
    assert_eq!(value(&res, "access-control-allow-origin"), Some(B));

    let star = app(CorsConfig::new().allowed_origins(["*"]));
    let res = star.handle(get_from(B)).await;
    assert_eq!(value(&res, "access-control-allow-origin"), Some("*"));
    assert_eq!(value(&res, "access-control-allow-credentials"), None);

    let credentials = app(only_a().allow_credentials(true));
    let res = credentials.handle(get_from(A)).await;
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
    assert_eq!(
        value(&res, "access-control-allow-credentials"),
        Some("true")
    );
}

#[tokio::test]
async fn allowed_preflight_is_answered_204_with_all_preflight_headers() {
    let config = only_a()
        .allowed_methods(["GET", "PUT"])
        .allowed_headers(["Content-Type", "X-Token"])
        .max_age(Duration::from_secs(600));
    let res = app(config)
        .handle(preflight(A, "PUT", Some("x-token, Content-Type")))
        .await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(res.body().is_empty());
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
    assert_eq!(
        value(&res, "access-control-allow-methods"),
        Some("GET, PUT")
    );
    assert_eq!(
        value(&res, "access-control-allow-headers"),
        Some("content-type, x-token")
    );
    assert_eq!(value(&res, "access-control-max-age"), Some("600"));
    assert_eq!(value(&res, "access-control-allow-credentials"), None);
    assert_eq!(
        vary(&res),
        [
            "origin",
            "access-control-request-method",
            "access-control-request-headers"
        ]
    );
}

#[tokio::test]
async fn default_preflight_allows_get_head_and_post_with_a_max_age_of_thirty_minutes() {
    let res = app(only_a().allow_credentials(true))
        .handle(preflight(A, "POST", None))
        .await;
    assert_eq!(res.status(), 204);
    assert_eq!(
        value(&res, "access-control-allow-methods"),
        Some("GET, HEAD, POST")
    );
    assert_eq!(value(&res, "access-control-max-age"), Some("1800"));
    assert_eq!(
        value(&res, "access-control-allow-credentials"),
        Some("true")
    );
}

#[tokio::test]
async fn preflight_with_a_wrong_method_passes_through_without_cors_headers() {
    let res = app(only_a()).handle(preflight(A, "DELETE", None)).await;
    // The router answers the passed-through `OPTIONS` itself, with `Allow`.
    assert_eq!(res.status(), 204);
    assert!(res.headers().contains_key(header::ALLOW));
    assert_no_cors_headers(res.headers());
    assert_eq!(vary(&res), ["origin"]);
    // A method is a case-sensitive token.
    let res = app(only_a()).handle(preflight(A, "post", None)).await;
    assert_no_cors_headers(res.headers());
}

#[tokio::test]
async fn preflight_with_a_wrong_request_header_passes_through_without_cors_headers() {
    let config = only_a().allowed_headers(["Content-Type"]);
    let res = app(config)
        .handle(preflight(A, "GET", Some("content-type, x-evil")))
        .await;
    assert_no_cors_headers(res.headers());
    let res = app(only_a())
        .handle(preflight(A, "GET", Some("x-token")))
        .await;
    assert_no_cors_headers(res.headers());
}

#[tokio::test]
async fn preflight_from_a_disallowed_origin_passes_through_without_cors_headers() {
    let res = app(only_a()).handle(preflight(B, "GET", None)).await;
    assert_no_cors_headers(res.headers());
    assert_eq!(vary(&res), ["origin"]);
}

#[tokio::test]
async fn options_without_a_request_method_is_not_a_preflight() {
    let res = app(only_a())
        .handle(request(HttpMethod::Options, "/hello", &[("origin", A)]))
        .await;
    // The router answers it, and the actual-request CORS headers are added.
    assert_eq!(res.status(), 204);
    assert!(res.headers().contains_key(header::ALLOW));
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
    assert_eq!(value(&res, "access-control-allow-methods"), None);
    assert_eq!(value(&res, "access-control-max-age"), None);
}

#[tokio::test]
async fn allowed_headers_star_echoes_the_requested_headers() {
    let config = only_a().allowed_headers(["*"]);
    let res = app(config)
        .handle(preflight(A, "GET", Some("X-One,  x-two")))
        .await;
    assert_eq!(res.status(), 204);
    assert_eq!(
        value(&res, "access-control-allow-headers"),
        Some("x-one, x-two")
    );
}

#[tokio::test]
async fn allowed_methods_star_echoes_the_requested_method() {
    let config = only_a().allowed_methods(["*"]);
    let res = app(config).handle(preflight(A, "PATCH", None)).await;
    assert_eq!(res.status(), 204);
    assert_eq!(value(&res, "access-control-allow-methods"), Some("PATCH"));
}

#[tokio::test]
async fn origin_null_is_allowed_only_when_listed_literally() {
    let res = app(only_a()).handle(get_from("null")).await;
    assert_no_cors_headers(res.headers());

    let res = app(CorsConfig::new().allowed_origins(["null"]))
        .handle(get_from("null"))
        .await;
    assert_eq!(value(&res, "access-control-allow-origin"), Some("null"));

    let res = app(CorsConfig::new().allowed_origins(["null"]))
        .handle(get_from("NULL"))
        .await;
    assert_no_cors_headers(res.headers());
}

#[tokio::test]
async fn origin_comparison_ignores_host_case_but_rejects_trailing_slash_and_path() {
    for bad in ["https://a.example/", "https://a.example/x", "a.example"] {
        let err = CorsFilter::new(CorsConfig::new().allowed_origins([bad]))
            .err()
            .unwrap_or_else(|| panic!("{bad} must be rejected"));
        assert!(err.to_string().contains("allowed-origins"), "{err}");
    }

    let app = app(CorsConfig::new().allowed_origins(["https://A.Example:8443"]));
    let res = app.handle(get_from("https://a.example:8443")).await;
    assert_eq!(
        value(&res, "access-control-allow-origin"),
        Some("https://a.example:8443")
    );
    let res = app.handle(get_from("https://A.EXAMPLE:8443")).await;
    assert_eq!(
        value(&res, "access-control-allow-origin"),
        Some("https://A.EXAMPLE:8443")
    );
    for bad in [
        "https://a.example:8443/",
        "https://a.example:8443/x",
        "a.example:8443",
        "http://a.example:8443",
        "https://a.example",
        "https://a.example:8443.evil",
        "https://evil.a.example:8443",
        "https://a.example:8443@evil",
    ] {
        let res = app.handle(get_from(bad)).await;
        assert_no_cors_headers(res.headers());
    }
}

#[tokio::test]
async fn two_origin_headers_get_no_cors_headers() {
    let req = request(HttpMethod::Get, "/hello", &[("origin", A), ("origin", A)]);
    let res = app(only_a()).handle(req).await;
    assert_no_cors_headers(res.headers());
}

fn error_text(config: CorsConfig) -> String {
    let err: CorsError = CorsFilter::new(config).expect_err("config must fail");
    let text = err.to_string();
    assert!(!text.contains("a.example"), "origin value leaked: {text}");
    assert!(!text.contains("b.example"), "origin value leaked: {text}");
    text
}

#[test]
fn star_mixed_with_other_origins_is_invalid() {
    let text = error_text(CorsConfig::new().allowed_origins(["*", A]));
    assert!(text.contains("allowed-origins"), "{text}");
}

#[test]
fn star_origins_with_credentials_is_invalid() {
    let text = error_text(
        CorsConfig::new()
            .allowed_origins(["*"])
            .allow_credentials(true),
    );
    assert!(text.contains("allow-credentials"), "{text}");
}

#[test]
fn star_in_headers_or_methods_with_credentials_is_invalid() {
    let text = error_text(only_a().allowed_headers(["*"]).allow_credentials(true));
    assert!(text.contains("allow-credentials"), "{text}");
    assert!(text.contains("allowed-headers"), "{text}");
    let text = error_text(only_a().allowed_methods(["*"]).allow_credentials(true));
    assert!(text.contains("allow-credentials"), "{text}");
    assert!(text.contains("allowed-methods"), "{text}");
}

#[test]
fn error_text_names_the_setting_and_never_the_origin() {
    let text = error_text(CorsConfig::new().allowed_origins([A, "https://b.example/path"]));
    assert!(text.contains("allowed-origins"), "{text}");
    let text = error_text(CorsConfig::new().allowed_origins(Vec::<String>::new()));
    assert!(text.contains("allowed-origins"), "{text}");
    let text = error_text(only_a().allowed_headers(["bad header"]));
    assert!(text.contains("allowed-headers"), "{text}");
    let text = error_text(only_a().exposed_headers(["bad header"]));
    assert!(text.contains("exposed-headers"), "{text}");
    let text = error_text(only_a().allowed_methods(["GE T"]));
    assert!(text.contains("allowed-methods"), "{text}");
}

#[test]
fn app_cors_reports_an_invalid_configuration() {
    let err = App::new()
        .cors(CorsConfig::new().allowed_origins(["*", A]))
        .err()
        .unwrap();
    assert!(err.to_string().contains("allowed-origins"));
}

/// A security-slot filter that counts its calls and answers 401.
struct Deny(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Filter for Deny {
    async fn filter(&self, _req: Request, _next: Next<'_>) -> Response {
        self.0.fetch_add(1, Ordering::SeqCst);
        let mut res = Response::new(Default::default());
        *res.status_mut() = StatusCode::UNAUTHORIZED;
        res
    }
}

#[tokio::test]
async fn cors_headers_are_on_a_404() {
    let res = app(only_a())
        .handle(request(HttpMethod::Get, "/missing", &[("origin", A)]))
        .await;
    assert_eq!(res.status(), 404);
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
}

#[tokio::test]
async fn cors_headers_are_on_a_401_from_a_security_filter_and_the_preflight_never_reaches_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let app = App::new()
        .filter_ordered(order::SECURITY, Deny(calls.clone()))
        .cors(only_a().allowed_methods(["PUT"]))
        .unwrap()
        .route(hello())
        .unwrap();

    let res = app.handle(get_from(A)).await;
    assert_eq!(res.status(), 401);
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let res = app.handle(preflight(A, "PUT", None)).await;
    assert_eq!(res.status(), 204);
    assert_eq!(value(&res, "access-control-allow-origin"), Some(A));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cors_headers_are_on_a_413() {
    let app = App::new().max_body_bytes(8).cors(only_a()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(app.serve(listener, async {
        let _ = stopped.await;
    }));
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(
        b"POST /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\nOrigin: https://a.example\r\n\
          Content-Length: 9\r\n\r\n123456789",
    )
    .await
    .unwrap();
    let mut reply = String::new();
    conn.read_to_string(&mut reply).await.unwrap();
    assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
    assert!(
        reply
            .to_ascii_lowercase()
            .contains("access-control-allow-origin: https://a.example\r\n"),
        "{reply}"
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
