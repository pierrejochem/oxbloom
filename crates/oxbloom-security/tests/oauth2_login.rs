//! The OAuth2 login endpoints in the security chain, driven through `App::handle` with an
//! in-memory session store, a `FakeProvider` and a manual clock. The browser carries the
//! session cookie by hand, as in `tests/revalidate.rs`.

#![cfg(feature = "oauth2")]

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use http::{Method, header};
use oxbloom_config::Secret;
use oxbloom_http_client::{
    HttpClient, HttpClientError, Request as OutRequest, Response as OutResponse,
};
use oxbloom_security::{
    Argon2PasswordEncoder, ClientAuthentication, ClientRegistration, DaoAuthenticationProvider,
    HttpSecurity, InMemorySessionStore, InMemoryUserDetailsService, OAuth2Login, PasswordEncoder,
    ProviderConfig, RequestSecurityExt, RequestSessionExt, SecurityBuildError, SessionBackend,
    SessionFilter, SessionSettings, SessionStore, UserDetails, UserDetailsError,
    UserDetailsService,
};
use oxbloom_test::{FakeProvider, FakeUser, Fault, ManualClock, MockResponse};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};
use serde_json::{Value, json};
use tracing_subscriber::fmt::MakeWriter;

const NAME: &str = "OXBLOOM_SESSION";
const HOST: &str = "app.example.test";
const BASE: &str = "https://app.example.test";
const ISSUER: &str = "https://idp.example.test";
const SECRET: &str = "app-secret-for-the-login-tests";
const FAILURE: &str = "/login?error";
const TOKENS_KEY: &str = "oxbloom.security.oauth2.tokens.idp";
const SAVED_KEY: &str = "oxbloom.security.saved-request";
const ERROR_DESCRIPTION: &str = "FAKE-PROVIDER-ERROR-DESCRIPTION";
const MAX_AGE: Duration = Duration::from_secs(600);
const PRINCIPAL_KEY: &str = "oxbloom.security.principal";
const PASSWORD: &str = "s3cret";

fn start_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn registration(id: &str, provider: &str, scopes: &[&str]) -> ClientRegistration {
    ClientRegistration {
        id: id.into(),
        client_id: "app".into(),
        client_secret: Some(Secret::new(SECRET.into())),
        scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        redirect_uri: "{base-url}/login/oauth2/code/{id}".into(),
        client_authentication: ClientAuthentication::Basic,
        provider: provider.into(),
    }
}

fn oidc_provider() -> ProviderConfig {
    ProviderConfig {
        issuer_uri: Some(ISSUER.into()),
        ..ProviderConfig::default()
    }
}

fn plain_provider() -> ProviderConfig {
    ProviderConfig {
        authorization_uri: Some(format!("{ISSUER}/authorize")),
        token_uri: Some(format!("{ISSUER}/token")),
        user_info_uri: Some(format!("{ISSUER}/userinfo")),
        user_name_attribute: Some("login".into()),
        ..ProviderConfig::default()
    }
}

/// A user service that counts its calls, for the revalidation test.
#[derive(Default)]
struct Counting(AtomicUsize);

#[async_trait]
impl UserDetailsService for Counting {
    async fn load_user_by_username(
        &self,
        _username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

/// A client whose every request times out.
struct TimeoutClient;

#[async_trait]
impl HttpClient for TimeoutClient {
    async fn execute(&self, _request: OutRequest) -> Result<OutResponse, HttpClientError> {
        Err(HttpClientError::Timeout)
    }
}

#[derive(Default)]
struct Options {
    client: Option<Arc<dyn HttpClient>>,
    session_limit: Option<usize>,
    remember_me: bool,
    revalidate: Option<Arc<Counting>>,
    extra: Vec<(ClientRegistration, ProviderConfig)>,
    password: bool,
}

struct Env {
    app: App,
    clock: ManualClock,
    fake: FakeProvider,
}

impl Env {
    fn new() -> Self {
        Self::with(Options::default())
    }

    fn with(options: Options) -> Self {
        logs();
        let clock = ManualClock::at(start_time());
        let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
        fake.expect_client_secret(SECRET);
        let client: Arc<dyn HttpClient> = options.client.clone().unwrap_or_else(|| fake.client());
        let settings = SessionSettings::default();
        let store = Arc::new(InMemorySessionStore::new(
            &settings,
            Arc::new(clock.clone()),
        ));
        let index = store.clone().principal_sessions().unwrap();
        let filter = SessionFilter::new(SessionBackend::Server(store), settings)
            .unwrap()
            .with_clock(Arc::new(clock.clone()));
        let extra = options.extra;
        let mut security = HttpSecurity::new();
        if options.password {
            let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
            let users = InMemoryUserDetailsService::new(vec![
                UserDetails::new("alice", encoder.encode(PASSWORD), []).roles(&["USER"]),
                UserDetails::new("bob", encoder.encode(PASSWORD), []).roles(&["USER"]),
                // A name in the namespace of the OAuth2 session keys.
                UserDetails::new("oauth2:idp:alice", encoder.encode(PASSWORD), []).roles(&["USER"]),
            ])
            .unwrap();
            security = security.password_provider(Arc::new(DaoAuthenticationProvider::new(
                Arc::new(users),
                encoder,
            )));
        }
        let mut security = security
            .form_login(|f| f)
            .clock(Arc::new(clock.clone()))
            .oauth2_login(|_| {
                let mut login = OAuth2Login::new(client)
                    .registration(registration("idp", "idp", &["email"]))
                    .provider("idp", oidc_provider())
                    .registration(registration("plain", "plain", &["read:user"]))
                    .provider("plain", plain_provider())
                    .base_url(BASE)
                    .pending_max_age(MAX_AGE);
                for (registration, provider) in extra {
                    let name = registration.provider.clone();
                    login = login.registration(registration).provider(&name, provider);
                }
                login
            })
            .authorize(|a| {
                a.get("/whoami")
                    .permit_all()
                    .get("/me")
                    .permit_all()
                    .get("/session")
                    .permit_all()
                    .get("/plant")
                    .permit_all()
                    .get("/csrf")
                    .permit_all()
                    .any_request()
                    .authenticated()
            });
        if let Some(max) = options.session_limit {
            security = security.session_limit(index, max);
        }
        if options.remember_me {
            security = security.remember_me(|r| {
                r.secret(Secret::new("a-remember-me-secret-of-32-bytes!".into()))
                    .user_details(Arc::new(Counting::default()))
            });
        }
        if let Some(service) = options.revalidate {
            security = security.revalidate(Duration::from_secs(1), service);
        }
        let chain = security.build().unwrap();
        let app = App::new()
            .filter(filter)
            .filter(chain)
            .route(Route::new(HttpMethod::Get, "/whoami", |req: Request| async move {
                req.authentication()
                    .map(|a| a.principal().to_string())
                    .unwrap_or_else(|| "anonymous".to_owned())
            }))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/me", |req: Request| async move {
                match req.authentication() {
                    Some(a) => json!({
                        "principal": a.principal(),
                        "authorities": a.authorities().iter().map(|g| g.as_str()).collect::<Vec<_>>(),
                        "details": a.details(),
                    })
                    .to_string(),
                    None => "null".to_owned(),
                }
            }))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/session", |req: Request| async move {
                let Some(session) = req.session() else {
                    return "null".to_owned();
                };
                let tokens = session
                    .get::<Value>(TOKENS_KEY)
                    .unwrap()
                    .unwrap_or(Value::Null);
                let saved = session.get::<Value>(SAVED_KEY).unwrap().unwrap_or(Value::Null);
                let principal = session
                    .get::<Value>(PRINCIPAL_KEY)
                    .unwrap()
                    .unwrap_or(Value::Null);
                json!({ "tokens": tokens, "saved": saved, "principal": principal }).to_string()
            }))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/plant", |req: Request| async move {
                // An unsafe saved request, as a bug elsewhere could leave one.
                if let Some(session) = req.session() {
                    session.set_reserved(SAVED_KEY, "//evil.example/x").unwrap();
                }
                "planted"
            }))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/csrf",
                |req: Request| async move {
                    req.csrf_token()
                        .map(|t| t.as_str().to_owned())
                        .unwrap_or_default()
                },
            ))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/orders", |_| async { "orders" }))
            .unwrap();
        Self { app, clock, fake }
    }

    fn browser(&self) -> Browser<'_> {
        Browser {
            env: self,
            jar: Mutex::new(None),
        }
    }
}

