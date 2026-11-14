//! `TestClient` sends one request style to an `App` in memory, through a server it owns, or to
//! a server that runs elsewhere, and answers every mode with a `TestResponse`.

#![cfg(feature = "web")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use http::{HeaderValue, StatusCode};
use oxbloom_test::TestClient;
use oxbloom_web::{
    App, ForwardStrategy, ForwardedConfig, HttpMethod, Request, Response, Route, TrustedProxies,
};
use tokio::net::TcpListener;

/// The text of a panic payload, for a `&str` or a `String` payload.
fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(text) => *text,
        Err(payload) => payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .unwrap_or_default(),
    }
}

/// An address on which nothing listens: a socket bound to it but never listening, so a connect
/// is refused. The caller keeps the socket alive for the whole test, so no other test can take
/// the port in the meantime.
fn closed_addr() -> (tokio::net::TcpSocket, SocketAddr) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = socket.local_addr().unwrap();
    (socket, addr)
}

/// The application most tests share: a text route, a JSON route, an echo route and a route
/// that names the client address.
fn app() -> App {
    App::new()
        .max_body_bytes(1024 * 1024)
        .route(Route::new(HttpMethod::Get, "/hello", |_| async { "hi" }))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/data", |_| async {
            http::Response::builder()
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(bytes::Bytes::from_static(br#"{"n":1}"#))
                .unwrap()
        }))
        .unwrap()
        .route(Route::new(
            HttpMethod::Post,
            "/echo",
            |r: Request| async move { String::from_utf8_lossy(&r.body).into_owned() },
        ))
        .unwrap()
        .route(Route::new(
            HttpMethod::Get,
            "/client",
            |r: Request| async move {
                r.client_addr()
                    .map(|a| a.ip().to_string())
                    .unwrap_or_default()
            },
        ))
        .unwrap()
        .route(Route::new(
            HttpMethod::Get,
            "/query",
            |r: Request| async move { r.query.clone().unwrap_or_default() },
        ))
        .unwrap()
}

