//! CSRF tokens in `SecurityAutoConfiguration`: the `security.csrf` keys, the login flow with
//! the token over HTTP with both session backends, the startup failures that never echo a
//! value and the metadata that lists exactly the keys read.

#![cfg(all(feature = "web", feature = "security"))]

use std::sync::{Arc, Mutex};

use http::StatusCode;
use oxbloom_boot::{
    AutoConfiguration, SecurityAutoConfiguration, StartupPhase, auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_security::{
    Argon2PasswordEncoder, CsrfConfig, CsrfProperties, DelegatingPasswordEncoder, PasswordEncoder,
    SecurityBuildError,
};
use oxbloom_test::{ContextRunner, RunningApp, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";
const SESSION: &str = "[security.session]\nstore = \"memory\"\n";
const FORM: &str = "[security.form-login]\nenabled = true\n";

fn hash(raw: &str) -> String {
    DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap()).encode(raw)
}

fn users() -> String {
    format!(
        "[security.users.alice]\npassword = \"{}\"\nroles = [\"USER\"]\n",
        hash("wonderland")
    )
}

struct Routes;

impl Controller for Routes {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/secret", |_| async {
            "secret"
        })]
    }
}

fn wiring(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Routes)
        .provides(|c: Arc<Routes>| -> Arc<dyn Controller> { c })
}

async fn start(toml: String) -> RunningApp {
    TestApp::secured()
        .config_str(toml)
        .configure(wiring)
        .start()
        .await
}

struct Reply {
    status: StatusCode,
    location: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// A browser: the session cookie of the last response that set one.
#[derive(Default)]
struct Jar(Mutex<Option<String>>);

impl Jar {
    async fn send(&self, app: &RunningApp, method: HttpMethod, path: &str, body: &str) -> Reply {
        let client = app.client();
        let mut req = match method {
            HttpMethod::Post => client.post(path),
            _ => client.get(path),
        }
        .header("accept", "text/html");
        if let Some(cookie) = self.0.lock().unwrap().clone() {
            req = req.header("cookie", cookie);
        }
        if !body.is_empty() {
            req = req
                .header("content-type", "application/x-www-form-urlencoded")
                .body(body.to_owned());
        }
        let res = req.send().await;
        if let Some(set) = res.header("set-cookie") {
            let pair = set.split(';').next().unwrap().to_owned();
            let gone = set.contains("Max-Age=0");
            *self.0.lock().unwrap() = if gone { None } else { Some(pair) };
        }
        Reply {
            status: res.status(),
            location: res.header("location").unwrap_or_default().to_owned(),
            headers: ["cache-control", "content-type"]
                .iter()
                .filter_map(|n| res.header(n).map(|v| ((*n).to_owned(), v.to_owned())))
                .collect(),
            body: res.text(),
        }
    }

