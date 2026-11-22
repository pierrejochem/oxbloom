//! `ActuatorAutoConfiguration`: the actuator endpoints, the availability listener, the health
//! indicators, the info contributors and the `health.public` security customizer, run in
//! process through `TestApp` and `ContextRunner`.
//!
//! Boot's dev-dependency on `oxbloom-test` turns `web`, `security` and `ssl` on for every test
//! build, so only `logging` changes what runs here.

#![cfg(feature = "actuator")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use jsonwebtoken::{EncodingKey, Header, encode};
use oxbloom_actuator::{
    ActuatorController, ActuatorSettings, Availability, BuildInfo, Health, HealthIndicator,
    Readiness,
};
use oxbloom_boot::{
    ActuatorAutoConfiguration, AutoConfiguration, RunListener, SslAutoConfiguration, StartupPhase,
    WebServer,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::Container;
use oxbloom_security::Algorithm;
use oxbloom_test::{ContextRunner, FixedClock, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};
use serde_json::Value;

const SECRET: &str = "an-example-secret-of-32-bytes!!!";

fn jwt_section() -> String {
    format!("[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\n")
}

fn token() -> String {
    let exp = jsonwebtoken::get_current_timestamp() + 300;
    encode(
        &Header::new(Algorithm::HS256),
        &serde_json::json!({ "sub": "alice", "exp": exp }),
        &EncodingKey::from_secret(SECRET.as_bytes()).unwrap(),
    )
    .unwrap()
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// The web slice with the actuator, the application controller `Ping` and no security chain.
fn web() -> TestApp {
    TestApp::web()
        .auto_configuration::<ActuatorAutoConfiguration>()
        .configure(ping)
}

/// The secured slice (default chain: everything needs authentication) with the actuator and
/// `Ping`.
fn secured() -> TestApp {
    TestApp::secured()
        .auto_configuration::<ActuatorAutoConfiguration>()
        .configure(ping)
}

/// The application's own controller: the actuator joins a web application, it never makes one.
struct Ping;
impl Controller for Ping {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    }
}

fn ping(b: oxbloom_di::ContainerBuilder) -> oxbloom_di::ContainerBuilder {
    b.instance(Ping)
        .provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c })
}

/// An application health indicator with a fixed status.
struct Db(Health);

#[oxbloom_boot::async_trait]
impl HealthIndicator for Db {
    fn name(&self) -> &str {
        "db"
    }

    async fn health(&self) -> Health {
        self.0.clone()
    }
}

fn with_db(
    health: Health,
) -> impl FnOnce(oxbloom_di::ContainerBuilder) -> oxbloom_di::ContainerBuilder + Send + 'static {
    move |b| {
        b.instance(Db(health))
            .provides(|d: Arc<Db>| -> Arc<dyn HealthIndicator> { d })
    }
}

/// The readiness the probe saw at each event, and whether the server was bound at `started`.
#[derive(Clone, Default)]
struct ReadinessProbe {
    seen: Arc<Mutex<Vec<(&'static str, Readiness)>>>,
    availability: Arc<Mutex<Option<Arc<Availability>>>>,
    bound_at_started: Arc<Mutex<bool>>,
}

impl ReadinessProbe {
    fn record(&self, event: &'static str) {
        let availability = self.availability.lock().unwrap().clone().unwrap();
        self.seen
            .lock()
            .unwrap()
            .push((event, availability.readiness()));
    }
}

impl RunListener for ReadinessProbe {
    fn context_ready(&self, c: &Container) {
        *self.availability.lock().unwrap() = Some(c.get::<Availability>().unwrap());
        self.record("context_ready");
    }

    fn started(&self, c: &Container) {
        *self.bound_at_started.lock().unwrap() = c
            .get_all::<WebServer>()
            .unwrap()
            .iter()
            .any(|s| s.local_addr().is_some());
        self.record("started");
    }

    fn ready(&self, _c: &Container) {
        self.record("ready");
    }

