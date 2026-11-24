//! OpenID Connect discovery: `ProviderMetadata::resolve` and `MetadataCache`, against
//! `FakeProvider` and scripted `MockHttpClient` answers.

#![cfg(feature = "oauth2")]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use http::Method;
use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response};
use oxbloom_security::__test_support::{MetadataCache, OAuth2Error, ProviderMetadata};
use oxbloom_security::{Algorithm, ProviderConfig};
use oxbloom_test::{FakeProvider, Fault, MockHttpClient, MockResponse};
use serde_json::json;

const ISSUER: &str = "https://idp.example.test";
const DISCOVERY: &str = "https://idp.example.test/.well-known/openid-configuration";

fn oidc(issuer: &str) -> ProviderConfig {
    ProviderConfig {
        issuer_uri: Some(issuer.to_owned()),
        ..ProviderConfig::default()
    }
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn document(token_endpoint: &str) -> serde_json::Value {
    json!({
        "issuer": ISSUER,
        "authorization_endpoint": format!("{ISSUER}/authorize"),
        "token_endpoint": token_endpoint,
        "jwks_uri": format!("{ISSUER}/jwks"),
    })
}

#[tokio::test]
async fn discovery_reads_the_endpoints() {
    let fake = FakeProvider::new(ISSUER);
    let metadata = ProviderMetadata::resolve(&oidc(ISSUER), fake.client().as_ref(), now())
        .await
        .unwrap();
    assert_eq!(metadata.issuer.as_deref(), Some(ISSUER));
    assert_eq!(
        metadata.authorization_endpoint,
        format!("{ISSUER}/authorize")
    );
    assert_eq!(metadata.token_endpoint, format!("{ISSUER}/token"));
    assert_eq!(
        metadata.userinfo_endpoint.as_deref(),
        Some(format!("{ISSUER}/userinfo").as_str())
    );
    assert_eq!(
        metadata.jwks_uri.as_deref(),
        Some(format!("{ISSUER}/jwks").as_str())
    );
    assert_eq!(metadata.id_token_algorithms, vec![Algorithm::ES256]);
    assert_eq!(metadata.resolved_at, now());
    assert!(metadata.is_oidc());
    let sent = fake.client().requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url(), DISCOVERY);
    assert_eq!(sent[0].headers()["accept"], "application/json");
}

#[tokio::test]
async fn the_algorithm_list_is_narrowed_to_asymmetric_ones_and_defaults_to_rs256_and_es256() {
    let mock = MockHttpClient::new();
    let mut doc = document(&format!("{ISSUER}/token"));
    doc["id_token_signing_alg_values_supported"] =
        json!(["HS256", "none", "RS256", "ES384", "PS256", "X"]);
    mock.on(Method::GET, DISCOVERY, MockResponse::status(200).json(&doc));
    let metadata = ProviderMetadata::resolve(&oidc(ISSUER), &mock, now())
        .await
        .unwrap();
    assert_eq!(
        metadata.id_token_algorithms,
        vec![Algorithm::RS256, Algorithm::ES384]
    );

    let mock = MockHttpClient::new();
    mock.on(
        Method::GET,
        DISCOVERY,
        MockResponse::status(200).json(&document(&format!("{ISSUER}/token"))),
    );
    let metadata = ProviderMetadata::resolve(&oidc(ISSUER), &mock, now())
        .await
        .unwrap();
    assert_eq!(
        metadata.id_token_algorithms,
        vec![Algorithm::RS256, Algorithm::ES256]
    );
}

#[tokio::test]
async fn a_mismatched_issuer_is_refused() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::DiscoveryIssuerMismatch);
    let err = ProviderMetadata::resolve(&oidc(ISSUER), fake.client().as_ref(), now())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::IssuerMismatch);
    assert!(fake.client().unmatched_requests().is_empty());
}