/// The query parameters of a URL, percent-decoded.
fn query(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map_or("", |(_, q)| q);
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn location(res: &Response) -> String {
    res.headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

fn assert_failure(res: &Response, context: &str) {
    assert_eq!(res.status(), 302, "{context}");
    assert_eq!(location(res), FAILURE, "{context}");
    assert!(res.body().is_empty(), "{context}");
}

/// One browser with its own session cookie.
struct Browser<'a> {
    env: &'a Env,
    jar: Mutex<Option<String>>,
}

impl Browser<'_> {
    fn request(&self, method: HttpMethod, target: &str) -> Request {
        let (path, query) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query.to_owned())),
            None => (target, None),
        };
        let mut req = Request::new(method, path);
        req.query = query;
        req.headers.insert(header::HOST, HOST.parse().unwrap());
        req.headers
            .insert(header::ACCEPT, "text/html".parse().unwrap());
        if let Some(value) = self.cookie() {
            req.headers
                .insert(header::COOKIE, format!("{NAME}={value}").parse().unwrap());
        }
        req
    }

    async fn send(&self, req: Request) -> Response {
        let res = self.env.app.handle(req).await;
        for value in res.headers().get_all(header::SET_COOKIE) {
            let text = value.to_str().unwrap();
            if let Some(rest) = text.strip_prefix(&format!("{NAME}=")) {
                let mut parts = rest.split("; ");
                let id = parts.next().unwrap().to_owned();
                let expired = parts.any(|p| p == "Max-Age=0");
                *self.jar.lock().unwrap() = (!expired).then_some(id);
            }
        }
        res
    }

    fn cookie(&self) -> Option<String> {
        self.jar.lock().unwrap().clone()
    }

    async fn get(&self, path: &str) -> Response {
        self.send(self.request(HttpMethod::Get, path)).await
    }

    async fn text(&self, path: &str) -> String {
        String::from_utf8_lossy(self.get(path).await.body()).into_owned()
    }

    async fn whoami(&self) -> String {
        self.text("/whoami").await
    }

    async fn me(&self) -> Value {
        serde_json::from_str(&self.text("/me").await).unwrap()
    }

    async fn session(&self) -> Value {
        serde_json::from_str(&self.text("/session").await).unwrap()
    }

    /// Logs in with the password of `username` and returns the response.
    async fn password_log_in(&self, username: &str) -> Response {
        let token = self.text("/csrf").await;
        let mut req = self.request(HttpMethod::Post, "/login");
        req.headers.insert(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded".parse().unwrap(),
        );
        req.body = format!("username={username}&password={PASSWORD}&_csrf={token}").into();
        self.send(req).await
    }

    /// Starts a login and returns the parameters of the authorization URL.
    async fn start(&self, id: &str) -> HashMap<String, String> {
        let res = self.get(&format!("/oauth2/authorization/{id}")).await;
        assert_eq!(res.status(), 302, "{res:?}");
        let target = location(&res);
        assert!(
            target.starts_with(&format!("{ISSUER}/authorize?")),
            "{target}"
        );
        query(&target)
    }

    /// The callback URL for `id` with these parameters.
    fn callback_path(id: &str, params: &[(&str, &str)]) -> String {
        let query: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{k}={}", encode(v)))
            .collect();
        format!("/login/oauth2/code/{id}?{}", query.join("&"))
    }

    async fn callback(&self, id: &str, params: &[(&str, &str)]) -> Response {
        self.get(&Self::callback_path(id, params)).await
    }

    /// Starts a login for `id`, lets the provider issue a code for `user`, and calls back.
    async fn log_in(&self, id: &str, user: FakeUser) -> Response {
        let params = self.start(id).await;
        if let Some(nonce) = params.get("nonce") {
            self.env.fake.expect_nonce(nonce);
        }
        let code = self.env.fake.issue_code(user);
        self.callback(id, &[("code", &code), ("state", &params["state"])])
            .await
    }
}

fn alice() -> FakeUser {
    FakeUser::new("alice")
        .email("alice@example.test")
        .scope("openid email")
}

// ---------------------------------------------------------------------------------------------
// The start of the flow
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn start_redirects_to_the_provider_with_pkce_state_and_nonce() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["client_id"], "app");
    assert_eq!(
        params["redirect_uri"],
        "https://app.example.test/login/oauth2/code/idp"
    );
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["code_challenge"].len(), 43);
    assert!(params["state"].len() >= 43);
    assert!(params["nonce"].len() >= 43);
    assert_ne!(params["state"], params["nonce"]);
    let scopes: Vec<&str> = params["scope"].split(' ').collect();
    assert_eq!(scopes, ["openid", "email"]);
    // The session now exists and holds the pending login.
    assert!(b.cookie().is_some());
    // A second start has a fresh state, nonce and challenge.
    let again = b.start("idp").await;
    for name in ["state", "nonce", "code_challenge"] {
        assert_ne!(params[name], again[name], "{name}");
    }
}

#[tokio::test]
async fn a_plain_oauth2_start_has_no_nonce_and_no_openid() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("plain").await;
    assert!(!params.contains_key("nonce"));
    assert_eq!(params["scope"], "read:user");
    assert_eq!(
        params["redirect_uri"],
        "https://app.example.test/login/oauth2/code/plain"
    );
    // A plain provider needs no discovery request.
    assert_eq!(env.fake.client().request_count(), 0);
}

