//! `OAuth2Client::client_credentials` and the token cache, against `FakeProvider` and a
//! `ManualClock`.

#![cfg(feature = "oauth2")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response};
use oxbloom_security::{
    ClientAuthentication, ClientRegistration, OAuth2Client, OAuth2Settings, ProviderConfig,
    TokenError,
};
use oxbloom_test::{FakeProvider, Fault, ManualClock};

const ISSUER: &str = "https://idp.example.test";
const SECRET: &str = "the-client-secret-0123456789";

fn registration(id: &str, scopes: &[&str], auth: ClientAuthentication) -> ClientRegistration {
    ClientRegistration {
        id: id.into(),
        client_id: format!("{id}-client"),
        client_secret: (auth != ClientAuthentication::None).then(|| Secret::new(SECRET.into())),
        scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        redirect_uri: "https://app.example.test/login/oauth2/code/idp".into(),
        client_authentication: auth,
        provider: "idp".into(),
    }
}

fn settings(registrations: Vec<ClientRegistration>) -> OAuth2Settings {
    let provider = ProviderConfig {
        issuer_uri: Some(ISSUER.into()),
        ..ProviderConfig::default()
    };
    OAuth2Settings {
        registrations,
        providers: [("idp".to_owned(), provider)].into(),
    }
}

struct Setup {
    fake: FakeProvider,
    clock: ManualClock,
    client: OAuth2Client,
}

fn setup_with(
    registrations: Vec<ClientRegistration>,
    wrap: impl FnOnce(Arc<dyn HttpClient>) -> Arc<dyn HttpClient>,
) -> Setup {
    let fake = FakeProvider::new(ISSUER);
    let clock = ManualClock::at(SystemTime::now());
    let http = wrap(fake.client());
    let client = OAuth2Client::new(settings(registrations), http).clock(Arc::new(clock.clone()));
    Setup {
        fake,
        clock,
        client,
    }
}

fn setup() -> Setup {
    setup_with(
        vec![registration(
            "svc",
            &["read", "write"],
            ClientAuthentication::Basic,
        )],
        |http| http,
    )
}

/// Delays every request, so concurrent callers really overlap.
struct Slow(Arc<dyn HttpClient>);

#[async_trait]
impl HttpClient for Slow {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        tokio::time::sleep(Duration::from_millis(25)).await;
        self.0.execute(request).await
    }
}

/// Fails the first `failures` requests to the token endpoint at the network level.
struct Flaky {
    inner: Arc<dyn HttpClient>,
    failures: AtomicUsize,
}

#[async_trait]
impl HttpClient for Flaky {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        if request.url().ends_with("/token")
            && self
                .failures
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            return Err(HttpClientError::Network);
        }
        self.inner.execute(request).await
    }
}

#[tokio::test]
async fn the_first_call_fetches_a_token_and_the_second_uses_the_cache() {
    let s = setup();
    let first = s.client.client_credentials("svc").await.unwrap();
    let second = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(first.value.expose(), second.value.expose());
    assert_eq!(first.expires_at, s.clock.now() + Duration::from_secs(3600));
    let requests = s.fake.token_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].field("grant_type"), Some("client_credentials"));
}

#[tokio::test]
async fn a_token_is_refreshed_once_it_is_within_the_skew_of_expiry() {
    let s = setup();
    s.fake.access_token_lifetime(Some(100));
    let first = s.client.client_credentials("svc").await.unwrap();
    // The default skew is 30 seconds, so the token is used until 70 seconds have passed.
    s.clock.advance(Duration::from_secs(69));
    let again = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(first.value.expose(), again.value.expose());
    assert_eq!(s.fake.token_requests().len(), 1);

    s.clock.advance(Duration::from_secs(1));
    let renewed = s.client.client_credentials("svc").await.unwrap();
    assert_ne!(first.value.expose(), renewed.value.expose());
    assert_eq!(s.fake.token_requests().len(), 2);
}

