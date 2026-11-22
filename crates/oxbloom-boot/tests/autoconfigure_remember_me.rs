//! Remember-me in boot: the `security.remember-me` keys, the startup failures that name their
//! key and never echo a value or a secret, the metadata that lists exactly the keys the reader
//! reads, and the cookie over HTTP: issued with the configured lifetime, logging in again after
//! the session is gone, refused for a disabled user and judged by the clock bean.

#![cfg(all(feature = "web", feature = "security"))]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use oxbloom_boot::{
    AutoConfiguration, SecurityAutoConfiguration, SecurityCustomizer, StartupPhase,
    auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_security::{
    Argon2PasswordEncoder, DelegatingPasswordEncoder, HttpSecurity, PasswordEncoder,
    RememberMeProperties, RememberMeSettings, SecurityFilterChain, UserDetails, UserDetailsError,
    UserDetailsService,
};
use oxbloom_test::{ContextRunner, ManualClock, RunningApp, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};

const SECRET: &str = "a-remember-me-secret-of-32-bytes";
const FORM: &str = "[security.form-login]\nenabled = true\n[security.csrf]\nenabled = false\n\
                    [security.session]\n";
const COOKIE: &str = "OXBLOOM_REMEMBER_ME";

fn hash(raw: &str) -> String {
    DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap()).encode(raw)
}

fn users() -> String {
    format!(
        "[security.users.alice]\npassword = \"{}\"\nroles = [\"USER\"]\n",
        hash("wonderland")
    )
}

fn remember(body: &str) -> String {
    format!("[security.remember-me]\nenabled = true\nsecret = \"{SECRET}\"\n{body}\n")
}

// ---- The reader ----------------------------------------------------------------------------

fn read(toml: &str) -> Result<Option<RememberMeSettings>, ConfigError> {
    RememberMeProperties::from_config(&Config::from_str(toml).unwrap())
}

/// The error of `result` names `key` and contains `value` neither in its text nor in `Debug`.
fn rejects(result: Result<Option<RememberMeSettings>, ConfigError>, key: &str, value: &str) {
    let err = result.expect_err("the value is refused");
    let text = format!("{err} {err:?}");
    assert!(text.contains(key), "{text}");
    assert!(!text.contains(value), "{text} echoes {value}");
}

#[test]
fn remember_me_is_off_by_default() {
    assert!(read("").unwrap().is_none());
    assert!(read("[security.remember-me]\n").unwrap().is_none());
    assert!(
        read("[security.remember-me]\nenabled = false\ncookie-name = \"R\"\n")
            .unwrap()
            .is_none()
    );
}

#[test]
fn every_key_is_parsed() {
    let settings = read(&remember(&format!(
        "old-secrets = [\"{SECRET}-old\"]\nlifetime = \"36h\"\nfield-name = \"stay\"\n\
         cookie-name = \"KEEP\""
    )))
    .unwrap()
    .expect("enabled");
    assert_eq!(settings.secret.as_ref().unwrap().expose(), SECRET);
    assert_eq!(settings.old_secrets.len(), 1);
    assert_eq!(settings.old_secrets[0].expose(), &format!("{SECRET}-old"));
    assert_eq!(settings.lifetime, Duration::from_secs(36 * 3600));
    assert_eq!(settings.field_name, "stay");
    assert_eq!(settings.cookie_name, "KEEP");
    // The defaults.
    let settings = read(&remember("")).unwrap().expect("enabled");
    assert!(settings.old_secrets.is_empty());
    assert_eq!(settings.lifetime, Duration::from_secs(14 * 24 * 3600));
    assert_eq!(settings.field_name, "remember-me");
    assert_eq!(settings.cookie_name, COOKIE);
}

#[test]
fn bad_values_name_their_key_without_echoing_the_value() {
    let enabled = |body: &str| read(&format!("[security.remember-me]\nenabled = true\n{body}\n"));
    rejects(
        enabled("secret = \"short-secret-value\""),
        "security.remember-me.secret",
        "short-secret-value",
    );
    rejects(
        enabled(&format!(
            "old-secrets = [\"{SECRET}\", \"short-old-secret-value\"]"
        )),
        "security.remember-me.old-secrets[1]",
        "short-old-secret-value",
    );
    rejects(
        enabled("lifetime = \"59s\""),
        "security.remember-me.lifetime",
        "59s",
    );
    rejects(
        enabled("lifetime = \"366d\""),
        "security.remember-me.lifetime",
        "366d",
    );
    rejects(
        enabled("lifetime = \"fortnight\""),
        "security.remember-me.lifetime",
        "fortnight",
    );
    rejects(
        enabled("field-name = \"stay signed\""),
        "security.remember-me.field-name",
        "stay signed",
    );
    rejects(
        enabled("cookie-name = \"bad;cookie\""),
        "security.remember-me.cookie-name",
        "bad;cookie",
    );
    rejects(
        read("[security.remember-me]\nenabled = \"sometimes\"\n"),
        "security.remember-me.enabled",
        "sometimes",
    );
}

