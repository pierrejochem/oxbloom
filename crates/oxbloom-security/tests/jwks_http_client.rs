//! `ReqwestJwksFetcher::with_client` over a mock `HttpClient`: parsing and error mapping.

#![cfg(feature = "jwks")]

use std::sync::Arc;

use async_trait::async_trait;
use http::Method;
use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response};
use oxbloom_security::{FetchError, JwksFetcher, ReqwestJwksFetcher};
use oxbloom_test::{MockHttpClient, MockResponse};

const URI: &str = "https://issuer.example.test/jwks.json";
const JWKS: &str = r#"{"keys":[{"kty":"oct","kid":"k1","k":"c2VjcmV0"}]}"#;

/// A client that fails every request with a given error.
struct Failing(HttpClientError);

#[async_trait]
impl HttpClient for Failing {
    async fn execute(&self, _request: Request) -> Result<Response, HttpClientError> {
        Err(self.0.clone())
    }
}

fn fetcher(client: impl HttpClient + 'static) -> ReqwestJwksFetcher {
    ReqwestJwksFetcher::with_client(Arc::new(client))
}

#[tokio::test]
async fn jwks_fetcher_over_a_mock_client_parses_the_key_set_and_maps_errors() {
    let mock = Arc::new(MockHttpClient::new());
    mock.on(Method::GET, URI, MockResponse::status(200).text(JWKS));
    let shared: Arc<dyn HttpClient> = mock.clone();
    let set = ReqwestJwksFetcher::with_client(shared)
        .fetch(URI)
        .await
        .unwrap();
    assert_eq!(set.keys.len(), 1);
    let sent = mock.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method(), Method::GET);
    assert_eq!(sent[0].headers()["accept"], "application/json");

    let not_found = MockHttpClient::new();
    not_found.on(Method::GET, URI, MockResponse::status(404).text(JWKS));
    assert_eq!(
        fetcher(not_found).fetch(URI).await.unwrap_err(),
        FetchError::BadStatus
    );

    let redirect = MockHttpClient::new();
    redirect.on(Method::GET, URI, MockResponse::status(302));
    assert_eq!(
        fetcher(redirect).fetch(URI).await.unwrap_err(),
        FetchError::BadStatus
    );

    let malformed = MockHttpClient::new();
    malformed.on(Method::GET, URI, MockResponse::status(200).text("not json"));
    assert_eq!(
        fetcher(malformed).fetch(URI).await.unwrap_err(),
        FetchError::Malformed
    );

    // A mock without a rule answers `Network`.
    assert_eq!(
        fetcher(MockHttpClient::new()).fetch(URI).await.unwrap_err(),
        FetchError::Network
    );

    for (error, expected) in [
        (HttpClientError::TooLarge, FetchError::TooLarge),
        (HttpClientError::Timeout, FetchError::Timeout),
        (HttpClientError::Network, FetchError::Network),
        (HttpClientError::Decode, FetchError::Network),
        (
            HttpClientError::BadStatus(http::StatusCode::BAD_GATEWAY),
            FetchError::BadStatus,
        ),
    ] {
        let err = fetcher(Failing(error.clone()))
            .fetch(URI)
            .await
            .unwrap_err();
        assert_eq!(err, expected, "{error:?}");
    }
}

#[tokio::test]
async fn fetcher_refuses_a_body_over_one_mib_even_if_the_client_allows_it() {
    let mut body = JWKS.as_bytes().to_vec();
    body.resize(1_048_577, b' ');
    let mock = MockHttpClient::new();
    mock.on(
        Method::GET,
        URI,
        MockResponse::status(200).bytes(body.clone()),
    );
    assert_eq!(
        fetcher(mock).fetch(URI).await.unwrap_err(),
        FetchError::TooLarge
    );
    body.truncate(1_048_576);
    let mock = MockHttpClient::new();
    mock.on(Method::GET, URI, MockResponse::status(200).bytes(body));
    assert!(fetcher(mock).fetch(URI).await.is_ok());
}
