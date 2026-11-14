//! `WebServerAutoConfiguration` and the full `auto_configurations()` list, run in process
//! through the boot builder and over real HTTP.

#![cfg(all(feature = "web", feature = "security"))]

mod common;

use std::any::type_name;
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxbloom_boot::{
    ApplicationArguments, ApplicationRunner, AutoConfiguration, AutoConfigurations, Banner,
    ConfigRef, JwtDecoderAutoConfiguration, OxbloomApplication, OxbloomApplicationBuilder,
    RunListener, RunnerError, SecurityAutoConfiguration, SecurityCustomizer, Service, StartupPhase,
    WebServer, WebServerAutoConfiguration, async_trait, auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{Container, ContainerBuilder};
use oxbloom_security::HttpSecurity;
use oxbloom_test::{Captured, ContextRunner, Log, RunningApp, TestApp, TestClient};
use oxbloom_web::{
    App, Controller, Filter, HttpMethod, Next, Request, Response, Route, RouteError,
};

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

/// One condition record: registration, condition, matched, reason.
type Record = (String, String, bool, String);

/// What the container held when it was ready.
struct Snapshot {
    servers: Vec<Arc<WebServer>>,
    services: Vec<Arc<dyn Service>>,
    records: Vec<Record>,
}

/// Captures a [`Snapshot`] at `context_ready`.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Option<Snapshot>>>);

impl RunListener for Capture {
    fn context_ready(&self, c: &Container) {
        let snapshot = Snapshot {
            servers: c.get_all::<WebServer>().unwrap(),
            services: c.get_all::<dyn Service>().unwrap(),
            records: c
                .condition_report()
                .records()
                .iter()
                .map(|r| {
                    (
                        r.registration().to_owned(),
                        r.condition().to_owned(),
                        r.matched(),
                        r.reason().to_owned(),
                    )
                })
                .collect(),
        };
        *self.0.lock().unwrap() = Some(snapshot);
    }
}

impl Capture {
    fn take(&self) -> Snapshot {
        self.0.lock().unwrap().take().expect("context_ready fired")
    }
}

const PORT_ZERO: &str = "[server]\nport = 0\n";

fn app(config: &str, out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str(config).unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .failure_report(true)
        .auto_configurations(auto_configurations())
}

/// Answers `GET /hello`.
struct Hello;

impl Controller for Hello {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/hello", |_| async { "hi" })]
    }
}

/// Answers `GET /same`, like [`AlsoSame`].
struct Same;

impl Controller for Same {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/same", |_| async { "one" })]
    }
}

struct AlsoSame;

impl Controller for AlsoSame {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/same", |_| async { "two" })]
    }
}

fn hello_wiring(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Hello)
        .provides(|c: Arc<Hello>| -> Arc<dyn Controller> { c })
}

fn hello(builder: OxbloomApplicationBuilder) -> OxbloomApplicationBuilder {
    builder.configure(hello_wiring)
}

/// Permits every request.
struct PermitAll;

impl SecurityCustomizer for PermitAll {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| a.any_request().permit_all())
    }
}

fn permit_all_wiring(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(PermitAll)
        .provides(|c: Arc<PermitAll>| -> Arc<dyn SecurityCustomizer> { c })
}

/// Logs its name and passes the request on.
struct Recording(Log, &'static str);

#[oxbloom_boot::async_trait]
impl Filter for Recording {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        self.0.push(self.1);
        next.run(req).await
    }
}

/// A second filter type, since a concrete type is registered once.
struct AlsoRecording(Recording);

#[oxbloom_boot::async_trait]
impl Filter for AlsoRecording {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        self.0.filter(req, next).await
    }
}

/// A running application with its bound server.
struct Running {
    app: RunningApp,
    capture: Capture,
}

impl Running {
    fn client(&self) -> &TestClient {
        self.app.client()
    }

    async fn stop(self) -> Snapshot {
        assert_eq!(self.app.stop().await.status().code(), 0);
        self.capture.take()
    }
}

/// Runs `app` until its `WebServer` is bound.
async fn serve(app: TestApp) -> Running {
    let capture = Capture::default();
    let app = app.listener(capture.clone()).start().await;
    Running { app, capture }
}

/// An application runner that logs once.
struct LogRunner(Log);

#[async_trait]
impl ApplicationRunner for LogRunner {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        self.0.push("ran");
        Ok(())
    }
}