#[test]
fn unknown_keys_and_wrong_types_are_rejected_also_when_disabled() {
    let text = read("[security.remember-me]\nbogus-key = 1\n")
        .unwrap_err()
        .to_string();
    assert!(text.contains("security.remember-me"), "{text}");
    assert!(text.contains("bogus"), "{text}");
    assert!(read("[security.remember-me]\nenabled = false\nlifetime = true\n").is_err());
    assert!(read("[security.remember-me]\nenabled = false\nsecret = [1]\n").is_err());
}

#[test]
fn bad_values_are_rejected_also_when_disabled() {
    let disabled = |body: &str| {
        read(&format!(
            "[security.remember-me]\nenabled = false\n{body}\n"
        ))
    };
    rejects(
        disabled("secret = \"short-secret-value\""),
        "security.remember-me.secret",
        "short-secret-value",
    );
    rejects(
        disabled("old-secrets = [\"short-old-secret-value\"]"),
        "security.remember-me.old-secrets[0]",
        "short-old-secret-value",
    );
    rejects(
        disabled("lifetime = \"366d\""),
        "security.remember-me.lifetime",
        "366d",
    );
    rejects(
        disabled("field-name = \"stay signed\""),
        "security.remember-me.field-name",
        "stay signed",
    );
    rejects(
        disabled("cookie-name = \"bad;cookie\""),
        "security.remember-me.cookie-name",
        "bad;cookie",
    );
    // Valid values and no secret: off.
    assert!(disabled("lifetime = \"1h\"").unwrap().is_none());
}

// ---- Startup failures ----------------------------------------------------------------------

/// Startup fails in the container phase; the error and the startup failure report name `key`,
/// the report has an action, and none of them contains `hidden`.
async fn fails(
    toml: String,
    configure: fn(ContainerBuilder) -> ContainerBuilder,
    key: &str,
    hidden: &str,
) {
    let (key, hidden) = (key.to_owned(), hidden.to_owned());
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
            assert!(output.contains(&key), "{output}");
            assert!(output.contains("Action"), "{output}");
            let all = format!("{error} {error:?} {output}");
            assert!(!all.contains(&hidden), "{all}");
        })
        .await;
}

fn nothing(b: ContainerBuilder) -> ContainerBuilder {
    b
}

#[tokio::test]
async fn enabled_without_form_login_fails_startup_naming_form_login() {
    fails(
        format!("[security.session]\n{}{}", remember(""), users()),
        nothing,
        "security.form-login.enabled",
        SECRET,
    )
    .await;
}

#[tokio::test]
async fn enabled_without_a_secret_fails_startup_naming_the_secret() {
    fails(
        format!("{FORM}[security.remember-me]\nenabled = true\n{}", users()),
        nothing,
        "security.remember-me.secret",
        "no-hidden-value",
    )
    .await;
}

#[tokio::test]
async fn enabled_without_a_user_service_fails_startup_naming_enabled() {
    fails(
        format!("{FORM}{}", remember("")),
        nothing,
        "security.remember-me.enabled",
        SECRET,
    )
    .await;
}

#[tokio::test]
async fn a_cookie_name_equal_to_the_session_cookie_fails_startup_naming_it() {
    fails(
        format!(
            "{FORM}{}{}",
            remember("cookie-name = \"OXBLOOM_SESSION\""),
            users()
        ),
        nothing,
        "security.remember-me.cookie-name",
        SECRET,
    )
    .await;
    // A configured session cookie name counts too.
    fails(
        format!(
            "[security.form-login]\nenabled = true\n[security.csrf]\nenabled = false\n\
             [security.session]\ncookie-name = \"SID\"\n{}{}",
            remember("cookie-name = \"SID\""),
            users()
        ),
        nothing,
        "security.remember-me.cookie-name",
        SECRET,
    )
    .await;
}

