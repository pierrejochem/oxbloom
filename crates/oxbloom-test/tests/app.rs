//! `TestApp` starts a whole boot application with a real web server on port 0, hands a client
//! connected to it, and stops it again under the C10a shutdown rules.

#![cfg(feature = "web")]

use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use oxbloom_boot::{ExitStatus, RunListener, WebServer};
use oxbloom_config::Config;
use oxbloom_di::{Container, ContainerBuilder};
use oxbloom_test::{Log, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};
use tokio::net::TcpListener;

/// The text of a panic payload, for a `&str` or a `String` payload.
fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(text) => *text,
        Err(payload) => payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .unwrap_or_default(),
    }
}

/// Runs `future` in its own task and returns the text of the panic it raised.
async fn panic_of<F>(future: F) -> String
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let error = tokio::spawn(future)
        .await
        .err()
        .expect("the future panicked");
    panic_text(error.into_panic())
}

/// A controller with one route, `GET /hello`.
struct Hello;

impl Controller for Hello {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/hello", |_| async { "hello" })]
    }
}

/// Registers the `Hello` controller.
fn hello(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Hello)
        .provides(|c: Arc<Hello>| -> Arc<dyn Controller> { c })
}

#[tokio::test(flavor = "multi_thread")]
async fn web_slice_serves_a_controller() {
    let app = TestApp::web().configure(hello).start().await;
    let response = app.client().get("/hello").send().await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text(), "hello");
    assert!(app.addr().ip().is_loopback());
    assert_ne!(app.addr().port(), 0);
    app.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn new_registers_no_auto_configuration() {
    let started = std::time::Instant::now();
    let text = panic_of(TestApp::new().configure(hello).start()).await;
    assert!(text.contains("no web server"), "{text}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "no web server")]
async fn start_without_a_controller_fails_fast() {
    let _running = tokio::time::timeout(Duration::from_secs(3), TestApp::web().start())
        .await
        .expect("start fails within 3 seconds");
}

#[tokio::test(flavor = "multi_thread")]
async fn web_slice_has_no_security_chain() {
    let app = TestApp::web().configure(hello).start().await;
    assert_eq!(
        app.client().get("/hello").send().await.status(),
        StatusCode::OK
    );
    app.stop().await;
}

#[cfg(feature = "security")]
mod secured {
    use std::time::{SystemTime, UNIX_EPOCH};

    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use oxbloom_test::ManualClock;

    use super::*;

    const SECRET: &str = "an-example-secret-of-32-bytes!!!";

    fn token(exp: SystemTime) -> String {
        let exp = exp.duration_since(UNIX_EPOCH).unwrap().as_secs();
        let claims = serde_json::json!({ "sub": "alice", "exp": exp });
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secured_slice_rejects_without_a_token_and_accepts_a_valid_one() {
        let now = UNIX_EPOCH + Duration::from_secs(1_900_000_000);
        let clock = Arc::new(ManualClock::at(now));
        let app = TestApp::secured()
            .config_str(format!(
                "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\nclock-skew-secs = 0\n"
            ))
            .clock(clock.clone())
            .configure(hello)
            .start()
            .await;
        let bearer = format!("Bearer {}", token(now + Duration::from_secs(60)));

        let anonymous = app.client().get("/hello").send().await;
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let valid = app
            .client()
            .get("/hello")
            .header("authorization", bearer.as_str())
            .send()
            .await;
        assert_eq!(valid.status(), StatusCode::OK);

        clock.advance(Duration::from_secs(120));
        let expired = app
            .client()
            .get("/hello")
            .header("authorization", bearer.as_str())
            .send()
            .await;
        assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
        app.stop().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_secret_property_never_appears_in_a_panic_message() {
        let bearer = token(UNIX_EPOCH + Duration::from_secs(1_900_000_000));
        // Secret keys are refused on the command line, so this startup fails.
        let text = panic_of(
            TestApp::secured()
                .config_str("[security.jwt]\nalgorithms = [\"HS256\"]\n")
                .property("security.jwt.secret", bearer.clone())
                .configure(hello)
                .start(),
        )
        .await;
        assert!(text.starts_with("startup failed"), "{text}");
        assert!(!text.contains(&bearer), "{text}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_bearer_token_never_appears_in_a_panic_message() {
    let token = "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJhbGljZSJ9.c2lnbmF0dXJlLW1hcmtlcg";
    let text = panic_of(
        TestApp::web()
            .property("demo.token", token)
            .property("server.port", "70000")
            .configure(hello)
            .start(),
    )
    .await;
    assert!(text.starts_with("startup failed"), "{text}");
    assert!(text.contains("server.port"), "{text}");
    assert!(!text.contains("eyJ"), "{text}");
}

/// What the container held when it was ready.
#[derive(Clone, Default)]
struct Servers(Log);

impl RunListener for Servers {
    fn context_ready(&self, c: &Container) {
        self.0.push(format!(
            "servers={}",
            c.get_all::<WebServer>().unwrap().len()
        ));
        #[cfg(feature = "security")]
        self.0.push(format!(
            "chains={}",
            c.get_all::<oxbloom_security::SecurityFilterChain>()
                .unwrap()
                .len()
        ));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn full_runs_every_enabled_auto_configuration() {
    let seen = Servers::default();
    let app = TestApp::full()
        .listener(seen.clone())
        .configure(hello)
        .start()
        .await;
    let status = app.client().get("/hello").send().await.status();
    app.stop().await;
    assert!(seen.0.entries().contains(&"servers=1".to_owned()));
    #[cfg(feature = "security")]
    {
        // With no customizer, the default chain needs authentication everywhere.
        assert!(seen.0.entries().contains(&"chains=1".to_owned()));
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    #[cfg(not(feature = "security"))]
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn start_failing_returns_the_error_for_a_bad_port() {
    let error = TestApp::web()
        .property("server.port", "70000")
        .configure(hello)
        .start_failing()
        .await;
    assert!(error.to_string().contains("server.port"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "startup succeeded but a failure was expected")]
async fn start_failing_panics_when_startup_succeeds() {
    TestApp::web().configure(hello).start_failing().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn config_is_captured_at_context_ready() {
    let app = TestApp::web()
        .property("demo.name", "x")
        .configure(hello)
        .start()
        .await;
    let config: Arc<Config> = app.config();
    assert_eq!(config.get::<String>("demo.name").unwrap(), "x");
    app.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_reports_a_zero_status_and_output() {
    let app = TestApp::web().configure(hello).start().await;
    assert_eq!(app.output().contents(), "");
    let stopped = app.stop().await;
    let status: &ExitStatus = stopped.status();
    assert_eq!(status.code(), 0);
    assert_eq!(stopped.output().contents(), "");
}

/// A service whose `stop` panics.
struct BadStop;

#[oxbloom_boot::async_trait]
impl oxbloom_boot::Service for BadStop {
    async fn start(&self) -> Result<(), oxbloom_boot::ServiceError> {
        Ok(())
    }

    async fn stop(&self) {
        panic!("stop marker");
    }

    fn name(&self) -> &'static str {
        "BadStop"
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_whose_stop_panics_fails_stop() {
    let app = TestApp::web()
        .configure(hello)
        .configure(|b| {
            b.instance(BadStop)
                .provides(|s: Arc<BadStop>| -> Arc<dyn oxbloom_boot::Service> { s })
        })
        .start()
        .await;
    let text = panic_of(app.stop()).await;
    assert!(text.starts_with("shutdown failed"), "{text}");
    assert!(
        text.contains("service BadStop panicked while stopping"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_a_running_app_shuts_it_down() {
    let app = TestApp::web().configure(hello).start().await;
    let addr = app.addr();
    drop(app);
    let free = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if TcpListener::bind(addr).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        free.is_ok(),
        "the port {addr} is free again within 5 seconds"
    );
}

/// Records the lifecycle events it sees.
#[derive(Clone, Default)]
struct Events(Log);

impl RunListener for Events {
    fn starting(&self) {
        self.0.push("starting");
    }
    fn context_ready(&self, _: &Container) {
        self.0.push("context_ready");
    }
    fn started(&self, _: &Container) {
        self.0.push("started");
    }
    fn ready(&self, _: &Container) {
        self.0.push("ready");
    }
    fn stopping(&self) {
        self.0.push("stopping");
    }
    fn stopped(&self, _: &ExitStatus) {
        self.0.push("stopped");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listener_passthrough_sees_lifecycle_events() {
    let events = Events::default();
    let app = TestApp::web()
        .listener(events.clone())
        .configure(hello)
        .start()
        .await;
    assert_eq!(
        events.0.entries(),
        ["starting", "context_ready", "started", "ready"]
    );
    app.stop().await;
    assert_eq!(
        events.0.entries(),
        [
            "starting",
            "context_ready",
            "started",
            "ready",
            "stopping",
            "stopped"
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wildcard_address_is_dialed_on_loopback() {
    let app = TestApp::web()
        .config_str("[server]\naddress = \"0.0.0.0\"\n")
        .configure(hello)
        .start()
        .await;
    assert!(app.addr().ip().is_unspecified(), "{}", app.addr());
    let dialed = app
        .client()
        .addr()
        .expect("a connected client has an address");
    assert!(dialed.ip().is_loopback(), "{dialed}");
    assert_eq!(dialed.port(), app.addr().port());
    assert_eq!(
        app.client().get("/hello").send().await.status(),
        StatusCode::OK
    );
    app.stop().await;
}