#[tokio::test]
async fn the_skew_can_be_changed() {
    let fake = FakeProvider::new(ISSUER);
    fake.access_token_lifetime(Some(100));
    let clock = ManualClock::at(SystemTime::now());
    let client = OAuth2Client::new(
        settings(vec![registration("svc", &[], ClientAuthentication::Basic)]),
        fake.client(),
    )
    .clock(Arc::new(clock.clone()))
    .skew(Duration::from_secs(90));
    client.client_credentials("svc").await.unwrap();
    clock.advance(Duration::from_secs(9));
    client.client_credentials("svc").await.unwrap();
    assert_eq!(fake.token_requests().len(), 1);
    clock.advance(Duration::from_secs(1));
    client.client_credentials("svc").await.unwrap();
    assert_eq!(fake.token_requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_callers_at_expiry_cause_one_request() {
    let s = setup_with(
        vec![registration("svc", &["read"], ClientAuthentication::Basic)],
        |http| Arc::new(Slow(http)),
    );
    let client = Arc::new(s.client.clone());
    let tasks: Vec<_> = (0..16)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.client_credentials("svc").await })
        })
        .collect();
    let mut values = Vec::new();
    for task in tasks {
        values.push(task.await.unwrap().unwrap().value.expose().clone());
    }
    assert_eq!(s.fake.token_requests().len(), 1);
    assert!(values.windows(2).all(|pair| pair[0] == pair[1]));

    // The same at expiry: sixteen callers find the token stale and one refreshes it.
    s.clock.advance(Duration::from_secs(3600));
    let tasks: Vec<_> = (0..16)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.client_credentials("svc").await })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(s.fake.token_requests().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_callers_share_the_failure_of_the_request_they_waited_for() {
    let s = setup_with(
        vec![registration("svc", &[], ClientAuthentication::Basic)],
        |http| {
            Arc::new(Slow(Arc::new(Flaky {
                inner: http,
                failures: AtomicUsize::new(1),
            })))
        },
    );
    let client = Arc::new(s.client.clone());
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.client_credentials("svc").await })
        })
        .collect();
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap_err(), TokenError::Http);
    }
    // The failure was not kept: the next caller tries again at once.
    assert!(s.client.client_credentials("svc").await.is_ok());
}

#[tokio::test]
async fn a_failed_request_is_not_cached() {
    let s = setup_with(
        vec![registration("svc", &[], ClientAuthentication::Basic)],
        |http| {
            Arc::new(Flaky {
                inner: http,
                failures: AtomicUsize::new(2),
            })
        },
    );
    assert_eq!(
        s.client.client_credentials("svc").await.unwrap_err(),
        TokenError::Http
    );
    assert_eq!(
        s.client.client_credentials("svc").await.unwrap_err(),
        TokenError::Http
    );
    // No backoff: the third call goes out at once and succeeds, and the fourth is cached.
    let token = s.client.client_credentials("svc").await.unwrap();
    let again = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(token.value.expose(), again.value.expose());
    assert_eq!(s.fake.token_requests().len(), 1);
}

#[tokio::test]
async fn a_missing_expires_in_means_60_seconds() {
    let s = setup();
    s.fake.access_token_lifetime(None);
    let token = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(token.expires_at, s.clock.now() + Duration::from_secs(60));
    // Valid for 60 seconds minus the skew of 30.
    s.clock.advance(Duration::from_secs(29));
    s.client.client_credentials("svc").await.unwrap();
    assert_eq!(s.fake.token_requests().len(), 1);
    s.clock.advance(Duration::from_secs(1));
    s.client.client_credentials("svc").await.unwrap();
    assert_eq!(s.fake.token_requests().len(), 2);
}

#[tokio::test]
async fn an_already_expired_token_is_returned_once_and_not_cached() {
    let s = setup();
    s.fake.access_token_lifetime(Some(0));
    let first = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(first.expires_at, s.clock.now());
    // One request per call, never a loop inside a call.
    assert_eq!(s.fake.token_requests().len(), 1);
    let second = s.client.client_credentials("svc").await.unwrap();
    assert_ne!(first.value.expose(), second.value.expose());
    assert_eq!(s.fake.token_requests().len(), 2);
}

#[tokio::test]
async fn a_lifetime_shorter_than_the_skew_is_returned_but_refreshed_on_the_next_call() {
    let s = setup();
    s.fake.access_token_lifetime(Some(10));
    s.client.client_credentials("svc").await.unwrap();
    s.client.client_credentials("svc").await.unwrap();
    assert_eq!(s.fake.token_requests().len(), 2);
}

