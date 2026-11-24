//! OAuth2 login in boot: `SecurityAutoConfiguration` turns on `oauth2_login` from the
//! `security.oauth2` keys. A booted application with a `FakeProvider` as its `dyn HttpClient`
//! bean is driven over HTTP; the browser follows the redirects and carries the session cookie
//! by hand. The startup failures name their key and never show a secret or a value.

#![cfg(all(feature = "oauth2", feature = "web"))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use oxbloom_boot::{
    HttpClientAutoConfiguration, SecurityCustomizer, StartupPhase, auto_configurations,
};
use oxbloom_config::{ConfigError, Secret};
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_http_client::HttpClient;
use oxbloom_security::{
    ClientAuthentication, ClientRegistration, HttpSecurity, OAuth2Login, ProviderConfig,
    RequestSecurityExt,
};
use oxbloom_test::{
    ContextRunner, FakeProvider, FakeUser, ManualClock, MockHttpClient, RunningApp, TestApp,
    TestResponse,
};
use oxbloom_web::{Controller, HttpMethod, Request, Route};
use serde_json::{Value, json};

const ISSUER: &str = "https://idp.example.test";
const BASE: &str = "https://app.example.test";
const SECRET: &str = "app-secret-for-the-boot-oauth2-tests";
const COOKIE: &str = "OXBLOOM_SESSION";
const BASE_URL_KEY: &str = "security.oauth2.base-url";

/// Form login, the session table, the base URL and two registrations: `idp` (OpenID Connect,
/// through discovery) and `github` (plain OAuth2, the numeric `id` names the user).
fn config(oauth2: &str) -> String {
    format!(
        "[security.form-login]\nenabled = true\n[security.session]\n{}",
        registrations(oauth2)
    )
}

/// The `security.oauth2` section with `oauth2` as its own keys.
fn registrations(oauth2: &str) -> String {
    format!(
        r#"[security.oauth2]
{oauth2}
[security.oauth2.client.registration.idp]
client-id = "app"
client-secret = "{SECRET}"
scope = ["email"]

[security.oauth2.client.provider.idp]
issuer-uri = "{ISSUER}"

[security.oauth2.client.registration.github]
client-id = "gh-app"
client-secret = "{SECRET}"
scope = ["read:user"]

[security.oauth2.client.provider.github]
authorization-uri = "{ISSUER}/authorize"
token-uri = "{ISSUER}/token"
user-info-uri = "{ISSUER}/userinfo"
user-name-attribute = "id"
"#
    )
}

fn base_url() -> String {
    format!("base-url = \"{BASE}\"")
}

/// `/me`: the principal, the authorities and the details, or `null`.
struct Routes;

impl Controller for Routes {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(
            HttpMethod::Get,
            "/me",
            |req: Request| async move {
                match req.authentication() {
                Some(a) => json!({
                    "principal": a.principal(),
                    "authorities": a.authorities().iter().map(|g| g.as_str()).collect::<Vec<_>>(),
                    "details": a.details(),
                })
                .to_string(),
                None => "null".to_owned(),
            }
            },
        )]
    }
}

/// Registers the controller and `client` as the `dyn HttpClient` bean.
fn wiring(client: Arc<MockHttpClient>) -> impl FnOnce(ContainerBuilder) -> ContainerBuilder {
    move |b| {
        b.instance(Routes)
            .provides(|c: Arc<Routes>| -> Arc<dyn Controller> { c })
            .instance(client)
            .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
    }
}

fn start_time() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000)
}

/// A fake provider that checks the client secret.
fn fake() -> FakeProvider {
    let fake = FakeProvider::new(ISSUER);
    fake.expect_client_secret(SECRET);
    fake
}

async fn start(toml: String, fake: &FakeProvider) -> RunningApp {
    TestApp::secured()
        .config_str(toml)
        .configure(wiring(fake.client()))
        .start()
        .await
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

/// One browser with its own session cookie.
struct Browser<'a> {
    app: &'a RunningApp,
    jar: Mutex<Option<String>>,
}

impl<'a> Browser<'a> {
    fn new(app: &'a RunningApp) -> Self {
        Self {
            app,
            jar: Mutex::new(None),
        }
    }

    async fn get(&self, path: &str) -> TestResponse {
        let mut req = self.app.client().get(path).header("accept", "text/html");
        if let Some(id) = self.jar.lock().unwrap().clone() {
            req = req.header("cookie", format!("{COOKIE}={id}"));
        }
        let res = req.send().await;
        for value in res.header_all("set-cookie") {
            if let Some(rest) = value.strip_prefix(&format!("{COOKIE}=")) {
                let mut parts = rest.split("; ");
                let id = parts.next().unwrap().to_owned();
                let expired = parts.any(|p| p == "Max-Age=0");
                *self.jar.lock().unwrap() = (!expired).then_some(id);
            }
        }
        res
    }

