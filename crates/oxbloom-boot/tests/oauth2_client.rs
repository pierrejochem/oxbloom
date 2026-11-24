//! The OAuth2 client in boot: `OAuth2ClientAutoConfiguration` registers an `OAuth2Client` bean
//! from the `security.oauth2.client.*` keys over the `dyn HttpClient` bean. A booted
//! application with a `FakeProvider` as its `dyn HttpClient` bean gets client credentials
//! tokens and the logged-in user's tokens through the bean, in handlers.

#![cfg(all(feature = "oauth2", feature = "web"))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use http::Method;
use oxbloom_boot::SecurityCustomizer;
use oxbloom_boot::{
    AutoConfiguration, ConfigRef, HttpClientAutoConfiguration, OAuth2ClientAutoConfiguration,
    SecurityAutoConfiguration, StartupPhase, auto_configurations,
};
use oxbloom_config::ConfigError;
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_http_client::{HttpClient, Request as OutRequest};
use oxbloom_security::{
    Argon2PasswordEncoder, BearerRetryClient, DelegatingPasswordEncoder, HttpSecurity,
    OAuth2Client, OAuth2Settings, PasswordEncoder, RequestSessionExt, SecurityFilterChain,
    TokenError,
};
use oxbloom_test::{
    ContextRunner, FakeProvider, FakeUser, ManualClock, MockHttpClient, MockResponse, RunningApp,
    TestApp, TestResponse,
};
use oxbloom_web::{Controller, HttpMethod, Request, Route};
use serde_json::Value;

const ISSUER: &str = "https://idp.example.test";
const BASE: &str = "https://app.example.test";
const SECRET: &str = "app-secret-for-the-boot-oauth2-client-tests";
const COOKIE: &str = "OXBLOOM_SESSION";
const TOKENS_KEY: &str = "oxbloom.security.oauth2.tokens.idp";
const DOWNSTREAM: &str = "https://orders.example.test/orders";
const PLANTED_ACCESS_TOKEN: &str = "planted-access-token";

/// Form login, the session table, the base URL and two registrations of the provider `idp`
/// (OpenID Connect, through discovery): `idp` for the login and `billing` for the client
/// credentials of the application itself.
fn config() -> String {
    format!(
        "[security.form-login]\nenabled = true\n[security.session]\n{}",
        registrations()
    )
}

fn registrations() -> String {
    format!(
        r#"[security.oauth2]
base-url = "{BASE}"

[security.oauth2.client.registration.idp]
client-id = "app"
client-secret = "{SECRET}"
scope = ["email"]

[security.oauth2.client.registration.billing]
client-id = "orders-service"
client-secret = "{SECRET}"
scope = ["invoices.read"]
provider = "idp"

[security.oauth2.client.provider.idp]
issuer-uri = "{ISSUER}"
"#
    )
}

fn start_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000)
}

/// The routes of the tests, over the `OAuth2Client` bean:
/// - `/downstream`: calls the downstream service through a `BearerRetryClient` with the client
///   credentials of `billing` and answers with the downstream body;
/// - `/user-token`: the access token of the logged-in user for `idp`, or the error text;
/// - `/tokens`: the tokens that the session holds for `idp`;
/// - `/plant-tokens`: writes a valid token record of `idp` into the session.
struct Routes {
    oauth2: Arc<OAuth2Client>,
    downstream: Arc<MockHttpClient>,
}