/// The parts of an answer that both modes must agree on.
async fn answer(client: &TestClient, method: &str, path: &str) -> (StatusCode, String, String) {
    let builder = match method {
        "GET" => client.get(path),
        "POST" => client.post(path).body("ping"),
        "DELETE" => client.delete(path),
        other => panic!("unexpected method {other}"),
    };
    let res = builder.send().await;
    (
        res.status(),
        res.header("content-type").unwrap_or_default().to_owned(),
        res.text(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn in_memory_and_served_answer_the_same() {
    let memory = TestClient::in_memory(app());
    let served = TestClient::serve(app()).await;
    let table = [
        ("GET", "/hello", StatusCode::OK),
        ("GET", "/data", StatusCode::OK),
        ("POST", "/echo", StatusCode::OK),
        ("GET", "/query?a=1&b=2", StatusCode::OK),
        ("GET", "/missing", StatusCode::NOT_FOUND),
        ("DELETE", "/hello", StatusCode::METHOD_NOT_ALLOWED),
    ];
    for (method, path, status) in table {
        let a = answer(&memory, method, path).await;
        let b = answer(&served, method, path).await;
        assert_eq!(a.0, status, "{method} {path}");
        assert_eq!(a, b, "{method} {path}");
    }
    let data = memory.get("/data").send().await;
    assert_eq!(data.text(), r#"{"n":1}"#);
    assert_eq!(memory.get("/query?a=1&b=2").send().await.text(), "a=1&b=2");
    served.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_over_the_limit_is_413_in_both_modes() {
    let small = || app().max_body_bytes(4);
    let memory = TestClient::in_memory(small());
    let served = TestClient::serve(small()).await;
    for client in [&memory, &served] {
        let res = client.post("/echo").body("far too long").send().await;
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let res = client.post("/echo").body("ok").send().await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.text(), "ok");
    }
    served.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_forwarded_header_is_ignored_in_both_modes() {
    let forwarding = || {
        app().forwarded(ForwardedConfig {
            strategy: ForwardStrategy::Framework,
            trusted: TrustedProxies::parse(&["10.0.0.0/8".into()]).unwrap(),
        })
    };
    let memory = TestClient::in_memory(forwarding());
    let served = TestClient::serve(forwarding()).await;
    for client in [&memory, &served] {
        let res = client
            .get("/client")
            .header("x-forwarded-for", "203.0.113.9")
            .send()
            .await;
        assert_eq!(res.text(), "127.0.0.1");
    }
    let trusted = TestClient::in_memory(forwarding()).peer("10.1.2.3:40000".parse().unwrap());
    let res = trusted
        .get("/client")
        .header("x-forwarded-for", "203.0.113.9")
        .send()
        .await;
    assert_eq!(res.text(), "203.0.113.9");
    served.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn head_204_and_304_have_no_body_and_do_not_hang() {
    let app = || {
        app()
            .route(Route::new(HttpMethod::Get, "/empty", |_| async {
                StatusCode::NO_CONTENT
            }))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/same", |_| async {
                StatusCode::NOT_MODIFIED
            }))
            .unwrap()
    };
    let memory = TestClient::in_memory(app());
    let served = TestClient::serve(app()).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        for client in [&memory, &served] {
            let res = client.head("/hello").send().await;
            assert_eq!(res.status(), StatusCode::OK);
            assert!(res.bytes().is_empty());
            let res = client.get("/empty").send().await;
            assert_eq!(res.status(), StatusCode::NO_CONTENT);
            assert!(res.bytes().is_empty());
            let res = client.get("/same").send().await;
            assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
            assert!(res.bytes().is_empty());
            let res = client.get("/hello").send().await;
            assert_eq!(res.text(), "hi");
        }
    })
    .await
    .expect("the bodiless answers did not hang");
    served.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_above_64_kib_arrives_whole() {
    let big = "x".repeat(200 * 1024);
    let memory = TestClient::in_memory(app());
    let served = TestClient::serve(app()).await;
    for client in [&memory, &served] {
        let res = client.post("/echo").body(big.clone()).send().await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.bytes().len(), big.len());
        assert_eq!(res.text(), big);
    }
    served.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_header_that_is_not_utf8_has_no_text_form() {
    let app = || {
        app()
            .route(Route::new(HttpMethod::Get, "/odd", |_| async {
                let mut res = Response::new(bytes::Bytes::new());
                res.headers_mut()
                    .insert("x-odd", HeaderValue::from_bytes(b"\xff").unwrap());
                res
            }))
            .unwrap()
    };
    let memory = TestClient::in_memory(app());
    let served = TestClient::serve(app()).await;
    for client in [&memory, &served] {
        let res = client.get("/odd").send().await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.header("x-odd"), None);
        assert_eq!(res.header("x-not-there"), None);
        assert_eq!(res.into_inner().headers()["x-odd"].as_bytes(), b"\xff");
    }
    served.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_alive_reuses_one_connection() {
    // The app listens behind a counting relay, so the test sees every connection the client opens.
    let inner = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let inner_addr = inner.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(app().serve(inner, async {
        let _ = stopped.await;
    }));
    let outer = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let outer_addr = outer.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    let relay = tokio::spawn(async move {
        loop {
            let (mut down, _) = outer.accept().await.unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut up = tokio::net::TcpStream::connect(inner_addr).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut down, &mut up).await;
            });
        }
    });

    let client = TestClient::connect(outer_addr);
    for _ in 0..3 {
        assert_eq!(client.get("/hello").send().await.text(), "hi");
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1, "accepted connections");

    drop(client);
    relay.abort();
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_served_client_frees_its_port() {
    let client = TestClient::serve(app()).await;
    assert_eq!(client.get("/hello").send().await.text(), "hi");
    let addr = client.addr().expect("a served client has an address");
    drop(client);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match TcpListener::bind(addr).await {
            Ok(_) => break,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("the port was not freed within 2 seconds: {e}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_stops_the_server() {
    let client = TestClient::serve(app()).await;
    let addr = client.addr().unwrap();
    assert_eq!(client.get("/hello").send().await.text(), "hi");
    client.shutdown().await;
    let later = tokio::spawn(async move { TestClient::connect(addr).get("/hello").send().await });
    let err = later.await.expect_err("the server still answered");
    assert!(err.is_panic());
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "GET /missing")]
async fn connect_to_a_closed_port_panics_with_method_and_path() {
    let (_reserved, addr) = closed_addr();
    TestClient::connect(addr)
        .get("/missing?secret=1")
        .send()
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_port_panic_names_no_query_string() {
    let (_reserved, addr) = closed_addr();
    let task = tokio::spawn(async move {
        TestClient::connect(addr)
            .get("/missing?secret=1")
            .send()
            .await
    });
    let text = panic_text(task.await.unwrap_err().into_panic());
    assert!(text.starts_with("GET /missing failed: "), "{text}");
    assert!(!text.contains("secret"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "10 seconds")]
async fn a_request_that_never_answers_panics_after_the_bound() {
    // A listener that accepts connections and never answers them.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _silent = tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            held.push(stream);
        }
    });
    let sent = tokio::time::timeout(
        Duration::from_secs(12),
        TestClient::connect(addr).get("/slow").send(),
    )
    .await;
    // Reaching this line means `send` did not panic, so the expected panic text is absent.
    match sent {
        Err(_) => panic!("the client's own bound did not fire within the 12 s outer bound"),
        Ok(_) => panic!("the silent listener answered"),
    }
}

#[cfg(feature = "json")]
#[tokio::test(flavor = "multi_thread")]
async fn the_json_helpers_round_trip() {
    use oxbloom_web::Json;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Todo {
        title: String,
        done: bool,
    }

    let app = || {
        App::new()
            .route(Route::new(
                HttpMethod::Post,
                "/todos",
                |r: Request| async move {
                    let ct = r
                        .headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    assert_eq!(ct, "application/json");
                    let todo: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                    Json(todo)
                },
            ))
            .unwrap()
    };
    let todo = Todo {
        title: "write tests".into(),
        done: false,
    };
    let memory = TestClient::in_memory(app());
    let served = TestClient::serve(app()).await;
    for client in [&memory, &served] {
        let res = client.post("/todos").json(&todo).send().await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.json::<Todo>(), todo);
    }
    served.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn headers_and_authorization_never_appear_in_panic_messages() {
    let (_reserved, addr) = closed_addr();
    let task = tokio::spawn(async move {
        TestClient::connect(addr)
            .post("/orders")
            .header("authorization", "Bearer top-secret-token")
            .header("x-api-key", "another-secret")
            .body("secret-body")
            .send()
            .await
    });
    let text = panic_text(task.await.unwrap_err().into_panic());
    assert!(text.contains("POST /orders"), "{text}");
    for secret in [
        "top-secret-token",
        "another-secret",
        "secret-body",
        "Bearer",
    ] {
        assert!(!text.contains(secret), "{text}");
    }
}

#[tokio::test]
async fn a_panicking_handler_answers_500_problem_json_in_memory() {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/boom", |_| async {
            panic!("handler marker");
            #[allow(unreachable_code)]
            ""
        }))
        .unwrap();
    let response = TestClient::in_memory(app).get("/boom").send().await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.header("content-type"),
        Some("application/problem+json")
    );
    assert!(!response.text().contains("handler marker"));
}