    async fn me(&self) -> Value {
        let res = self.get("/me").await;
        assert_eq!(res.status(), 200, "{}", res.text());
        serde_json::from_str(&res.text()).unwrap()
    }

    /// Starts a login and returns the parameters of the authorization URL.
    async fn start(&self, id: &str) -> HashMap<String, String> {
        let res = self.get(&format!("/oauth2/authorization/{id}")).await;
        assert_eq!(res.status(), 302, "{}", res.text());
        let target = res.header("location").unwrap_or_default().to_owned();
        assert!(
            target.starts_with(&format!("{ISSUER}/authorize?")),
            "{target}"
        );
        query(&target)
    }

    async fn callback(&self, id: &str, code: &str, state: &str) -> TestResponse {
        self.get(&format!(
            "/login/oauth2/code/{id}?code={}&state={}",
            encode(code),
            encode(state)
        ))
        .await
    }

    /// Starts a login for `id`, lets `fake` issue a code for `user` with the checks on, and
    /// calls back.
    async fn log_in(&self, fake: &FakeProvider, id: &str, user: FakeUser) -> TestResponse {
        let params = self.start(id).await;
        fake.expect_pkce(&params["code_challenge"]);
        fake.expect_redirect_uri(&format!("{BASE}/login/oauth2/code/{id}"));
        if let Some(nonce) = params.get("nonce") {
            fake.expect_nonce(nonce);
        }
        let code = fake.issue_code(user);
        self.callback(id, &code, &params["state"]).await
    }
}

fn alice() -> FakeUser {
    FakeUser::new("alice")
        .email("alice@example.test")
        .scope("openid email")
}

// ---- Flows ---------------------------------------------------------------------------------

#[tokio::test]
async fn a_full_oidc_login_through_a_booted_app() {
    // The application's clock bean is years behind the system time, and the fake signs with
    // the same clock: the id token (valid for an hour) passes only if OAuth2 login uses it.
    let clock = ManualClock::at(start_time());
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(clock.clone()));
    fake.expect_client_secret(SECRET);
    let app = TestApp::secured()
        .config_str(config(&base_url()))
        .clock(Arc::new(clock))
        .configure(wiring(fake.client()))
        .start()
        .await;
    let b = Browser::new(&app);
    let params = b.start("idp").await;
    assert_eq!(params["client_id"], "app");
    assert_eq!(
        params["redirect_uri"],
        format!("{BASE}/login/oauth2/code/idp")
    );
    assert_eq!(params["code_challenge_method"], "S256");
    assert!(params.contains_key("nonce"));
    fake.expect_pkce(&params["code_challenge"]);
    fake.expect_redirect_uri(&format!("{BASE}/login/oauth2/code/idp"));
    fake.expect_nonce(&params["nonce"]);
    let code = fake.issue_code(alice());
    let res = b.callback("idp", &code, &params["state"]).await;
    assert_eq!(res.status(), 302);
    assert_eq!(res.header("location"), Some("/"));
    let me = b.me().await;
    assert_eq!(me["principal"], "alice");
    assert_eq!(me["details"]["oauth2.registration"], "idp");
    assert_eq!(fake.token_requests().len(), 1);
    app.stop().await;
}

#[tokio::test]
async fn a_plain_oauth2_login_through_a_booted_app() {
    let fake = fake();
    let app = start(config(&base_url()), &fake).await;
    let b = Browser::new(&app);
    let params = b.start("github").await;
    assert!(!params.contains_key("nonce"));
    assert_eq!(params["scope"], "read:user");
    fake.expect_pkce(&params["code_challenge"]);
    // GitHub's `id` is a stable number; `login` can be renamed and claimed again.
    let user = FakeUser::new("gh-subject")
        .attr("id", 4242)
        .attr("login", "alice-gh")
        .scope("read:user");
    let code = fake.issue_code(user);
    let res = b.callback("github", &code, &params["state"]).await;
    assert_eq!(res.header("location"), Some("/"));
    let me = b.me().await;
    assert_eq!(me["principal"], "4242");
    assert_eq!(me["authorities"], json!(["OAUTH2_USER", "SCOPE_read:user"]));
    assert_eq!(me["details"]["oauth2.registration"], "github");
    app.stop().await;
}

#[tokio::test]
async fn an_authenticated_oauth2_user_reaches_a_protected_route_with_the_right_authorities() {
    let fake = fake();
    let app = start(config(&base_url()), &fake).await;
    let b = Browser::new(&app);
    // No customizer: every route needs authentication, and a browser goes to the login page.
    let res = b.get("/me").await;
    assert_eq!(res.status(), 302);
    assert_eq!(res.header("location"), Some("/login"));
    let res = b.log_in(&fake, "idp", alice()).await;
    // Back to the saved request.
    assert_eq!(res.header("location"), Some("/me"));
    let me = b.me().await;
    assert_eq!(me["principal"], "alice");
    assert_eq!(
        me["authorities"],
        json!(["OIDC_USER", "SCOPE_email", "SCOPE_openid"])
    );
    assert_eq!(me["details"]["oauth2.principal"], true);
    app.stop().await;
}