impl Controller for Routes {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        let this = self.clone();
        let downstream = Route::new(HttpMethod::Get, "/downstream", move |_req: Request| {
            let this = this.clone();
            async move {
                let http =
                    BearerRetryClient::new(this.downstream.clone(), this.oauth2.clone(), "billing");
                match http.execute(OutRequest::get(DOWNSTREAM)).await {
                    Ok(response) => String::from_utf8_lossy(response.bytes()).into_owned(),
                    Err(error) => format!("error: {error}"),
                }
            }
        });
        let this = self.clone();
        let user_token = Route::new(HttpMethod::Get, "/user-token", move |req: Request| {
            let this = this.clone();
            async move {
                match this.oauth2.access_token_for(&req, "idp").await {
                    Ok(token) => token.value.expose().clone(),
                    Err(error) => format!("error: {error}"),
                }
            }
        });
        let tokens = Route::new(HttpMethod::Get, "/tokens", |req: Request| async move {
            req.session()
                .and_then(|s| s.get::<Value>(TOKENS_KEY).ok().flatten())
                .unwrap_or(Value::Null)
                .to_string()
        });
        // Writes a valid token record of `idp` into the session of the request, whoever is
        // logged in: the binding check, not the absence of tokens, must then refuse them.
        let plant = Route::new(
            HttpMethod::Get,
            "/plant-tokens",
            |req: Request| async move {
                let session = req.session().expect("a session");
                session
                .set_reserved(
                    TOKENS_KEY,
                    &serde_json::json!({
                        "v": 1, "access_token": PLANTED_ACCESS_TOKEN, "token_type": "Bearer",
                        "expires_at": 4_102_444_800u64, "refresh_token": "planted-refresh-token",
                        "id_token": null, "scope": "openid email",
                    }),
                )
                .expect("the reserved key is writable");
                "planted"
            },
        );
        vec![downstream, user_token, tokens, plant]
    }
}

/// Registers `provider` as the `dyn HttpClient` bean and the routes over the `OAuth2Client`
/// bean with `downstream` as the downstream service.
fn wiring(
    provider: Arc<MockHttpClient>,
    downstream: Arc<MockHttpClient>,
) -> impl FnOnce(ContainerBuilder) -> ContainerBuilder {
    move |b| {
        b.instance(provider)
            .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
            .bean(move |oauth2: Arc<OAuth2Client>| Routes {
                oauth2,
                downstream: downstream.clone(),
            })
            .provides(|c: Arc<Routes>| -> Arc<dyn Controller> { c })
    }
}

/// A downstream service that answers every GET of `DOWNSTREAM` with `orders`.
fn downstream() -> Arc<MockHttpClient> {
    let downstream = Arc::new(MockHttpClient::new());
    downstream.on(
        Method::GET,
        DOWNSTREAM,
        MockResponse::status(200).text("orders"),
    );
    downstream
}

/// The secured test application with the OAuth2 client auto-configuration.
fn secured() -> TestApp {
    TestApp::secured().auto_configuration::<OAuth2ClientAutoConfiguration>()
}

// ---- The bean ------------------------------------------------------------------------------

#[tokio::test]
async fn the_client_bean_is_registered_with_registrations_and_an_http_client() {
    // From the `HttpClientAutoConfiguration` bean.
    ContextRunner::new()
        .config_str(registrations())
        .auto_configuration::<HttpClientAutoConfiguration>()
        .auto_configuration::<OAuth2ClientAutoConfiguration>()
        .run(|context| {
            let client = context.get::<OAuth2Client>().unwrap();
            assert!(
                format!("{client:?}").contains("registrations: 2"),
                "{client:?}"
            );
        })
        .await;
    // From an application `HttpClient` bean, with every auto-configuration of boot.
    let fake = FakeProvider::new(ISSUER);
    let provider = fake.client();
    ContextRunner::new()
        .config_str(config())
        .auto_configurations(auto_configurations())
        .configure(move |b| {
            b.instance(provider)
                .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
        })
        .run(|context| {
            assert!(context.contains::<OAuth2Client>());
        })
        .await;
}

#[tokio::test]
async fn the_client_answers_through_the_application_http_client_and_the_clock_bean() {
    let clock = ManualClock::at(start_time());
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
    fake.expect_client_secret(SECRET);
    let provider = fake.client();
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    ContextRunner::new()
        .config_str(config())
        .clock(Arc::new(clock))
        .auto_configurations(auto_configurations())
        .configure(move |b| {
            b.instance(provider)
                .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
        })
        .run(move |context| {
            *out.lock().unwrap() = Some(context.get::<OAuth2Client>().unwrap());
        })
        .await;
    let client = seen.lock().unwrap().clone().unwrap();
    let token = client.client_credentials("billing").await.unwrap();
    // The expiry follows the clock bean, years behind the system time.
    assert_eq!(token.expires_at, start_time() + Duration::from_secs(3600));
    assert_eq!(fake.token_requests().len(), 1);
}

