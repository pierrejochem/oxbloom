//! `OAuth2Client::access_token_for`: the user's access token from the session, the refresh
//! with the refresh token grant, the check of the current authentication, and the per-session
//! serialization of refreshes. Against `FakeProvider` and a `ManualClock`.

#![cfg(feature = "oauth2")]

use std::future::Future;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use http::{Method, header};
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_http_client::{
    Body, HttpClient, HttpClientError, Request as OutRequest, Response as OutResponse,
};
use oxbloom_security::__test_support::UserInfo;
use oxbloom_security::{
    AccessToken, Authentication, ClientAuthentication, ClientRegistration, HttpSecurity,
    InMemorySessionStore, OAuth2Client, OAuth2Login, OAuth2Settings, ProviderConfig,
    RequestSessionExt, SecurityContext, Session, SessionBackend, SessionFilter, SessionSettings,
    TokenError,
};
use oxbloom_test::{FakeProvider, FakeUser, Fault, ManualClock};
use oxbloom_web::{App, HttpMethod, Request, Route};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tracing_subscriber::fmt::MakeWriter;

const ISSUER: &str = "https://idp.example.test";
const SECRET: &str = "the-client-secret-0123456789";
const TOKENS_KEY: &str = "oxbloom.security.oauth2.tokens.idp";

fn start_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn epoch(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Waits for `future`, failing the test instead of hanging when a regression blocks it.
async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("the test did not finish within 10 seconds")
}

