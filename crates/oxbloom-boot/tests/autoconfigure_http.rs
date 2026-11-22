//! The auto-configured web and security stack over a real socket: the application runs
//! in process on a free port, a `TestClient` drives it and `RunningApp::stop` ends it.

#![cfg(all(feature = "web", feature = "security"))]

mod common;

use std::sync::Arc;

use jsonwebtoken::{EncodingKey, Header, encode};
use oxbloom_boot::SecurityCustomizer;
use oxbloom_di::DiError;
use oxbloom_security::{Algorithm, HttpSecurity, RequestSecurityExt, SecurityFilterChain};
use oxbloom_test::{RunningApp, Stopped, TestApp, TestClient};
use oxbloom_web::{Controller, HttpMethod, Request, Route};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";

fn config(extra: &str) -> String {
    format!(
        "[server]\nport = 0\n\
         [security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\n{extra}"
    )
}

/// Answers `/data` and `/open/x`; `/data` echoes the authenticated principal.
struct Api;

impl Controller for Api {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![
            Route::new(HttpMethod::Get, "/data", |req: Request| async move {
                let who = req.authentication().map(|a| a.principal().to_owned());
                format!("data for {}", who.as_deref().unwrap_or("nobody"))
            }),
            Route::new(HttpMethod::Get, "/open/x", |_| async { "open" }),
        ]
    }
}

/// Permits `/open/**`; everything else stays authenticated.
struct OpenRules;

impl SecurityCustomizer for OpenRules {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.path("/open/**")
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}

/// A running application whose `stop` also checks the exit code.
struct Running(RunningApp);

impl Running {
    fn client(&self) -> &TestClient {
        self.0.client()
    }

    async fn stop(self) -> Stopped {
        let stopped = self.0.stop().await;
        assert_eq!(stopped.status().code(), 0);
        stopped
    }
}

/// An application with the `Api` controller, the given config and `extra` beans.
async fn serve(config: &str, extra: impl FnOnce(TestApp) -> TestApp) -> Running {
    let app = TestApp::full().config_str(config).configure(|b| {
        b.instance(Api)
            .provides(|c: Arc<Api>| -> Arc<dyn Controller> { c })
    });
    Running(extra(app).start().await)
}

fn sign(secret: &str, sub: &str) -> String {
    let exp = jsonwebtoken::get_current_timestamp() + 300;
    encode(
        &Header::new(Algorithm::HS256),
        &serde_json::json!({ "sub": sub, "exp": exp }),
        &EncodingKey::from_secret(secret.as_bytes()).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn default_chain_answers_401() {
    let app = serve(&config(""), |b| b).await;
    let status = app.client().get("/data").send().await.status().as_u16();
    assert_eq!(status, 401);
    // An unknown path is denied too, not 404.
    assert_eq!(
        app.client().get("/nope").send().await.status().as_u16(),
        401
    );
    app.stop().await;
}

#[tokio::test]
async fn valid_jwt_gets_through() {
    let app = serve(&config(""), |b| b).await;
    let token = sign(SECRET, "alice");
    let reply = app
        .client()
        .get("/data")
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await;
    assert_eq!(
        (reply.status().as_u16(), reply.text()),
        (200, "data for alice".into())
    );
    app.stop().await;
}

#[tokio::test]
async fn invalid_jwt_gets_401() {
    let app = serve(&config(""), |b| b).await;
    let other = sign("a-different-secret-of-32-bytes!!!", "mallory");
    let status = app
        .client()
        .get("/data")
        .header("Authorization", format!("Bearer {other}"))
        .send()
        .await
        .status()
        .as_u16();
    assert_eq!(status, 401);
    let status = app
        .client()
        .get("/data")
        .header("Authorization", "Bearer not-a-token")
        .send()
        .await
        .status()
        .as_u16();
    assert_eq!(status, 401);
    app.stop().await;
}

#[tokio::test]
async fn public_path_via_customizer() {
    let app = serve(&config(""), |b| {
        b.configure(|b| {
            b.instance(OpenRules)
                .provides(|c: Arc<OpenRules>| -> Arc<dyn SecurityCustomizer> { c })
        })
    })
    .await;
    let open = app.client().get("/open/x").send().await;
    assert_eq!((open.status().as_u16(), open.text()), (200, "open".into()));
    assert_eq!(
        app.client().get("/data").send().await.status().as_u16(),
        401
    );
    app.stop().await;
}

#[tokio::test]
async fn user_chain_overrides_default() {
    let app = serve(&config(""), |b| {
        b.configure(|b| {
            b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
                Ok(HttpSecurity::new()
                    .authorize(|a| a.any_request().permit_all())
                    .build()?)
            })
        })
    })
    .await;
    let reply = app.client().get("/data").send().await;
    assert_eq!(
        (reply.status().as_u16(), reply.text()),
        (200, "data for nobody".into())
    );
    app.stop().await;
}

#[tokio::test]
async fn conditions_report_shows_skips() {
    let toml = config("[oxbloom.main]\nconditions-report = \"console\"\n");
    let app = serve(&toml, |b| {
        b.configure(|b| {
            b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
                Ok(HttpSecurity::new()
                    .authorize(|a| a.any_request().permit_all())
                    .build()?)
            })
        })
    })
    .await;
    let printed = app.stop().await.output().contents();
    let (_, skipped) = printed
        .split_once("Skipped:")
        .unwrap_or_else(|| panic!("no skipped section: {printed}"));
    let line = skipped
        .lines()
        .find(|l| l.contains("SecurityFilterChain"))
        .unwrap_or_else(|| panic!("the default chain is not named: {printed}"));
    assert!(line.contains("on_missing_bean"), "{line}");
    assert!(!printed.contains(SECRET), "the secret leaked: {printed}");
}