#[tokio::test]
async fn the_scope_is_sent_and_omitted_when_empty() {
    let s = setup_with(
        vec![
            registration("svc", &["read", "write"], ClientAuthentication::Basic),
            registration("bare", &[], ClientAuthentication::Basic),
        ],
        |http| http,
    );
    s.client.client_credentials("svc").await.unwrap();
    s.client.client_credentials("bare").await.unwrap();
    let requests = s.fake.token_requests();
    assert_eq!(requests[0].field("scope"), Some("read write"));
    assert_eq!(requests[1].field("scope"), None);
    assert_eq!(requests[1].field("grant_type"), Some("client_credentials"));
}

#[tokio::test]
async fn the_client_authentication_of_the_registration_is_used() {
    let s = setup_with(
        vec![
            registration("basic", &[], ClientAuthentication::Basic),
            registration("post", &[], ClientAuthentication::Post),
        ],
        |http| http,
    );
    s.fake.expect_client_secret(SECRET);
    s.client.client_credentials("basic").await.unwrap();
    s.client.client_credentials("post").await.unwrap();
    let requests = s.fake.token_requests();
    assert!(requests[0].authorization().unwrap().starts_with("Basic "));
    assert_eq!(requests[0].field("client_secret"), None);
    assert_eq!(requests[1].authorization(), None);
    assert_eq!(requests[1].field("client_id"), Some("post-client"));
    assert_eq!(requests[1].field("client_secret"), Some(SECRET));
}

#[tokio::test]
async fn registrations_have_their_own_tokens() {
    let s = setup_with(
        vec![
            registration("a", &[], ClientAuthentication::Basic),
            registration("b", &[], ClientAuthentication::Basic),
        ],
        |http| http,
    );
    let a = s.client.client_credentials("a").await.unwrap();
    let b = s.client.client_credentials("b").await.unwrap();
    assert_ne!(a.value.expose(), b.value.expose());
    assert_eq!(s.fake.token_requests().len(), 2);
    s.client.client_credentials("a").await.unwrap();
    s.client.client_credentials("b").await.unwrap();
    assert_eq!(s.fake.token_requests().len(), 2);
}

#[tokio::test]
async fn an_unknown_registration_is_an_error() {
    let s = setup();
    assert_eq!(
        s.client.client_credentials("nope").await.unwrap_err(),
        TokenError::UnknownRegistration
    );
    assert_eq!(s.fake.client().request_count(), 0);
}

#[tokio::test]
async fn the_provider_error_code_is_surfaced_from_the_allow_list() {
    let s = setup();
    s.fake.expect_client_secret("not-the-secret");
    assert_eq!(
        s.client.client_credentials("svc").await.unwrap_err(),
        TokenError::Provider("invalid_client")
    );

    let s = setup();
    s.fake
        .misbehave(Fault::TokenEndpointError("<script>alert(1)</script>"));
    let error = s.client.client_credentials("svc").await.unwrap_err();
    assert_eq!(error, TokenError::Provider("unknown"));
    assert!(!error.to_string().contains("script"));
    assert!(!format!("{error:?}").contains("script"));

    let s = setup();
    s.fake.misbehave(Fault::TokenEndpointError("invalid_scope"));
    assert_eq!(
        s.client.client_credentials("svc").await.unwrap_err(),
        TokenError::Provider("invalid_scope")
    );
}

#[tokio::test]
async fn the_other_failures_have_their_own_error() {
    let s = setup();
    s.fake.misbehave(Fault::TokenEndpointStatus(503));
    assert_eq!(
        s.client.client_credentials("svc").await.unwrap_err(),
        TokenError::Invalid
    );

    let s = setup();
    s.fake.misbehave(Fault::DiscoveryIssuerMismatch);
    assert_eq!(
        s.client.client_credentials("svc").await.unwrap_err(),
        TokenError::Discovery
    );

    // A provider that cannot be reached at all.
    let client = OAuth2Client::new(
        settings(vec![registration("svc", &[], ClientAuthentication::Basic)]),
        Arc::new(oxbloom_test::MockHttpClient::new()),
    );
    assert_eq!(
        client.client_credentials("svc").await.unwrap_err(),
        TokenError::Http
    );
}