// ---------------------------------------------------------------------------------------------
// The callback
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_callback_logs_the_user_in_and_redirects_to_the_default_success_url() {
    let env = Env::new();
    let b = env.browser();
    assert_eq!(b.whoami().await, "anonymous");
    let res = b.log_in("idp", alice()).await;
    assert_eq!(res.status(), 302, "{res:?}");
    assert_eq!(location(&res), "/");
    assert_eq!(b.whoami().await, "alice");
    // The exchange sent the PKCE verifier and the exact redirect URI.
    let requests = env.fake.token_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].field("redirect_uri"),
        Some("https://app.example.test/login/oauth2/code/idp")
    );
    assert!(requests[0].field("code_verifier").is_some());
}

#[tokio::test]
async fn the_pkce_verifier_matches_the_challenge_of_the_start() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_pkce(&params["code_challenge"]);
    env.fake
        .expect_redirect_uri("https://app.example.test/login/oauth2/code/idp");
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let res = b
        .callback("idp", &[("code", &code), ("state", &params["state"])])
        .await;
    assert_eq!(location(&res), "/");
    assert_eq!(b.whoami().await, "alice");
}

#[tokio::test]
async fn a_plain_oauth2_login_uses_the_user_info() {
    let env = Env::new();
    let b = env.browser();
    let user = FakeUser::new("1234")
        .attr("login", "alice-gh")
        .scope("read:user");
    let res = b.log_in("plain", user).await;
    assert_eq!(location(&res), "/");
    let me = b.me().await;
    assert_eq!(me["principal"], "alice-gh");
    assert_eq!(me["authorities"], json!(["OAUTH2_USER", "SCOPE_read:user"]));
    assert_eq!(me["details"]["oauth2.registration"], "plain");
}

#[tokio::test]
async fn the_callback_redirects_to_the_saved_request() {
    let env = Env::new();
    let b = env.browser();
    let res = b.get("/orders?page=2").await;
    assert_eq!(res.status(), 302);
    assert_eq!(location(&res), "/login");
    let res = b.log_in("idp", alice()).await;
    assert_eq!(location(&res), "/orders?page=2");
    // The saved request is used once.
    assert_eq!(b.session().await["saved"], Value::Null);
}

#[tokio::test]
async fn an_unsafe_saved_request_goes_to_the_default_success_url() {
    let env = Env::new();
    let b = env.browser();
    assert_eq!(b.text("/plant").await, "planted");
    let res = b.log_in("idp", alice()).await;
    assert_eq!(location(&res), "/");
    assert_eq!(b.whoami().await, "alice");
}

#[tokio::test]
async fn the_session_id_changes_at_login() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    let before = b.cookie().unwrap();
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let res = b
        .callback("idp", &[("code", &code), ("state", &params["state"])])
        .await;
    assert_eq!(location(&res), "/");
    let after = b.cookie().unwrap();
    assert_ne!(before, after);
    // The old id carries no login.
    let old = env.browser();
    *old.jar.lock().unwrap() = Some(before);
    assert_eq!(old.whoami().await, "anonymous");
}

#[tokio::test]
async fn the_authentication_has_oidc_user_and_scope_authorities() {
    let env = Env::new();
    let b = env.browser();
    b.log_in("idp", alice()).await;
    let me = b.me().await;
    assert_eq!(me["principal"], "alice");
    assert_eq!(
        me["authorities"],
        json!(["OIDC_USER", "SCOPE_email", "SCOPE_openid"])
    );
    assert_eq!(me["details"]["email"], "alice@example.test");
    assert_eq!(me["details"]["sub"], "alice");
    assert_eq!(me["details"]["oauth2.registration"], "idp");
    assert_eq!(me["details"]["oauth2.principal"], true);
}

#[tokio::test]
async fn granted_scopes_with_bad_characters_are_dropped() {
    let env = Env::new();
    let b = env.browser();
    let user = FakeUser::new("alice").scope("openid  email bad\"scope bad\\scope \u{7f}x");
    b.log_in("idp", user).await;
    let me = b.me().await;
    assert_eq!(
        me["authorities"],
        json!(["OIDC_USER", "SCOPE_email", "SCOPE_openid"])
    );
}

#[tokio::test]
async fn the_tokens_are_stored_in_the_session_under_the_reserved_key() {
    let env = Env::new();
    let b = env.browser();
    b.log_in("idp", alice()).await;
    let tokens = &b.session().await["tokens"];
    let now = start_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(tokens["v"], 1);
    assert!(!tokens["access_token"].as_str().unwrap().is_empty());
    assert_eq!(tokens["token_type"], "Bearer");
    assert_eq!(tokens["expires_at"], now + 3600);
    assert!(!tokens["refresh_token"].as_str().unwrap().is_empty());
    assert_eq!(tokens["id_token"].as_str().unwrap().split('.').count(), 3);
    assert_eq!(tokens["scope"], "openid email");
    let keys: Vec<&String> = tokens.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 7, "{keys:?}");
}

#[tokio::test]
async fn a_state_works_once() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let path = Browser::callback_path("idp", &[("code", &code), ("state", &params["state"])]);
    assert_eq!(location(&b.get(&path).await), "/");
    // The same callback again, in the same browser and in another one, fails.
    assert_failure(&b.get(&path).await, "replay in the same browser");
    let attacker = env.browser();
    assert_failure(&attacker.get(&path).await, "replay in another browser");
    assert_eq!(attacker.whoami().await, "anonymous");
    assert_eq!(env.fake.token_requests().len(), 1);
}

#[tokio::test]
async fn a_missing_or_wrong_state_is_a_failure() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let long = "a".repeat(65);
    let cases: [&[(&str, &str)]; 6] = [
        &[("code", &code)],
        &[("code", &code), ("state", "")],
        &[("code", &code), ("state", "not-the-state")],
        &[("code", &code), ("state", "has space")],
        &[("code", &code), ("state", "dot.dot")],
        &[("code", &code), ("state", &long)],
    ];
    for case in cases {
        let res = b.callback("idp", case).await;
        assert_failure(&res, &format!("{case:?}"));
        assert_eq!(b.whoami().await, "anonymous");
    }
    // A repeated state parameter is refused too.
    let res = b
        .callback(
            "idp",
            &[("code", &code), ("state", &params["state"]), ("state", "x")],
        )
        .await;
    assert_failure(&res, "a repeated state");
    // None of them touched the pending login: the right state still works.
    let res = b
        .callback("idp", &[("code", &code), ("state", &params["state"])])
        .await;
    assert_eq!(location(&res), "/");
    assert_eq!(b.whoami().await, "alice");
}

#[tokio::test]
async fn a_malformed_state_does_not_touch_the_session() {
    let env = Env::new();
    let b = env.browser();
    for state in ["has space", "dot.dot", &"a".repeat(65), "%00"] {
        let res = b.callback("idp", &[("code", "c"), ("state", state)]).await;
        assert_failure(&res, state);
        assert!(!res.headers().contains_key(header::SET_COOKIE), "{state}");
    }
    assert_eq!(b.cookie(), None);
}