#[tokio::test]
async fn no_registrations_means_no_client_bean() {
    for toml in [
        String::new(),
        format!("[security.oauth2]\nbase-url = \"{BASE}\"\n"),
    ] {
        ContextRunner::new()
            .config_str(toml)
            .auto_configurations(auto_configurations())
            .run(|context| {
                assert!(!context.contains::<OAuth2Client>());
                let skipped = context.conditions().records().iter().any(|r| {
                    !r.matched() && r.reason().contains("security.oauth2.client.registration")
                });
                assert!(skipped, "{:?}", context.conditions());
            })
            .await;
    }
}

#[tokio::test]
async fn the_application_bean_wins() {
    let own = OAuth2Client::new(OAuth2Settings::default(), Arc::new(MockHttpClient::new()));
    ContextRunner::new()
        .config_str(config())
        .auto_configurations(auto_configurations())
        .configure(move |b| b.instance(own))
        .run(|context| {
            let client = context.get::<OAuth2Client>().unwrap();
            // The application's client has no registrations; boot's would have two.
            assert!(
                format!("{client:?}").contains("registrations: 0"),
                "{client:?}"
            );
        })
        .await;
}

/// One client-credentials registration, without form login, a session or a base URL.
fn client_only() -> String {
    format!(
        "[security.oauth2.client.registration.billing]\nclient-id = \"orders-service\"\n\
         client-secret = \"{SECRET}\"\nprovider = \"idp\"\n\
         [security.oauth2.client.provider.idp]\nissuer-uri = \"{ISSUER}\"\n"
    )
}

#[tokio::test]
async fn a_client_only_application_with_its_own_chain_boots_with_the_client() {
    ContextRunner::new()
        .config_str(client_only())
        .auto_configurations(auto_configurations())
        .configure(|b| {
            b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
                Ok(HttpSecurity::new()
                    .authorize(|a| a.any_request().authenticated())
                    .build()?)
            })
        })
        .run(|context| {
            let client = context.get::<OAuth2Client>().unwrap();
            assert!(
                format!("{client:?}").contains("registrations: 1"),
                "{client:?}"
            );
        })
        .await;
}

#[tokio::test]
async fn a_client_only_application_without_security_auto_configuration_boots_with_the_client() {
    ContextRunner::new()
        .config_str(client_only())
        .auto_configurations(auto_configurations())
        .exclude::<SecurityAutoConfiguration>()
        .run(|context| {
            assert!(context.contains::<OAuth2Client>());
            assert!(!context.contains::<SecurityFilterChain>());
        })
        .await;
}

#[test]
fn the_client_runs_after_the_http_client_and_security() {
    assert_eq!(
        OAuth2ClientAutoConfiguration::after(),
        [
            ConfigRef::of::<HttpClientAutoConfiguration>(),
            ConfigRef::of::<SecurityAutoConfiguration>(),
        ]
    );
}

const LOGIN_OFF: &str = "[security.oauth2.login]\nenabled = false\n";

/// The sentence that the startup failures of OAuth2 login show for a client-only application.
const OPT_OUT: &str = "If you only use the OAuth2 client (client credentials, refresh), set \
                       `security.oauth2.login.enabled = false`, supply your own \
                       `SecurityFilterChain` bean, or exclude `SecurityAutoConfiguration`.";