#[tokio::test]
async fn the_failure_url_key_is_used() {
    let fake = fake();
    let toml = config(&format!("{}\nfailure-url = \"/signin?failed\"", base_url()));
    let app = start(toml, &fake).await;
    let b = Browser::new(&app);
    let params = b.start("idp").await;
    let res = b.callback("idp", "no-such-code", &params["state"]).await;
    assert_eq!(res.status(), 302);
    assert_eq!(res.header("location"), Some("/signin?failed"));
    app.stop().await;
}

/// Logs in through `oauth2_login` of the customizer, with its own registration `custom`.
struct OwnOAuth2(Arc<MockHttpClient>);

impl SecurityCustomizer for OwnOAuth2 {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        let client: Arc<dyn HttpClient> = self.0.clone();
        http.oauth2_login(|_| {
            OAuth2Login::new(client)
                .registration(ClientRegistration {
                    id: "custom".into(),
                    client_id: "custom-app".into(),
                    client_secret: Some(Secret::new(SECRET.into())),
                    scopes: vec!["email".into()],
                    redirect_uri: "{base-url}/login/oauth2/code/{id}".into(),
                    client_authentication: ClientAuthentication::Basic,
                    provider: "idp".into(),
                })
                .provider(
                    "idp",
                    ProviderConfig {
                        issuer_uri: Some(ISSUER.into()),
                        ..ProviderConfig::default()
                    },
                )
                .base_url(BASE)
        })
        .authorize(|a| a.any_request().authenticated())
    }
}

#[tokio::test]
async fn the_application_customizer_wins() {
    let fake = fake();
    let client = fake.client();
    // No base URL in the configuration: boot steps aside, so it does not need one.
    let app = TestApp::secured()
        .config_str(config(""))
        .configure(wiring(fake.client()))
        .configure(move |b| {
            b.instance(OwnOAuth2(client))
                .provides(|c: Arc<OwnOAuth2>| -> Arc<dyn SecurityCustomizer> { c })
        })
        .start()
        .await;
    let b = Browser::new(&app);
    let params = b.start("custom").await;
    assert_eq!(params["client_id"], "custom-app");
    // The registrations of the configuration are not used: `idp` is an unknown id.
    let res = b.get("/oauth2/authorization/idp").await;
    assert_eq!(res.header("location"), Some("/login?error"));
    app.stop().await;
}

// ---- Startup failures ----------------------------------------------------------------------

/// Startup fails in the container phase; the error and the report name `key`, the report has
/// an action, and neither contains the client secret or any of `hidden`. Returns the
/// analysis description and action.
async fn fails(toml: String, key: &str, hidden: &[&str]) -> (String, String) {
    fails_with(toml, key, hidden, |b| b).await
}

