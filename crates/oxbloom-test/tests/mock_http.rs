//! `MockHttpClient`: scripted answers by method and exact URL, recorded requests and the
//! network error for everything else.

#![cfg(feature = "http-client")]

use http::{Method, StatusCode};
use oxbloom_http_client::{Body, HttpClient, HttpClientError, Request, Response};
use oxbloom_test::{MockHttpClient, MockResponse};

#[tokio::test]
async fn mock_matches_exact_url_and_serves_in_order() {
    let mock = MockHttpClient::new();
    mock.on(
        Method::GET,
        "https://idp.example.test/a",
        MockResponse::status(200).text("one"),
    )
    .on(
        Method::GET,
        "https://idp.example.test/a",
        MockResponse::status(503),
    )
    .on(
        Method::GET,
        "https://idp.example.test/b",
        MockResponse::status(404),
    );

    let a = |n: &str| {
        let _ = n;
        Request::get("https://idp.example.test/a")
    };
    let first = mock.execute(a("1")).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.text().unwrap(), "one");
    // The second answer, then the last one repeats.
    for _ in 0..3 {
        assert_eq!(
            mock.execute(a("2")).await.unwrap().status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    let b = mock
        .execute(Request::get("https://idp.example.test/b"))
        .await
        .unwrap();
    assert_eq!(b.status(), StatusCode::NOT_FOUND);

    // The method counts, and so does every part of the URL.
    for request in [
        Request::post("https://idp.example.test/a"),
        Request::get("https://idp.example.test/a?x=1"),
        Request::get("https://idp.example.test/A"),
    ] {
        assert_eq!(
            mock.execute(request).await.unwrap_err(),
            HttpClientError::Network
        );
    }
}

#[tokio::test]
async fn mock_records_requests_and_headers() {
    let mock = MockHttpClient::new();
    mock.on(
        Method::POST,
        "https://idp.example.test/token",
        MockResponse::status(200)
            .header("content-type", "application/json")
            .json(&serde_json::json!({"access_token": "t"})),
    );
    assert_eq!(mock.request_count(), 0);
    let request = Request::post("https://idp.example.test/token")
        .header("Authorization", "Basic abc")
        .body(Body::Json(serde_json::json!({"a": 1})));
    let response = mock.execute(request).await.unwrap();
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(
        response.json::<serde_json::Value>().unwrap()["access_token"],
        "t"
    );

    assert_eq!(mock.request_count(), 1);
    let seen = mock.requests();
    assert_eq!(seen[0].method(), Method::POST);
    assert_eq!(seen[0].url(), "https://idp.example.test/token");
    assert_eq!(seen[0].headers()["authorization"], "Basic abc");
    assert!(matches!(seen[0].body_ref(), Body::Json(_)));
}

#[tokio::test]
async fn mock_unmatched_request_is_a_network_error() {
    let mock = MockHttpClient::new();
    let error = mock
        .execute(Request::get("https://nowhere.example.test/"))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::Network);
    // The request is recorded all the same.
    assert_eq!(mock.request_count(), 1);
}

#[tokio::test]
async fn mock_rejects_an_invalid_request_like_every_client() {
    let mock = MockHttpClient::new();
    mock.on(
        Method::GET,
        "https://idp.example.test/",
        MockResponse::status(200),
    );
    let bad = Request::get("https://idp.example.test/").header("bad name", "v");
    assert_eq!(
        mock.execute(bad).await.unwrap_err(),
        HttpClientError::InvalidRequest
    );
    assert_eq!(mock.request_count(), 0);
}

#[tokio::test]
async fn on_fn_answers_from_the_request_and_can_decline() {
    let mock = MockHttpClient::new();
    mock.on_fn(|request| {
        request.url().ends_with("/echo").then(|| {
            Response::new(
                StatusCode::OK,
                http::HeaderMap::new(),
                request.url().as_bytes().to_vec(),
            )
        })
    });
    let ok = mock
        .execute(Request::get("https://h.example.test/echo"))
        .await
        .unwrap();
    assert_eq!(ok.text().unwrap(), "https://h.example.test/echo");
    assert_eq!(
        mock.execute(Request::get("https://h.example.test/other"))
            .await
            .unwrap_err(),
        HttpClientError::Network
    );
}

#[tokio::test]
async fn the_builders_set_status_bytes_and_headers() {
    let mock = MockHttpClient::new();
    mock.on(
        Method::GET,
        "https://h.example.test/bin",
        MockResponse::status(201)
            .header("x-id", "7")
            .bytes(vec![1, 2, 3]),
    );
    let r = mock
        .execute(Request::get("https://h.example.test/bin"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::CREATED);
    assert_eq!(r.headers()["x-id"], "7");
    assert_eq!(r.bytes(), [1, 2, 3]);
}

#[test]
fn debug_shows_counts_and_never_a_url() {
    let mock = MockHttpClient::new();
    mock.on(
        Method::GET,
        "https://secret.example.test/path",
        MockResponse::status(200).text("hunter2"),
    );
    let text = format!("{mock:?}");
    assert!(!text.contains("secret.example.test"), "{text}");
    assert!(!text.contains("hunter2"), "{text}");
    assert!(!format!("{:?}", MockResponse::status(200).text("hunter2")).contains("hunter2"));
}