#[tokio::test]
async fn login_can_be_turned_off_for_a_client_only_application() {
    // No form login, no session, no base URL, every auto-configuration.
    let toml = format!("{}{LOGIN_OFF}", client_only());
    ContextRunner::new()
        .config_str(toml.clone())
        .auto_configurations(auto_configurations())
        .run(|context| {
            assert!(context.contains::<OAuth2Client>());
            assert!(context.contains::<SecurityFilterChain>());
        })
        .await;
    // The login endpoints are not served: the closed default chain answers 401 as for any
    // other path.
    let fake = FakeProvider::new(ISSUER);
    let app = TestApp::full()
        .config_str(toml)
        .configure(wiring(fake.client(), downstream()))
        .start()
        .await;
    let res = app
        .client()
        .get("/oauth2/authorization/billing")
        .send()
        .await;
    assert_eq!(res.status(), 401);
    let res = app.client().get("/no-such-path").send().await;
    assert_eq!(res.status(), 401);
    app.stop().await;
}

#[tokio::test]
async fn login_enabled_true_keeps_the_login() {
    let fake = FakeProvider::new(ISSUER);
    let toml = format!("{}[security.oauth2.login]\nenabled = true\n", config());
    let app = secured()
        .config_str(toml)
        .configure(wiring(fake.client(), downstream()))
        .start()
        .await;
    let res = app.client().get("/oauth2/authorization/idp").send().await;
    assert_eq!(res.status(), 302);
    assert!(
        res.header("location")
            .unwrap_or_default()
            .starts_with(&format!("{ISSUER}/authorize?"))
    );
    app.stop().await;
}

#[tokio::test]
async fn an_unknown_key_under_login_fails_the_boot_with_the_key() {
    let toml = format!("{}[security.oauth2.login]\nenable = false\n", client_only());
    ContextRunner::new()
        .config_str(toml)
        .auto_configurations(auto_configurations())
        .run_failing(|failure| {
            let error = failure.error();
            assert!(error.to_string().contains("security.oauth2"), "{error}");
            let analysis = error.analysis().expect("an analysis");
            assert!(
                analysis.description().contains("`enable`"),
                "{}",
                analysis.description()
            );
        })
        .await;
}

/// The analysis of the startup failure of `toml` with every auto-configuration.
async fn analysis_of(toml: String) -> (String, String) {
    let seen = Arc::new(Mutex::new((String::new(), String::new())));
    let out = seen.clone();
    ContextRunner::new()
        .config_str(toml)
        .auto_configurations(auto_configurations())
        .run_failing(move |failure| {
            let error = failure.error();
            assert!(!format!("{error} {error:?}").contains(SECRET));
            let analysis = error.analysis().expect("an analysis");
            *out.lock().unwrap() = (
                analysis.description().to_owned(),
                analysis.action().unwrap_or_default().to_owned(),
            );
        })
        .await;
    let guard = seen.lock().unwrap();
    guard.clone()
}

#[tokio::test]
async fn the_first_try_of_a_client_only_application_names_the_opt_outs() {
    // No base URL: the report says that the login needs it, and how to turn the login off.
    let (description, action) = analysis_of(client_only()).await;
    assert!(
        description.contains("security.oauth2.base-url"),
        "{description}"
    );
    assert!(description.contains("OAuth2 login"), "{description}");
    assert!(action.contains(OPT_OUT), "{action}");
    // With a base URL but no form login: the same opt-outs, and the advice for login users.
    let toml = format!(
        "[security.oauth2]\nbase-url = \"{BASE}\"\n{}",
        client_only()
    );
    let (description, action) = analysis_of(toml).await;
    assert!(description.contains("form login"), "{description}");
    assert!(
        action.contains("security.form-login.enabled = true"),
        "{action}"
    );
    assert!(action.contains(OPT_OUT), "{action}");
    assert!(!action.contains("remove the registrations"), "{action}");
}

// ---- Tokens through the bean ---------------------------------------------------------------

/// Permits `/downstream`; everything else stays authenticated.
struct OpenDownstream;

impl SecurityCustomizer for OpenDownstream {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.path("/downstream")
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}