async fn fails_with(
    toml: String,
    key: &str,
    hidden: &[&str],
    exclude: fn(ContextRunner) -> ContextRunner,
) -> (String, String) {
    let key = key.to_owned();
    let hidden: Vec<String> = hidden.iter().map(|h| (*h).to_owned()).collect();
    let seen = Arc::new(Mutex::new((String::new(), String::new())));
    let out = seen.clone();
    exclude(
        ContextRunner::new()
            .config_str(toml)
            .auto_configurations(auto_configurations()),
    )
    .run_failing(move |failure| {
        let error = failure.error();
        assert_eq!(error.phase(), StartupPhase::Container, "{error}");
        let di = error.find_cause::<DiError>().expect("a DiError cause");
        assert!(matches!(di, DiError::Creation { .. }), "{di:?}");
        let output = failure.output().contents();
        assert!(error.to_string().contains(&key), "{error}");
        assert!(output.contains(&key), "{output}");
        assert!(output.contains("Action"), "{output}");
        let all = format!("{error} {error:?} {output}");
        assert!(!all.contains(SECRET), "the secret leaked: {all}");
        for value in &hidden {
            assert!(!all.contains(value.as_str()), "{value} leaked: {all}");
        }
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
async fn a_missing_base_url_fails_the_boot_with_the_key() {
    let (description, _) = fails(config(""), BASE_URL_KEY, &[]).await;
    assert!(description.contains(BASE_URL_KEY), "{description}");
    // The typed source is a missing key.
    ContextRunner::new()
        .config_str(config(""))
        .auto_configurations(auto_configurations())
        .run_failing(|failure| {
            let cause = failure.error().find_cause::<ConfigError>().unwrap();
            assert!(matches!(cause, ConfigError::Missing { key } if key == BASE_URL_KEY));
        })
        .await;
}

#[tokio::test]
async fn a_base_url_with_a_path_is_refused_with_the_key() {
    for value in [
        "https://app.example.test/hidden-prefix",
        "https://app.example.test/hidden-prefix/",
        "https://app.example.test/?q=hidden-query",
        "https://app.example.test#hidden-fragment",
        "ftp://app.example.test",
        "http://app.example.test",
        "app.example.test",
    ] {
        let (description, action) = fails(
            config(&format!("base-url = \"{value}\"")),
            BASE_URL_KEY,
            &["hidden-query", "hidden-fragment", "hidden-prefix"],
        )
        .await;
        assert!(description.contains(BASE_URL_KEY), "{value}: {description}");
        assert!(action.contains("no path"), "{value}: {action}");
    }
}

#[tokio::test]
async fn a_base_url_with_a_trailing_slash_or_a_loopback_http_host_is_accepted() {
    for value in ["https://app.example.test/", "http://localhost:8080"] {
        let fake = fake();
        let app = start(config(&format!("base-url = \"{value}\"")), &fake).await;
        let params = Browser::new(&app).start("github").await;
        assert_eq!(
            params["redirect_uri"],
            format!("{}/login/oauth2/code/github", value.trim_end_matches('/'))
        );
        app.stop().await;
    }
}

#[tokio::test]
async fn a_broken_registration_fails_the_boot_with_the_key_and_no_secret() {
    let toml = format!(
        "[security.form-login]\nenabled = true\n[security.session]\n[security.oauth2]\n{}\n\
         [security.oauth2.client.registration.idp]\nclient-secret = \"{SECRET}\"\n\
         [security.oauth2.client.provider.idp]\nissuer-uri = \"{ISSUER}\"\n",
        base_url()
    );
    let key = "security.oauth2.client.registration.idp.client-id";
    let (description, _) = fails(toml, key, &[]).await;
    assert!(description.contains(key), "{description}");
}

#[tokio::test]
async fn a_custom_redirect_uri_fails_the_boot_with_the_registration_key() {
    let toml = config(&base_url()).replace(
        "scope = [\"email\"]",
        "scope = [\"email\"]\nredirect-uri = \"{base-url}/hidden-callback/{id}\"",
    );
    let key = "security.oauth2.client.registration.idp.redirect-uri";
    let (description, action) = fails(toml, key, &["hidden-callback"]).await;
    assert!(description.contains(key), "{description}");
    assert!(action.contains("/login/oauth2/code/{id}"), "{action}");
}

#[tokio::test]
async fn oauth2_without_form_login_tells_the_user_to_enable_it() {
    let toml = format!("[security.session]\n{}", registrations(&base_url()));
    let (description, action) = fails(toml, "security.form-login.enabled", &[]).await;
    assert!(description.contains("form login"), "{description}");
    assert!(
        action.contains("security.form-login.enabled = true"),
        "{action}"
    );
}

#[tokio::test]
async fn oauth2_without_an_http_client_bean_names_the_cause() {
    let (description, action) = fails_with(config(&base_url()), "HttpClient", &[], |runner| {
        runner.exclude::<HttpClientAutoConfiguration>()
    })
    .await;
    assert!(description.contains("`HttpClient`"), "{description}");
    assert!(action.contains("HttpClientAutoConfiguration"), "{action}");
}

#[tokio::test]
async fn the_failure_analyzer_names_the_key() {
    // An unsafe failure URL.
    let toml = config(&format!(
        "{}\nfailure-url = \"//hidden-evil.example.test/x\"",
        base_url()
    ));
    let key = "security.oauth2.failure-url";
    let (description, action) = fails(toml, key, &["hidden-evil"]).await;
    assert!(description.contains(key), "{description}");
    assert!(action.contains("local path"), "{action}");
    // A failure URL on an OAuth2 endpoint would loop.
    let toml = config(&format!(
        "{}\nfailure-url = \"/login/oauth2/code/idp\"",
        base_url()
    ));
    fails(toml, key, &[]).await;
    // An unknown key of the section: the report names it and still shows no value.
    let toml = config(&format!("{}\nbase-path = \"/hidden-value\"", base_url()));
    let (description, _) = fails(toml, "security.oauth2", &["hidden-value"]).await;
    assert!(description.contains("`base-path`"), "{description}");
    let toml = config(&base_url()).replace(
        "[security.oauth2.client.registration.idp]",
        "[security.oauth2.client.registrations.idp]\nclient-id = \"hidden-id\"\n\
         [security.oauth2.client.registration.idp]",
    );
    let (description, _) = fails(toml, "security.oauth2.client", &["hidden-id"]).await;
    assert!(description.contains("`registrations`"), "{description}");
}