#[tokio::test]
async fn a_short_secret_fails_startup_and_is_never_echoed() {
    let short = "short-remember-me-secret";
    for toml in [
        format!(
            "{FORM}[security.remember-me]\nenabled = true\nsecret = \"{short}\"\n{}",
            users()
        ),
        format!(
            "{FORM}[security.remember-me]\nenabled = true\nsecret = \"{SECRET}\"\n\
             old-secrets = [\"{short}\"]\n{}",
            users()
        ),
    ] {
        ContextRunner::new()
            .config_str(toml)
            .auto_configurations(auto_configurations())
            .run_failing(move |failure| {
                let error = failure.error();
                match error.find_cause::<ConfigError>().expect("a ConfigError") {
                    ConfigError::Invalid { key, .. } => {
                        assert!(key.starts_with("security.remember-me."), "{key}")
                    }
                    other => panic!("expected Invalid, got {other:?}"),
                }
                let text = format!("{error} {error:?} {}", failure.output().contents());
                assert!(!text.contains(short), "{text}");
                assert!(!text.contains(SECRET), "{text}");
            })
            .await;
    }
}

#[tokio::test]
async fn a_valid_configuration_starts_and_a_disabled_one_needs_nothing() {
    ContextRunner::new()
        .config_str(format!("{FORM}{}{}", remember(""), users()))
        .auto_configurations(auto_configurations())
        .run(|view| assert!(view.contains::<SecurityFilterChain>()))
        .await;
    // Disabled: no form login, secret or user service is needed.
    ContextRunner::new()
        .config_str("[security.remember-me]\nenabled = false\nlifetime = \"1h\"\n")
        .auto_configurations(auto_configurations())
        .run(|view| assert!(view.contains::<SecurityFilterChain>()))
        .await;
}

// ---- End to end ----------------------------------------------------------------------------

struct Routes;

impl Controller for Routes {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/secret", |_| async {
            "secret"
        })]
    }
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

/// Starts the application with the `Switchable` user service, which is also the password
/// provider's user source (`security.users` is not set), and `clock`.
async fn start(toml: String, users: Switchable, clock: ManualClock) -> RunningApp {
    TestApp::secured()
        .config_str(toml)
        .clock(Arc::new(clock))
        .configure(|b| {
            b.instance(Routes)
                .provides(|c: Arc<Routes>| -> Arc<dyn Controller> { c })
        })
        .configure(move |b| {
            b.instance(users.clone())
                .provides(|u: Arc<Switchable>| -> Arc<dyn UserDetailsService> { u })
        })
        .start()
        .await
}

/// A browser: every cookie the responses set, by name, and the last `Set-Cookie` per name.
#[derive(Default)]
struct Browser {
    cookies: Mutex<BTreeMap<String, String>>,
    set: Mutex<BTreeMap<String, String>>,
}

impl Browser {
    async fn send(&self, app: &RunningApp, method: HttpMethod, path: &str, body: &str) -> String {
        let client = app.client();
        let mut req = match method {
            HttpMethod::Post => client.post(path),
            _ => client.get(path),
        }
        .header("accept", "text/html");
        let cookie = self
            .cookies
            .lock()
            .unwrap()
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if !cookie.is_empty() {
            req = req.header("cookie", cookie);
        }
        if !body.is_empty() {
            req = req
                .header("content-type", "application/x-www-form-urlencoded")
                .body(body.to_owned());
        }
        let res = req.send().await;
        self.set.lock().unwrap().clear();
        for set in res.header_all("set-cookie") {
            let pair = set.split(';').next().unwrap();
            let (name, value) = pair.split_once('=').unwrap();
            self.set
                .lock()
                .unwrap()
                .insert(name.to_owned(), set.to_owned());
            let mut cookies = self.cookies.lock().unwrap();
            if set.contains("Max-Age=0") {
                cookies.remove(name);
            } else {
                cookies.insert(name.to_owned(), value.to_owned());
            }
        }
        format!(
            "{} {}",
            res.status().as_u16(),
            res.header("location").unwrap_or_default()
        )
    }

    fn drop_session(&self) {
        self.cookies.lock().unwrap().remove("OXBLOOM_SESSION");
    }

    fn has(&self, name: &str) -> bool {
        self.cookies.lock().unwrap().contains_key(name)
    }

    fn last_set(&self, name: &str) -> Option<String> {
        self.set.lock().unwrap().get(name).cloned()
    }

    async fn login(&self, app: &RunningApp, checkbox: bool) {
        let mut body = "username=alice&password=wonderland".to_owned();
        if checkbox {
            body.push_str("&remember-me=on");
        }
        let reply = self.send(app, HttpMethod::Post, "/login", &body).await;
        assert_eq!(reply, "302 /", "the login succeeds");
    }