#[tokio::test]
async fn a_booted_app_fetches_a_client_credentials_token_through_the_bean() {
    let fake = FakeProvider::new(ISSUER);
    fake.expect_client_secret(SECRET);
    let api = downstream();
    let app = secured()
        .config_str(config())
        .configure(wiring(fake.client(), api.clone()))
        .configure(|b| {
            b.instance(OpenDownstream)
                .provides(|c: Arc<OpenDownstream>| -> Arc<dyn SecurityCustomizer> { c })
        })
        .start()
        .await;
    for _ in 0..3 {
        let res = app.client().get("/downstream").send().await;
        assert_eq!(res.status(), 200);
        assert_eq!(res.text(), "orders");
    }
    // One token request for the three calls, with the client credentials of `billing`.
    let requests = fake.token_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].field("grant_type"), Some("client_credentials"));
    assert_eq!(requests[0].field("scope"), Some("invoices.read"));
    // Every downstream call carried the same bearer token, and the provider issued it to the
    // client `orders-service`: its user info answers for that token.
    let sent = api.requests();
    assert_eq!(sent.len(), 3);
    let bearer = sent[0].headers()["authorization"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        sent.iter()
            .all(|r| r.headers()["authorization"] == bearer.as_str())
    );
    let token = bearer.strip_prefix("Bearer ").unwrap();
    let info = fake
        .client()
        .execute(OutRequest::get(format!("{ISSUER}/userinfo")).header("authorization", &bearer))
        .await
        .unwrap();
    assert_eq!(info.status(), 200);
    let info: Value = info.json().unwrap();
    assert_eq!(info["sub"], "orders-service");
    assert!(!token.is_empty());
    app.stop().await;
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

/// One browser with its own cookies (the session cookie and the remember-me cookie).
struct Browser<'a> {
    app: &'a RunningApp,
    jar: Mutex<HashMap<String, String>>,
}

impl<'a> Browser<'a> {
    fn new(app: &'a RunningApp) -> Self {
        Self {
            app,
            jar: Mutex::new(HashMap::new()),
        }
    }

    fn drop_session(&self) {
        self.jar.lock().unwrap().remove(COOKIE);
    }

    fn has(&self, name: &str) -> bool {
        self.jar.lock().unwrap().contains_key(name)
    }

    fn take(&self, name: &str) -> Option<String> {
        self.jar.lock().unwrap().remove(name)
    }

    fn put(&self, name: &str, value: String) {
        self.jar.lock().unwrap().insert(name.to_owned(), value);
    }

    async fn send(&self, method: HttpMethod, path: &str, form: &str) -> TestResponse {
        let client = self.app.client();
        let mut req = match method {
            HttpMethod::Post => client
                .post(path)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(form.to_owned()),
            _ => client.get(path),
        }
        .header("accept", "text/html");
        let cookies: Vec<String> = self
            .jar
            .lock()
            .unwrap()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        if !cookies.is_empty() {
            req = req.header("cookie", cookies.join("; "));
        }
        let res = req.send().await;
        for value in res.header_all("set-cookie") {
            let mut parts = value.split("; ");
            let Some((name, id)) = parts.next().unwrap().split_once('=') else {
                continue;
            };
            let expired = parts.any(|p| p == "Max-Age=0");
            let mut jar = self.jar.lock().unwrap();
            if expired {
                jar.remove(name);
            } else {
                jar.insert(name.to_owned(), id.to_owned());
            }
        }
        res
    }

    async fn get(&self, path: &str) -> TestResponse {
        self.send(HttpMethod::Get, path, "").await
    }

    async fn text(&self, path: &str) -> String {
        let res = self.get(path).await;
        assert_eq!(res.status(), 200, "{path}: {}", res.text());
        res.text()
    }

    async fn tokens(&self) -> Value {
        serde_json::from_str(&self.text("/tokens").await).unwrap()
    }

