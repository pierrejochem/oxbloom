//! Session management and security headers in boot: the `security.headers` keys, the
//! `security.session.max-sessions-per-user` and `security.session.revalidate-interval` keys,
//! the `SessionRegistry` bean, the limit and the revalidation over HTTP, the startup failures
//! that name their key and never echo a value, and the metadata that lists exactly the keys
//! the readers read.

#![cfg(all(feature = "web", feature = "security"))]

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use http::StatusCode;
use oxbloom_boot::{
    AutoConfiguration, RunListener, SecurityAutoConfiguration, SecurityCustomizer,
    SessionAutoConfiguration, StartupPhase, auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_core::SystemClock;
use oxbloom_di::{Container, ContainerBuilder, DiError};
use oxbloom_security::{
    Argon2PasswordEncoder, DelegatingPasswordEncoder, HeadersConfig, HeadersProperties,
    HttpSecurity, InMemorySessionStore, PasswordEncoder, SessionConfig, SessionData, SessionError,
    SessionManagement, SessionManagementProperties, SessionRegistry, SessionSettings, SessionStore,
    UserDetails, UserDetailsError, UserDetailsService,
};
use oxbloom_test::{ContextRunner, ManualClock, RunningApp, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";
const FORM: &str = "[security.form-login]\nenabled = true\n[security.csrf]\nenabled = false\n";

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

/// A user service whose only user, alice, can be disabled while the application runs.
#[derive(Clone)]
struct Switchable {
    enabled: Arc<AtomicBool>,
    password: String,
}

impl Switchable {
    fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(true)),
            password: hash("wonderland"),
        }
    }
}

#[async_trait]
impl UserDetailsService for Switchable {
    async fn load_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        Ok((username == "alice").then(|| {
            UserDetails::new("alice", self.password.clone(), [])
                .roles(&["USER"])
                .enabled(self.enabled.load(Ordering::SeqCst))
        }))
    }
}

/// A server-side store without the per-user index.
#[derive(Clone)]
struct Plain(Arc<InMemorySessionStore>);

impl Plain {
    fn new() -> Self {
        Self(Arc::new(InMemorySessionStore::new(
            &SessionSettings::default(),
            Arc::new(SystemClock),
        )))
    }
}

#[async_trait]
impl SessionStore for Plain {
    async fn load(&self, id: &str) -> Result<Option<SessionData>, SessionError> {
        self.0.load(id).await
    }

    async fn save(&self, id: &str, data: &SessionData, ttl: Duration) -> Result<(), SessionError> {
        self.0.save(id, data, ttl).await
    }

    async fn update(
        &self,
        id: &str,
        data: &SessionData,
        ttl: Duration,
    ) -> Result<bool, SessionError> {
        self.0.update(id, data, ttl).await
    }

    async fn delete(&self, id: &str) -> Result<(), SessionError> {
        self.0.delete(id).await
    }

    async fn rotate(&self, old: &str, new: &str) -> Result<bool, SessionError> {
        self.0.rotate(old, new).await
    }
}

/// A browser: the session cookie of the last response that set one.
#[derive(Default)]
struct Jar(Mutex<Option<String>>);

impl Jar {
    async fn send(&self, app: &RunningApp, method: HttpMethod, path: &str, body: &str) -> String {
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
        format!(
            "{} {}",
            res.status().as_u16(),
            res.header("location").unwrap_or_default()
        )
    }

    async fn login(&self, app: &RunningApp) {
        let reply = self
            .send(
                app,
                HttpMethod::Post,
                "/login",
                "username=alice&password=wonderland",
            )
            .await;
        assert_eq!(reply, "302 /", "the login succeeds");
    }

    async fn secret(&self, app: &RunningApp) -> String {
        self.send(app, HttpMethod::Get, "/secret", "").await
    }
}

const IN: &str = "200 ";
const OUT: &str = "302 /login";

// ---- Headers -----------------------------------------------------------------------------

