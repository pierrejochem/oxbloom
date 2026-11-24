//! The bearer interceptors: `ClientCredentialsInterceptor`, `BearerRetryClient` and
//! `UserTokenInterceptor`, against `FakeProvider` for the tokens and a `MockHttpClient` for the
//! downstream service.

#![cfg(feature = "oauth2")]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use http::{Method, header};
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_http_client::Request as OutRequest;
use oxbloom_http_client::{Body, HttpClient, HttpClientError, InterceptedClient, Interceptor};
use oxbloom_security::{
    Authentication, BearerRetryClient, ClientAuthentication, ClientCredentialsInterceptor,
    ClientRegistration, OAuth2Client, OAuth2Settings, ProviderConfig, SecurityContext, Session,
    UserTokenInterceptor,
};
use oxbloom_test::{FakeProvider, FakeUser, ManualClock, MockHttpClient, MockResponse};
use oxbloom_web::{HttpMethod, Request};
use serde_json::{Value, json};

const ISSUER: &str = "https://idp.example.test";
const API: &str = "https://api.example.test/orders";
const SECRET: &str = "the-client-secret-0123456789";
const TOKENS_KEY: &str = "oxbloom.security.oauth2.tokens.idp";

fn registration(id: &str) -> ClientRegistration {
    ClientRegistration {
        id: id.into(),
        client_id: "app".into(),
        client_secret: Some(Secret::new(SECRET.into())),
        scopes: vec!["orders.read".into()],
        redirect_uri: "https://app.example.test/login/oauth2/code/idp".into(),
        client_authentication: ClientAuthentication::Basic,
        provider: "idp".into(),
    }
}

fn settings() -> OAuth2Settings {
    let provider = ProviderConfig {
        issuer_uri: Some(ISSUER.into()),
        ..ProviderConfig::default()
    };
    OAuth2Settings {
        registrations: vec![registration("svc"), registration("idp")],
        providers: [("idp".to_owned(), provider)].into(),
    }
}

struct Setup {
    fake: FakeProvider,
    clock: ManualClock,
    client: Arc<OAuth2Client>,
    api: Arc<MockHttpClient>,
}