    /// Logs in through `idp` with `fake` checking PKCE, the redirect URI and the nonce.
    async fn log_in(&self, fake: &FakeProvider, user: FakeUser) {
        let res = self.get("/oauth2/authorization/idp").await;
        assert_eq!(res.status(), 302, "{}", res.text());
        let params = query(res.header("location").unwrap());
        fake.expect_pkce(&params["code_challenge"]);
        fake.expect_redirect_uri(&format!("{BASE}/login/oauth2/code/idp"));
        fake.expect_nonce(&params["nonce"]);
        let code = fake.issue_code(user);
        let res = self
            .get(&format!(
                "/login/oauth2/code/idp?code={}&state={}",
                encode(&code),
                encode(&params["state"])
            ))
            .await;
        assert_eq!(res.status(), 302, "{}", res.text());
        assert_eq!(res.header("location"), Some("/"));
    }
}

fn alice() -> FakeUser {
    FakeUser::new("alice")
        .email("alice@example.test")
        .scope("openid email")
}

fn refresh_requests(fake: &FakeProvider) -> Vec<oxbloom_test::TokenRequest> {
    fake.token_requests()
        .into_iter()
        .filter(|r| r.field("grant_type") == Some("refresh_token"))
        .collect()
}

#[tokio::test]
async fn a_logged_in_user_token_is_refreshed_through_the_bean() {
    // The clock bean is years behind the system time; the fake signs with the same clock.
    let clock = ManualClock::at(start_time());
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
    fake.expect_client_secret(SECRET);
    // Ten minutes, shorter than the idle timeout of the session.
    fake.access_token_lifetime(Some(600));
    let app = secured()
        .config_str(config())
        .clock(Arc::new(clock.clone()))
        .configure(wiring(fake.client(), downstream()))
        .start()
        .await;
    let b = Browser::new(&app);
    b.log_in(&fake, alice()).await;
    let login = b.tokens().await;
    let login_access = login["access_token"].as_str().unwrap().to_owned();
    let login_refresh = login["refresh_token"].as_str().unwrap().to_owned();

    // Within its lifetime the stored token is returned as it is.
    assert_eq!(b.text("/user-token").await, login_access);
    assert!(refresh_requests(&fake).is_empty());

    // Past its expiry the token is refreshed with the stored refresh token.
    clock.advance(Duration::from_secs(600));
    let renewed = b.text("/user-token").await;
    assert!(!renewed.starts_with("error"), "{renewed}");
    assert_ne!(renewed, login_access);
    let refreshes = refresh_requests(&fake);
    assert_eq!(refreshes.len(), 1);
    assert_eq!(
        refreshes[0].field("refresh_token"),
        Some(login_refresh.as_str())
    );
    assert!(refreshes[0].field("scope").is_none());

    // The session holds the new tokens, so the next request of the same browser gets the
    // rotated token without asking the provider.
    assert_eq!(b.text("/user-token").await, renewed);
    assert_eq!(refresh_requests(&fake).len(), 1);
    let stored = b.tokens().await;
    assert_eq!(stored["access_token"], renewed.as_str());
    assert_ne!(stored["refresh_token"], login_refresh.as_str());
    assert_eq!(stored["id_token"], login["id_token"]);
    app.stop().await;
}

const PASSWORD: &str = "wonderland-for-the-oauth2-client-test";