fn registration(id: &str, auth: ClientAuthentication) -> ClientRegistration {
    ClientRegistration {
        id: id.into(),
        client_id: "app".into(),
        client_secret: (auth != ClientAuthentication::None).then(|| Secret::new(SECRET.into())),
        scopes: vec!["openid".into(), "email".into()],
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

/// While armed, holds every refresh request at a gate: it signals `entered` and waits for
/// `release`, so the order of events in a test is deterministic.
struct Gate {
    inner: Arc<dyn HttpClient>,
    armed: Arc<AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl HttpClient for Gate {
    async fn execute(&self, request: OutRequest) -> Result<OutResponse, HttpClientError> {
        let refresh = matches!(request.body_ref(), Body::Form(fields)
            if fields.iter().any(|(k, v)| k == "grant_type" && v == "refresh_token"));
        if refresh && self.armed.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        self.inner.execute(request).await
    }
}

#[derive(Clone)]
struct GateHandles {
    armed: Arc<AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

fn gate(inner: Arc<dyn HttpClient>) -> (Arc<dyn HttpClient>, GateHandles) {
    let handles = GateHandles {
        armed: Arc::new(AtomicBool::new(false)),
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let gate = Gate {
        inner,
        armed: handles.armed.clone(),
        entered: handles.entered.clone(),
        release: handles.release.clone(),
    };
    (Arc::new(gate), handles)
}

/// A client whose every request fails at the network level.
struct Down;

#[async_trait]
impl HttpClient for Down {
    async fn execute(&self, _request: OutRequest) -> Result<OutResponse, HttpClientError> {
        Err(HttpClientError::Network)
    }
}

struct Setup {
    fake: FakeProvider,
    clock: ManualClock,
    client: Arc<OAuth2Client>,
    gate: GateHandles,
}

fn setup_with(auth: ClientAuthentication) -> Setup {
    let clock = ManualClock::at(start_time());
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
    let (http, gate) = gate(fake.client());
    let client = OAuth2Client::new(
        settings(vec![registration("idp", auth), registration("other", auth)]),
        http,
    )
    .clock(Arc::new(clock.clone()));
    Setup {
        fake,
        clock,
        client: Arc::new(client),
        gate,
    }
}

fn setup() -> Setup {
    setup_with(ClientAuthentication::Basic)
}

/// The tokens a login stores: an authorization code exchange at the fake, with the lifetime
/// the fake reports.
struct Login {
    access_token: String,
    refresh_token: String,
    id_token: String,
}

impl Setup {
    /// Lets the fake issue tokens for `user`, as the login callback would.
    async fn login_tokens(&self, user: FakeUser) -> Login {
        let code = self.fake.issue_code(user);
        let request = OutRequest::post(format!("{ISSUER}/token")).body(Body::Form(vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code),
            ("client_id".into(), "app".into()),
        ]));
        let body: Value = self
            .fake
            .client()
            .execute(request)
            .await
            .unwrap()
            .json()
            .unwrap();
        Login {
            access_token: body["access_token"].as_str().unwrap().to_owned(),
            refresh_token: body["refresh_token"].as_str().unwrap().to_owned(),
            id_token: body["id_token"].as_str().unwrap().to_owned(),
        }
    }

    /// A session holding the tokens of a login of `user` for `idp`, valid for `lifetime`
    /// seconds from now.
    async fn session_with_tokens(&self, user: FakeUser, lifetime: u64) -> (Session, Login) {
        let login = self.login_tokens(user).await;
        let session = oxbloom_security::__test_support::session();
        session
            .set_reserved(
                TOKENS_KEY,
                &json!({
                    "v": 1,
                    "access_token": login.access_token,
                    "token_type": "Bearer",
                    "expires_at": epoch(self.clock.now()) + lifetime,
                    "refresh_token": login.refresh_token,
                    "id_token": login.id_token,
                    "scope": "openid email",
                }),
            )
            .unwrap();
        (session, login)
    }

    fn refresh_requests(&self) -> usize {
        self.fake
            .token_requests()
            .iter()
            .filter(|r| r.field("grant_type") == Some("refresh_token"))
            .count()
    }
}

fn oauth2_authentication(principal: &str, registration: &str) -> Authentication {
    let mut details = serde_json::Map::new();
    details.insert("oauth2.registration".into(), json!(registration));
    details.insert("oauth2.principal".into(), json!(true));
    Authentication::new(principal, []).with_details(details)
}

fn request(session: Option<&Session>, authentication: Option<Authentication>) -> Request {
    let mut req = Request::new(HttpMethod::Get, "/orders");
    if let Some(session) = session {
        req.extensions_mut().insert(session.clone());
    }
    if let Some(authentication) = authentication {
        req.extensions_mut()
            .insert(SecurityContext::authenticated(authentication));
    }
    req
}

fn alice_request(session: &Session) -> Request {
    request(Some(session), Some(oauth2_authentication("alice", "idp")))
}

fn stored(session: &Session) -> Value {
    session
        .get::<Value>(TOKENS_KEY)
        .unwrap()
        .unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------------------------
// The stored token and the refresh
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_valid_stored_token_is_returned_without_a_request() {
    let s = setup();
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 3600).await;
    let before = s.fake.token_requests().len();
    let token = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_eq!(token.value.expose(), &login.access_token);
    assert_eq!(token.expires_at, start_time() + Duration::from_secs(3600));
    assert_eq!(s.fake.token_requests().len(), before);
}

#[tokio::test]
async fn an_expired_token_is_refreshed_and_the_session_updated() {
    let s = setup();
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 100).await;
    // The default skew is 30 seconds: at 70 seconds the token is due.
    s.clock.advance(Duration::from_secs(69));
    let same = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_eq!(same.value.expose(), &login.access_token);
    assert_eq!(s.refresh_requests(), 0);

    s.clock.advance(Duration::from_secs(1));
    let token = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_ne!(token.value.expose(), &login.access_token);
    assert_eq!(s.refresh_requests(), 1);
    let refresh = s.fake.token_requests().last().unwrap().clone();
    assert_eq!(
        refresh.field("refresh_token"),
        Some(login.refresh_token.as_str())
    );
    // No scope parameter: the refresh asks for the scope of the login.
    assert_eq!(refresh.field("scope"), None);
    assert!(refresh.authorization().unwrap().starts_with("Basic "));

    let now = s.clock.now();
    let value = stored(&session);
    assert_eq!(value["v"], 1);
    assert_eq!(value["access_token"], token.value.expose().as_str());
    assert_eq!(value["token_type"], "Bearer");
    assert_eq!(value["expires_at"], epoch(now) + 3600);
    assert_eq!(value["id_token"], login.id_token.as_str());
    assert_eq!(value["scope"], "openid email");
    assert_eq!(token.expires_at, now + Duration::from_secs(3600));

    // The next call uses the stored new token.
    let again = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_eq!(again.value.expose(), token.value.expose());
    assert_eq!(s.refresh_requests(), 1);
}

#[tokio::test]
async fn a_rotated_refresh_token_replaces_the_old_one() {
    let s = setup();
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    s.client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    let rotated = stored(&session)["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(rotated, login.refresh_token);

    // The next refresh uses the rotated token, which the fake accepts (the old one it would
    // refuse).
    s.clock.advance(Duration::from_secs(3600));
    s.client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    let last = s.fake.token_requests().last().unwrap().clone();
    assert_eq!(last.field("refresh_token"), Some(rotated.as_str()));
    assert_eq!(s.refresh_requests(), 2);
}

#[tokio::test]
async fn a_missing_refresh_token_in_the_answer_keeps_the_old_one() {
    let s = setup();
    s.fake.rotate_refresh_tokens(false);
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let token = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_ne!(token.value.expose(), &login.access_token);
    let value = stored(&session);
    assert_eq!(value["refresh_token"], login.refresh_token.as_str());
    assert_eq!(value["id_token"], login.id_token.as_str());
}

#[tokio::test]
async fn invalid_grant_removes_the_tokens_and_requires_reauthentication() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    s.fake.misbehave(Fault::TokenEndpointError("invalid_grant"));
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    assert_eq!(stored(&session), Value::Null);
    // Nothing is left to refresh with, so no further request is made.
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    assert_eq!(s.refresh_requests(), 1);
}

#[tokio::test]
async fn a_refresh_token_the_provider_no_longer_knows_requires_reauthentication() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let mut value = stored(&session);
    value["refresh_token"] = json!("a-refresh-token-the-fake-never-issued");
    session.set_reserved(TOKENS_KEY, &value).unwrap();
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    assert_eq!(stored(&session), Value::Null);
}

#[tokio::test]
async fn another_refresh_error_keeps_the_tokens() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let before = stored(&session);
    s.fake
        .misbehave(Fault::TokenEndpointError("invalid_client"));
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::Provider("invalid_client")
    );
    assert_eq!(stored(&session), before);

    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let before = stored(&session);
    s.fake.misbehave(Fault::TokenEndpointStatus(503));
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::Invalid
    );
    assert_eq!(stored(&session), before);

    // The provider cannot be reached at all.
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let before = stored(&session);
    let down = OAuth2Client::new(
        settings(vec![registration("idp", ClientAuthentication::Basic)]),
        Arc::new(Down),
    )
    .clock(Arc::new(s.clock.clone()));
    assert_eq!(
        down.access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::Http
    );
    assert_eq!(stored(&session), before);
}

