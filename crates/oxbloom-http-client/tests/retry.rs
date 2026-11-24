//! `RetryingClient`: which failures and statuses are retried, for which methods, and what is
//! returned when the attempts run out.

#![cfg(feature = "retry")]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response, RetryingClient};
use oxbloom_tasks::{Jitter, RetryPolicy};

/// An inner client that plays back scripted outcomes and counts its calls.
struct Scripted {
    outcomes: Mutex<VecDeque<Result<Response, HttpClientError>>>,
    calls: Mutex<Vec<http::Method>>,
}

impl Scripted {
    fn new(outcomes: Vec<Result<Response, HttpClientError>>) -> Arc<Scripted> {
        Arc::new(Scripted {
            outcomes: Mutex::new(outcomes.into()),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl HttpClient for Scripted {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        self.calls.lock().unwrap().push(request.method());
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("more calls than scripted outcomes")
    }
}

fn status(code: u16, body: &str) -> Result<Response, HttpClientError> {
    Ok(Response::new(
        http::StatusCode::from_u16(code).unwrap(),
        http::HeaderMap::new(),
        body.as_bytes().to_vec(),
    ))
}

/// Three attempts with a 1 ms fixed delay, so the tests run fast on real time.
fn policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        initial: Duration::from_millis(1),
        jitter: Jitter::None,
        ..RetryPolicy::default()
    }
}

fn retrying(inner: &Arc<Scripted>) -> RetryingClient {
    RetryingClient::new(Arc::clone(inner) as Arc<dyn HttpClient>, policy())
}

#[tokio::test]
async fn a_get_is_retried_on_503_then_succeeds() {
    let inner = Scripted::new(vec![status(503, "busy"), status(200, "done")]);
    let response = retrying(&inner)
        .execute(Request::get("https://api.example.test/"))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.text().unwrap(), "done");
    assert_eq!(inner.calls(), 2);
}

#[tokio::test]
async fn statuses_502_and_504_are_retried_too() {
    let inner = Scripted::new(vec![status(502, ""), status(504, ""), status(200, "ok")]);
    let response = retrying(&inner)
        .execute(Request::new(
            http::Method::PUT,
            "https://api.example.test/item",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(inner.calls(), 3);
}

#[tokio::test]
async fn a_post_is_not_retried() {
    let inner = Scripted::new(vec![status(503, "busy")]);
    let response = retrying(&inner)
        .execute(Request::post("https://api.example.test/"))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(inner.calls(), 1);

    let inner = Scripted::new(vec![Err(HttpClientError::Network)]);
    let error = retrying(&inner)
        .execute(Request::post("https://api.example.test/"))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::Network);
    assert_eq!(inner.calls(), 1);
}

#[tokio::test]
async fn retries_stop_after_max_attempts_and_return_the_last_response() {
    let inner = Scripted::new(vec![
        status(503, "first"),
        status(502, "second"),
        status(504, "last"),
    ]);
    let response = retrying(&inner)
        .execute(Request::get("https://api.example.test/"))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(response.text().unwrap(), "last");
    assert_eq!(inner.calls(), 3);
}

#[tokio::test]
async fn a_network_error_on_get_is_retried() {
    let inner = Scripted::new(vec![
        Err(HttpClientError::Network),
        Err(HttpClientError::Timeout),
        status(200, "ok"),
    ]);
    let response = retrying(&inner)
        .execute(Request::get("https://api.example.test/"))
        .await
        .unwrap();
    assert_eq!(response.text().unwrap(), "ok");
    assert_eq!(inner.calls(), 3);

    // When every attempt fails, the last error is returned.
    let inner = Scripted::new(vec![
        Err(HttpClientError::Timeout),
        Err(HttpClientError::Timeout),
        Err(HttpClientError::Network),
    ]);
    let error = retrying(&inner)
        .execute(Request::get("https://api.example.test/"))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::Network);
    assert_eq!(inner.calls(), 3);
}

#[tokio::test]
async fn other_errors_and_statuses_are_not_retried() {
    let inner = Scripted::new(vec![Err(HttpClientError::TooLarge)]);
    let error = retrying(&inner)
        .execute(Request::get("https://api.example.test/"))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::TooLarge);
    assert_eq!(inner.calls(), 1);

    for code in [500, 404, 429] {
        let inner = Scripted::new(vec![status(code, "")]);
        let response = retrying(&inner)
            .execute(Request::get("https://api.example.test/"))
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), code);
        assert_eq!(inner.calls(), 1, "status {code}");
    }
}

#[tokio::test]
async fn every_idempotent_method_is_retried_and_patch_is_not() {
    for method in [
        http::Method::GET,
        http::Method::HEAD,
        http::Method::PUT,
        http::Method::DELETE,
        http::Method::OPTIONS,
    ] {
        let inner = Scripted::new(vec![status(503, ""), status(200, "")]);
        retrying(&inner)
            .execute(Request::new(method.clone(), "https://api.example.test/"))
            .await
            .unwrap();
        assert_eq!(inner.calls(), 2, "{method}");
    }
    let inner = Scripted::new(vec![status(503, "")]);
    retrying(&inner)
        .execute(Request::new(
            http::Method::PATCH,
            "https://api.example.test/",
        ))
        .await
        .unwrap();
    assert_eq!(inner.calls(), 1);
}

#[tokio::test]
async fn an_invalid_request_is_rejected_and_never_reaches_the_inner_client() {
    let inner = Scripted::new(vec![]);
    let error = retrying(&inner)
        .execute(Request::get("https://api.example.test/").header("bad name", "x"))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::InvalidRequest);
    assert_eq!(inner.calls(), 0);
}

#[cfg(feature = "reqwest")]
#[tokio::test]
async fn a_get_over_a_real_socket_is_retried_on_503() {
    use oxbloom_http_client::ClientBuilder;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let answer = if n == 0 {
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            };
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    let inner: Arc<dyn HttpClient> = Arc::new(ClientBuilder::new().build().unwrap());
    let client = RetryingClient::new(inner, policy());
    let response = client
        .execute(Request::get(format!("http://{addr}/")))
        .await
        .unwrap();
    assert_eq!(response.text().unwrap(), "ok");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