fn setup() -> Setup {
    let clock = ManualClock::at(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
    let client = OAuth2Client::new(settings(), fake.client()).clock(Arc::new(clock.clone()));
    Setup {
        fake,
        clock,
        client: Arc::new(client),
        api: Arc::new(MockHttpClient::new()),
    }
}

fn bearer(request: &OutRequest) -> String {
    request.headers()[header::AUTHORIZATION]
        .to_str()
        .unwrap()
        .to_owned()
}

fn client_credentials_requests(fake: &FakeProvider) -> usize {
    fake.token_requests()
        .iter()
        .filter(|r| r.field("grant_type") == Some("client_credentials"))
        .count()
}

// ---------------------------------------------------------------------------------------------
// ClientCredentialsInterceptor
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_client_credentials_interceptor_sets_the_bearer_header_and_replaces_an_existing_authorization()
 {
    let s = setup();
    s.api.on(Method::GET, API, MockResponse::status(200));
    let http = InterceptedClient::new(s.api.clone()).with(Arc::new(
        ClientCredentialsInterceptor::new(s.client.clone(), "svc"),
    ));
    let response = http
        .execute(OutRequest::get(API).header("authorization", "Basic c29tZW9uZTplbHNl"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let sent = s.api.requests().pop().unwrap();
    let token = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(bearer(&sent), format!("Bearer {}", token.value.expose()));
    assert_eq!(
        sent.headers().get_all(header::AUTHORIZATION).iter().count(),
        1
    );
    // The second request uses the cached token.
    http.execute(OutRequest::get(API)).await.unwrap();
    assert_eq!(client_credentials_requests(&s.fake), 1);
}

#[tokio::test]
async fn a_token_error_stops_the_request_with_fixed_text() {
    let s = setup();
    let http = InterceptedClient::new(s.api.clone()).with(Arc::new(
        ClientCredentialsInterceptor::new(s.client.clone(), "unknown"),
    ));
    let error = http.execute(OutRequest::get(API)).await.unwrap_err();
    assert_eq!(error, HttpClientError::InvalidRequest);
    assert_eq!(s.api.request_count(), 0);
}

// ---------------------------------------------------------------------------------------------
// BearerRetryClient
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn bearer_retry_client_retries_once_on_401_with_a_fresh_token() {
    let s = setup();
    s.api
        .on(Method::POST, API, MockResponse::status(401).text("expired"));
    s.api
        .on(Method::POST, API, MockResponse::status(201).text("created"));
    let http = BearerRetryClient::new(s.api.clone(), s.client.clone(), "svc");
    let request = OutRequest::post(API)
        .header("content-type", "application/json")
        .body(Body::Bytes(br#"{"item":"book"}"#.to_vec()));
    let response = http.execute(request).await.unwrap();
    assert_eq!(response.status(), 201);
    assert_eq!(response.bytes(), b"created");

    let sent = s.api.requests();
    assert_eq!(sent.len(), 2);
    assert_ne!(bearer(&sent[0]), bearer(&sent[1]));
    // The retry is the same request: method, headers and body.
    assert_eq!(sent[1].method(), Method::POST);
    assert_eq!(sent[1].headers()["content-type"], "application/json");
    assert_eq!(
        sent[1].body_ref(),
        &Body::Bytes(br#"{"item":"book"}"#.to_vec())
    );
    assert_eq!(client_credentials_requests(&s.fake), 2);
    // The fresh token is the cached one now.
    let cached = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(
        bearer(&sent[1]),
        format!("Bearer {}", cached.value.expose())
    );
    assert_eq!(client_credentials_requests(&s.fake), 2);
}

#[tokio::test]
async fn bearer_retry_client_does_not_retry_twice() {
    let s = setup();
    s.api
        .on(Method::GET, API, MockResponse::status(401).text("first"));
    s.api
        .on(Method::GET, API, MockResponse::status(401).text("second"));
    let http = BearerRetryClient::new(s.api.clone(), s.client.clone(), "svc");
    let response = http.execute(OutRequest::get(API)).await.unwrap();
    // The answer of the retry is returned as it is.
    assert_eq!(response.status(), 401);
    assert_eq!(response.bytes(), b"second");
    assert_eq!(s.api.request_count(), 2);
    assert_eq!(client_credentials_requests(&s.fake), 2);
}

#[tokio::test]
async fn bearer_retry_client_does_not_retry_a_non_401() {
    for status in [200u16, 403, 404, 500] {
        let s = setup();
        s.api.on(Method::GET, API, MockResponse::status(status));
        let http = BearerRetryClient::new(s.api.clone(), s.client.clone(), "svc");
        let response = http.execute(OutRequest::get(API)).await.unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(s.api.request_count(), 1, "{status}");
        assert_eq!(client_credentials_requests(&s.fake), 1, "{status}");
    }
}

#[tokio::test]
async fn bearer_retry_client_does_not_discard_a_token_another_caller_already_replaced() {
    let s = setup();
    let old = s.client.client_credentials("svc").await.unwrap();
    // Another caller replaced the token meanwhile.
    s.client.invalidate_token("svc", &old);
    let fresh = s.client.client_credentials("svc").await.unwrap();
    assert_eq!(client_credentials_requests(&s.fake), 2);
    // A 401 for the fresh token makes one refetch; a late one for `old` would not.
    s.client.invalidate_token("svc", &old);
    s.api.on(Method::GET, API, MockResponse::status(200));
    let http = BearerRetryClient::new(s.api.clone(), s.client.clone(), "svc");
    http.execute(OutRequest::get(API)).await.unwrap();
    assert_eq!(
        bearer(&s.api.requests()[0]),
        format!("Bearer {}", fresh.value.expose())
    );
    assert_eq!(client_credentials_requests(&s.fake), 2);
}

#[tokio::test]
async fn bearer_retry_client_passes_errors_through() {
    let s = setup();
    // No rule: the mock fails at the network level.
    let http = BearerRetryClient::new(s.api.clone(), s.client.clone(), "svc");
    assert_eq!(
        http.execute(OutRequest::get(API)).await.unwrap_err(),
        HttpClientError::Network
    );
    assert_eq!(s.api.request_count(), 1);
    // A token error stops the request before it is sent.
    let http = BearerRetryClient::new(s.api.clone(), s.client.clone(), "unknown");
    assert_eq!(
        http.execute(OutRequest::get(API)).await.unwrap_err(),
        HttpClientError::InvalidRequest
    );
    assert_eq!(s.api.request_count(), 1);
    // So does an invalid request, before a token is fetched.
    let http = BearerRetryClient::new(s.api.clone(), s.client.clone(), "svc");
    let before = s.fake.token_requests().len();
    assert_eq!(
        http.execute(OutRequest::get(API).header("bad name", "x"))
            .await
            .unwrap_err(),
        HttpClientError::InvalidRequest
    );
    assert_eq!(s.fake.token_requests().len(), before);
}

// ---------------------------------------------------------------------------------------------
// UserTokenInterceptor
// ---------------------------------------------------------------------------------------------

/// A request of a user logged in with `idp`, whose session holds tokens the fake issued.
async fn user_request(
    s: &Setup,
    lifetime: u64,
    authentication: Authentication,
) -> (Request, String) {
    let code = s.fake.issue_code(FakeUser::new("alice"));
    let body: Value = s
        .fake
        .client()
        .execute(
            OutRequest::post(format!("{ISSUER}/token")).body(Body::Form(vec![
                ("grant_type".into(), "authorization_code".into()),
                ("code".into(), code),
                ("client_id".into(), "app".into()),
            ])),
        )
        .await
        .unwrap()
        .json()
        .unwrap();
    let session = oxbloom_security::__test_support::session();
    let now = s
        .clock
        .now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    session
        .set_reserved(
            TOKENS_KEY,
            &json!({
                "v": 1,
                "access_token": body["access_token"],
                "token_type": "Bearer",
                "expires_at": now + lifetime,
                "refresh_token": body["refresh_token"],
                "id_token": body["id_token"],
                "scope": "openid",
            }),
        )
        .unwrap();
    let mut req = Request::new(HttpMethod::Get, "/orders");
    req.extensions_mut().insert::<Session>(session);
    req.extensions_mut()
        .insert(SecurityContext::authenticated(authentication));
    (req, body["access_token"].as_str().unwrap().to_owned())
}

fn oauth2_authentication(registration: &str) -> Authentication {
    let mut details = serde_json::Map::new();
    details.insert("oauth2.registration".into(), json!(registration));
    details.insert("oauth2.principal".into(), json!(true));
    Authentication::new("alice", []).with_details(details)
}

#[tokio::test]
async fn the_user_token_interceptor_uses_the_request_session() {
    let s = setup();
    s.api.on(Method::GET, API, MockResponse::status(200));
    let (req, access_token) = user_request(&s, 3600, oauth2_authentication("idp")).await;
    let http = InterceptedClient::new(s.api.clone()).with(Arc::new(
        UserTokenInterceptor::for_request(s.client.clone(), &req, "idp"),
    ));
    http.execute(OutRequest::get(API).header("authorization", "Basic eDp5"))
        .await
        .unwrap();
    let sent = s.api.requests().pop().unwrap();
    assert_eq!(bearer(&sent), format!("Bearer {access_token}"));
    assert_eq!(
        sent.headers().get_all(header::AUTHORIZATION).iter().count(),
        1
    );
}

#[tokio::test]
async fn the_user_token_interceptor_refreshes_an_expired_token_and_never_retries() {
    let s = setup();
    s.api.on(Method::GET, API, MockResponse::status(401));
    let (req, access_token) = user_request(&s, 0, oauth2_authentication("idp")).await;
    let interceptor = UserTokenInterceptor::for_request(s.client.clone(), &req, "idp");
    let http = InterceptedClient::new(s.api.clone()).with(Arc::new(interceptor));
    let response = http.execute(OutRequest::get(API)).await.unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(s.api.request_count(), 1);
    let sent = s.api.requests().pop().unwrap();
    assert_ne!(bearer(&sent), format!("Bearer {access_token}"));
    // The refreshed token is in the session of the request.
    let token = s.client.access_token_for(&req, "idp").await.unwrap();
    assert_eq!(bearer(&sent), format!("Bearer {}", token.value.expose()));
}

#[tokio::test]
async fn the_user_token_interceptor_refuses_another_authentication() {
    let s = setup();
    let (req, _) = user_request(&s, 3600, Authentication::new("alice", [])).await;
    let http = InterceptedClient::new(s.api.clone()).with(Arc::new(
        UserTokenInterceptor::for_request(s.client.clone(), &req, "idp"),
    ));
    assert_eq!(
        http.execute(OutRequest::get(API)).await.unwrap_err(),
        HttpClientError::InvalidRequest
    );
    assert_eq!(s.api.request_count(), 0);
    // A request without a session or an authentication.
    let bare = Request::new(HttpMethod::Get, "/");
    let mut out = OutRequest::get(API);
    assert_eq!(
        UserTokenInterceptor::for_request(s.client.clone(), &bare, "idp")
            .before(&mut out)
            .await
            .unwrap_err(),
        HttpClientError::InvalidRequest
    );
    assert!(out.headers().get(header::AUTHORIZATION).is_none());
}

// ---------------------------------------------------------------------------------------------
// Debug
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_interceptors_show_only_the_registration_id_in_debug() {
    let s = setup();
    let token = s.client.client_credentials("svc").await.unwrap();
    let (req, access_token) = user_request(&s, 3600, oauth2_authentication("idp")).await;
    let session_id = oxbloom_security::RequestSessionExt::session(&req)
        .unwrap()
        .id();
    let texts = [
        format!(
            "{:?}",
            ClientCredentialsInterceptor::new(s.client.clone(), "svc")
        ),
        format!(
            "{:?}",
            BearerRetryClient::new(s.api.clone(), s.client.clone(), "svc")
        ),
        format!(
            "{:?}",
            UserTokenInterceptor::for_request(s.client.clone(), &req, "idp")
        ),
    ];
    for text in &texts {
        assert!(text.contains("registration"), "{text}");
        for secret in [
            token.value.expose().as_str(),
            access_token.as_str(),
            session_id.as_str(),
            SECRET,
            "alice",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
    }
    assert!(texts[2].contains("\"idp\""), "{}", texts[2]);
}

#[test]
fn the_interceptors_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ClientCredentialsInterceptor>();
    assert_send_sync::<BearerRetryClient>();
    assert_send_sync::<UserTokenInterceptor>();
}