#[tokio::test]
async fn a_trailing_slash_difference_is_a_mismatch() {
    // The document names "https://idp.example.test"; the configuration has a trailing slash.
    let fake = FakeProvider::new(ISSUER);
    let err = ProviderMetadata::resolve(
        &oidc("https://idp.example.test/"),
        fake.client().as_ref(),
        now(),
    )
    .await
    .unwrap_err();
    assert_eq!(err, OAuth2Error::IssuerMismatch);
    // The discovery URL itself was found: the failure is the issuer comparison.
    assert!(fake.client().unmatched_requests().is_empty());

    // And the other way round.
    let fake = FakeProvider::new("https://idp.example.test/");
    let err = ProviderMetadata::resolve(&oidc(ISSUER), fake.client().as_ref(), now())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::IssuerMismatch);
}

#[tokio::test]
async fn a_failed_discovery_is_not_cached() {
    let mock = MockHttpClient::new();
    mock.on(Method::GET, DISCOVERY, MockResponse::status(503))
        .on(
            Method::GET,
            DISCOVERY,
            MockResponse::status(200).json(&document(&format!("{ISSUER}/token"))),
        );
    let cache = MetadataCache::new();
    let err = cache
        .get_or_resolve("idp", &oidc(ISSUER), &mock, now())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::Discovery);
    // After the backoff window (5 seconds by default) the next call tries again.
    let later = now() + Duration::from_secs(5);
    let metadata = cache
        .get_or_resolve("idp", &oidc(ISSUER), &mock, later)
        .await
        .unwrap();
    assert_eq!(metadata.token_endpoint, format!("{ISSUER}/token"));
    // A third call within the TTL is served from the cache.
    cache
        .get_or_resolve(
            "idp",
            &oidc(ISSUER),
            &mock,
            later + Duration::from_secs(3599),
        )
        .await
        .unwrap();
    assert_eq!(mock.request_count(), 2);
    // After the TTL (one hour by default) it is fetched again.
    cache
        .get_or_resolve(
            "idp",
            &oidc(ISSUER),
            &mock,
            later + Duration::from_secs(3600),
        )
        .await
        .unwrap();
    assert_eq!(mock.request_count(), 3);
}