#[tokio::test]
async fn no_stored_tokens_requires_reauthentication() {
    let s = setup();
    let session = oxbloom_security::__test_support::session();
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    // Tokens of another registration do not count.
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 3600).await;
    assert_eq!(
        s.client
            .access_token_for(
                &request(
                    Some(&session),
                    Some(oauth2_authentication("alice", "other"))
                ),
                "other"
            )
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
}

#[tokio::test]
async fn no_session_requires_reauthentication() {
    let s = setup();
    let req = request(None, Some(oauth2_authentication("alice", "idp")));
    assert_eq!(
        s.client.access_token_for(&req, "idp").await.unwrap_err(),
        TokenError::ReauthenticationRequired
    );
}

#[tokio::test]
async fn no_refresh_token_on_an_expired_token_requires_reauthentication() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let mut value = stored(&session);
    value["refresh_token"] = Value::Null;
    session.set_reserved(TOKENS_KEY, &value).unwrap();
    let before = s.fake.token_requests().len();
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    assert_eq!(s.fake.token_requests().len(), before);
    // The expired token cannot be used for anything, so it is removed.
    assert_eq!(stored(&session), Value::Null);
}

#[tokio::test]
async fn an_unknown_registration_is_an_error() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 3600).await;
    let req = request(Some(&session), Some(oauth2_authentication("alice", "nope")));
    assert_eq!(
        s.client.access_token_for(&req, "nope").await.unwrap_err(),
        TokenError::UnknownRegistration
    );
}

#[tokio::test]
async fn a_token_stored_without_an_expiry_is_used_as_is() {
    let s = setup();
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let mut value = stored(&session);
    value["expires_at"] = Value::Null;
    session.set_reserved(TOKENS_KEY, &value).unwrap();
    s.clock.advance(Duration::from_secs(86_400));
    let token = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_eq!(token.value.expose(), &login.access_token);
    assert_eq!(token.expires_at, s.clock.now() + Duration::from_secs(60));
    assert_eq!(s.refresh_requests(), 0);
}

#[tokio::test]
async fn a_missing_expires_in_on_refresh_means_60_seconds() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    s.fake.access_token_lifetime(None);
    let token = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    let now = s.clock.now();
    assert_eq!(token.expires_at, now + Duration::from_secs(60));
    assert_eq!(stored(&session)["expires_at"], epoch(now) + 60);
}

#[tokio::test]
async fn expires_in_zero_on_refresh_returns_the_token_once_then_refreshes_again() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    s.fake.access_token_lifetime(Some(0));
    let first = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    // One call makes one request, even though the new token is already due.
    assert_eq!(s.refresh_requests(), 1);
    assert_eq!(stored(&session)["expires_at"], epoch(s.clock.now()));
    let second = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_ne!(first.value.expose(), second.value.expose());
    assert_eq!(s.refresh_requests(), 2);
}

#[tokio::test]
async fn a_public_client_refreshes_with_its_client_id_only() {
    let s = setup_with(ClientAuthentication::None);
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    s.client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    let refresh = s.fake.token_requests().last().unwrap().clone();
    assert_eq!(refresh.field("client_id"), Some("app"));
    assert_eq!(refresh.field("client_secret"), None);
    assert_eq!(refresh.authorization(), None);
}

#[tokio::test]
async fn a_post_client_sends_its_secret_in_the_form() {
    let s = setup_with(ClientAuthentication::Post);
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    s.client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    let refresh = s.fake.token_requests().last().unwrap().clone();
    assert_eq!(refresh.field("client_secret"), Some(SECRET));
    assert_eq!(refresh.authorization(), None);
}

#[tokio::test]
async fn the_refresh_request_has_the_provider_timeout() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    s.client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    let refresh = s
        .fake
        .client()
        .requests()
        .into_iter()
        .rfind(|r| r.method() == Method::POST)
        .unwrap();
    assert_eq!(refresh.timeout_value(), Some(Duration::from_secs(10)));
}