    async fn secret(&self, app: &RunningApp) -> String {
        self.send(app, HttpMethod::Get, "/secret", "").await
    }
}

const IN: &str = "200 ";
const OUT: &str = "302 /login";

#[tokio::test]
async fn the_checkbox_sets_the_cookie_with_the_configured_lifetime() {
    let clock = ManualClock::at(SystemTime::now());
    let app = start(
        format!("{FORM}{}", remember("lifetime = \"36h\"")),
        Switchable::new(),
        clock,
    )
    .await;
    let browser = Browser::default();
    browser.login(&app, true).await;
    let set = browser
        .last_set(COOKIE)
        .expect("the remember-me cookie is set");
    assert!(set.contains("Max-Age=129600"), "{set}");
    assert!(set.contains("HttpOnly"), "{set}");
    assert!(set.contains("SameSite=Lax"), "{set}");
    // Without the checkbox there is no cookie.
    let other = Browser::default();
    other.login(&app, false).await;
    assert!(!other.has(COOKIE));
    app.stop().await;
}

#[tokio::test]
async fn the_cookie_logs_in_again_after_the_session_is_gone() {
    let clock = ManualClock::at(SystemTime::now());
    let app = start(format!("{FORM}{}", remember("")), Switchable::new(), clock).await;
    let browser = Browser::default();
    browser.login(&app, true).await;
    assert!(browser.secret(&app).await.starts_with(IN));
    browser.drop_session();
    assert!(browser.secret(&app).await.starts_with(IN));
    // The auto login started a new session.
    assert!(browser.has("OXBLOOM_SESSION"));
    // A browser without the cookie is sent to the login page.
    let other = Browser::default();
    other.login(&app, false).await;
    other.drop_session();
    assert_eq!(other.secret(&app).await, OUT);
    app.stop().await;
}

#[tokio::test]
async fn a_disabled_user_is_not_logged_in_from_the_cookie() {
    let users = Switchable::new();
    let clock = ManualClock::at(SystemTime::now());
    let app = start(format!("{FORM}{}", remember("")), users.clone(), clock).await;
    let browser = Browser::default();
    browser.login(&app, true).await;
    assert!(browser.has(COOKIE));
    browser.drop_session();
    users.enabled.store(false, Ordering::SeqCst);
    assert_eq!(browser.secret(&app).await, OUT);
    // The refused cookie is cleared.
    assert!(!browser.has(COOKIE));
    app.stop().await;
}

#[tokio::test]
async fn the_cookie_expires_by_the_clock_bean() {
    let clock = ManualClock::at(SystemTime::now());
    let app = start(
        format!("{FORM}{}", remember("lifetime = \"1h\"")),
        Switchable::new(),
        clock.clone(),
    )
    .await;
    let early = Browser::default();
    early.login(&app, true).await;
    let late = Browser::default();
    late.login(&app, true).await;
    // Within the lifetime the cookie works.
    clock.advance(Duration::from_secs(3599));
    early.drop_session();
    assert!(early.secret(&app).await.starts_with(IN));
    // Past it, by the same clock, it does not.
    clock.advance(Duration::from_secs(2));
    late.drop_session();
    assert_eq!(late.secret(&app).await, OUT);
    app.stop().await;
}

/// Changes only the remember-me field name, and owns the rules.
struct StayField;

impl SecurityCustomizer for StayField {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.remember_me(|r| r.field_name("stay"))
            .authorize(|a| a.any_request().authenticated())
    }
}

#[tokio::test]
async fn a_customizer_composes_on_the_configured_remember_me() {
    let clock = ManualClock::at(SystemTime::now());
    let app = TestApp::secured()
        .config_str(format!("{FORM}{}", remember("lifetime = \"1h\"")))
        .clock(Arc::new(clock.clone()))
        .configure(|b| {
            b.instance(Routes)
                .provides(|c: Arc<Routes>| -> Arc<dyn Controller> { c })
                .instance(StayField)
                .provides(|c: Arc<StayField>| -> Arc<dyn SecurityCustomizer> { c })
        })
        .configure(|b| {
            b.instance(Switchable::new())
                .provides(|u: Arc<Switchable>| -> Arc<dyn UserDetailsService> { u })
        })
        .start()
        .await;
    // The customizer's field name asks for the cookie; the configured lifetime is kept.
    let early = Browser::default();
    let body = "username=alice&password=wonderland&stay=on";
    assert_eq!(
        early.send(&app, HttpMethod::Post, "/login", body).await,
        "302 /"
    );
    let set = early
        .last_set(COOKIE)
        .expect("the remember-me cookie is set");
    assert!(set.contains("Max-Age=3600"), "{set}");
    let late = Browser::default();
    assert_eq!(
        late.send(&app, HttpMethod::Post, "/login", body).await,
        "302 /"
    );
    // The configured secret and user service are kept: the cookie logs in again.
    clock.advance(Duration::from_secs(3599));
    early.drop_session();
    assert!(early.secret(&app).await.starts_with(IN));
    // The clock bean is still the one that judges the expiry.
    clock.advance(Duration::from_secs(2));
    late.drop_session();
    assert_eq!(late.secret(&app).await, OUT);
    app.stop().await;
}