#[tokio::test]
async fn headers_are_on_by_default_on_a_boot_response() {
    let app = start(String::new()).await;
    let res = app.client().get("/secret").send().await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(res.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(res.header("x-frame-options"), Some("DENY"));
    assert_eq!(res.header("referrer-policy"), Some("no-referrer"));
    // Plain HTTP: no HSTS.
    assert_eq!(res.header("strict-transport-security"), None);
    app.stop().await;
}

#[tokio::test]
async fn headers_keys_change_the_response() {
    let app = start(
        "[security.headers]\nframe-options = \"SAMEORIGIN\"\nreferrer-policy = \"off\"\n".into(),
    )
    .await;
    let res = app.client().get("/secret").send().await;
    assert_eq!(res.header("x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(res.header("referrer-policy"), None);
    assert_eq!(res.header("x-content-type-options"), Some("nosniff"));
    app.stop().await;

    let app = start("[security.headers]\nenabled = false\n".into()).await;
    let res = app.client().get("/secret").send().await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    for name in [
        "x-frame-options",
        "referrer-policy",
        "x-content-type-options",
    ] {
        assert_eq!(res.header(name), None, "{name}");
    }
    app.stop().await;
}

/// Opens everything and changes one header, on top of whatever the configuration set.
struct SameOriginFrames;

impl SecurityCustomizer for SameOriginFrames {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| a.any_request().permit_all())
            .headers(|h| h.frame_options(Some("SAMEORIGIN")))
    }
}

#[tokio::test]
async fn a_customizer_composes_on_the_configured_headers() {
    let app = TestApp::secured()
        .config_str(
            "[security.headers]\nreferrer-policy = \"same-origin\"\ncontent-type-options = \"off\"\n",
        )
        .configure(wiring)
        .configure(|b| {
            b.instance(SameOriginFrames)
                .provides(|c: Arc<SameOriginFrames>| -> Arc<dyn SecurityCustomizer> { c })
        })
        .start()
        .await;
    let res = app.client().get("/secret").send().await;
    assert_eq!(res.status(), StatusCode::OK);
    // The customizer's header and the configured ones are all there.
    assert_eq!(res.header("x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(res.header("referrer-policy"), Some("same-origin"));
    assert_eq!(res.header("x-content-type-options"), None);
    app.stop().await;
}

// ---- Defaults and the readers --------------------------------------------------------------

#[test]
fn readers_give_the_defaults_without_keys() {
    let empty = Config::from_str("").unwrap();
    assert_eq!(
        HeadersProperties::from_config(&empty).unwrap(),
        HeadersConfig::default()
    );
    let management = SessionManagementProperties::from_config(&empty).unwrap();
    assert_eq!(management.max_sessions_per_user, None);
    assert_eq!(
        management.revalidate_interval,
        Some(Duration::from_secs(300))
    );
    assert!(!management.revalidate_explicit);
    assert_eq!(management, SessionManagement::default());
}

#[test]
fn every_key_is_parsed() {
    let config = Config::from_str(
        "[security.headers]\nenabled = true\ncontent-type-options = \"off\"\n\
         frame-options = \"SAMEORIGIN\"\nreferrer-policy = \"same-origin\"\n\
         cache-control = \"no-store, max-age=0\"\n\
         [security.headers.hsts]\nenabled = true\nmax-age = \"1h\"\n\
         include-subdomains = false\npreload = true\n\
         [security.session]\nmax-sessions-per-user = 3\nrevalidate-interval = \"1m\"\n",
    )
    .unwrap();
    assert_eq!(
        HeadersProperties::from_config(&config).unwrap(),
        HeadersConfig::default()
            .content_type_options(None)
            .frame_options(Some("SAMEORIGIN"))
            .referrer_policy(Some("same-origin"))
            .cache_control(Some("no-store, max-age=0"))
            .hsts(|h| {
                h.max_age(Duration::from_secs(3600))
                    .include_subdomains(false)
                    .preload(true)
            })
    );
    let management = SessionManagementProperties::from_config(&config).unwrap();
    assert_eq!(management.max_sessions_per_user, Some(3));
    assert_eq!(
        management.revalidate_interval,
        Some(Duration::from_secs(60))
    );
    assert!(management.revalidate_explicit);
    // The whole session section is still accepted by the 6b reader.
    assert!(SessionConfig::from_config(&config).is_ok());

    // `0s` and `0` turn revalidation off, explicitly.
    for zero in ["\"0s\"", "0"] {
        let config = Config::from_str(&format!(
            "[security.session]\nrevalidate-interval = {zero}\n"
        ))
        .unwrap();
        let management = SessionManagementProperties::from_config(&config).unwrap();
        assert_eq!(management.revalidate_interval, None, "{zero}");
        assert!(management.revalidate_explicit);
    }
}

/// The error of `toml` names `key` and contains `value` neither in its text nor in `Debug`.
fn rejects(result: Result<impl std::fmt::Debug, ConfigError>, key: &str, value: &str) {
    let err = result.expect_err("the value is refused");
    let text = format!("{err} {err:?}");
    assert!(text.contains(key), "{text}");
    assert!(!text.contains(value), "{text} echoes {value}");
}

fn headers(body: &str) -> Result<HeadersConfig, ConfigError> {
    HeadersProperties::from_config(&Config::from_str(body).unwrap())
}

fn management(body: &str) -> Result<SessionManagement, ConfigError> {
    let toml = format!("[security.session]\n{body}\n");
    SessionManagementProperties::from_config(&Config::from_str(&toml).unwrap())
}

#[test]
fn bad_values_name_their_key_without_echoing_the_value() {
    rejects(
        management("max-sessions-per-user = 0"),
        "security.session.max-sessions-per-user",
        "= 0",
    );
    rejects(
        management("max-sessions-per-user = \"many-sessions\""),
        "security.session.max-sessions-per-user",
        "many-sessions",
    );
    rejects(
        management("revalidate-interval = \"500ms\""),
        "security.session.revalidate-interval",
        "500ms",
    );
    rejects(
        management("revalidate-interval = \"forever\""),
        "security.session.revalidate-interval",
        "forever",
    );
    rejects(
        headers("[security.headers]\nframe-options = \"DENY\\u0007bell\"\n"),
        "security.headers.frame-options",
        "bell",
    );
    rejects(
        headers("[security.headers]\nreferrer-policy = \"\"\n"),
        "security.headers.referrer-policy",
        "\"\"",
    );
    rejects(
        headers("[security.headers]\ncache-control = \"caf\u{e9}-policy\"\n"),
        "security.headers.cache-control",
        "policy",
    );
    rejects(
        headers("[security.headers]\ncontent-type-options = 7\n"),
        "security.headers.content-type-options",
        "7",
    );
    rejects(
        headers("[security.headers.hsts]\nmax-age = \"0s\"\n"),
        "security.headers.hsts.max-age",
        "0s",
    );
    rejects(
        headers("[security.headers.hsts]\nmax-age = \"300ms\"\n"),
        "security.headers.hsts.max-age",
        "300ms",
    );
    rejects(
        headers("[security.headers]\nenabled = \"sometimes\"\n"),
        "security.headers.enabled",
        "sometimes",
    );
}

#[test]
fn unknown_keys_are_rejected_naming_them() {
    let text = headers("[security.headers]\nbogus-key = 1\n")
        .unwrap_err()
        .to_string();
    assert!(text.contains("security.headers"), "{text}");
    assert!(text.contains("bogus"), "{text}");
    let text = headers("[security.headers.hsts]\nbogus-key = 1\n")
        .unwrap_err()
        .to_string();
    assert!(text.contains("security.headers.hsts"), "{text}");
    assert!(text.contains("bogus"), "{text}");
    // The 6b reader still rejects unknown `security.session` keys, and accepts the new ones.
    let config = Config::from_str(
        "[security.session]\nmax-sessions-per-user = 2\nrevalidate-interval = \"1m\"\n",
    )
    .unwrap();
    assert!(SessionConfig::from_config(&config).is_ok());
    let config = Config::from_str("[security.session]\nbogus-key = 1\n").unwrap();
    let text = SessionConfig::from_config(&config).unwrap_err().to_string();
    assert!(text.contains("bogus"), "{text}");
}

// ---- Startup failures ----------------------------------------------------------------------

async fn fails(toml: String, configure: fn(ContainerBuilder) -> ContainerBuilder, key: &str) {
    let key = key.to_owned();
    ContextRunner::new()
        .config_str(toml)
        .auto_configurations(auto_configurations())
        .configure(configure)
        .run_failing(move |failure| {
            let error = failure.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let di = error.find_cause::<DiError>().expect("a DiError cause");
            assert!(matches!(di, DiError::Creation { .. }), "{di:?}");
            let output = failure.output().contents();
            assert!(error.to_string().contains(&key), "{error}");
            // The analyzer explains it in the startup failure report.
            assert!(output.contains(&key), "{output}");
            assert!(output.contains("Action"), "{output}");
        })
        .await;
}

fn nothing(b: ContainerBuilder) -> ContainerBuilder {
    b
}

#[tokio::test]
async fn limit_with_the_cookie_store_fails_startup_naming_the_key() {
    fails(
        format!(
            "{FORM}[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n\
             max-sessions-per-user = 2\n{}",
            users()
        ),
        nothing,
        "security.session.max-sessions-per-user",
    )
    .await;
}

fn plain_store(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Plain::new())
        .provides(|s: Arc<Plain>| -> Arc<dyn SessionStore> { s })
}

#[tokio::test]
async fn limit_with_a_store_without_the_index_fails_startup_naming_the_key() {
    fails(
        format!(
            "{FORM}[security.session]\nmax-sessions-per-user = 2\n{}",
            users()
        ),
        plain_store,
        "security.session.max-sessions-per-user",
    )
    .await;
}

#[tokio::test]
async fn explicit_interval_without_a_user_service_fails_startup_naming_the_key() {
    fails(
        "[security.session]\nrevalidate-interval = \"1m\"\n".into(),
        nothing,
        "security.session.revalidate-interval",
    )
    .await;
}

#[tokio::test]
async fn bad_value_fails_startup_naming_the_key_without_echoing_it() {
    for (toml, key, value) in [
        (
            "[security.session]\nrevalidate-interval = \"250ms\"\n",
            "security.session.revalidate-interval",
            "250ms",
        ),
        (
            "[security.headers]\nframe-options = \"x\\u0001secret-frame\"\n",
            "security.headers.frame-options",
            "secret-frame",
        ),
    ] {
        ContextRunner::new()
            .config_str(toml)
            .auto_configurations(auto_configurations())
            .run_failing(move |failure| {
                let error = failure.error();
                match error.find_cause::<ConfigError>().expect("a ConfigError") {
                    ConfigError::Invalid { key: k, .. } => assert_eq!(k, key),
                    other => panic!("expected Invalid, got {other:?}"),
                }
                let text = format!("{error} {error:?} {}", failure.output().contents());
                assert!(!text.contains(value), "{text}");
            })
            .await;
    }
}

#[tokio::test]
async fn the_default_interval_stays_off_without_a_user_service_or_form_login() {
    // No user service, no form login.
    ContextRunner::new()
        .config_str("[security.session]\n")
        .auto_configurations(auto_configurations())
        .run(|view| assert!(view.contains::<oxbloom_security::SecurityFilterChain>()))
        .await;
    // A user service but no form login.
    ContextRunner::new()
        .config_str(format!("[security.session]\n{}", users()))
        .auto_configurations(auto_configurations())
        .run(|view| assert!(view.contains::<oxbloom_security::SecurityFilterChain>()))
        .await;
    // An explicit `0s` needs no user service.
    ContextRunner::new()
        .config_str("[security.session]\nrevalidate-interval = \"0s\"\n")
        .auto_configurations(auto_configurations())
        .run(|view| assert!(view.contains::<oxbloom_security::SecurityFilterChain>()))
        .await;
}

// ---- The registry --------------------------------------------------------------------------

#[tokio::test]
async fn registry_bean_follows_the_index_of_the_store() {
    ContextRunner::new()
        .config_str("[security.session]\nstore = \"memory\"\n")
        .auto_configurations(auto_configurations())
        .run(|view| assert!(view.contains::<SessionRegistry>()))
        .await;
    ContextRunner::new()
        .config_str(format!(
            "[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n"
        ))
        .auto_configurations(auto_configurations())
        .run(|view| assert!(!view.contains::<SessionRegistry>()))
        .await;
    // A store without the index starts, and has no registry.
    ContextRunner::new()
        .config_str("[security.session]\n")
        .auto_configurations(auto_configurations())
        .configure(plain_store)
        .run(|view| assert!(!view.contains::<SessionRegistry>()))
        .await;
    // No session table: no session filter and no registry.
    ContextRunner::new()
        .auto_configurations(auto_configurations())
        .run(|view| assert!(!view.contains::<SessionRegistry>()))
        .await;
}

/// Keeps the `SessionRegistry` bean of the running application.
#[derive(Clone, Default)]
struct Grab(Arc<Mutex<Option<Arc<SessionRegistry>>>>);

impl RunListener for Grab {
    fn context_ready(&self, c: &Container) {
        *self.0.lock().unwrap() = c.get::<SessionRegistry>().ok();
    }
}

#[tokio::test]
async fn the_registry_sees_the_sessions_of_a_login() {
    let grab = Grab::default();
    let app = TestApp::secured()
        .config_str(format!("{FORM}[security.session]\n{}", users()))
        .configure(wiring)
        .listener(grab.clone())
        .start()
        .await;
    let (phone, laptop) = (Jar::default(), Jar::default());
    phone.login(&app).await;
    laptop.login(&app).await;
    let registry = grab.0.lock().unwrap().clone().expect("the registry bean");
    assert_eq!(registry.sessions_of("alice").await.unwrap().len(), 2);
    assert_eq!(registry.invalidate_all("alice").await.unwrap(), 2);
    assert_eq!(phone.secret(&app).await, OUT);
    assert_eq!(laptop.secret(&app).await, OUT);
    app.stop().await;
}

// ---- End to end ----------------------------------------------------------------------------

#[tokio::test]
async fn the_limit_is_enforced_end_to_end() {
    let app = start(format!(
        "{FORM}[security.session]\nmax-sessions-per-user = 1\n{}",
        users()
    ))
    .await;
    let (first, second) = (Jar::default(), Jar::default());
    first.login(&app).await;
    assert!(first.secret(&app).await.starts_with(IN));
    second.login(&app).await;
    assert!(second.secret(&app).await.starts_with(IN));
    // The second login evicted the oldest session.
    assert_eq!(first.secret(&app).await, OUT);
    app.stop().await;
}

async fn start_switchable(toml: String, users: Switchable, clock: ManualClock) -> RunningApp {
    TestApp::secured()
        .config_str(toml)
        .clock(Arc::new(clock))
        .configure(wiring)
        .configure(move |b| {
            b.instance(users.clone())
                .provides(|u: Arc<Switchable>| -> Arc<dyn UserDetailsService> { u })
        })
        .start()
        .await
}

#[tokio::test]
async fn a_disabled_user_is_revalidated_end_to_end() {
    let users = Switchable::new();
    let clock = ManualClock::at(SystemTime::now());
    let app = start_switchable(
        format!("{FORM}[security.session]\nrevalidate-interval = \"1m\"\n"),
        users.clone(),
        clock.clone(),
    )
    .await;
    let jar = Jar::default();
    jar.login(&app).await;
    users.enabled.store(false, Ordering::SeqCst);
    // Within the interval the session is not checked again.
    clock.advance(Duration::from_secs(30));
    assert!(jar.secret(&app).await.starts_with(IN));
    // Past it, the disabled user loses the session.
    clock.advance(Duration::from_secs(31));
    assert_eq!(jar.secret(&app).await, OUT);
    app.stop().await;
}

#[tokio::test]
async fn revalidation_is_on_by_default_with_a_user_service_and_form_login() {
    let users = Switchable::new();
    let clock = ManualClock::at(SystemTime::now());
    let app = start_switchable(
        format!("{FORM}[security.session]\n"),
        users.clone(),
        clock.clone(),
    )
    .await;
    let jar = Jar::default();
    jar.login(&app).await;
    users.enabled.store(false, Ordering::SeqCst);
    clock.advance(Duration::from_secs(299));
    assert!(jar.secret(&app).await.starts_with(IN));
    clock.advance(Duration::from_secs(2));
    assert_eq!(jar.secret(&app).await, OUT);
    app.stop().await;
}

#[tokio::test]
async fn a_zero_interval_turns_revalidation_off() {
    let users = Switchable::new();
    let clock = ManualClock::at(SystemTime::now());
    let app = start_switchable(
        format!("{FORM}[security.session]\nrevalidate-interval = \"0s\"\n"),
        users.clone(),
        clock.clone(),
    )
    .await;
    let jar = Jar::default();
    jar.login(&app).await;
    users.enabled.store(false, Ordering::SeqCst);
    // Twice the default interval, within the idle timeout of 30 minutes.
    clock.advance(Duration::from_secs(600));
    assert!(jar.secret(&app).await.starts_with(IN));
    app.stop().await;
}

// ---- Metadata ------------------------------------------------------------------------------

/// A TOML document that sets `key` alone to a value other than its default.
fn sample(key: &str) -> String {
    let (table, line) = match key {
        "security.headers.enabled" => ("security.headers", "enabled = false"),
        "security.headers.content-type-options" => {
            ("security.headers", "content-type-options = \"off\"")
        }
        "security.headers.frame-options" => ("security.headers", "frame-options = \"SAMEORIGIN\""),
        "security.headers.referrer-policy" => ("security.headers", "referrer-policy = \"off\""),
        "security.headers.cache-control" => ("security.headers", "cache-control = \"private\""),
        "security.headers.hsts.enabled" => ("security.headers.hsts", "enabled = false"),
        "security.headers.hsts.max-age" => ("security.headers.hsts", "max-age = \"1h\""),
        "security.headers.hsts.include-subdomains" => {
            ("security.headers.hsts", "include-subdomains = false")
        }
        "security.headers.hsts.preload" => ("security.headers.hsts", "preload = true"),
        "security.session.max-sessions-per-user" => {
            ("security.session", "max-sessions-per-user = 2")
        }
        "security.session.revalidate-interval" => {
            ("security.session", "revalidate-interval = \"1m\"")
        }
        other => panic!("no sample for {other}"),
    };
    format!("[{table}]\n{line}\n")
}

#[test]
fn security_metadata_lists_exactly_the_header_keys_read() {
    let props = SecurityAutoConfiguration::properties();
    let keys: Vec<&str> = props
        .iter()
        .map(|p| p.key)
        .filter(|k| k.starts_with("security.headers."))
        .collect();
    assert_eq!(
        keys,
        [
            "security.headers.cache-control",
            "security.headers.content-type-options",
            "security.headers.enabled",
            "security.headers.frame-options",
            "security.headers.hsts.enabled",
            "security.headers.hsts.include-subdomains",
            "security.headers.hsts.max-age",
            "security.headers.hsts.preload",
            "security.headers.referrer-policy",
        ]
    );
    assert_eq!(keys.iter().collect::<BTreeSet<_>>().len(), keys.len());
    let default = HeadersConfig::default();
    for key in &keys {
        let config = Config::from_str(&sample(key)).unwrap();
        assert_ne!(
            HeadersProperties::from_config(&config).unwrap(),
            default,
            "{key} is not read"
        );
    }
    // The documented defaults are the real ones.
    let default_of = |key: &str| props.iter().find(|p| p.key == key).unwrap().default;
    assert_eq!(default_of("security.headers.enabled"), Some("true"));
    assert_eq!(default_of("security.headers.frame-options"), Some("DENY"));
    assert_eq!(
        default_of("security.headers.content-type-options"),
        Some("nosniff")
    );
    assert_eq!(
        default_of("security.headers.referrer-policy"),
        Some("no-referrer")
    );
    assert_eq!(
        default_of("security.headers.cache-control"),
        Some("no-store")
    );
    assert_eq!(default_of("security.headers.hsts.max-age"), Some("365d"));
    assert_eq!(
        default.hsts.max_age,
        Duration::from_secs(365 * 24 * 60 * 60)
    );
    // Every key of the security metadata belongs to one of its readers.
    assert!(props.iter().all(|p| {
        p.key.starts_with("security.form-login.")
            || p.key.starts_with("security.csrf.")
            || p.key.starts_with("security.headers.")
    }));
}

#[test]
fn session_metadata_lists_the_session_management_keys_read() {
    let props = SessionAutoConfiguration::properties();
    let keys: BTreeSet<&str> = props.iter().map(|p| p.key).collect();
    for key in [
        "security.session.max-sessions-per-user",
        "security.session.revalidate-interval",
    ] {
        assert!(keys.contains(key), "{key} is not listed");
        let config = Config::from_str(&sample(key)).unwrap();
        assert_ne!(
            SessionManagementProperties::from_config(&config).unwrap(),
            SessionManagement::default(),
            "{key} is not read"
        );
    }
    let default_of = |key: &str| props.iter().find(|p| p.key == key).unwrap().default;
    assert_eq!(default_of("security.session.max-sessions-per-user"), None);
    assert_eq!(
        default_of("security.session.revalidate-interval"),
        Some("5m")
    );
}