#[tokio::test]
async fn a_public_client_cannot_use_client_credentials() {
    let s = setup_with(
        vec![registration("spa", &["read"], ClientAuthentication::None)],
        |http| http,
    );
    assert_eq!(
        s.client.client_credentials("spa").await.unwrap_err(),
        TokenError::Invalid
    );
    assert_eq!(s.fake.client().request_count(), 0);
}

#[tokio::test]
async fn a_registration_without_a_secret_is_invalid() {
    let mut broken = registration("svc", &[], ClientAuthentication::Basic);
    broken.client_secret = None;
    let s = setup_with(vec![broken], |http| http);
    assert_eq!(
        s.client.client_credentials("svc").await.unwrap_err(),
        TokenError::Invalid
    );
    assert!(s.fake.token_requests().is_empty());
}

#[tokio::test]
async fn invalidate_forces_a_new_request() {
    let s = setup();
    let first = s.client.client_credentials("svc").await.unwrap();
    s.client.invalidate("svc");
    let second = s.client.client_credentials("svc").await.unwrap();
    assert_ne!(first.value.expose(), second.value.expose());
    assert_eq!(s.fake.token_requests().len(), 2);
    // Invalidating a registration that was never used, or does not exist, is harmless.
    s.client.invalidate("never-used");
    s.client.client_credentials("svc").await.unwrap();
    assert_eq!(s.fake.token_requests().len(), 2);
}

#[tokio::test]
async fn the_metadata_is_resolved_once() {
    let s = setup();
    s.fake.access_token_lifetime(Some(10));
    for _ in 0..3 {
        s.client.client_credentials("svc").await.unwrap();
    }
    let discoveries = s
        .fake
        .client()
        .requests()
        .iter()
        .filter(|r| r.url().ends_with("/.well-known/openid-configuration"))
        .count();
    assert_eq!(discoveries, 1);
}

#[tokio::test]
async fn every_request_has_the_provider_timeout() {
    let s = setup();
    s.client.client_credentials("svc").await.unwrap();
    for request in s.fake.client().requests() {
        assert_eq!(request.timeout_value(), Some(Duration::from_secs(10)));
    }
}

#[tokio::test]
async fn tokens_never_appear_in_debug_or_error_text() {
    let s = setup();
    let token = s.client.client_credentials("svc").await.unwrap();
    let value = token.value.expose().clone();
    let debug = format!("{token:?}");
    assert!(!debug.contains(&value));
    assert!(debug.contains("REDACTED"));
    assert!(!format!("{:?}", s.client).contains(&value));
    assert!(!format!("{:?}", s.client).contains(SECRET));

    for error in [
        TokenError::UnknownRegistration,
        TokenError::Provider("invalid_grant"),
        TokenError::Http,
        TokenError::Discovery,
        TokenError::ReauthenticationRequired,
        TokenError::Invalid,
    ] {
        let text = format!("{error} {error:?}");
        assert!(!text.contains(&value));
        assert!(!text.contains("https://"));
    }
    assert_eq!(
        TokenError::Provider("invalid_grant").to_string(),
        "the OAuth2 provider answered with the error code invalid_grant"
    );
}

#[test]
fn the_client_and_the_errors_are_send_sync_and_cheap_to_clone() {
    fn assert_send_sync<T: Send + Sync + Clone>() {}
    assert_send_sync::<OAuth2Client>();
    assert_send_sync::<TokenError>();
    assert_send_sync::<oxbloom_security::AccessToken>();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_invalidated_while_its_request_is_in_flight_is_not_kept() {
    let s = setup_with(
        vec![registration("svc", &[], ClientAuthentication::Basic)],
        |http| Arc::new(Slow(http)),
    );
    let client = s.client.clone();
    let call = tokio::spawn(async move { client.client_credentials("svc").await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    s.client.invalidate("svc");
    // The caller that was already waiting still gets its answer.
    call.await.unwrap().unwrap();
    assert_eq!(s.fake.token_requests().len(), 1);
    // But the token was not stored, so the next call asks again.
    s.client.client_credentials("svc").await.unwrap();
    assert_eq!(s.fake.token_requests().len(), 2);
}