#[tokio::test]
async fn a_callback_without_a_pending_login_is_a_failure() {
    let env = Env::new();
    let b = env.browser();
    let code = env.fake.issue_code(alice());
    let res = b
        .callback(
            "idp",
            &[("code", &code), ("state", "a-state-nobody-issued")],
        )
        .await;
    assert_failure(&res, "no pending login");
    assert_eq!(b.whoami().await, "anonymous");
    assert!(env.fake.token_requests().is_empty());
}

#[tokio::test]
async fn an_expired_pending_login_is_a_failure() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    env.clock.advance(MAX_AGE + Duration::from_secs(1));
    let res = b
        .callback("idp", &[("code", &code), ("state", &params["state"])])
        .await;
    assert_failure(&res, "expired");
    assert_eq!(b.whoami().await, "anonymous");
    assert!(env.fake.token_requests().is_empty());
}

#[tokio::test]
async fn two_tabs_log_in_independently() {
    let env = Env::new();
    let b = env.browser();
    let first = b.start("idp").await;
    let second = b.start("idp").await;
    env.fake.expect_nonce(&first["nonce"]);
    let code_first = env.fake.issue_code(FakeUser::new("first-user"));
    env.fake.expect_nonce(&second["nonce"]);
    let code_second = env.fake.issue_code(FakeUser::new("second-user"));
    let res = b
        .callback(
            "idp",
            &[("code", &code_second), ("state", &second["state"])],
        )
        .await;
    assert_eq!(location(&res), "/");
    assert_eq!(b.whoami().await, "second-user");
    let res = b
        .callback("idp", &[("code", &code_first), ("state", &first["state"])])
        .await;
    assert_eq!(location(&res), "/");
    assert_eq!(b.whoami().await, "first-user");
}

#[tokio::test]
async fn the_registration_in_the_path_must_match_the_pending_registration() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let res = b
        .callback("plain", &[("code", &code), ("state", &params["state"])])
        .await;
    assert_failure(&res, "other registration");
    // The pending login was used up by the failed attempt.
    let res = b
        .callback("idp", &[("code", &code), ("state", &params["state"])])
        .await;
    assert_failure(&res, "after the mismatch");
    assert_eq!(b.whoami().await, "anonymous");
    assert!(env.fake.token_requests().is_empty());
}

#[tokio::test]
async fn a_provider_error_parameter_is_a_failure_with_no_detail_in_the_response() {
    let env = Env::new();
    let b = env.browser();
    for code in ["access_denied", "made_up_error"] {
        let params = b.start("idp").await;
        let res = b
            .callback(
                "idp",
                &[
                    ("error", code),
                    ("error_description", ERROR_DESCRIPTION),
                    ("state", &params["state"]),
                ],
            )
            .await;
        assert_failure(&res, code);
        for (_, value) in res.headers() {
            let value = value.to_str().unwrap_or_default();
            assert!(!value.contains(code), "{value}");
            assert!(!value.contains(ERROR_DESCRIPTION), "{value}");
        }
    }
    assert_eq!(b.whoami().await, "anonymous");
    assert!(env.fake.token_requests().is_empty());
}

#[tokio::test]
async fn a_mismatched_iss_parameter_is_a_failure() {
    let env = Env::new();
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let res = b
        .callback(
            "idp",
            &[
                ("code", &code),
                ("state", &params["state"]),
                ("iss", "https://evil.example.test"),
            ],
        )
        .await;
    assert_failure(&res, "wrong iss");
    assert!(env.fake.token_requests().is_empty());
    // The right `iss` is accepted.
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let res = b
        .callback(
            "idp",
            &[
                ("code", &code),
                ("state", &params["state"]),
                ("iss", ISSUER),
            ],
        )
        .await;
    assert_eq!(location(&res), "/");
    // A provider without an issuer cannot match any `iss`.
    let b = env.browser();
    let params = b.start("plain").await;
    let code = env.fake.issue_code(FakeUser::new("1").attr("login", "x"));
    let res = b
        .callback(
            "plain",
            &[
                ("code", &code),
                ("state", &params["state"]),
                ("iss", ISSUER),
            ],
        )
        .await;
    assert_failure(&res, "iss for a provider without an issuer");
}

#[tokio::test]
async fn a_wrong_nonce_wrong_audience_wrong_issuer_expired_and_alg_none_token_each_fail() {
    let faults = [
        Fault::WrongNonce,
        Fault::WrongAudience,
        Fault::WrongAzp,
        Fault::WrongIssuer,
        Fault::Expired,
        Fault::NotYetValid,
        Fault::AlgNone,
        Fault::Hs256WithPublicKey,
        Fault::MissingSub,
    ];
    for fault in faults {
        let env = Env::new();
        env.fake.misbehave(fault);
        let b = env.browser();
        let res = b.log_in("idp", alice()).await;
        assert_failure(&res, &format!("{fault:?}"));
        assert_eq!(b.whoami().await, "anonymous", "{fault:?}");
        assert_eq!(b.session().await["tokens"], Value::Null, "{fault:?}");
    }
}

#[tokio::test]
async fn the_token_endpoint_error_is_a_failure() {
    for fault in [
        Fault::TokenEndpointError("invalid_grant"),
        Fault::TokenEndpointError("something_else"),
        Fault::TokenEndpointStatus(503),
        Fault::OversizedBody,
    ] {
        let env = Env::new();
        env.fake.misbehave(fault);
        let b = env.browser();
        let res = b.log_in("idp", alice()).await;
        assert_failure(&res, &format!("{fault:?}"));
        assert_eq!(b.whoami().await, "anonymous", "{fault:?}");
    }
}

#[tokio::test]
async fn a_user_info_failure_is_a_failure() {
    let env = Env::new();
    env.fake.misbehave(Fault::UserinfoError);
    let b = env.browser();
    let res = b
        .log_in("plain", FakeUser::new("1").attr("login", "x"))
        .await;
    assert_failure(&res, "user info error");
    assert_eq!(b.whoami().await, "anonymous");
}

#[tokio::test]
async fn a_failed_discovery_at_the_start_is_a_failure() {
    let env = Env::new();
    env.fake.misbehave(Fault::DiscoveryIssuerMismatch);
    let b = env.browser();
    let res = b.get("/oauth2/authorization/idp").await;
    assert_failure(&res, "discovery");
}

#[tokio::test]
async fn a_provider_timeout_is_a_failure_not_a_hang() {
    let env = Env::with(Options {
        client: Some(Arc::new(TimeoutClient)),
        ..Options::default()
    });
    let b = env.browser();
    // The plain provider needs no request to start; the exchange times out.
    let params = b.start("plain").await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        b.callback("plain", &[("code", "a-code"), ("state", &params["state"])]),
    )
    .await
    .expect("the callback must not hang");
    assert_failure(&res, "timeout");
    // The OIDC start needs discovery, which times out too.
    let res = tokio::time::timeout(Duration::from_secs(5), b.get("/oauth2/authorization/idp"))
        .await
        .expect("the start must not hang");
    assert_failure(&res, "discovery timeout");
}

