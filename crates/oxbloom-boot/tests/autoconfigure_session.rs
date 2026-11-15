//! `SessionAutoConfiguration`: the session filter of the `security.session` keys, the in-memory
//! and the cookie backend over HTTP, the application's store bean, the clock bean, the order
//! before the security chain and the startup failures that never echo a value.

#![cfg(all(feature = "web", feature = "security"))]

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use jsonwebtoken::{EncodingKey, Header, encode};
use oxbloom_boot::{
    AutoConfiguration, ConfigRef, PasswordEncoderAutoConfiguration, SecurityAutoConfiguration,
    SecurityCustomizer, SessionAutoConfiguration, StartupPhase, auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_security::{
    Algorithm, HttpSecurity, RequestSessionExt, SessionConfig, SessionData, SessionError,
    SessionFilter, SessionStore,
};
use oxbloom_test::{ContextRunner, ManualClock, RunningApp, TestApp};
use oxbloom_web::{Controller, Filter, HttpMethod, Request, Route};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";
const OLD_SECRET: &str = "an-older-secret-of-at-least-32-bytes";

/// `/visit` counts the visits of the session; `/data` answers whether a session is there.
struct Visits;

impl Controller for Visits {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![
            Route::new(HttpMethod::Get, "/visit", |req: Request| async move {
                let session = req.session().expect("the session filter ran");
                let visits = session.get::<u32>("visits").ok().flatten().unwrap_or(0) + 1;
                session
                    .set("visits", &visits)
                    .expect("the value serializes");
                visits.to_string()
            }),
            Route::new(HttpMethod::Get, "/data", |req: Request| async move {
                if req.session().is_some() {
                    "session"
                } else {
                    "none"
                }
            }),
        ]
    }
}

fn visits(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Visits)
        .provides(|c: Arc<Visits>| -> Arc<dyn Controller> { c })
}

async fn start(toml: String) -> RunningApp {
    TestApp::web()
        .auto_configuration::<SessionAutoConfiguration>()
        .config_str(toml)
        .configure(visits)
        .start()
        .await
}

/// The status, the body and the `name=value` part of the session `Set-Cookie`, if any.
async fn visit(app: &RunningApp, cookie: Option<&str>) -> (u16, String, Option<String>) {
    let mut req = app.client().get("/visit");
    if let Some(cookie) = cookie {
        req = req.header("cookie", cookie);
    }
    let res = req.send().await;
    let set = res
        .header("set-cookie")
        .map(|v| v.split(';').next().unwrap().to_owned());
    (res.status().as_u16(), res.text(), set)
}

#[tokio::test]
async fn no_session_keys_registers_no_filter() {
    ContextRunner::new()
        .auto_configuration::<SessionAutoConfiguration>()
        .run(|view| {
            assert!(!view.contains::<SessionFilter>());
            assert!(!view.contains::<dyn Filter>());
        })
        .await;
}

#[tokio::test]
async fn memory_store_default_works_over_http() {
    let app = start("[security.session]\nstore = \"memory\"\n".into()).await;
    let (status, body, cookie) = visit(&app, None).await;
    assert_eq!((status, body.as_str()), (200, "1"));
    let cookie = cookie.expect("the first write sets the cookie");
    assert!(cookie.starts_with("OXBLOOM_SESSION="), "{cookie}");
    let (status, body, _) = visit(&app, Some(&cookie)).await;
    assert_eq!((status, body.as_str()), (200, "2"));
    app.stop().await;
}

#[tokio::test]
async fn cookie_backend_works_over_http() {
    let app = start(format!(
        "[security.session]\nstore = \"cookie\"\ncookie-name = \"SID\"\nsecret = \"{SECRET}\"\n"
    ))
    .await;
    let (_, body, cookie) = visit(&app, None).await;
    assert_eq!(body, "1");
    let cookie = cookie.expect("the first write sets the cookie");
    assert!(cookie.starts_with("SID="), "{cookie}");
    let (_, body, next) = visit(&app, Some(&cookie)).await;
    assert_eq!(body, "2");
    // The cookie carries the data: the newest cookie continues the count.
    let (_, body, _) = visit(&app, Some(&next.unwrap())).await;
    assert_eq!(body, "3");
    app.stop().await;
}