#[tokio::test]
async fn a_password_user_cannot_get_the_oauth2_tokens_in_the_session() {
    let fake = FakeProvider::new(ISSUER);
    fake.expect_client_secret(SECRET);
    let hash = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap())
        .encode(PASSWORD);
    let toml = format!(
        "{}[security.csrf]\ntoken-endpoint = \"/csrf\"\n\
         [security.users.bob]\npassword = \"{hash}\"\nroles = [\"USER\"]\n",
        config()
    );
    let app = secured()
        .config_str(toml)
        .configure(wiring(fake.client(), downstream()))
        .start()
        .await;
    let b = Browser::new(&app);
    // An OAuth2 login first, so the session held tokens of `idp`; then a password login in
    // the same browser.
    b.log_in(&fake, alice()).await;
    let csrf: Value = serde_json::from_str(&b.text("/csrf").await).unwrap();
    let res = b
        .send(
            HttpMethod::Post,
            "/login",
            &format!(
                "username=bob&password={}&_csrf={}",
                encode(PASSWORD),
                encode(csrf["token"].as_str().unwrap())
            ),
        )
        .await;
    assert_eq!(res.status(), 302, "{}", res.text());
    assert_ne!(res.header("location"), Some("/login?error"));
    // The password login removed the tokens of the OAuth2 login. Put a usable token record of
    // `idp` back into the session of the password user: only the binding check stands between
    // that user and the tokens now.
    assert_eq!(b.tokens().await, Value::Null);
    assert_eq!(b.text("/plant-tokens").await, "planted");
    assert_eq!(b.tokens().await["access_token"], PLANTED_ACCESS_TOKEN);
    let answer = b.text("/user-token").await;
    assert_eq!(
        answer,
        format!("error: {}", TokenError::ReauthenticationRequired)
    );
    // Only the code exchange of the OAuth2 login reached the token endpoint.
    assert_eq!(fake.token_requests().len(), 1);
    app.stop().await;
}

/// The configuration with the `github`-like plain registration `gh` next to `idp` and
/// `billing`.
fn config_with_second_login() -> String {
    format!(
        "{}\n[security.oauth2.client.registration.gh]\nclient-id = \"gh-app\"\n\
         client-secret = \"{SECRET}\"\nscope = [\"read:user\"]\n\
         [security.oauth2.client.provider.gh]\nauthorization-uri = \"{ISSUER}/authorize\"\n\
         token-uri = \"{ISSUER}/token\"\nuser-info-uri = \"{ISSUER}/userinfo\"\n\
         user-name-attribute = \"sub\"\n",
        config()
    )
}

#[tokio::test]
async fn an_oauth2_login_of_another_registration_removes_the_tokens_of_the_first() {
    let fake = FakeProvider::new(ISSUER);
    let app = secured()
        .config_str(config_with_second_login())
        .configure(wiring(fake.client(), downstream()))
        .start()
        .await;
    let b = Browser::new(&app);
    b.log_in(&fake, alice()).await;
    assert!(b.tokens().await.is_object());
    // A login with `gh` in the same browser.
    let res = b.get("/oauth2/authorization/gh").await;
    assert_eq!(res.status(), 302, "{}", res.text());
    let params = query(res.header("location").unwrap());
    fake.expect_pkce(&params["code_challenge"]);
    fake.expect_redirect_uri(&format!("{BASE}/login/oauth2/code/gh"));
    let code = fake.issue_code(FakeUser::new("alice-gh").scope("read:user"));
    let res = b
        .get(&format!(
            "/login/oauth2/code/gh?code={}&state={}",
            encode(&code),
            encode(&params["state"])
        ))
        .await;
    assert_eq!(res.status(), 302, "{}", res.text());
    assert_eq!(res.header("location"), Some("/"));
    // `/tokens` shows the tokens of `idp`: gone.
    assert_eq!(b.tokens().await, Value::Null);
    assert_eq!(
        b.text("/user-token").await,
        format!("error: {}", TokenError::ReauthenticationRequired)
    );
    app.stop().await;
}

/// Permits `/plant-tokens` and `/tokens`, so an anonymous session can hold tokens.
struct OpenTokens;

impl SecurityCustomizer for OpenTokens {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.path("/plant-tokens")
                .permit_all()
                .path("/tokens")
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}