#[tokio::test]
async fn user_info_with_an_id_token_must_have_the_same_subject() {
    let mut provider = oidc_provider();
    provider.user_info_uri = Some(format!("{ISSUER}/userinfo"));
    let mut other = oidc_provider();
    other.user_info_uri = Some(format!("{ISSUER}/userinfo-of-someone-else"));
    let env = Env::with(Options {
        extra: vec![
            (registration("merged", "merged", &[]), provider),
            (registration("mismatch", "mismatch", &[]), other),
        ],
        ..Options::default()
    });
    env.fake.client().on(
        Method::GET,
        &format!("{ISSUER}/userinfo-of-someone-else"),
        MockResponse::status(200).json(&json!({"sub": "mallory", "email": "m@example.test"})),
    );
    let b = env.browser();
    let user = FakeUser::new("alice").attr("department", "research");
    let res = b.log_in("merged", user).await;
    assert_eq!(location(&res), "/");
    let me = b.me().await;
    assert_eq!(me["principal"], "alice");
    assert_eq!(me["details"]["department"], "research");
    assert_eq!(me["details"]["oauth2.registration"], "merged");

    let b = env.browser();
    let res = b.log_in("mismatch", FakeUser::new("alice")).await;
    assert_failure(&res, "a user info of another subject");
    assert_eq!(b.whoami().await, "anonymous");
}

// ---------------------------------------------------------------------------------------------
// Interactions with the session features
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_session_limit_applies_to_an_oauth2_login() {
    let env = Env::with(Options {
        session_limit: Some(1),
        ..Options::default()
    });
    let a = env.browser();
    let b = env.browser();
    a.log_in("idp", alice()).await;
    assert_eq!(a.whoami().await, "alice");
    b.log_in("idp", alice()).await;
    assert_eq!(b.whoami().await, "alice");
    assert_eq!(a.whoami().await, "anonymous");
}

#[tokio::test]
async fn no_remember_me_cookie_is_issued() {
    let env = Env::with(Options {
        remember_me: true,
        ..Options::default()
    });
    let b = env.browser();
    let res = b.log_in("idp", alice()).await;
    assert_eq!(location(&res), "/");
    for value in res.headers().get_all(header::SET_COOKIE) {
        let text = value.to_str().unwrap();
        assert!(text.starts_with(&format!("{NAME}=")), "{text}");
    }
    // A remember-me cookie left in the browser is cleared by the login.
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    let mut req = b.request(
        HttpMethod::Get,
        &Browser::callback_path("idp", &[("code", &code), ("state", &params["state"])]),
    );
    let cookie = format!(
        "{}; OXBLOOM_REMEMBER_ME=bGVmdC1vdmVy",
        req.headers[header::COOKIE].to_str().unwrap()
    );
    req.headers.insert(header::COOKIE, cookie.parse().unwrap());
    let res = b.send(req).await;
    assert_eq!(location(&res), "/");
    let cleared = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .find(|v| v.starts_with("OXBLOOM_REMEMBER_ME="))
        .expect("the remember-me cookie is cleared");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
}

#[tokio::test]
async fn revalidation_skips_an_oauth2_principal() {
    let service = Arc::new(Counting::default());
    let env = Env::with(Options {
        revalidate: Some(service.clone()),
        ..Options::default()
    });
    let b = env.browser();
    b.log_in("idp", alice()).await;
    env.clock.advance(Duration::from_secs(30));
    assert_eq!(b.whoami().await, "alice");
    env.clock.advance(Duration::from_secs(30));
    assert_eq!(b.whoami().await, "alice");
    assert_eq!(service.0.load(Ordering::SeqCst), 0);
}

// ---------------------------------------------------------------------------------------------
// The endpoints
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn other_methods_on_the_endpoints_are_405() {
    let env = Env::new();
    let b = env.browser();
    for path in [
        "/oauth2/authorization/idp",
        "/login/oauth2/code/idp?code=c&state=s",
    ] {
        for method in [
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Delete,
            HttpMethod::Patch,
            HttpMethod::Head,
        ] {
            let res = b.send(b.request(method.clone(), path)).await;
            assert_eq!(res.status(), 405, "{method:?} {path}");
            assert_eq!(res.headers()[header::ALLOW], "GET", "{method:?} {path}");
        }
    }
    assert_eq!(env.fake.client().request_count(), 0);
}

#[tokio::test]
async fn an_unknown_registration_redirects_to_the_failure_url() {
    let env = Env::new();
    let b = env.browser();
    for path in [
        "/oauth2/authorization/nope",
        "/oauth2/authorization/",
        "/oauth2/authorization/idp/extra",
        "/login/oauth2/code/nope?code=c&state=s",
        "/login/oauth2/code/IDP?code=c&state=s",
    ] {
        let res = b.get(path).await;
        assert_failure(&res, path);
        assert!(!res.headers().contains_key(header::SET_COOKIE), "{path}");
    }
    // No discovery request was made for an unknown id.
    assert_eq!(env.fake.client().request_count(), 0);
}

#[tokio::test]
async fn the_build_fails_without_form_login() {
    let fake = FakeProvider::new(ISSUER);
    let err = HttpSecurity::new()
        .oauth2_login(|_| {
            OAuth2Login::new(fake.client())
                .registration(registration("idp", "idp", &[]))
                .provider("idp", oidc_provider())
                .base_url(BASE)
        })
        .authorize(|a| a.any_request().authenticated())
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::OAuth2LoginWithoutFormLogin));
}

#[tokio::test]
async fn the_build_fails_without_a_base_url() {
    let fake = FakeProvider::new(ISSUER);
    let build = |base: Option<&str>| {
        HttpSecurity::new()
            .form_login(|f| f)
            .oauth2_login(|_| {
                let login = OAuth2Login::new(fake.client())
                    .registration(registration("idp", "idp", &[]))
                    .provider("idp", oidc_provider());
                match base {
                    Some(base) => login.base_url(base),
                    None => login,
                }
            })
            .authorize(|a| a.any_request().authenticated())
            .build()
            .err()
    };
    assert_eq!(
        build(None),
        Some(SecurityBuildError::OAuth2LoginWithoutBaseUrl)
    );
    for bad in [
        "",
        "app.example.test",
        "ftp://app.example.test",
        "https://app.example.test/?a=1",
        "https://app.example.test/#x",
        // A path prefix could never reach the fixed callback path.
        "https://app.example.test/app",
        "https://app.example.test/app/",
        "https://app.example.test//",
    ] {
        assert_eq!(
            build(Some(bad)),
            Some(SecurityBuildError::OAuth2LoginWithoutBaseUrl),
            "{bad}"
        );
    }
    assert_eq!(build(Some("https://app.example.test/")), None);
    assert_eq!(build(Some("http://localhost:8080")), None);
}