#[tokio::test]
async fn cookie_backend_reads_cookies_of_an_old_secret() {
    // Sealed with the old secret as the current one...
    let app = start(format!(
        "[security.session]\nstore = \"cookie\"\nsecret = \"{OLD_SECRET}\"\n"
    ))
    .await;
    let (_, _, cookie) = visit(&app, None).await;
    app.stop().await;
    // ...and opened after the rotation, with the old secret in `old-secrets`.
    let app = start(format!(
        "[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n\
         old-secrets = [\"{OLD_SECRET}\"]\n"
    ))
    .await;
    let (_, body, _) = visit(&app, Some(&cookie.unwrap())).await;
    assert_eq!(body, "2");
    app.stop().await;
}

#[tokio::test]
async fn the_clock_bean_drives_the_cookie_backend() {
    let clock = ManualClock::at(SystemTime::now());
    let app = TestApp::web()
        .auto_configuration::<SessionAutoConfiguration>()
        .config_str(format!(
            "[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n\
             idle-timeout = \"1m\"\n"
        ))
        .clock(Arc::new(clock.clone()))
        .configure(visits)
        .start()
        .await;
    let (_, _, cookie) = visit(&app, None).await;
    let cookie = cookie.unwrap();
    clock.advance(Duration::from_secs(30));
    assert_eq!(visit(&app, Some(&cookie)).await.1, "2");
    // Past the idle timeout by the bean's time, not the system time: a fresh session.
    clock.advance(Duration::from_secs(120));
    assert_eq!(visit(&app, Some(&cookie)).await.1, "1");
    app.stop().await;
}

#[tokio::test]
async fn secret_on_the_command_line_is_refused() {
    for (key, value) in [
        ("security.session.secret", SECRET),
        ("security.session.old-secrets[0]", OLD_SECRET),
    ] {
        ContextRunner::new()
            .auto_configuration::<SessionAutoConfiguration>()
            .config_str(format!(
                "[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n"
            ))
            .property(key, value)
            .run_failing(move |failure| {
                let error = failure.error();
                let text = format!("{error} {error:?} {}", failure.output().contents());
                assert!(text.contains("command line"), "{text}");
                assert!(text.contains(key), "{text}");
                assert!(!text.contains(value), "{value} leaked: {text}");
            })
            .await;
    }
}

/// Runs the session auto-configuration with `body` under `[security.session]` and expects a
/// `ConfigError` on `key` whose text carries none of `hidden`.
async fn fails_naming(body: String, key: &'static str, hidden: Vec<String>) {
    ContextRunner::new()
        .config_str(format!("[security.session]\n{body}\n"))
        .auto_configurations(auto_configurations())
        .run_failing(move |failure| {
            let error = failure.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let di = error.find_cause::<DiError>().expect("a DiError cause");
            assert!(matches!(di, DiError::Creation { .. }), "{di:?}");
            let cause = error
                .find_cause::<ConfigError>()
                .expect("a ConfigError cause");
            match cause {
                ConfigError::Invalid { key: k, .. } => assert_eq!(k, key),
                other => panic!("expected Invalid on {key}, got {other:?}"),
            }
            let text = format!(
                "{error} {error:?} {cause:?} {}",
                failure.output().contents()
            );
            for needle in &hidden {
                assert!(!text.contains(needle.as_str()), "{needle} leaked: {text}");
            }
        })
        .await;
}