#[tokio::test]
async fn no_controller_no_server() {
    let ran = Log::new();
    let capture = Capture::default();
    let status = within(
        OxbloomApplication::builder()
            .with_config(Config::from_str("").unwrap())
            .output(std::io::sink())
            .auto_configurations(auto_configurations())
            .configure({
                let ran = ran.clone();
                move |b| {
                    b.instance(LogRunner(ran))
                        .provides(|c: Arc<LogRunner>| -> Arc<dyn ApplicationRunner> { c })
                }
            })
            .listener(capture.clone())
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(ran.entries(), ["ran"], "the runner ran");
    assert_eq!(status.code(), 0, "run() returned code 0");
    let s = capture.take();
    assert!(s.servers.is_empty());
    assert!(s.services.is_empty());
    let group = type_name::<WebServerAutoConfiguration>();
    let record = s
        .records
        .iter()
        .find(|(r, _, _, _)| r == group)
        .expect("a group record");
    assert_eq!(record.1, "on_bean");
    assert!(!record.2);
    assert!(record.3.contains("Controller"), "{}", record.3);
}

#[tokio::test]
async fn controller_builds_server_and_service() {
    let running = serve(
        TestApp::full()
            .config_str(PORT_ZERO)
            .configure(hello_wiring),
    )
    .await;
    // The default chain is in front of the routes.
    assert_eq!(
        running
            .client()
            .get("/hello")
            .send()
            .await
            .status()
            .as_u16(),
        401
    );
    let s = running.stop().await;
    assert_eq!(s.servers.len(), 1);
    assert_eq!(s.services.len(), 1);
    assert_eq!(s.services[0].name(), "WebServer");
    let group = type_name::<WebServerAutoConfiguration>();
    assert!(
        s.records
            .iter()
            .any(|(r, c, m, _)| r == group && c == "on_bean" && *m),
        "{:?}",
        s.records
    );
}

#[tokio::test]
async fn chain_comes_before_filters() {
    let log = Log::new();
    let filter_log = log.clone();
    let builder = TestApp::full()
        .config_str(PORT_ZERO)
        .configure(hello_wiring)
        .configure(move |b| {
            b.instance(Recording(filter_log, "recorded"))
                .provides(|f: Arc<Recording>| -> Arc<dyn Filter> { f })
        });
    let running = serve(builder).await;
    assert_eq!(
        running
            .client()
            .get("/hello")
            .send()
            .await
            .status()
            .as_u16(),
        401
    );
    running.stop().await;
    assert!(log.entries().is_empty(), "{:?}", log.entries());

    // The control: with the chain open, the same filter runs.
    let filter_log = log.clone();
    let builder = TestApp::full()
        .config_str(PORT_ZERO)
        .configure(hello_wiring)
        .configure(permit_all_wiring)
        .configure(move |b| {
            b.instance(Recording(filter_log, "recorded"))
                .provides(|f: Arc<Recording>| -> Arc<dyn Filter> { f })
        });
    let running = serve(builder).await;
    let reply = running.client().get("/hello").send().await;
    assert_eq!(
        (reply.status().as_u16(), reply.text()),
        (200, "hi".to_owned())
    );
    running.stop().await;
    assert_eq!(log.entries(), ["recorded"]);
}

#[tokio::test]
async fn filters_are_ordered() {
    let log = Log::new();
    let (one, two) = (log.clone(), log.clone());
    // Registered in reverse: `order` decides.
    let builder = TestApp::full()
        .config_str(PORT_ZERO)
        .configure(hello_wiring)
        .configure(permit_all_wiring)
        .configure(move |b| {
            b.instance(Recording(two, "two"))
                .provides(|f: Arc<Recording>| -> Arc<dyn Filter> { f })
                .order(2)
                .instance(AlsoRecording(Recording(one, "one")))
                .provides(|f: Arc<AlsoRecording>| -> Arc<dyn Filter> { f })
                .order(1)
        });
    let running = serve(builder).await;
    assert_eq!(
        running
            .client()
            .get("/hello")
            .send()
            .await
            .status()
            .as_u16(),
        200
    );
    running.stop().await;
    assert_eq!(log.entries(), ["one", "two"]);
}

#[tokio::test]
async fn duplicate_route_fails_startup() {
    let out = Captured::new();
    let builder = app(PORT_ZERO, &out).configure(|b| {
        b.instance(Same)
            .provides(|c: Arc<Same>| -> Arc<dyn Controller> { c })
            .instance(AlsoSame)
            .provides(|c: Arc<AlsoSame>| -> Arc<dyn Controller> { c })
    });
    let err = within(builder.run()).await.unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    assert!(err.find_cause::<RouteError>().is_some(), "{err:?}");
    assert!(err.to_string().contains("WebServer"), "{err}");
}

#[tokio::test]
async fn bad_port_fails_with_the_key() {
    let out = Captured::new();
    let err = within(hello(app("[server]\nport = \"x\"\n", &out)).run())
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    let cause = err.find_cause::<ConfigError>().expect("a typed source");
    assert!(
        matches!(cause, ConfigError::Invalid { key, .. } if key == "server.port"),
        "{cause:?}"
    );
    assert!(
        err.to_string().contains("WebServer"),
        "the failure names the bean: {err}"
    );
    let printed = out.contents();
    assert!(printed.contains("server.port"), "{printed}");
}

#[tokio::test]
async fn user_server_wins() {
    let user_app = App::new()
        .route(Route::new(HttpMethod::Get, "/user", |_| async { "user" }))
        .unwrap();
    let server = WebServer::new(user_app, &Config::from_str(PORT_ZERO).unwrap()).unwrap();
    let builder = TestApp::full()
        .config_str(PORT_ZERO)
        .configure(hello_wiring)
        .configure(move |b| {
            b.instance(server)
                .provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })
        });
    let running = serve(builder).await;
    // The user's server, without a chain, answers.
    let reply = running.client().get("/user").send().await;
    assert_eq!(
        (reply.status().as_u16(), reply.text()),
        (200, "user".to_owned())
    );
    let s = running.stop().await;
    assert_eq!(s.servers.len(), 1);
    assert_eq!(s.services.len(), 1);
    assert!(
        s.records
            .iter()
            .any(|(_, c, m, reason)| c == "on_missing_bean" && !*m && reason.contains("WebServer")),
        "{:?}",
        s.records
    );
}