#[tokio::test]
async fn the_build_fails_without_a_usable_registration() {
    let fake = FakeProvider::new(ISSUER);
    let build = |f: &dyn Fn(OAuth2Login) -> OAuth2Login| {
        HttpSecurity::new()
            .form_login(|f| f)
            .oauth2_login(|_| f(OAuth2Login::new(fake.client()).base_url(BASE)))
            .authorize(|a| a.any_request().authenticated())
            .build()
            .err()
    };
    let missing = Some(SecurityBuildError::OAuth2LoginWithoutRegistration);
    assert_eq!(build(&|l| l), missing);
    // A registration whose provider was never added.
    assert_eq!(
        build(&|l| l.registration(registration("idp", "idp", &[]))),
        missing
    );
    // A registration id that can never appear in the URL.
    assert_eq!(
        build(&|l| l
            .registration(registration("Bad/Id", "idp", &[]))
            .provider("idp", oidc_provider())),
        missing
    );
    // A failure URL that is not a safe local target.
    assert_eq!(
        build(&|l| l
            .registration(registration("idp", "idp", &[]))
            .provider("idp", oidc_provider())
            .failure_url("https://evil.example/")),
        Some(SecurityBuildError::InvalidFormLoginUrl)
    );
}

#[tokio::test]
async fn the_build_checks_the_shape_of_code_built_registrations_and_providers() {
    let fake = FakeProvider::new(ISSUER);
    let build = |registration: ClientRegistration, provider: ProviderConfig| {
        let name = registration.provider.clone();
        HttpSecurity::new()
            .form_login(|f| f)
            .oauth2_login(|_| {
                OAuth2Login::new(fake.client())
                    .registration(registration)
                    .provider(&name, provider)
                    .base_url(BASE)
            })
            .authorize(|a| a.any_request().authenticated())
            .build()
            .err()
    };
    let refused = Some(SecurityBuildError::OAuth2LoginWithoutRegistration);
    let plain = || registration("plain", "plain", &[]);
    // The valid shapes build.
    assert_eq!(build(plain(), plain_provider()), None);
    assert_eq!(
        build(registration("idp", "idp", &[]), oidc_provider()),
        None
    );
    // A plain provider needs both the user info and the user name attribute.
    let mut no_info = plain_provider();
    no_info.user_info_uri = None;
    assert_eq!(build(plain(), no_info), refused, "no user-info-uri");
    let mut no_name = plain_provider();
    no_name.user_name_attribute = None;
    assert_eq!(build(plain(), no_name), refused, "no user-name-attribute");
    // An empty client id.
    let mut empty_id = plain();
    empty_id.client_id = String::new();
    assert_eq!(
        build(empty_id, plain_provider()),
        refused,
        "empty client id"
    );
    // Basic or post authentication without a secret, or with an empty one.
    for auth in [ClientAuthentication::Basic, ClientAuthentication::Post] {
        let mut no_secret = plain();
        no_secret.client_authentication = auth;
        no_secret.client_secret = None;
        assert_eq!(build(no_secret, plain_provider()), refused, "{auth:?}");
        let mut empty_secret = plain();
        empty_secret.client_authentication = auth;
        empty_secret.client_secret = Some(Secret::new(String::new()));
        assert_eq!(build(empty_secret, plain_provider()), refused, "{auth:?}");
    }
    // A public client needs no secret.
    let mut public = plain();
    public.client_authentication = ClientAuthentication::None;
    public.client_secret = None;
    assert_eq!(build(public, plain_provider()), None);
    // A provider without an issuer needs both the authorization and the token endpoint.
    let mut no_token = plain_provider();
    no_token.token_uri = None;
    assert_eq!(build(plain(), no_token), refused, "no token-uri");
    let mut no_authorization = plain_provider();
    no_authorization.authorization_uri = None;
    assert_eq!(
        build(plain(), no_authorization),
        refused,
        "no authorization-uri"
    );
    // Every endpoint must be acceptable: https, or http for a loopback host only.
    for field in 0..4 {
        let mut provider = plain_provider();
        let slot = match field {
            0 => &mut provider.authorization_uri,
            1 => &mut provider.token_uri,
            2 => &mut provider.user_info_uri,
            _ => &mut provider.jwk_set_uri,
        };
        *slot = Some("http://idp.example.test/endpoint".into());
        assert_eq!(build(plain(), provider), refused, "endpoint {field}");
    }
    let http_issuer = ProviderConfig {
        issuer_uri: Some("http://idp.example.test".into()),
        ..ProviderConfig::default()
    };
    assert_eq!(build(registration("idp", "idp", &[]), http_issuer), refused);
    let issuer_with_query = ProviderConfig {
        issuer_uri: Some(format!("{ISSUER}?tenant=1")),
        ..ProviderConfig::default()
    };
    assert_eq!(
        build(registration("idp", "idp", &[]), issuer_with_query),
        refused
    );
    // A scope with a character outside RFC 6749 section 3.3.
    assert_eq!(
        build(
            registration("plain", "plain", &["bad scope"]),
            plain_provider()
        ),
        refused
    );
    // A provider name outside a to z, 0 to 9 and -.
    assert_eq!(
        build(registration("plain", "Bad_Provider", &[]), plain_provider()),
        refused
    );
}

#[tokio::test]
async fn the_build_fails_without_a_client() {
    let err = HttpSecurity::new()
        .form_login(|f| f)
        .oauth2_login(|l| {
            l.registration(registration("idp", "idp", &[]))
                .provider("idp", oidc_provider())
                .base_url(BASE)
        })
        .authorize(|a| a.any_request().authenticated())
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::OAuth2LoginWithoutClient));
}

#[tokio::test]
async fn form_login_without_a_password_provider_is_allowed_with_oauth2_login() {
    let env = Env::new();
    let b = env.browser();
    // The password post always fails.
    let mut req = b.request(HttpMethod::Post, "/login");
    req.headers.insert(
        header::CONTENT_TYPE,
        "application/x-www-form-urlencoded".parse().unwrap(),
    );
    req.headers.insert(header::ORIGIN, BASE.parse().unwrap());
    req.body = "username=alice&password=x".into();
    let res = b.send(req).await;
    assert_ne!(location(&res), "/");
    assert_eq!(b.whoami().await, "anonymous");
}

#[tokio::test]
async fn a_custom_failure_url_is_used() {
    let fake = FakeProvider::new(ISSUER);
    let chain = HttpSecurity::new()
        .form_login(|f| f)
        .oauth2_login(|_| {
            OAuth2Login::new(fake.client())
                .registration(registration("idp", "idp", &[]))
                .provider("idp", oidc_provider())
                .base_url(BASE)
                .failure_url("/signin?failed")
        })
        .authorize(|a| a.any_request().authenticated())
        .build()
        .unwrap();
    let app = App::new().filter(chain);
    let res = app
        .handle(Request::new(HttpMethod::Get, "/oauth2/authorization/nope"))
        .await;
    assert_eq!(res.status(), 302);
    assert_eq!(location(&res), "/signin?failed");
}