// ---------------------------------------------------------------------------------------------
// The current authentication must be the OAuth2 login of the registration
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_password_authentication_does_not_get_the_tokens_of_the_session() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let before = stored(&session);
    let requests = s.fake.token_requests().len();
    let password = Authentication::new("alice", []);
    let req = request(Some(&session), Some(password));
    assert_eq!(
        s.client.access_token_for(&req, "idp").await.unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    // A valid token is not handed out either.
    let (valid, _) = s.session_with_tokens(FakeUser::new("alice"), 3600).await;
    let req = request(Some(&valid), Some(Authentication::new("alice", [])));
    assert_eq!(
        s.client.access_token_for(&req, "idp").await.unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    // Neither the tokens nor the provider were touched.
    assert_eq!(stored(&session), before);
    assert_eq!(s.fake.token_requests().len(), requests + 1);
}

#[tokio::test]
async fn an_oauth2_authentication_of_another_registration_does_not_get_the_tokens() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 3600).await;
    let req = request(
        Some(&session),
        Some(oauth2_authentication("alice", "other")),
    );
    assert_eq!(
        s.client.access_token_for(&req, "idp").await.unwrap_err(),
        TokenError::ReauthenticationRequired
    );
}

#[tokio::test]
async fn an_anonymous_or_forged_authentication_does_not_get_the_tokens() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 3600).await;
    // No authentication at all.
    assert_eq!(
        s.client
            .access_token_for(&request(Some(&session), None), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    // The registration detail alone, without the OAuth2 principal marker, or with a marker
    // that is not the boolean `true`.
    let mut details = serde_json::Map::new();
    details.insert("oauth2.registration".into(), json!("idp"));
    let partial = Authentication::new("alice", []).with_details(details.clone());
    details.insert("oauth2.principal".into(), json!("true"));
    let stringly = Authentication::new("alice", []).with_details(details);
    for authentication in [partial, stringly] {
        assert_eq!(
            s.client
                .access_token_for(&request(Some(&session), Some(authentication)), "idp")
                .await
                .unwrap_err(),
            TokenError::ReauthenticationRequired
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------------------------

/// Waits until `count` calls are queued behind a refresh in flight.
async fn wait_for_refresh_waiters(client: &OAuth2Client, count: usize) {
    while client.__refresh_waiters() < count {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_refreshes_for_one_session_send_one_request() {
    bounded(async {
        let s = setup();
        let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
        s.gate.armed.store(true, Ordering::SeqCst);
        let first = {
            let client = s.client.clone();
            let req = alice_request(&session);
            tokio::spawn(async move { client.access_token_for(&req, "idp").await })
        };
        s.gate.entered.notified().await;
        let second = {
            let client = s.client.clone();
            let req = alice_request(&session);
            tokio::spawn(async move { client.access_token_for(&req, "idp").await })
        };
        wait_for_refresh_waiters(&s.client, 1).await;
        // Disarmed first, so a second refresh (a regression) fails instead of waiting.
        s.gate.armed.store(false, Ordering::SeqCst);
        s.gate.release.notify_one();
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        // The second call saw the rotated token and did not use the spent refresh token.
        assert_eq!(first.value.expose(), second.value.expose());
        assert_ne!(first.value.expose(), &login.access_token);
        assert_eq!(s.refresh_requests(), 1);
        assert_eq!(s.client.__refresh_locks(), 0);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_refreshes_of_two_sessions_each_get_their_own_tokens() {
    bounded(async {
        let s = setup();
        let (alice_session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
        let (bob_session, _) = s.session_with_tokens(FakeUser::new("bob"), 0).await;
        let alice = {
            let client = s.client.clone();
            let req = alice_request(&alice_session);
            tokio::spawn(async move { client.access_token_for(&req, "idp").await })
        };
        let bob = {
            let client = s.client.clone();
            let req = request(
                Some(&bob_session),
                Some(oauth2_authentication("bob", "idp")),
            );
            tokio::spawn(async move { client.access_token_for(&req, "idp").await })
        };
        let alice = alice.await.unwrap().unwrap();
        let bob = bob.await.unwrap().unwrap();
        assert_eq!(s.refresh_requests(), 2);
        // Each token belongs to its own user at the provider.
        for (token, sub) in [(alice, "alice"), (bob, "bob")] {
            let attributes = UserInfo
                .fetch(
                    &format!("{ISSUER}/userinfo"),
                    &token.value,
                    s.fake.client().as_ref(),
                )
                .await
                .unwrap();
            assert_eq!(attributes["sub"], sub);
        }
    })
    .await;
}

/// Another copy of the tokens of `session`, under a different session id: what a second
/// request of the same browser holds with the cookie session backend, or a stale copy that a
/// server store writes back.
fn copy_of(session: &Session) -> Session {
    let copy = oxbloom_security::__test_support::session();
    copy.set_reserved(TOKENS_KEY, &stored(session)).unwrap();
    assert_ne!(copy.id(), session.id());
    copy
}

fn refresh_token_of(session: &Session) -> String {
    stored(session)["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_dropped_mid_refresh_still_leaves_a_record_that_the_next_request_adopts() {
    bounded(async {
        let s = setup();
        let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
        s.gate.armed.store(true, Ordering::SeqCst);
        let call = {
            let client = s.client.clone();
            let req = alice_request(&session);
            tokio::spawn(async move { client.access_token_for(&req, "idp").await })
        };
        s.gate.entered.notified().await;
        // The handler is dropped (the client disconnected) while the provider call is in
        // flight; the refresh token is spent at the provider anyway.
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        s.gate.armed.store(false, Ordering::SeqCst);
        s.gate.release.notify_one();
        while s.client.__rotation_records() < 1 {
            tokio::task::yield_now().await;
        }
        while s.client.__refresh_locks() > 0 {
            tokio::task::yield_now().await;
        }
        // The session still holds the spent refresh token; the next request adopts the
        // recorded tokens without asking the provider.
        assert_eq!(refresh_token_of(&session), login.refresh_token);
        let token = s
            .client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap();
        assert_ne!(token.value.expose(), &login.access_token);
        assert_eq!(s.refresh_requests(), 1);
        assert_ne!(refresh_token_of(&session), login.refresh_token);
        assert_eq!(
            stored(&session)["access_token"],
            token.value.expose().as_str()
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_requests_with_different_session_ids_and_the_same_refresh_token_refresh_once() {
    bounded(async {
        let s = setup();
        let (first_session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
        let second_session = copy_of(&first_session);
        s.gate.armed.store(true, Ordering::SeqCst);
        let first = {
            let client = s.client.clone();
            let req = alice_request(&first_session);
            tokio::spawn(async move { client.access_token_for(&req, "idp").await })
        };
        s.gate.entered.notified().await;
        let second = {
            let client = s.client.clone();
            let req = alice_request(&second_session);
            tokio::spawn(async move { client.access_token_for(&req, "idp").await })
        };
        wait_for_refresh_waiters(&s.client, 1).await;
        s.gate.armed.store(false, Ordering::SeqCst);
        s.gate.release.notify_one();
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first.value.expose(), second.value.expose());
        assert_eq!(s.refresh_requests(), 1);
        assert_eq!(stored(&first_session), stored(&second_session));
    })
    .await;
}

#[tokio::test]
async fn a_stale_session_copy_with_a_spent_token_adopts_the_rotated_one() {
    let s = setup();
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let stale = copy_of(&session);
    let token = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_eq!(s.refresh_requests(), 1);
    // Later, a copy written back with the spent refresh token: no provider call.
    let adopted = s
        .client
        .access_token_for(&alice_request(&stale), "idp")
        .await
        .unwrap();
    assert_eq!(adopted.value.expose(), token.value.expose());
    assert_eq!(s.refresh_requests(), 1);
    assert_ne!(refresh_token_of(&stale), login.refresh_token);
    assert_eq!(stored(&stale), stored(&session));
}

#[tokio::test]
async fn invalid_grant_for_a_token_with_a_rotation_record_adopts_instead_of_removing() {
    let s = setup();
    s.fake.access_token_lifetime(Some(0));
    let (driver, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let stale = copy_of(&driver);
    // Five rotations whose access tokens are all due at once: R1 -> R2 -> ... -> R6.
    let mut last = None;
    for _ in 0..5 {
        last = Some(
            s.client
                .access_token_for(&alice_request(&driver), "idp")
                .await
                .unwrap(),
        );
    }
    assert_eq!(s.refresh_requests(), 5);
    assert_eq!(s.client.__rotation_records(), 5);
    // The stale copy follows four records, then spends R5 at the provider, which refuses it
    // as spent; the record of R5 turns the refusal into an adoption of the newest tokens.
    let adopted = s
        .client
        .access_token_for(&alice_request(&stale), "idp")
        .await
        .unwrap();
    assert_eq!(s.refresh_requests(), 6);
    assert_eq!(
        s.fake
            .token_requests()
            .last()
            .unwrap()
            .field("refresh_token"),
        Some(s.fake.token_requests()[5].field("refresh_token").unwrap())
    );
    assert_eq!(adopted.value.expose(), last.unwrap().value.expose());
    assert_eq!(stored(&stale), stored(&driver));
}

#[tokio::test]
async fn invalid_grant_for_a_token_without_a_record_removes_the_tokens() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let mut value = stored(&session);
    value["refresh_token"] = json!("revoked-at-the-provider");
    session.set_reserved(TOKENS_KEY, &value).unwrap();
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&session), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    assert_eq!(stored(&session), Value::Null);
    assert_eq!(s.client.__rotation_records(), 0);
}

#[tokio::test]
async fn a_rotation_record_expires_after_ten_minutes() {
    let s = setup();
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let stale = copy_of(&session);
    s.client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    s.clock.advance(Duration::from_secs(599));
    assert_eq!(s.client.__rotation_records(), 1);
    s.clock.advance(Duration::from_secs(1));
    assert_eq!(s.client.__rotation_records(), 0);
    // The very old spent token now goes to the provider, which refuses it.
    assert_eq!(
        s.client
            .access_token_for(&alice_request(&stale), "idp")
            .await
            .unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    assert_eq!(s.refresh_requests(), 2);
    assert_eq!(stored(&stale), Value::Null);
}

#[tokio::test]
async fn the_rotation_record_keeps_at_most_256_entries() {
    let s = setup();
    let mut sessions = Vec::new();
    for i in 0..257 {
        let (session, _) = s
            .session_with_tokens(FakeUser::new(&format!("user-{i}")), 0)
            .await;
        sessions.push((copy_of(&session), i));
        let req = request(
            Some(&session),
            Some(oauth2_authentication(&format!("user-{i}"), "idp")),
        );
        s.client.access_token_for(&req, "idp").await.unwrap();
    }
    assert_eq!(s.refresh_requests(), 257);
    assert_eq!(s.client.__rotation_records(), 256);
    // The least recently used record, the first one, is gone; the last one is kept.
    let (newest, i) = &sessions[256];
    let req = request(
        Some(newest),
        Some(oauth2_authentication(&format!("user-{i}"), "idp")),
    );
    s.client.access_token_for(&req, "idp").await.unwrap();
    assert_eq!(s.refresh_requests(), 257);
    let (oldest, i) = &sessions[0];
    let req = request(
        Some(oldest),
        Some(oauth2_authentication(&format!("user-{i}"), "idp")),
    );
    assert_eq!(
        s.client.access_token_for(&req, "idp").await.unwrap_err(),
        TokenError::ReauthenticationRequired
    );
    assert_eq!(s.refresh_requests(), 258);
}

#[tokio::test]
async fn a_chain_of_two_rotations_is_followed() {
    let s = setup();
    s.fake.access_token_lifetime(Some(100));
    let (session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let stale = copy_of(&session);
    s.client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    s.clock.advance(Duration::from_secs(70));
    let second = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    assert_eq!(s.refresh_requests(), 2);
    // The stale copy holds R1: R1 -> T1 (due) -> R2 -> T2 (usable).
    let adopted = s
        .client
        .access_token_for(&alice_request(&stale), "idp")
        .await
        .unwrap();
    assert_eq!(adopted.value.expose(), second.value.expose());
    assert_eq!(s.refresh_requests(), 2);
    assert_eq!(stored(&stale), stored(&session));
}

#[tokio::test]
async fn the_records_of_one_registration_do_not_serve_another() {
    let s = setup();
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let renewed = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    // A session of `other` that holds the same (spent) refresh token string.
    let other = oxbloom_security::__test_support::session();
    let mut value = stored(&session);
    value["refresh_token"] = json!(login.refresh_token);
    value["expires_at"] = json!(0);
    other
        .set_reserved("oxbloom.security.oauth2.tokens.other", &value)
        .unwrap();
    let req = request(Some(&other), Some(oauth2_authentication("alice", "other")));
    let result = s.client.access_token_for(&req, "other").await;
    assert_eq!(result.unwrap_err(), TokenError::ReauthenticationRequired);
    // The provider was asked, so the record of `idp` was not used.
    assert_eq!(s.refresh_requests(), 2);
    let _ = renewed;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_refresh_is_given_to_the_callers_that_waited_for_it() {
    bounded(async {
        for (fault, expected) in [
            (Fault::TokenEndpointStatus(503), TokenError::Invalid),
            (
                Fault::TokenEndpointError("invalid_grant"),
                TokenError::ReauthenticationRequired,
            ),
        ] {
            let s = setup();
            let (first_session, _) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
            let second_session = copy_of(&first_session);
            let before = stored(&first_session);
            s.fake.misbehave(fault);
            s.gate.armed.store(true, Ordering::SeqCst);
            let first = {
                let client = s.client.clone();
                let req = alice_request(&first_session);
                tokio::spawn(async move { client.access_token_for(&req, "idp").await })
            };
            s.gate.entered.notified().await;
            let second = {
                let client = s.client.clone();
                let req = alice_request(&second_session);
                tokio::spawn(async move { client.access_token_for(&req, "idp").await })
            };
            wait_for_refresh_waiters(&s.client, 1).await;
            s.gate.armed.store(false, Ordering::SeqCst);
            s.gate.release.notify_one();
            assert_eq!(first.await.unwrap().unwrap_err(), expected);
            assert_eq!(second.await.unwrap().unwrap_err(), expected);
            assert_eq!(s.refresh_requests(), 1, "{fault:?}");
            if expected == TokenError::ReauthenticationRequired {
                assert_eq!(stored(&first_session), Value::Null);
                assert_eq!(stored(&second_session), Value::Null);
            } else {
                assert_eq!(stored(&first_session), before);
                assert_eq!(stored(&second_session), before);
            }
        }
    })
    .await;
}

#[tokio::test]
async fn the_lock_map_holds_no_entry_after_the_calls() {
    let s = setup();
    for i in 0..20 {
        let (session, _) = s
            .session_with_tokens(FakeUser::new(&format!("user-{i}")), 0)
            .await;
        let req = request(
            Some(&session),
            Some(oauth2_authentication(&format!("user-{i}"), "idp")),
        );
        s.client.access_token_for(&req, "idp").await.unwrap();
    }
    assert_eq!(s.refresh_requests(), 20);
    assert_eq!(s.client.__refresh_locks(), 0);
}

// ---------------------------------------------------------------------------------------------
// Two concurrent requests of one session, through the session filter: each request has its
// own session handle, loaded from the store.
// ---------------------------------------------------------------------------------------------

const COOKIE: &str = "OXBLOOM_SESSION";
const HOST: &str = "app.example.test";
const BASE: &str = "https://app.example.test";

struct Env {
    app: App,
    clock: ManualClock,
    fake: FakeProvider,
    gate: GateHandles,
    client: Arc<OAuth2Client>,
}

fn env() -> Env {
    let clock = ManualClock::at(start_time());
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
    fake.access_token_lifetime(Some(100));
    let (http, gate) = gate(fake.client());
    let mut login_registration = registration("idp", ClientAuthentication::Basic);
    login_registration.redirect_uri = "{base-url}/login/oauth2/code/{id}".into();
    let client = Arc::new(
        OAuth2Client::new(
            settings(vec![registration("idp", ClientAuthentication::Basic)]),
            http,
        )
        .clock(Arc::new(clock.clone())),
    );
    let session_settings = SessionSettings::default();
    let store = Arc::new(InMemorySessionStore::new(
        &session_settings,
        Arc::new(clock.clone()),
    ));
    let filter = SessionFilter::new(SessionBackend::Server(store), session_settings)
        .unwrap()
        .with_clock(Arc::new(clock.clone()));
    let login_client: Arc<dyn HttpClient> = fake.client();
    let chain = HttpSecurity::new()
        .form_login(|f| f)
        .clock(Arc::new(clock.clone()))
        .oauth2_login(|_| {
            OAuth2Login::new(login_client)
                .registration(login_registration)
                .provider(
                    "idp",
                    ProviderConfig {
                        issuer_uri: Some(ISSUER.into()),
                        ..ProviderConfig::default()
                    },
                )
                .base_url(BASE)
        })
        .authorize(|a| a.get("/token").permit_all().any_request().authenticated())
        .build()
        .unwrap();
    let handler_client = client.clone();
    let app = App::new()
        .filter(filter)
        .filter(chain)
        .route(Route::new(
            HttpMethod::Get,
            "/token",
            move |req: Request| {
                let client = handler_client.clone();
                async move {
                    match client.access_token_for(&req, "idp").await {
                        Ok(token) => token.value.expose().clone(),
                        Err(error) => format!("error: {error}"),
                    }
                }
            },
        ))
        .unwrap()
        .route(Route::new(
            HttpMethod::Get,
            "/stored",
            |req: Request| async move {
                req.session()
                    .and_then(|s| s.get::<Value>(TOKENS_KEY).unwrap())
                    .unwrap_or(Value::Null)
                    .to_string()
            },
        ))
        .unwrap();
    Env {
        app,
        clock,
        fake,
        gate,
        client,
    }
}

/// One browser with its own session cookie.
struct Browser<'a> {
    env: &'a Env,
    jar: Mutex<Option<String>>,
}

impl<'a> Browser<'a> {
    fn new(env: &'a Env) -> Self {
        Browser {
            env,
            jar: Mutex::new(None),
        }
    }

    fn request(&self, target: &str) -> Request {
        let (path, query) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query.to_owned())),
            None => (target, None),
        };
        let mut req = Request::new(HttpMethod::Get, path);
        req.query = query;
        req.headers.insert(header::HOST, HOST.parse().unwrap());
        req.headers
            .insert(header::ACCEPT, "text/html".parse().unwrap());
        if let Some(value) = self.jar.lock().unwrap().clone() {
            req.headers
                .insert(header::COOKIE, format!("{COOKIE}={value}").parse().unwrap());
        }
        req
    }

    async fn get(&self, target: &str) -> oxbloom_web::Response {
        let res = self.env.app.handle(self.request(target)).await;
        for value in res.headers().get_all(header::SET_COOKIE) {
            let text = value.to_str().unwrap();
            if let Some(rest) = text.strip_prefix(&format!("{COOKIE}=")) {
                let mut parts = rest.split("; ");
                let id = parts.next().unwrap().to_owned();
                let expired = parts.any(|p| p == "Max-Age=0");
                *self.jar.lock().unwrap() = (!expired).then_some(id);
            }
        }
        res
    }

    async fn text(&self, target: &str) -> String {
        String::from_utf8_lossy(self.get(target).await.body()).into_owned()
    }

    async fn log_in(&self, user: FakeUser) {
        let res = self.get("/oauth2/authorization/idp").await;
        assert_eq!(res.status(), 302);
        let location = res.headers()[header::LOCATION].to_str().unwrap().to_owned();
        let param = |name: &str| {
            location
                .split(['?', '&'])
                .find_map(|p| p.strip_prefix(&format!("{name}=")))
                .unwrap()
                .to_owned()
        };
        self.env.fake.expect_nonce(&param("nonce"));
        let code = self.env.fake.issue_code(user);
        let res = self
            .get(&format!(
                "/login/oauth2/code/idp?code={code}&state={}",
                param("state")
            ))
            .await;
        assert_eq!(res.status(), 302);
        assert_ne!(
            res.headers()[header::LOCATION].to_str().unwrap(),
            "/login?error"
        );
    }
}

#[tokio::test]
async fn the_tokens_of_a_login_are_returned_and_refreshed() {
    let env = env();
    let b = Browser::new(&env);
    b.log_in(FakeUser::new("alice").scope("openid email")).await;
    let stored: Value = serde_json::from_str(&b.text("/stored").await).unwrap();
    let first = b.text("/token").await;
    assert_eq!(first, stored["access_token"].as_str().unwrap());

    env.clock.advance(Duration::from_secs(70));
    let renewed = b.text("/token").await;
    assert_ne!(renewed, first);
    assert!(!renewed.starts_with("error"), "{renewed}");
    // The session filter saved the new tokens: the next request sees them.
    let stored: Value = serde_json::from_str(&b.text("/stored").await).unwrap();
    assert_eq!(stored["access_token"], renewed.as_str());
    assert_eq!(b.text("/token").await, renewed);
}

#[tokio::test]
async fn an_anonymous_request_gets_no_token() {
    let env = env();
    let b = Browser::new(&env);
    assert_eq!(
        b.text("/token").await,
        format!("error: {}", TokenError::ReauthenticationRequired)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_concurrent_requests_of_one_session_refresh_once() {
    bounded(async {
        let env = env();
        let b = Browser::new(&env);
        b.log_in(FakeUser::new("alice")).await;
        env.clock.advance(Duration::from_secs(70));
        env.gate.armed.store(true, Ordering::SeqCst);
        // Both requests carry the same cookie and load their own copy of the session.
        let first = env.app.handle(b.request("/token"));
        let second = env.app.handle(b.request("/token"));
        let control = async {
            env.gate.entered.notified().await;
            wait_for_refresh_waiters(&env.client, 1).await;
            // Disarmed first, so a second refresh (a regression) fails instead of waiting.
            env.gate.armed.store(false, Ordering::SeqCst);
            env.gate.release.notify_one();
        };
        let (first, second, ()) = tokio::join!(first, second, control);
        let first = String::from_utf8_lossy(first.body()).into_owned();
        let second = String::from_utf8_lossy(second.body()).into_owned();
        assert!(!first.starts_with("error"), "{first}");
        assert_eq!(first, second);
        let refreshes = env
            .fake
            .token_requests()
            .iter()
            .filter(|r| r.field("grant_type") == Some("refresh_token"))
            .count();
        assert_eq!(refreshes, 1);
        assert_eq!(env.client.__refresh_locks(), 0);
        // Whichever request was saved last, the session holds the rotated refresh token,
        // which still works. (The new token lives 100 seconds, so it is due after 70, well
        // within the idle timeout of the session.)
        env.clock.advance(Duration::from_secs(70));
        let third = b.text("/token").await;
        assert!(!third.starts_with("error"), "{third}");
        assert_ne!(third, first);
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// Secrets
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn tokens_never_appear_in_logs_or_errors() {
    let logs = logs();
    let s = setup();
    let (session, login) = s.session_with_tokens(FakeUser::new("alice"), 0).await;
    let session_id = session.id();
    let token = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap();
    let rotated = stored(&session)["refresh_token"]
        .as_str()
        .unwrap()
        .to_owned();
    s.clock.advance(Duration::from_secs(7200));
    s.fake.misbehave(Fault::TokenEndpointError("invalid_grant"));
    let error = s
        .client
        .access_token_for(&alice_request(&session), "idp")
        .await
        .unwrap_err();
    let secrets = [
        login.access_token.as_str(),
        login.refresh_token.as_str(),
        login.id_token.as_str(),
        token.value.expose().as_str(),
        rotated.as_str(),
        session_id.as_str(),
        SECRET,
    ];
    let text = format!("{error} {error:?} {token:?} {:?} {}", s.client, logs.text());
    for secret in secrets {
        assert!(!text.contains(secret), "a secret leaks: {text}");
    }
    let _: &AccessToken = &token;
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl LogBuffer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn logs() -> LogBuffer {
    static LOGS: OnceLock<LogBuffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        logs
    })
    .clone()
}