#[tokio::test]
async fn bad_values_fail_startup_without_echoing_them() {
    let short = "short-secret-value";
    fails_naming(
        format!("store = \"cookie\"\nsecret = \"{short}\""),
        "security.session.secret",
        vec![short.into()],
    )
    .await;
    fails_naming(
        "store = \"cookie\"".into(),
        "security.session.secret",
        vec![],
    )
    .await;
    fails_naming(
        "store = \"redis-at-db-host\"".into(),
        "security.session.store",
        vec!["redis-at-db-host".into()],
    )
    .await;
    fails_naming(
        "same-site = \"none\"".into(),
        "security.session.same-site",
        vec![],
    )
    .await;
    fails_naming(
        "cookie-name = \"bad;cookie-name\"".into(),
        "security.session.cookie-name",
        vec!["bad;cookie-name".into()],
    )
    .await;
    fails_naming(
        "max-cookie-bytes = 255".into(),
        "security.session.max-cookie-bytes",
        vec!["255".into()],
    )
    .await;
    fails_naming(
        "idle-timeout = \"9h\"".into(),
        "security.session.idle-timeout",
        vec!["9h".into()],
    )
    .await;
    fails_naming("bogus = 1".into(), "security.session", vec![]).await;
}

/// A store that keeps sessions in a map and counts the saves, or fails every call.
/// Clones share the sessions and the count.
#[derive(Clone, Default)]
struct CountingStore {
    sessions: Arc<Mutex<HashMap<String, SessionData>>>,
    saves: Arc<Mutex<usize>>,
    broken: bool,
}