#[tokio::test]
async fn exclude_chain_keeps_server() {
    let builder = TestApp::full()
        .config_str(PORT_ZERO)
        .configure(hello_wiring)
        .exclude::<SecurityAutoConfiguration>();
    // Excluding the chain keeps the JWT keys; excluding the decoder too drops them.
    // This mirrors the `both` block below: `TestApp` has no `property_metadata`, so a plain builder gives it.
    let metadata = OxbloomApplication::builder()
        .auto_configurations(auto_configurations())
        .exclude::<SecurityAutoConfiguration>()
        .property_metadata();
    assert!(metadata.iter().any(|p| p.key == "security.jwt.secret"));
    assert!(metadata.iter().any(|p| p.key == "server.port"));
    let running = serve(builder).await;
    // The default chain would answer 401.
    let reply = running.client().get("/hello").send().await;
    assert_eq!(
        (reply.status().as_u16(), reply.text()),
        (200, "hi".to_owned())
    );
    running.stop().await;

    let both = OxbloomApplication::builder()
        .auto_configurations(auto_configurations())
        .exclude::<SecurityAutoConfiguration>()
        .exclude::<JwtDecoderAutoConfiguration>()
        .property_metadata();
    assert!(!both.iter().any(|p| p.key.starts_with("security.jwt")));
    assert!(both.iter().any(|p| p.key == "server.address"));

    let no_web = OxbloomApplication::builder()
        .auto_configurations(auto_configurations())
        .exclude::<WebServerAutoConfiguration>()
        .property_metadata();
    // Only `server.ssl.bundle` stays: it is one of the `boot_properties()`.
    let server: Vec<_> = no_web
        .iter()
        .map(|p| p.key)
        .filter(|k| k.starts_with("server."))
        .collect();
    assert_eq!(server, ["server.ssl.bundle"]);
}