// ---- Metadata ------------------------------------------------------------------------------

/// The remember-me keys `SecurityAutoConfiguration` lists, in order.
const KEYS: [&str; 6] = [
    "security.remember-me.cookie-name",
    "security.remember-me.enabled",
    "security.remember-me.field-name",
    "security.remember-me.lifetime",
    "security.remember-me.old-secrets",
    "security.remember-me.secret",
];

/// A TOML line that sets `key` to a value other than its default.
fn sample(key: &str) -> String {
    match key {
        "security.remember-me.cookie-name" => "cookie-name = \"KEEP\"".to_owned(),
        "security.remember-me.enabled" => "enabled = true".to_owned(),
        "security.remember-me.field-name" => "field-name = \"stay\"".to_owned(),
        "security.remember-me.lifetime" => "lifetime = \"1h\"".to_owned(),
        "security.remember-me.old-secrets" => format!("old-secrets = [\"{SECRET}-old\"]"),
        "security.remember-me.secret" => format!("secret = \"{SECRET}\""),
        other => panic!("no sample for {other}"),
    }
}

/// Everything the reader gave, secrets included, so two results can be compared.
fn fingerprint(settings: Option<RememberMeSettings>) -> Option<String> {
    settings.map(|s| {
        let old: Vec<&String> = s.old_secrets.iter().map(|o| o.expose()).collect();
        format!(
            "{:?} {old:?} {:?} {} {}",
            s.secret.as_ref().map(|k| k.expose()),
            s.lifetime,
            s.field_name,
            s.cookie_name
        )
    })
}

#[test]
fn security_metadata_lists_exactly_the_remember_me_keys_read() {
    let props = SecurityAutoConfiguration::properties();
    let keys: Vec<&str> = props
        .iter()
        .map(|p| p.key)
        .filter(|k| k.starts_with("security.remember-me."))
        .collect();
    assert_eq!(keys, KEYS);
    assert_eq!(keys.iter().collect::<BTreeSet<_>>().len(), keys.len());
    // Every listed key is read: setting it alone changes what the reader gives.
    let base = "[security.remember-me]\nenabled = true\n";
    for key in KEYS {
        let (with, without) = if key == "security.remember-me.enabled" {
            (
                format!("[security.remember-me]\n{}\n", sample(key)),
                "[security.remember-me]\n".to_owned(),
            )
        } else {
            (format!("{base}{}\n", sample(key)), base.to_owned())
        };
        assert_ne!(
            fingerprint(read(&with).unwrap()),
            fingerprint(read(&without).unwrap()),
            "{key} is not read"
        );
    }
    // Every read key is listed: all listed keys together are accepted, and the reader rejects
    // any other key in the section.
    let all: String = KEYS.iter().map(|k| sample(k) + "\n").collect();
    assert!(
        read(&format!("[security.remember-me]\n{all}"))
            .unwrap()
            .is_some()
    );
    assert!(read(&format!("[security.remember-me]\n{all}unlisted = 1\n")).is_err());
    // The documented defaults are the real ones, and the secrets have none.
    let default_of = |key: &str| props.iter().find(|p| p.key == key).unwrap().default;
    let defaults = read(base).unwrap().unwrap();
    assert_eq!(default_of("security.remember-me.enabled"), Some("false"));
    assert_eq!(default_of("security.remember-me.lifetime"), Some("14d"));
    assert_eq!(defaults.lifetime, Duration::from_secs(14 * 24 * 3600));
    assert_eq!(
        default_of("security.remember-me.field-name"),
        Some(defaults.field_name.as_str())
    );
    assert_eq!(
        default_of("security.remember-me.cookie-name"),
        Some(defaults.cookie_name.as_str())
    );
    for key in [
        "security.remember-me.secret",
        "security.remember-me.old-secrets",
    ] {
        let prop = props.iter().find(|p| p.key == key).unwrap();
        assert_eq!(prop.default, None, "{key}");
        assert!(prop.description.contains("never printed"), "{key}");
    }
}