#[tokio::test]
async fn without_a_session_filter_the_start_fails_and_logs_why() {
    let logs = logs();
    let fake = FakeProvider::new(ISSUER);
    let chain = HttpSecurity::new()
        .form_login(|f| f)
        .oauth2_login(|_| {
            OAuth2Login::new(fake.client())
                .registration(registration("idp", "idp", &[]))
                .provider("idp", oidc_provider())
                .base_url(BASE)
        })
        .authorize(|a| a.any_request().authenticated())
        .build()
        .unwrap();
    let app = App::new().filter(chain);
    let res = app
        .handle(Request::new(HttpMethod::Get, "/oauth2/authorization/idp"))
        .await;
    assert_failure(&res, "no session filter");
    assert!(logs.text().contains("OAuth2 login has no session"));
}

#[tokio::test]
async fn a_key_set_without_an_issuer_logs_one_warning_at_build() {
    let logs = logs();
    let fake = FakeProvider::new(ISSUER);
    let provider = ProviderConfig {
        authorization_uri: Some(format!("{ISSUER}/authorize")),
        token_uri: Some(format!("{ISSUER}/token")),
        jwk_set_uri: Some(format!("{ISSUER}/jwks")),
        ..ProviderConfig::default()
    };
    HttpSecurity::new()
        .form_login(|f| f)
        .oauth2_login(|_| {
            OAuth2Login::new(fake.client())
                .registration(registration("keys-without-issuer", "kwi", &[]))
                .provider("kwi", provider)
                .registration(registration("idp", "idp", &[]))
                .provider("idp", oidc_provider())
                .base_url(BASE)
        })
        .authorize(|a| a.any_request().authenticated())
        .build()
        .unwrap();
    let text = logs.text();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("keys-without-issuer"))
        .collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].contains("WARN"), "{}", lines[0]);
    assert!(lines[0].contains("oxbloom::security"), "{}", lines[0]);
    assert!(!lines[0].contains(ISSUER), "{}", lines[0]);
}

// ---------------------------------------------------------------------------------------------
// No secret leaks
// ---------------------------------------------------------------------------------------------

/// Runs a success and several failures; returns every response and the secrets involved.
async fn leak_scenarios(env: &Env) -> (Vec<Response>, Vec<String>) {
    let mut responses = Vec::new();
    let mut secrets = vec![SECRET.to_owned(), ERROR_DESCRIPTION.to_owned()];
    // A success, with its tokens read back from the session.
    let b = env.browser();
    let params = b.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(alice());
    responses.push(
        b.callback("idp", &[("code", &code), ("state", &params["state"])])
            .await,
    );
    let tokens = b.session().await["tokens"].clone();
    for name in ["access_token", "refresh_token", "id_token"] {
        secrets.push(tokens[name].as_str().unwrap().to_owned());
    }
    secrets.extend([code, params["state"].clone(), params["nonce"].clone()]);
    // A token endpoint error, a wrong nonce, a provider error and a replay.
    for fault in [
        Fault::TokenEndpointError("invalid_grant"),
        Fault::WrongNonce,
    ] {
        let env = Env::new();
        env.fake.misbehave(fault);
        let b = env.browser();
        let params = b.start("idp").await;
        env.fake.expect_nonce(&params["nonce"]);
        let code = env.fake.issue_code(alice());
        let path = Browser::callback_path("idp", &[("code", &code), ("state", &params["state"])]);
        responses.push(b.get(&path).await);
        responses.push(b.get(&path).await);
        secrets.extend([code, params["state"].clone(), params["nonce"].clone()]);
    }
    let b = env.browser();
    let params = b.start("idp").await;
    responses.push(
        b.callback(
            "idp",
            &[
                ("error", "access_denied"),
                ("error_description", ERROR_DESCRIPTION),
                ("state", &params["state"]),
            ],
        )
        .await,
    );
    secrets.push(params["state"].clone());
    (responses, secrets)
}

#[tokio::test]
async fn the_failure_response_never_contains_tokens_codes_or_secrets() {
    let env = Env::new();
    let (responses, secrets) = leak_scenarios(&env).await;
    assert_eq!(responses.len(), 6);
    for res in &responses[1..] {
        assert_failure(res, "failure");
    }
    for res in &responses {
        let body = String::from_utf8_lossy(res.body()).into_owned();
        let headers = format!("{:?}", res.headers());
        for secret in &secrets {
            assert!(!body.contains(secret.as_str()), "body leaks a secret");
            assert!(
                !headers.contains(secret.as_str()),
                "headers leak a secret: {headers}"
            );
        }
    }
}

#[tokio::test]
async fn logs_never_contain_tokens_codes_or_secrets() {
    let logs = logs();
    let env = Env::new();
    let (_, secrets) = leak_scenarios(&env).await;
    // A principal with a line break is logged escaped.
    let b = env.browser();
    b.log_in("idp", FakeUser::new("eve\nINJECTED-LINE")).await;
    let text = logs.text();
    assert!(text.contains("OAuth2 login failed"), "{text}");
    assert!(text.contains("OAuth2 login succeeded"), "{text}");
    for secret in &secrets {
        assert!(!text.contains(secret.as_str()), "the logs leak a secret");
    }
    assert!(!text.contains("\nINJECTED-LINE"), "{text}");
    assert!(text.contains("eve\\nINJECTED-LINE"), "{text}");
}

// ---------------------------------------------------------------------------------------------
// Log capture
// ---------------------------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------------------------
// Fix round 1
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_live_state_and_code_of_another_browser_are_refused() {
    let env = Env::new();
    let attacker = env.browser();
    let victim = env.browser();
    let params = attacker.start("idp").await;
    env.fake.expect_nonce(&params["nonce"]);
    let code = env.fake.issue_code(FakeUser::new("attacker-user"));
    let callback = [("code", code.as_str()), ("state", params["state"].as_str())];
    // The victim loads the attacker's callback, before and after starting its own login.
    assert_failure(
        &victim.callback("idp", &callback).await,
        "victim, no login started",
    );
    assert_eq!(victim.whoami().await, "anonymous");
    let _ = victim.start("idp").await;
    assert_failure(
        &victim.callback("idp", &callback).await,
        "victim, own login started",
    );
    assert_eq!(victim.whoami().await, "anonymous");
    assert!(env.fake.token_requests().is_empty());
    // The attacker's pending login is untouched and still works in the attacker's browser.
    let res = attacker.callback("idp", &callback).await;
    assert_eq!(location(&res), "/");
    assert_eq!(attacker.whoami().await, "attacker-user");
}