#[tokio::test]
async fn metadata_keys_match_server() {
    let props = WebServerAutoConfiguration::properties();
    let keys: BTreeSet<&str> = props.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        BTreeSet::from([
            "server.address",
            "server.compression.enabled",
            "server.compression.level",
            "server.compression.max-body-bytes",
            "server.compression.mime-types",
            "server.compression.min-size",
            "server.cors.allow-credentials",
            "server.cors.allowed-headers",
            "server.cors.allowed-methods",
            "server.cors.allowed-origins",
            "server.cors.exposed-headers",
            "server.cors.max-age",
            "server.error.include-message",
            "server.forward-headers.strategy",
            "server.forward-headers.trusted-proxies",
            "server.header-read-timeout",
            "server.max-body-bytes",
            "server.port",
            "server.shutdown-timeout",
            "server.static.cache-max-age",
            "server.static.directory",
            "server.static.enabled",
            "server.static.index",
            "server.static.max-file-bytes",
            "server.static.path",
        ])
    );
    assert_eq!(keys.len(), props.len(), "no duplicates");

    // `server.port = 0` makes the OS pick a port, so the bound port is neither the default
    // 8080 nor a fixed one: the key is read, and nothing races for a free port.
    let running = serve(
        TestApp::full()
            .config_str("[server]\naddress = \"127.0.0.1\"\nport = 0\n")
            .configure(hello_wiring)
            .configure(permit_all_wiring),
    )
    .await;
    let addr = running.app.addr();
    assert_eq!(addr.ip(), std::net::Ipv4Addr::LOCALHOST);
    assert_ne!(addr.port(), 0);
    assert_ne!(addr.port(), 8080);
    assert_eq!(
        running
            .client()
            .get("/hello")
            .send()
            .await
            .status()
            .as_u16(),
        200
    );
    running.stop().await;

    // A non-default address is bound as written (the wildcard needs no second loopback
    // address, so this works on every platform); it is not connected to.
    let running = serve(
        TestApp::full()
            .config_str("[server]\naddress = \"0.0.0.0\"\nport = 0\n")
            .configure(hello_wiring)
            .configure(permit_all_wiring),
    )
    .await;
    let addr = running.app.addr();
    assert!(addr.ip().is_unspecified(), "{addr}");
    running.stop().await;
}

#[tokio::test]
async fn auto_configurations_order_by_feature() {
    // `ssl` adds `SslAutoConfiguration`, which the web server also runs after; `actuator` adds
    // `ActuatorAutoConfiguration`, which runs before it; `tasks` adds `TasksAutoConfiguration`
    // and `RetryAutoConfiguration`.
    let ssl = usize::from(cfg!(feature = "ssl"));
    let actuator = usize::from(cfg!(feature = "actuator"));
    let tasks = 2 * usize::from(cfg!(feature = "tasks"));
    // `security` has three auto-configurations and `web` two.
    assert_eq!(auto_configurations().len(), 5 + ssl + actuator + tasks);
    #[allow(unused_mut)]
    let mut after = vec![ConfigRef::of::<SecurityAutoConfiguration>()];
    #[cfg(feature = "ssl")]
    after.push(ConfigRef::of::<oxbloom_boot::SslAutoConfiguration>());
    assert_eq!(WebServerAutoConfiguration::after(), after);

    // Listed in reverse, the web one still runs after the security one.
    let reversed = AutoConfigurations::new()
        .add::<WebServerAutoConfiguration>()
        .add::<SecurityAutoConfiguration>()
        .add::<JwtDecoderAutoConfiguration>();
    let builder = TestApp::new()
        .config_str(PORT_ZERO)
        .auto_configurations(reversed);
    let running = serve(builder.configure(hello_wiring)).await;
    assert_eq!(
        running
            .client()
            .get("/hello")
            .send()
            .await
            .status()
            .as_u16(),
        401
    );
    let s = running.stop().await;
    let position = |name: &str| {
        s.records
            .iter()
            .position(|(r, _, _, _)| r == name)
            .unwrap_or_else(|| panic!("no record for {name}: {:?}", s.records))
    };
    assert!(
        position(type_name::<JwtDecoderAutoConfiguration>())
            < position(type_name::<WebServerAutoConfiguration>())
    );
    let chain = s
        .records
        .iter()
        .position(|(_, c, _, reason)| {
            c == "on_missing_bean" && reason.contains("SecurityFilterChain")
        })
        .expect("the chain's record");
    assert!(chain < position(type_name::<WebServerAutoConfiguration>()));
}

#[tokio::test]
async fn user_request_log_filter_wins() {
    ContextRunner::new()
        .config_str("[logging.request]\nenabled = true")
        .auto_configuration::<oxbloom_boot::RequestLogAutoConfiguration>()
        .configure(|b| {
            b.instance(oxbloom_web::RequestLogFilter::new(
                tracing::Level::WARN,
                Vec::new(),
            ))
        })
        .run(|view| {
            // `get` is an error when there is no bean, or several without a single primary, so this
            // is one provider or a single primary.
            assert!(view.get::<oxbloom_web::RequestLogFilter>().is_ok());
            let records = view.conditions().records();
            assert!(
                records.iter().any(|r| r.condition() == "on_missing_bean"
                    && !r.matched()
                    && r.reason().contains("RequestLogFilter")),
                "{records:?}"
            );
        })
        .await;
}