impl CountingStore {
    fn check(&self) -> Result<(), SessionError> {
        if self.broken {
            Err(SessionError::Store)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl SessionStore for CountingStore {
    async fn load(&self, id: &str) -> Result<Option<SessionData>, SessionError> {
        self.check()?;
        Ok(self.sessions.lock().unwrap().get(id).cloned())
    }

    async fn save(&self, id: &str, data: &SessionData, _ttl: Duration) -> Result<(), SessionError> {
        self.check()?;
        *self.saves.lock().unwrap() += 1;
        self.sessions
            .lock()
            .unwrap()
            .insert(id.to_owned(), data.clone());
        Ok(())
    }

    async fn update(
        &self,
        id: &str,
        data: &SessionData,
        _ttl: Duration,
    ) -> Result<bool, SessionError> {
        self.check()?;
        *self.saves.lock().unwrap() += 1;
        match self.sessions.lock().unwrap().get_mut(id) {
            Some(stored) => {
                *stored = data.clone();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    async fn delete(&self, id: &str) -> Result<(), SessionError> {
        self.check()?;
        self.sessions.lock().unwrap().remove(id);
        Ok(())
    }

    async fn rotate(&self, old: &str, new: &str) -> Result<bool, SessionError> {
        self.check()?;
        let mut map = self.sessions.lock().unwrap();
        match map.remove(old) {
            Some(data) => {
                map.insert(new.to_owned(), data);
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

async fn start_with_store(toml: String, store: CountingStore) -> RunningApp {
    TestApp::web()
        .auto_configuration::<SessionAutoConfiguration>()
        .config_str(toml)
        .configure(visits)
        .configure(move |b| {
            b.instance(store.clone())
                .provides(|s: Arc<CountingStore>| -> Arc<dyn SessionStore> { s })
        })
        .start()
        .await
}

#[tokio::test]
async fn user_store_bean_replaces_the_memory_store() {
    let store = CountingStore::default();
    let app = start_with_store("[security.session]\n".into(), store.clone()).await;
    let (_, body, cookie) = visit(&app, None).await;
    assert_eq!(body, "1");
    assert_eq!(*store.saves.lock().unwrap(), 1);
    assert_eq!(store.sessions.lock().unwrap().len(), 1);
    let (_, body, _) = visit(&app, Some(&cookie.unwrap())).await;
    assert_eq!(body, "2");
    assert_eq!(*store.saves.lock().unwrap(), 2);
    app.stop().await;
}

#[tokio::test]
async fn user_store_is_ignored_with_cookie_store() {
    let store = CountingStore::default();
    let app = start_with_store(
        format!("[security.session]\nstore = \"cookie\"\nsecret = \"{SECRET}\"\n"),
        store.clone(),
    )
    .await;
    let (_, body, cookie) = visit(&app, None).await;
    assert_eq!(body, "1");
    let (_, body, _) = visit(&app, Some(&cookie.unwrap())).await;
    assert_eq!(body, "2");
    assert_eq!(*store.saves.lock().unwrap(), 0, "the store bean was used");
    app.stop().await;
}

#[tokio::test]
async fn store_failure_gives_503_end_to_end() {
    let store = CountingStore {
        broken: true,
        ..CountingStore::default()
    };
    let app = start_with_store("[security.session]\n".into(), store).await;
    let (status, body, cookie) = visit(&app, None).await;
    assert_eq!(status, 503);
    // The server's error renderer turns the filter's empty 503 into a problem document.
    assert!(body.contains("\"status\":503"), "{body}");
    assert_eq!(cookie, None);
    app.stop().await;
}

/// A TOML line for each key of the metadata.
fn sample(key: &str) -> String {
    let value = match key {
        "security.session.store" => "\"cookie\"".to_owned(),
        "security.session.cookie-name" => "\"SID\"".to_owned(),
        "security.session.secure" => "\"always\"".to_owned(),
        "security.session.same-site" => "\"strict\"".to_owned(),
        "security.session.idle-timeout" => "\"10m\"".to_owned(),
        "security.session.absolute-timeout" => "\"2h\"".to_owned(),
        "security.session.max-sessions" => "10".to_owned(),
        "security.session.max-cookie-bytes" => "4000".to_owned(),
        "security.session.secret" => format!("\"{SECRET}\""),
        "security.session.old-secrets" => format!("[\"{OLD_SECRET}\"]"),
        other => panic!("no sample for {other}"),
    };
    format!(
        "{} = {value}\n",
        key.trim_start_matches("security.session.")
    )
}

#[test]
fn metadata_lists_exactly_the_keys_read() {
    let props = SessionAutoConfiguration::properties();
    let keys: Vec<&str> = props.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "security.session.absolute-timeout",
            "security.session.cookie-name",
            "security.session.idle-timeout",
            "security.session.max-cookie-bytes",
            "security.session.max-sessions",
            "security.session.old-secrets",
            "security.session.same-site",
            "security.session.secret",
            "security.session.secure",
            "security.session.store",
        ]
    );
    assert_eq!(
        keys.iter().collect::<BTreeSet<_>>().len(),
        keys.len(),
        "no duplicates"
    );

    // Every listed key is read: alone (with a secret for the cookie store), it changes a value.
    // `Debug` shows every setting, and each secret as `[REDACTED]`, so a set secret shows too.
    let read = |body: &str| {
        let config = Config::from_str(&format!("[security.session]\n{body}")).unwrap();
        format!("{:?}", SessionConfig::from_config(&config).unwrap())
    };
    let base = read("");
    for key in &keys {
        let body = sample(key);
        if *key == "security.session.store" {
            // The cookie store needs a secret; compared with the secret alone.
            let secret = sample("security.session.secret");
            assert_ne!(read(&format!("{body}{secret}")), read(&secret));
            continue;
        }
        assert_ne!(read(&body), base, "{key} is not read");
    }

    // All together are accepted, and any other key in the section is rejected.
    let all: String = keys.iter().map(|k| sample(k)).collect();
    let _ = read(&all);
    let config = Config::from_str(&format!("[security.session]\n{all}bogus = 1\n")).unwrap();
    assert!(SessionConfig::from_config(&config).is_err());

    // The documented defaults are the real ones.
    let settings = oxbloom_security::SessionSettings::default();
    let default_of = |key: &str| props.iter().find(|p| p.key == key).unwrap().default;
    assert_eq!(default_of("security.session.store"), Some("memory"));
    assert_eq!(
        default_of("security.session.cookie-name"),
        Some(settings.cookie_name.as_str())
    );
    assert_eq!(default_of("security.session.secure"), Some("auto"));
    assert_eq!(default_of("security.session.same-site"), Some("lax"));
    assert_eq!(default_of("security.session.idle-timeout"), Some("30m"));
    assert_eq!(settings.idle_timeout, Duration::from_secs(30 * 60));
    assert_eq!(default_of("security.session.absolute-timeout"), Some("8h"));
    assert_eq!(settings.absolute_timeout, Duration::from_secs(8 * 3600));
    assert_eq!(
        default_of("security.session.max-sessions"),
        Some(settings.max_sessions.to_string().as_str())
    );
    assert_eq!(
        default_of("security.session.max-cookie-bytes"),
        Some(settings.max_cookie_bytes.to_string().as_str())
    );
    // The secrets are described, never given a value.
    for key in ["security.session.secret", "security.session.old-secrets"] {
        let p = props.iter().find(|p| p.key == key).unwrap();
        assert_eq!(p.default, None);
        assert!(p.description.contains("never printed"), "{key}");
        assert!(p.description.contains("command line"), "{key}");
    }
    // The filter is registered only when the section exists; the store key says so.
    let store = props
        .iter()
        .find(|p| p.key == "security.session.store")
        .unwrap();
    assert!(
        store
            .description
            .contains("only when the `security.session` table exists"),
        "{}",
        store.description
    );
}

#[tokio::test]
async fn condition_report_shows_the_skip() {
    ContextRunner::new()
        .auto_configuration::<SessionAutoConfiguration>()
        .run(|view| {
            let records = view.conditions().records();
            assert!(
                records.iter().any(|r| !r.matched()
                    && r.condition() == "on_property"
                    && r.reason().contains("security.session")),
                "{records:?}"
            );
        })
        .await;

    // The application's own filter: the configured one steps aside.
    ContextRunner::new()
        .config_str("[security.session]\n")
        .auto_configuration::<SessionAutoConfiguration>()
        .configure(|b| {
            b.try_bean(|| -> Result<SessionFilter, DiError> {
                let settings = oxbloom_security::SessionSettings::default();
                let store = oxbloom_security::InMemorySessionStore::new(
                    &settings,
                    Arc::new(oxbloom_core::SystemClock),
                );
                SessionFilter::new(
                    oxbloom_security::SessionBackend::Server(Arc::new(store)),
                    settings,
                )
                .map_err(DiError::creation_from::<SessionFilter>)
            })
        })
        .run(|view| {
            let records = view.conditions().records();
            assert!(
                records.iter().any(|r| !r.matched()
                    && r.condition() == "on_missing_bean"
                    && r.reason().contains("SessionFilter")),
                "{records:?}"
            );
            assert!(!view.contains::<dyn Filter>());
        })
        .await;
}

#[test]
fn order_is_after_the_password_encoder_and_before_security() {
    assert_eq!(
        SessionAutoConfiguration::after(),
        [ConfigRef::of::<PasswordEncoderAutoConfiguration>()]
    );
    assert_eq!(
        SessionAutoConfiguration::before(),
        [ConfigRef::of::<SecurityAutoConfiguration>()]
    );
}

/// Opens nothing: every request must be authenticated.
struct Closed;

impl SecurityCustomizer for Closed {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| a.any_request().authenticated())
    }
}

#[tokio::test]
async fn filter_order_is_before_security() {
    let app = TestApp::secured()
        .config_str(format!(
            "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\n\
             [security.session]\n"
        ))
        .configure(visits)
        .configure(|b| {
            b.instance(Closed)
                .provides(|c: Arc<Closed>| -> Arc<dyn SecurityCustomizer> { c })
        })
        .start()
        .await;
    let exp = jsonwebtoken::get_current_timestamp() + 300;
    let token = encode(
        &Header::new(Algorithm::HS256),
        &serde_json::json!({ "sub": "jwt-user", "exp": exp }),
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    let res = app
        .client()
        .get("/data")
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await;
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(res.text(), "session");
    // Without a token the chain refuses, after the session filter ran.
    assert_eq!(
        app.client().get("/data").send().await.status().as_u16(),
        401
    );
    app.stop().await;
}