#[tokio::test]
async fn an_oauth2_login_does_not_share_the_session_limit_with_a_password_user() {
    let env = Env::with(Options {
        password: true,
        session_limit: Some(1),
        ..Options::default()
    });
    let password = env.browser();
    let res = password.password_log_in("alice").await;
    assert_eq!(location(&res), "/", "{res:?}");
    assert_eq!(password.whoami().await, "alice");
    assert_eq!(password.session().await["principal"], "alice");
    // An OAuth2 user whose principal is also `alice`.
    let first = env.browser();
    first.log_in("idp", alice()).await;
    assert_eq!(first.whoami().await, "alice");
    assert_eq!(first.session().await["principal"], "oauth2:idp:alice");
    assert_eq!(password.whoami().await, "alice");
    // The OAuth2 user's own sessions are still limited.
    let second = env.browser();
    second.log_in("idp", alice()).await;
    assert_eq!(second.whoami().await, "alice");
    assert_eq!(first.whoami().await, "anonymous");
    assert_eq!(password.whoami().await, "alice");
}

#[tokio::test]
async fn a_password_login_after_an_oauth2_login_removes_the_tokens() {
    let env = Env::with(Options {
        password: true,
        ..Options::default()
    });
    let b = env.browser();
    b.log_in("idp", alice()).await;
    assert_ne!(b.session().await["tokens"], Value::Null);
    let res = b.password_log_in("bob").await;
    assert_eq!(location(&res), "/", "{res:?}");
    assert_eq!(b.whoami().await, "bob");
    let session = b.session().await;
    assert_eq!(session["tokens"], Value::Null);
    assert_eq!(session["principal"], "bob");
}

#[tokio::test]
async fn an_oidc_token_response_without_an_id_token_is_a_failure() {
    let provider = ProviderConfig {
        authorization_uri: Some(format!("{ISSUER}/authorize")),
        token_uri: Some(format!("{ISSUER}/token-without-id-token")),
        jwk_set_uri: Some(format!("{ISSUER}/jwks")),
        ..ProviderConfig::default()
    };
    let env = Env::with(Options {
        extra: vec![(registration("noid", "noid", &[]), provider)],
        ..Options::default()
    });
    env.fake.client().on(
        Method::POST,
        &format!("{ISSUER}/token-without-id-token"),
        MockResponse::status(200).json(&json!({"access_token": "at", "token_type": "Bearer"})),
    );
    let b = env.browser();
    let params = b.start("noid").await;
    assert!(params.contains_key("nonce"));
    let res = b
        .callback("noid", &[("code", "a-code"), ("state", &params["state"])])
        .await;
    assert_failure(&res, "no id token");
    assert_eq!(b.whoami().await, "anonymous");
    assert_eq!(b.session().await["tokens"], Value::Null);
}

fn build_with(
    form: impl FnOnce(oxbloom_security::FormLogin) -> oxbloom_security::FormLogin,
    f: impl FnOnce(OAuth2Login) -> OAuth2Login,
) -> Result<oxbloom_security::SecurityFilterChain, SecurityBuildError> {
    let fake = FakeProvider::new(ISSUER);
    HttpSecurity::new()
        .form_login(form)
        .oauth2_login(|_| {
            f(OAuth2Login::new(fake.client())
                .provider("idp", oidc_provider())
                .base_url(BASE))
        })
        .authorize(|a| a.any_request().authenticated())
        .build()
}

#[tokio::test]
async fn the_redirect_uri_must_be_the_callback_path_of_the_registration() {
    let invalid = Some(SecurityBuildError::InvalidOAuth2Login);
    let with_uri = |uri: &str| {
        let mut r = registration("idp", "idp", &[]);
        r.redirect_uri = uri.to_owned();
        r
    };
    // The default template, and the same URI written out, pass; a trailing slash on the base
    // URL is dropped as in the template.
    assert!(build_with(|f| f, |l| l.registration(registration("idp", "idp", &[]))).is_ok());
    assert!(
        build_with(
            |f| f,
            |l| l.registration(with_uri("https://app.example.test/login/oauth2/code/idp"))
        )
        .is_ok()
    );
    assert!(
        build_with(
            |f| f,
            |l| l
                .base_url("https://app.example.test/")
                .registration(registration("idp", "idp", &[]))
        )
        .is_ok()
    );
    for uri in [
        "{base-url}/elsewhere/{id}",
        "https://app.example.test/login/oauth2/code/other",
        "https://evil.example.test/login/oauth2/code/idp",
        "{base-url}/login/oauth2/code/{id}?x=1",
    ] {
        assert_eq!(
            build_with(|f| f, |l| l.registration(with_uri(uri))).err(),
            invalid,
            "{uri}"
        );
    }
    // Two registrations that share one redirect URI.
    let mut shared = registration("second", "idp", &[]);
    shared.redirect_uri = "{base-url}/login/oauth2/code/idp".into();
    assert_eq!(
        build_with(
            |f| f,
            |l| l
                .registration(registration("idp", "idp", &[]))
                .registration(shared)
        )
        .err(),
        invalid
    );
}

#[tokio::test]
async fn a_zero_pending_max_age_fails_the_build() {
    assert_eq!(
        build_with(
            |f| f,
            |l| l
                .registration(registration("idp", "idp", &[]))
                .pending_max_age(Duration::ZERO)
        )
        .err(),
        Some(SecurityBuildError::InvalidOAuth2Login)
    );
}

#[tokio::test]
async fn a_failure_url_on_an_oauth2_endpoint_fails_the_build() {
    for url in ["/oauth2/authorization/idp", "/login/oauth2/code/idp?error"] {
        assert_eq!(
            build_with(
                |f| f,
                |l| l
                    .registration(registration("idp", "idp", &[]))
                    .failure_url(url)
            )
            .err(),
            Some(SecurityBuildError::InvalidFormLoginUrl),
            "{url}"
        );
    }
}

#[tokio::test]
async fn the_failure_url_follows_form_login_unless_set() {
    let chain = build_with(
        |f| f.failure_url("/signin?bad"),
        |l| l.registration(registration("idp", "idp", &[])),
    )
    .unwrap();
    let app = App::new().filter(chain);
    let res = app
        .handle(Request::new(HttpMethod::Get, "/oauth2/authorization/nope"))
        .await;
    assert_eq!(location(&res), "/signin?bad");
    let chain = build_with(
        |f| f.failure_url("/signin?bad"),
        |l| {
            l.registration(registration("idp", "idp", &[]))
                .failure_url("/oauth-failed")
        },
    )
    .unwrap();
    let app = App::new().filter(chain);
    let res = app
        .handle(Request::new(HttpMethod::Get, "/oauth2/authorization/nope"))
        .await;
    assert_eq!(location(&res), "/oauth-failed");
}

#[tokio::test]
async fn a_password_user_named_in_the_oauth2_namespace_cannot_log_in() {
    let env = Env::with(Options {
        password: true,
        ..Options::default()
    });
    let b = env.browser();
    let res = b.password_log_in("oauth2:idp:alice").await;
    assert_eq!(res.status(), 302);
    assert_eq!(location(&res), FAILURE);
    assert!(res.body().is_empty());
    assert_eq!(b.whoami().await, "anonymous");
    assert_eq!(b.session().await["principal"], Value::Null);
    // A normal user of the same store is not affected.
    let other = env.browser();
    let res = other.password_log_in("alice").await;
    assert_eq!(location(&res), "/");
    assert_eq!(other.whoami().await, "alice");
}