#[tokio::test]
async fn a_remember_me_auto_login_clears_the_oauth2_tokens_of_the_session() {
    let fake = FakeProvider::new(ISSUER);
    let hash = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap())
        .encode(PASSWORD);
    let toml = format!(
        "{}[security.csrf]\nenabled = false\n\
         [security.remember-me]\nenabled = true\nsecret = \"a-remember-me-secret-of-32-bytes\"\n\
         [security.users.bob]\npassword = \"{hash}\"\nroles = [\"USER\"]\n",
        config()
    );
    let app = secured()
        .config_str(toml)
        .configure(wiring(fake.client(), downstream()))
        .configure(|b| {
            b.instance(OpenTokens)
                .provides(|c: Arc<OpenTokens>| -> Arc<dyn SecurityCustomizer> { c })
        })
        .start()
        .await;
    let b = Browser::new(&app);
    let res = b
        .send(
            HttpMethod::Post,
            "/login",
            &format!("username=bob&password={}&remember-me=on", encode(PASSWORD)),
        )
        .await;
    assert_eq!(res.status(), 302, "{}", res.text());
    assert!(b.has("OXBLOOM_REMEMBER_ME"));
    // A new anonymous session that holds tokens of `idp`, made without the remember-me cookie
    // so that no auto login happens yet.
    b.drop_session();
    let remember = b.take("OXBLOOM_REMEMBER_ME").unwrap();
    assert_eq!(b.text("/plant-tokens").await, "planted");
    assert_eq!(b.tokens().await["access_token"], PLANTED_ACCESS_TOKEN);
    b.put("OXBLOOM_REMEMBER_ME", remember);
    // A protected request logs bob in again from the cookie; his session holds no OAuth2 tokens.
    let res = b.get("/user-token").await;
    assert_eq!(res.status(), 200, "{}", res.text());
    assert_eq!(
        res.text(),
        format!("error: {}", TokenError::ReauthenticationRequired)
    );
    assert_eq!(b.tokens().await, Value::Null);
    app.stop().await;
}

// ---- Startup failures ----------------------------------------------------------------------

#[tokio::test]
async fn a_broken_registration_fails_the_boot_with_the_key() {
    // A registration without a client id; only the client auto-configuration reads it (no
    // form login, no base URL), so the failure comes from the client wiring.
    let toml = format!(
        "[security.oauth2.client.registration.billing]\nclient-secret = \"{SECRET}\"\n\
         provider = \"idp\"\n[security.oauth2.client.provider.idp]\nissuer-uri = \"{ISSUER}\"\n"
    );
    let key = "security.oauth2.client.registration.billing.client-id";
    let fake = FakeProvider::new(ISSUER);
    let provider = fake.client();
    ContextRunner::new()
        .config_str(toml)
        .auto_configuration::<OAuth2ClientAutoConfiguration>()
        .configure(move |b| {
            b.instance(provider)
                .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
        })
        .run_failing(move |failure| {
            let error = failure.error();
            assert_eq!(error.phase(), StartupPhase::Container, "{error}");
            let di = error.find_cause::<DiError>().expect("a DiError cause");
            assert!(matches!(di, DiError::Creation { .. }), "{di:?}");
            let cause = error
                .find_cause::<ConfigError>()
                .expect("a ConfigError cause");
            assert!(
                matches!(cause, ConfigError::Invalid { key: k, .. } if k == key),
                "{cause:?}"
            );
            let output = failure.output().contents();
            assert!(error.to_string().contains(key), "{error}");
            assert!(output.contains(key), "{output}");
            assert!(output.contains("Action"), "{output}");
            let all = format!("{error} {error:?} {output}");
            assert!(!all.contains(SECRET), "the secret leaked: {all}");
            let analysis = error.analysis().expect("an analysis");
            assert!(
                analysis.description().contains(key),
                "{}",
                analysis.description()
            );
        })
        .await;
}

#[tokio::test]
async fn registrations_without_an_http_client_bean_name_the_cause() {
    ContextRunner::new()
        .config_str(registrations())
        .auto_configuration::<OAuth2ClientAutoConfiguration>()
        .run_failing(|failure| {
            let error = failure.error();
            assert!(error.to_string().contains("`HttpClient` bean"), "{error}");
            let analysis = error.analysis().expect("an analysis");
            assert!(
                analysis.description().contains("`HttpClient`"),
                "{}",
                analysis.description()
            );
            assert!(!format!("{error:?}").contains(SECRET));
        })
        .await;
}