    fn stopping(&self) {
        self.record("stopping");
    }
}

#[tokio::test]
async fn readiness_refusing_before_ready_accepting_after_and_refusing_while_stopping() {
    let probe = ReadinessProbe::default();
    let bean = probe.clone();
    let app = web()
        .configure(move |b| {
            b.instance(bean)
                .provides(|p: Arc<ReadinessProbe>| -> Arc<dyn RunListener> { p })
        })
        .start()
        .await;
    let res = app.client().get("/actuator/health/readiness").send().await;
    assert_eq!(res.status(), 200);
    assert_eq!(json(&res.text())["status"], "UP");
    let res = app.client().get("/actuator/health/liveness").send().await;
    assert_eq!(res.status(), 200);
    app.stop().await;

    // The server is up at `started`, and readiness still refuses until `ready`.
    assert!(*probe.bound_at_started.lock().unwrap());
    assert_eq!(
        *probe.seen.lock().unwrap(),
        [
            ("context_ready", Readiness::RefusingTraffic),
            ("started", Readiness::RefusingTraffic),
            ("ready", Readiness::AcceptingTraffic),
            ("stopping", Readiness::RefusingTraffic),
        ]
    );
}

#[tokio::test]
async fn health_requires_authentication_by_default_with_security() {
    let app = secured().config_str(jwt_section()).start().await;
    for path in [
        "/actuator/health",
        "/actuator/health/liveness",
        "/actuator/health/readiness",
        "/actuator/info",
        "/actuator/loggers",
    ] {
        let res = app.client().get(path).send().await;
        assert_eq!(res.status(), 401, "{path}");
    }
    let bearer = format!("Bearer {}", token());
    for path in ["/actuator/health", "/actuator/info"] {
        let res = app
            .client()
            .get(path)
            .header("authorization", bearer.as_str())
            .send()
            .await;
        assert_eq!(res.status(), 200, "{path}");
    }
    app.stop().await;
}

#[tokio::test]
async fn health_public_opens_only_health() {
    let app = secured()
        .config_str("[management.endpoints.health]\npublic = true\n")
        .start()
        .await;
    for path in [
        "/actuator/health",
        "/actuator/health/liveness",
        "/actuator/health/readiness",
    ] {
        let res = app.client().get(path).send().await;
        assert_eq!(res.status(), 200, "{path}");
    }
    for path in [
        "/actuator/info",
        "/actuator/loggers",
        "/actuator/loggers/app",
    ] {
        let res = app.client().get(path).send().await;
        assert_eq!(res.status(), 401, "{path}");
    }
    let res = app.client().post("/actuator/loggers/app").send().await;
    assert_eq!(res.status(), 401);
    app.stop().await;
}

#[tokio::test]
async fn disabled_group_registers_nothing() {
    let toml = "[management.endpoints]\nenabled = false\n";
    let app = web().config_str(toml).start().await;
    assert_eq!(app.client().get("/ping").send().await.status(), 200);
    assert_eq!(
        app.client().get("/actuator/health").send().await.status(),
        404
    );
    app.stop().await;

    ContextRunner::new()
        .config_str(toml)
        .auto_configuration::<ActuatorAutoConfiguration>()
        .configure(ping)
        .run(|view| {
            assert!(!view.contains::<ActuatorController>());
            assert!(!view.contains::<Availability>());
            assert!(!view.contains::<ActuatorSettings>());
        })
        .await;

    // Missing means enabled.
    ContextRunner::new()
        .auto_configuration::<ActuatorAutoConfiguration>()
        .configure(ping)
        .run(|view| assert!(view.contains::<ActuatorController>()))
        .await;
}

#[tokio::test]
async fn skipped_without_an_application_controller() {
    // No controller of the application: the actuator does not turn the run into a server.
    ContextRunner::new()
        .auto_configuration::<ActuatorAutoConfiguration>()
        .run(|view| {
            assert!(!view.contains::<ActuatorController>());
            assert!(!view.contains::<Availability>());
            assert!(view.conditions().records().iter().any(|r| {
                r.registration().contains("ActuatorAutoConfiguration") && !r.matched()
            }));
        })
        .await;
}

#[tokio::test]
async fn app_indicator_bean_is_collected() {
    let app = web()
        .config_str(
            "[management.health]\nshow-details = \"always\"\nreadiness.include = [\"db\"]\n",
        )
        .configure(with_db(Health::down().detail("pool", "exhausted")))
        .start()
        .await;
    let res = app.client().get("/actuator/health").send().await;
    assert_eq!(res.status(), 503);
    let body = json(&res.text());
    assert_eq!(body["status"], "DOWN");
    assert_eq!(body["components"]["db"]["status"], "DOWN");
    assert_eq!(body["components"]["db"]["details"]["pool"], "exhausted");
    // Readiness includes `db`; liveness never runs an indicator.
    let res = app.client().get("/actuator/health/readiness").send().await;
    assert_eq!(res.status(), 503);
    let res = app.client().get("/actuator/health/liveness").send().await;
    assert_eq!(res.status(), 200);
    app.stop().await;
}

#[tokio::test]
async fn show_details_flag_reaches_the_response() {
    for (toml, details) in [
        ("", false),
        ("[management.health]\nshow-details = \"never\"\n", false),
        ("[management.health]\nshow-details = \"always\"\n", true),
    ] {
        let app = web()
            .config_str(toml)
            .configure(with_db(Health::up()))
            .start()
            .await;
        let res = app.client().get("/actuator/health").send().await;
        assert_eq!(res.status(), 200);
        let body = json(&res.text());
        assert_eq!(body["status"], "UP");
        assert_eq!(body.get("components").is_some(), details, "{toml}");
        app.stop().await;
    }
}

#[tokio::test]
async fn info_serves_the_build_info_bean() {
    let app = web()
        .configure(|b| b.instance(BuildInfo::new(Some("app"), Some("1.2.3"), None, None, None)))
        .start()
        .await;
    let res = app.client().get("/actuator/info").send().await;
    assert_eq!(res.status(), 200);
    let body = json(&res.text());
    assert_eq!(body["build"]["name"], "app");
    assert_eq!(body["build"]["version"], "1.2.3");
    app.stop().await;

    // Without a bean, every build field is absent.
    let app = web().start().await;
    let body = json(&app.client().get("/actuator/info").send().await.text());
    assert_eq!(body["build"], serde_json::json!({}));
    app.stop().await;
}

#[cfg(feature = "logging")]
#[tokio::test]
async fn post_loggers_changes_level_end_to_end() {
    use oxbloom_logging::{Level, LoggingSystem};

    // A scoped system: no global subscriber, so the tests of this binary stay independent.
    let (_dispatch, logging) =
        oxbloom_logging::__private::scoped(&Config::from_str("").unwrap(), std::io::sink).unwrap();
    let bean: LoggingSystem = logging.clone();
    let app = web().configure(move |b| b.instance(bean)).start().await;
    let res = app
        .client()
        .post("/actuator/loggers/app")
        .header("content-type", "application/json")
        .body(r#"{"configuredLevel":"DEBUG"}"#)
        .send()
        .await;
    assert_eq!(res.status(), 204, "{}", res.text());
    assert_eq!(logging.level("app"), Level::Debug);
    let res = app.client().get("/actuator/loggers/app").send().await;
    assert_eq!(res.status(), 200);
    assert_eq!(json(&res.text())["configuredLevel"], "DEBUG");
    let res = app.client().get("/actuator/loggers").send().await;
    assert_eq!(json(&res.text())["levels"]["app"], "DEBUG");
    app.stop().await;
}

#[cfg(feature = "ssl")]
#[tokio::test]
async fn ssl_indicator_registered_with_bundles() {
    let ca = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .unwrap()
        .cert
        .pem();
    let toml = format!(
        "[management.health]\nshow-details = \"always\"\n\
         [ssl.bundle.pem.client]\ntrust = '''\n{ca}'''\n"
    );
    let app = web()
        .auto_configuration::<SslAutoConfiguration>()
        .config_str(toml.clone())
        .start()
        .await;
    let res = app.client().get("/actuator/health").send().await;
    assert_eq!(res.status(), 200);
    let body = json(&res.text());
    assert_eq!(body["components"]["ssl"]["status"], "UP");
    assert_eq!(
        body["components"]["ssl"]["details"]["bundles"],
        serde_json::json!(["client"])
    );
    app.stop().await;

    // The injected clock reaches the indicator: after the certificate's `not_after` (rcgen's
    // default is the year 4096) it is expired.
    let later = SystemTime::now() + Duration::from_secs(3000 * 365 * 24 * 3600);
    let app = web()
        .auto_configuration::<SslAutoConfiguration>()
        .config_str(toml)
        .clock(Arc::new(FixedClock::at(later)))
        .start()
        .await;
    let res = app.client().get("/actuator/health").send().await;
    assert_eq!(res.status(), 503);
    let body = json(&res.text());
    assert_eq!(body["components"]["ssl"]["status"], "DOWN");
    assert_eq!(
        body["components"]["ssl"]["details"]["expired"],
        serde_json::json!(["client"])
    );
    app.stop().await;

    // Without bundles there is no `ssl` indicator.
    ContextRunner::new()
        .auto_configuration::<SslAutoConfiguration>()
        .auto_configuration::<ActuatorAutoConfiguration>()
        .configure(ping)
        .run(|view| {
            assert!(view.contains::<ActuatorController>());
            assert!(!view.contains::<oxbloom_actuator::SslHealthIndicator>());
            assert!(!view.contains::<dyn HealthIndicator>());
        })
        .await;
}

/// A sample TOML value for each `management` key, in the order of the metadata.
const SAMPLES: [(&str, &str); 9] = [
    ("management.endpoints.enabled", "false"),
    ("management.endpoints.base-path", "\"/ops\""),
    ("management.endpoints.health.public", "true"),
    ("management.endpoint.health.enabled", "false"),
    ("management.endpoint.info.enabled", "false"),
    ("management.endpoint.loggers.enabled", "false"),
    ("management.health.show-details", "\"always\""),
    ("management.health.timeout", "\"2s\""),
    ("management.health.readiness.include", "[\"db\"]"),
];

#[test]
fn metadata_matches_keys_read() {
    let keys: Vec<_> = ActuatorAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .collect();
    let expected: Vec<_> = SAMPLES.iter().map(|(k, _)| *k).collect();
    assert_eq!(keys, expected);

    let defaults = ActuatorSettings::default();
    let read = |toml: &str| ActuatorSettings::from_config(&Config::from_str(toml).unwrap());
    // Every listed key is read: alone, it changes the settings.
    for (key, value) in SAMPLES {
        let settings = read(&format!("{key} = {value}")).unwrap();
        assert_ne!(settings, defaults, "{key} is not read");
    }
    // Together they change every field, so no field is read from an unlisted key.
    let all: String = SAMPLES
        .iter()
        .map(|(k, v)| format!("{k} = {v}\n"))
        .collect();
    let s = read(&all).unwrap();
    assert!(!s.enabled && s.health_public && s.show_details);
    assert!(!s.health_enabled && !s.info_enabled && !s.loggers_enabled);
    assert_eq!(s.base_path, "/ops");
    assert_eq!(s.timeout, Duration::from_secs(2));
    assert_eq!(s.readiness_include, ["db"]);
    // Any other key is rejected.
    assert!(read("management.endpoint.env.enabled = true").is_err());
    assert!(read("management.port = 8081").is_err());
    // The documented defaults are the real ones.
    let default_of = |key: &str| {
        ActuatorAutoConfiguration::properties()
            .iter()
            .find(|p| p.key == key)
            .unwrap()
            .default
    };
    assert_eq!(
        default_of("management.endpoints.base-path"),
        Some("/actuator")
    );
    assert_eq!(default_of("management.health.timeout"), Some("5s"));
    assert_eq!(defaults.timeout, Duration::from_secs(5));
}

#[tokio::test]
async fn unknown_management_key_fails_startup_naming_it() {
    let error = web()
        .config_str("[management]\nbogus-key = 1\n")
        .start_failing()
        .await;
    assert_eq!(error.phase(), StartupPhase::Container);
    let cause = error
        .find_cause::<ConfigError>()
        .expect("a ConfigError cause");
    assert!(cause.to_string().contains("bogus-key"), "{cause}");
}

#[tokio::test]
async fn readiness_include_naming_no_indicator_fails_startup() {
    let error = web()
        .config_str("[management.health.readiness]\ninclude = [\"db\", \"cache\", \"queue\"]\n")
        .configure(with_db(Health::up()))
        .start_failing()
        .await;
    assert_eq!(error.phase(), StartupPhase::Container);
    let cause = error
        .find_cause::<ConfigError>()
        .expect("a ConfigError cause");
    let text = cause.to_string();
    assert!(
        text.contains("management.health.readiness.include"),
        "{text}"
    );
    assert!(text.contains("cache") && text.contains("queue"), "{text}");
    assert!(!text.contains("\"db\""), "{text}");
}