/// Panics with a secret in the message.
struct Boom;

impl Controller for Boom {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/boom", |_| async {
            if true {
                panic!("secret-token-123");
            }
            "unreachable"
        })]
    }
}

fn boom_wiring(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Boom)
        .provides(|c: Arc<Boom>| -> Arc<dyn Controller> { c })
}

#[tokio::test]
async fn default_renderer_hides_the_message_and_always_shows_it() {
    let running = serve(
        TestApp::full()
            .config_str(PORT_ZERO)
            .configure(boom_wiring)
            .configure(permit_all_wiring),
    )
    .await;
    let reply = running.client().get("/boom").send().await;
    let (status, body) = (reply.status().as_u16(), reply.text());
    assert_eq!(status, 500);
    assert!(body.contains("\"status\":500"), "{body}");
    assert!(!body.contains("secret-token-123"), "{body}");
    running.stop().await;

    let config = "[server]\nport = 0\n[server.error]\ninclude-message = \"always\"\n";
    let running = serve(
        TestApp::full()
            .config_str(config)
            .configure(boom_wiring)
            .configure(permit_all_wiring),
    )
    .await;
    let reply = running.client().get("/boom").send().await;
    let (status, body) = (reply.status().as_u16(), reply.text());
    assert_eq!(status, 500);
    assert!(body.contains("secret-token-123"), "{body}");
    running.stop().await;
}

/// A renderer that answers every error with plain text.
struct Plain;

impl oxbloom_web::ErrorRenderer for Plain {
    fn render(
        &self,
        attrs: &oxbloom_web::ErrorAttributes,
        _: &oxbloom_web::RequestInfo,
    ) -> Response {
        let mut res = Response::new(format!("custom {}", attrs.status.as_u16()).into());
        *res.status_mut() = attrs.status;
        res
    }
}

#[tokio::test]
async fn user_error_renderer_bean_wins() {
    let builder = TestApp::full()
        .config_str(PORT_ZERO)
        .configure(hello_wiring)
        .configure(permit_all_wiring)
        .configure(|b| {
            b.instance(Plain)
                .provides(|r: Arc<Plain>| -> Arc<dyn oxbloom_web::ErrorRenderer> { r })
        });
    let running = serve(builder).await;
    let reply = running.client().get("/missing").send().await;
    let (status, body) = (reply.status().as_u16(), reply.text());
    assert_eq!(status, 404);
    assert_eq!(body, "custom 404");
    let s = running.stop().await;
    assert!(
        s.records
            .iter()
            .any(|(_, c, m, reason)| c == "on_missing_bean"
                && !*m
                && reason.contains("ErrorRenderer")),
        "{:?}",
        s.records
    );
}

#[tokio::test]
async fn invalid_server_values_fail_startup_naming_the_key() {
    let cases = [
        ("shutdown-timeout = \"hunter2\"", "server.shutdown-timeout"),
        ("max-body-bytes = \"hunter2\"", "server.max-body-bytes"),
        (
            "header-read-timeout = \"hunter2\"",
            "server.header-read-timeout",
        ),
        (
            "error.include-message = \"hunter2\"",
            "server.error.include-message",
        ),
        (
            "[server.forward-headers]\nstrategy = \"hunter2\"",
            "server.forward-headers.strategy",
        ),
        (
            "[server.forward-headers]\ntrusted-proxies = [\"hunter2\"]",
            "server.forward-headers.trusted-proxies",
        ),
    ];
    for (line, key) in cases {
        let out = Captured::new();
        let config = format!("[server]\nport = 0\n{line}\n");
        let err = within(hello(app(&config, &out)).run()).await.unwrap_err();
        assert_eq!(err.phase(), StartupPhase::Container);
        let cause = err.find_cause::<ConfigError>().expect("a typed source");
        assert!(
            matches!(cause, ConfigError::Invalid { key: k, .. } if k == key),
            "{cause:?}"
        );
        let printed = out.contents();
        assert!(printed.contains(key), "{printed}");
        assert!(!printed.contains("hunter2"), "{printed}");
        assert!(!err.to_string().contains("hunter2"), "{err}");
    }
}