#[tokio::test]
async fn a_call_within_the_backoff_window_gets_the_recorded_error_and_a_later_one_retries() {
    let mock = MockHttpClient::new();
    mock.on(Method::GET, DISCOVERY, MockResponse::status(503))
        .on(
            Method::GET,
            DISCOVERY,
            MockResponse::status(200).json(&document(&format!("{ISSUER}/token"))),
        );
    let cache = MetadataCache::new();
    for offset in [0, 1, 4] {
        let err = cache
            .get_or_resolve(
                "idp",
                &oidc(ISSUER),
                &mock,
                now() + Duration::from_secs(offset),
            )
            .await
            .unwrap_err();
        assert_eq!(err, OAuth2Error::Discovery);
    }
    assert_eq!(mock.request_count(), 1);
    cache
        .get_or_resolve("idp", &oidc(ISSUER), &mock, now() + Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(mock.request_count(), 2);

    // The window can be set; zero retries every time.
    let mock = MockHttpClient::new();
    mock.on(Method::GET, DISCOVERY, MockResponse::status(503));
    let cache = MetadataCache::new().failure_backoff(Duration::ZERO);
    for _ in 0..3 {
        assert!(
            cache
                .get_or_resolve("idp", &oidc(ISSUER), &mock, now())
                .await
                .is_err()
        );
    }
    assert_eq!(mock.request_count(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_callers_during_an_outage_share_one_failed_request() {
    let mock = Arc::new(MockHttpClient::new());
    mock.on(Method::GET, DISCOVERY, MockResponse::status(503));
    let client = Arc::new(Slow(mock.clone()));
    let cache = Arc::new(MetadataCache::new());
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let cache = cache.clone();
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            cache
                .get_or_resolve("idp", &oidc(ISSUER), client.as_ref(), now())
                .await
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap_err(), OAuth2Error::Discovery);
    }
    assert_eq!(mock.request_count(), 1);
}

#[tokio::test]
async fn the_cache_ttl_can_be_set_and_providers_are_cached_separately() {
    let fake = FakeProvider::new(ISSUER);
    let other = FakeProvider::new("https://other.example.test");
    let cache = MetadataCache::with_ttl(Duration::from_secs(10));
    cache
        .get_or_resolve("a", &oidc(ISSUER), fake.client().as_ref(), now())
        .await
        .unwrap();
    let b = cache
        .get_or_resolve(
            "b",
            &oidc("https://other.example.test"),
            other.client().as_ref(),
            now(),
        )
        .await
        .unwrap();
    assert_eq!(b.issuer.as_deref(), Some("https://other.example.test"));
    cache
        .get_or_resolve(
            "a",
            &oidc(ISSUER),
            fake.client().as_ref(),
            now() + Duration::from_secs(10),
        )
        .await
        .unwrap();
    assert_eq!(fake.client().request_count(), 2);
    assert_eq!(other.client().request_count(), 1);
}

/// Delays every answer, so that concurrent resolutions overlap.
struct Slow(Arc<MockHttpClient>);

#[async_trait]
impl HttpClient for Slow {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.0.execute(request).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_resolutions_fetch_once() {
    let fake = FakeProvider::new(ISSUER);
    let client = Arc::new(Slow(fake.client()));
    let cache = Arc::new(MetadataCache::new());
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let cache = cache.clone();
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            cache
                .get_or_resolve("idp", &oidc(ISSUER), client.as_ref(), now())
                .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(fake.client().request_count(), 1);
}

#[tokio::test]
async fn explicit_uris_override_discovery() {
    let fake = FakeProvider::new(ISSUER);
    let provider = ProviderConfig {
        issuer_uri: Some(ISSUER.into()),
        token_uri: Some("https://tokens.example.test/oauth/token".into()),
        user_info_uri: Some("https://api.example.test/me".into()),
        ..ProviderConfig::default()
    };
    let metadata = ProviderMetadata::resolve(&provider, fake.client().as_ref(), now())
        .await
        .unwrap();
    assert_eq!(
        metadata.token_endpoint,
        "https://tokens.example.test/oauth/token"
    );
    assert_eq!(
        metadata.userinfo_endpoint.as_deref(),
        Some("https://api.example.test/me")
    );
    assert_eq!(
        metadata.authorization_endpoint,
        format!("{ISSUER}/authorize")
    );
}

#[tokio::test]
async fn a_provider_without_an_issuer_needs_no_discovery() {
    let mock = MockHttpClient::new();
    let provider = ProviderConfig {
        authorization_uri: Some("https://github.example.test/login/oauth/authorize".into()),
        token_uri: Some("https://github.example.test/login/oauth/access_token".into()),
        user_info_uri: Some("https://api.github.example.test/user".into()),
        user_name_attribute: Some("login".into()),
        ..ProviderConfig::default()
    };
    let metadata = ProviderMetadata::resolve(&provider, &mock, now())
        .await
        .unwrap();
    assert_eq!(metadata.issuer, None);
    assert!(!metadata.is_oidc());
    assert_eq!(
        metadata.token_endpoint,
        "https://github.example.test/login/oauth/access_token"
    );
    assert_eq!(mock.request_count(), 0);
}

#[tokio::test]
async fn a_discovered_http_endpoint_is_refused_unless_loopback() {
    let mock = MockHttpClient::new();
    mock.on(
        Method::GET,
        DISCOVERY,
        MockResponse::status(200).json(&document("http://idp.example.test/token")),
    );
    let err = ProviderMetadata::resolve(&oidc(ISSUER), &mock, now())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::Discovery);
    assert!(mock.unmatched_requests().is_empty());

    // A loopback issuer may use http for everything.
    let fake = FakeProvider::new("http://127.0.0.1:8081");
    let metadata = ProviderMetadata::resolve(
        &oidc("http://127.0.0.1:8081"),
        fake.client().as_ref(),
        now(),
    )
    .await
    .unwrap();
    assert_eq!(metadata.token_endpoint, "http://127.0.0.1:8081/token");
}

#[tokio::test]
async fn a_bad_or_oversized_document_is_a_discovery_failure() {
    let missing_token = json!({"issuer": ISSUER, "authorization_endpoint": format!("{ISSUER}/a")});
    let mut oversized = document(&format!("{ISSUER}/token"));
    oversized["padding"] = json!("x".repeat(1024 * 1024));
    for answer in [
        MockResponse::status(200).text("not json"),
        MockResponse::status(200).json(&json!(["an array"])),
        MockResponse::status(200).json(&missing_token),
        MockResponse::status(200).json(&oversized),
        MockResponse::status(302).header("location", "https://evil.example.test/"),
    ] {
        let mock = MockHttpClient::new();
        mock.on(Method::GET, DISCOVERY, answer);
        let err = ProviderMetadata::resolve(&oidc(ISSUER), &mock, now())
            .await
            .unwrap_err();
        assert_eq!(err, OAuth2Error::Discovery);
    }
    // A transport failure is an HTTP failure.
    let err = ProviderMetadata::resolve(&oidc(ISSUER), &MockHttpClient::new(), now())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::Http);
}

#[tokio::test]
async fn debug_and_errors_hold_no_url() {
    let fake = FakeProvider::new(ISSUER);
    let metadata = ProviderMetadata::resolve(&oidc(ISSUER), fake.client().as_ref(), now())
        .await
        .unwrap();
    assert!(!format!("{metadata:?}").contains("idp.example.test"));
    for err in [OAuth2Error::Discovery, OAuth2Error::IssuerMismatch] {
        assert!(!err.to_string().contains("http"));
    }
}

#[tokio::test]
async fn a_failure_time_in_the_future_counts_as_expired() {
    let mock = MockHttpClient::new();
    mock.on(Method::GET, DISCOVERY, MockResponse::status(503));
    let cache = MetadataCache::new();
    assert!(
        cache
            .get_or_resolve("idp", &oidc(ISSUER), &mock, now())
            .await
            .is_err()
    );
    assert_eq!(mock.request_count(), 1);
    // The clock stepped back a minute: the recorded failure lies in the future. The backoff
    // must not last until the clock catches up, so the next call tries again.
    let earlier = now() - Duration::from_secs(60);
    assert!(
        cache
            .get_or_resolve("idp", &oidc(ISSUER), &mock, earlier)
            .await
            .is_err()
    );
    assert_eq!(mock.request_count(), 2);
}

#[tokio::test]
async fn callers_queued_behind_a_failed_attempt_take_its_failure_even_without_a_backoff() {
    let mock = Arc::new(MockHttpClient::new());
    mock.on(Method::GET, DISCOVERY, MockResponse::status(503));
    let client = Slow(mock.clone());
    // Without a backoff window only the attempt counter keeps the queued callers from each
    // making their own request.
    let cache = MetadataCache::new().failure_backoff(Duration::ZERO);
    let provider = oidc(ISSUER);
    let call = || cache.get_or_resolve("idp", &provider, &client, now());
    // On one thread, `join!` polls the first call into its request and queues the others
    // behind the slot lock before the attempt finishes.
    let (a, b, c, d) = tokio::join!(call(), call(), call(), call());
    for result in [a, b, c, d] {
        assert_eq!(result.unwrap_err(), OAuth2Error::Discovery);
    }
    assert_eq!(mock.request_count(), 1);
    // A call that did not wait for that attempt tries again.
    assert!(call().await.is_err());
    assert_eq!(mock.request_count(), 2);
}