    /// Reads the token from the hidden field of the default login page.
    async fn token_from_login_page(&self, app: &RunningApp, field: &str) -> String {
        let page = self.send(app, HttpMethod::Get, "/login", "").await;
        assert_eq!(page.status, StatusCode::OK);
        let marker = format!("name=\"{field}\" value=\"");
        let start = page.body.find(&marker).expect("a hidden token field") + marker.len();
        let end = page.body[start..].find('"').unwrap() + start;
        page.body[start..end].to_owned()
    }
}

async fn flow(toml: String) {
    let app = start(toml).await;
    let jar = Jar::default();
    let r = jar.send(&app, HttpMethod::Get, "/secret", "").await;
    assert_eq!(
        (r.status, r.location.as_str()),
        (StatusCode::FOUND, "/login")
    );
    let token = jar.token_from_login_page(&app, "_csrf").await;
    let r = jar
        .send(
            &app,
            HttpMethod::Post,
            "/login",
            &format!("username=alice&password=wrong&_csrf={token}"),
        )
        .await;
    assert_eq!(
        (r.status, r.location.as_str()),
        (StatusCode::FOUND, "/login?error")
    );
    let r = jar
        .send(
            &app,
            HttpMethod::Post,
            "/login",
            &format!("username=alice&password=wonderland&_csrf={token}"),
        )
        .await;
    assert_eq!(
        (r.status, r.location.as_str()),
        (StatusCode::FOUND, "/secret")
    );
    let r = jar.send(&app, HttpMethod::Get, "/secret", "").await;
    assert_eq!(r.status, StatusCode::OK);
    // The login replaced the token, so the old one no longer logs out.
    let r = jar
        .send(&app, HttpMethod::Post, "/logout", &format!("_csrf={token}"))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    app.stop().await;
}

#[tokio::test]
async fn login_with_the_token_over_http() {
    flow(format!("{FORM}{SESSION}{}", users())).await;
}

#[tokio::test]
async fn works_with_the_cookie_session_backend() {
    flow(format!(
        "{FORM}[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n{}",
        users()
    ))
    .await;
}

#[tokio::test]
async fn a_post_without_the_token_is_403() {
    let app = start(format!("{FORM}{SESSION}{}", users())).await;
    let jar = Jar::default();
    jar.token_from_login_page(&app, "_csrf").await;
    let r = jar
        .send(
            &app,
            HttpMethod::Post,
            "/login",
            "username=alice&password=wonderland",
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Boot's error rendering turns the empty 403 into a problem document without a detail.
    assert!(
        !r.body.contains("csrf") && !r.body.contains("token"),
        "{}",
        r.body
    );
    app.stop().await;
}

#[tokio::test]
async fn token_endpoint_over_http() {
    let app = start(format!(
        "{FORM}{SESSION}{}[security.csrf]\ntoken-endpoint = \"/csrf\"\nfield-name = \"tok\"\n\
         header-name = \"X-Tok\"\n",
        users()
    ))
    .await;
    let jar = Jar::default();
    let r = jar.send(&app, HttpMethod::Get, "/csrf", "").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let json: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(json["headerName"], "X-Tok");
    assert_eq!(json["parameterName"], "tok");
    let token = json["token"].as_str().unwrap().to_owned();
    // The login page of the same session carries the same token under the configured name.
    assert_eq!(jar.token_from_login_page(&app, "tok").await, token);
    let r = jar
        .send(
            &app,
            HttpMethod::Post,
            "/login",
            &format!("username=alice&password=wonderland&tok={token}"),
        )
        .await;
    assert_eq!(r.status, StatusCode::FOUND);
    app.stop().await;
}

#[tokio::test]
async fn disabled_keeps_the_6c_behavior() {
    let app = start(format!(
        "{FORM}[security.csrf]\nenabled = false\n{SESSION}{}",
        users()
    ))
    .await;
    let jar = Jar::default();
    let r = jar
        .send(
            &app,
            HttpMethod::Post,
            "/login",
            "username=alice&password=wonderland",
        )
        .await;
    assert_eq!((r.status, r.location.as_str()), (StatusCode::FOUND, "/"));
    let r = jar.send(&app, HttpMethod::Post, "/logout", "").await;
    assert_eq!(r.location, "/login?logout");
    app.stop().await;
}

async fn fails(
    toml: String,
    check: impl FnOnce(&oxbloom_boot::StartupError, &str) + Send + 'static,
) {
    ContextRunner::new()
        .config_str(toml)
        .auto_configurations(auto_configurations())
        .run_failing(move |failure| {
            let error = failure.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let di = error.find_cause::<DiError>().expect("a DiError cause");
            assert!(matches!(di, DiError::Creation { .. }), "{di:?}");
            let text = format!("{error} {error:?} {}", failure.output().contents());
            check(error, &text);
        })
        .await;
}

#[tokio::test]
async fn bad_endpoint_fails_startup_without_echoing_it() {
    fails(
        format!(
            "{FORM}[security.csrf]\ntoken-endpoint = \"https://evil.example/x\"\n{SESSION}{}",
            users()
        ),
        |error, text| {
            match error.find_cause::<ConfigError>().expect("a ConfigError") {
                ConfigError::Invalid { key, .. } => {
                    assert_eq!(key, "security.csrf.token-endpoint")
                }
                other => panic!("expected Invalid, got {other:?}"),
            }
            assert!(!text.contains("evil.example"), "{text}");
        },
    )
    .await;
}

#[tokio::test]
async fn endpoint_equal_to_a_form_login_path_fails_the_build() {
    fails(
        format!(
            "{FORM}[security.csrf]\ntoken-endpoint = \"/login\"\n{SESSION}{}",
            users()
        ),
        |error, _| {
            let cause = error
                .find_cause::<SecurityBuildError>()
                .expect("a SecurityBuildError cause");
            assert!(matches!(cause, SecurityBuildError::InvalidCsrfConfig));
        },
    )
    .await;
}

#[tokio::test]
async fn csrf_without_form_login_fails_startup_naming_the_key() {
    fails(
        format!("[security.csrf]\nenabled = true\n{SESSION}{}", users()),
        |error, _| match error.find_cause::<ConfigError>().expect("a ConfigError") {
            ConfigError::Invalid { key, .. } => assert_eq!(key, "security.csrf.enabled"),
            other => panic!("expected Invalid, got {other:?}"),
        },
    )
    .await;
}

#[tokio::test]
async fn csrf_off_without_form_login_starts_and_changes_nothing() {
    let app = start(format!(
        "[security.csrf]\nenabled = false\n{SESSION}{}",
        users()
    ))
    .await;
    let r = app.client().get("/secret").send().await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    app.stop().await;
}

/// A TOML line for each key of the metadata and the value it changes.
fn sample(key: &str) -> &'static str {
    match key {
        "security.csrf.enabled" => "false",
        "security.csrf.field-name" => "\"tok\"",
        "security.csrf.header-name" => "\"X-Tok\"",
        "security.csrf.token-endpoint" => "\"/csrf\"",
        other => panic!("no sample for {other}"),
    }
}

#[test]
fn metadata_lists_exactly_the_keys_read() {
    let keys: Vec<&str> = SecurityAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .filter(|k| k.starts_with("security.csrf."))
        .collect();
    assert_eq!(
        keys,
        [
            "security.csrf.enabled",
            "security.csrf.field-name",
            "security.csrf.header-name",
            "security.csrf.token-endpoint",
        ]
    );
    // Every listed key is read: alone, it changes a value.
    let default = CsrfConfig::default();
    for key in keys {
        let name = key.strip_prefix("security.csrf.").unwrap();
        let toml = format!("[security.csrf]\n{name} = {}\n", sample(key));
        let csrf = CsrfProperties::from_config(&Config::from_str(&toml).unwrap(), true)
            .unwrap()
            .unwrap();
        assert_ne!(csrf, default, "{key} is not read");
    }
}
