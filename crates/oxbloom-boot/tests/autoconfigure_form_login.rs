//! Form login in `SecurityAutoConfiguration`: the `security.form-login` keys, the login flow
//! over HTTP with both session backends, the startup failures that never echo a value and the
//! metadata that lists exactly the keys read.

#![cfg(all(feature = "web", feature = "security"))]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use http::StatusCode;
use oxbloom_boot::{
    AutoConfiguration, SecurityAutoConfiguration, StartupPhase, auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_security::{
    Argon2PasswordEncoder, DelegatingPasswordEncoder, FormLogin, FormLoginConfig, PasswordEncoder,
    SecurityBuildError,
};
use oxbloom_test::{ContextRunner, RunningApp, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";
const SESSION: &str = "[security.session]\nstore = \"memory\"\n";
const FORM: &str = "[security.form-login]\nenabled = true\n";
const CSRF: &str = "[security.csrf]\ntoken-endpoint = \"/csrf\"\n";

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

/// A browser: the session cookie of the last response that set one.
#[derive(Default)]
struct Jar(Mutex<Option<String>>);

impl Jar {
    async fn send(
        &self,
        app: &RunningApp,
        method: HttpMethod,
        path: &str,
        body: &str,
    ) -> (StatusCode, String) {
        let (status, location, _) = self.send_full(app, method, path, body).await;
        (status, location)
    }

    async fn send_body(&self, app: &RunningApp, path: &str) -> (StatusCode, String) {
        let (status, _, text) = self.send_full(app, HttpMethod::Get, path, "").await;
        (status, text)
    }

    async fn send_full(
        &self,
        app: &RunningApp,
        method: HttpMethod,
        path: &str,
        body: &str,
    ) -> (StatusCode, String, String) {
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
        (
            res.status(),
            res.header("location").unwrap_or_default().to_owned(),
            res.text(),
        )
    }
}

/// The token of the current session, read from the token endpoint. The jar keeps the cookie
/// the answer sets, which the cookie backend needs.
async fn token(jar: &Jar, app: &RunningApp) -> String {
    let (status, body) = jar.send_body(app, "/csrf").await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    json["token"].as_str().unwrap().to_owned()
}

async fn flow(toml: String) {
    let app = start(toml).await;
    let jar = Jar::default();
    let (status, to) = jar.send(&app, HttpMethod::Get, "/secret", "").await;
    assert_eq!((status, to.as_str()), (StatusCode::FOUND, "/login"));
    let t = token(&jar, &app).await;
    let (status, to) = jar
        .send(
            &app,
            HttpMethod::Post,
            "/login",
            &format!("username=alice&password=wrong&_csrf={t}"),
        )
        .await;
    assert_eq!((status, to.as_str()), (StatusCode::FOUND, "/login?error"));
    let (status, to) = jar
        .send(
            &app,
            HttpMethod::Post,
            "/login",
            &format!("username=alice&password=wonderland&_csrf={t}"),
        )
        .await;
    assert_eq!((status, to.as_str()), (StatusCode::FOUND, "/secret"));
    let (status, _) = jar.send(&app, HttpMethod::Get, "/secret", "").await;
    assert_eq!(status, StatusCode::OK);
    let t = token(&jar, &app).await;
    let (status, to) = jar
        .send(&app, HttpMethod::Post, "/logout", &format!("_csrf={t}"))
        .await;
    assert_eq!((status, to.as_str()), (StatusCode::FOUND, "/login?logout"));
    let (status, to) = jar.send(&app, HttpMethod::Get, "/secret", "").await;
    assert_eq!((status, to.as_str()), (StatusCode::FOUND, "/login"));
    app.stop().await;
}

#[tokio::test]
async fn login_flow_over_http() {
    flow(format!("{FORM}{SESSION}{CSRF}{}", users())).await;
}

#[tokio::test]
async fn works_with_the_cookie_session_backend() {
    flow(format!(
        "{FORM}[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n{CSRF}{}",
        users()
    ))
    .await;
}

#[tokio::test]
async fn disabled_by_default_keeps_the_401() {
    for toml in [
        format!("{SESSION}{}", users()),
        format!(
            "[security.form-login]\nenabled = false\n{SESSION}{}",
            users()
        ),
    ] {
        let app = start(toml).await;
        let res = app
            .client()
            .get("/secret")
            .header("accept", "text/html")
            .send()
            .await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(res.header("location").is_none());
        app.stop().await;
    }
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
async fn enabled_without_session_table_fails_startup_naming_the_key() {
    fails(format!("{FORM}{}", users()), |_, text| {
        assert!(text.contains("security.session"), "{text}");
    })
    .await;
}

#[tokio::test]
async fn enabled_without_a_password_provider_fails_startup() {
    fails(format!("{FORM}{SESSION}"), |error, _| {
        let cause = error
            .find_cause::<SecurityBuildError>()
            .expect("a SecurityBuildError cause");
        assert!(matches!(
            cause,
            SecurityBuildError::FormLoginWithoutProvider
        ));
    })
    .await;
}

#[tokio::test]
async fn bad_url_fails_startup_without_echoing_it() {
    fails(
        format!(
            "{FORM}failure-url = \"https://evil.example/x\"\n{SESSION}{}",
            users()
        ),
        |error, text| {
            let cause = error
                .find_cause::<ConfigError>()
                .expect("a ConfigError cause");
            match cause {
                ConfigError::Invalid { key, .. } => {
                    assert_eq!(key, "security.form-login.failure-url")
                }
                other => panic!("expected Invalid, got {other:?}"),
            }
            assert!(!text.contains("evil.example"), "{text}");
        },
    )
    .await;
}

/// A TOML line for each key of the metadata and the value it changes.
fn sample(key: &str) -> &'static str {
    match key {
        "security.form-login.enabled" => "true",
        "security.form-login.login-page" => "\"/signin\"",
        "security.form-login.login-processing-url" => "\"/do-login\"",
        "security.form-login.logout-url" => "\"/signout\"",
        "security.form-login.default-success-url" => "\"/home\"",
        "security.form-login.failure-url" => "\"/signin?bad\"",
        "security.form-login.logout-success-url" => "\"/signin?bye\"",
        "security.form-login.username-field" => "\"user_name\"",
        "security.form-login.password-field" => "\"pass_word\"",
        "security.form-login.login-page-enabled" => "false",
        other => panic!("no sample for {other}"),
    }
}

#[test]
fn metadata_lists_exactly_the_keys_read() {
    let keys: Vec<&str> = SecurityAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .filter(|k| k.starts_with("security.form-login."))
        .collect();
    assert_eq!(
        keys,
        [
            "security.form-login.default-success-url",
            "security.form-login.enabled",
            "security.form-login.failure-url",
            "security.form-login.login-page",
            "security.form-login.login-page-enabled",
            "security.form-login.login-processing-url",
            "security.form-login.logout-success-url",
            "security.form-login.logout-url",
            "security.form-login.password-field",
            "security.form-login.username-field",
        ]
    );
    assert_eq!(keys.iter().collect::<BTreeSet<_>>().len(), keys.len());

    // Every listed key is read: alone (with `enabled`), it changes a value.
    let default = FormLogin::default();
    for key in keys {
        let name = key.strip_prefix("security.form-login.").unwrap();
        if name == "enabled" {
            let off = Config::from_str("[security.form-login]\nenabled = false\n").unwrap();
            let on = Config::from_str(&format!(
                "[security.form-login]\nenabled = {}\n",
                sample(key)
            ))
            .unwrap();
            assert!(FormLoginConfig::from_config(&off).unwrap().is_none());
            assert!(FormLoginConfig::from_config(&on).unwrap().is_some());
            continue;
        }
        let toml = format!(
            "[security.form-login]\nenabled = true\n{name} = {}\n",
            sample(key)
        );
        let form = FormLoginConfig::from_config(&Config::from_str(&toml).unwrap())
            .unwrap()
            .unwrap();
        assert_ne!(form, default, "{key} is not read");
    }
}
