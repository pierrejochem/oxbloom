//! `PasswordEncoderAutoConfiguration` and HTTP Basic in `SecurityAutoConfiguration`: users
//! from `security.users`, the `security.http-basic` switch, the back-off rules and the
//! startup failures that never echo a value.

#![cfg(feature = "security")]

use std::collections::BTreeSet;
use std::sync::Arc;

use http::StatusCode;
use jsonwebtoken::{EncodingKey, Header, encode};
use oxbloom_boot::{
    AutoConfiguration, PasswordEncoderAutoConfiguration, SecurityAutoConfiguration,
    SecurityCustomizer, StartupPhase, auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_security::{
    Algorithm, Argon2PasswordEncoder, AuthenticationProvider, DelegatingPasswordEncoder,
    HttpBasicSettings, HttpSecurity, InMemoryUserDetailsService, PasswordEncoder, PasswordSettings,
    SecurityBuildError, UserDetails, UserDetailsService, users_from_config,
};
use oxbloom_test::{ContextRunner, RunningApp, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";

/// A low-cost encoder, so the tests hash fast.
fn cheap() -> DelegatingPasswordEncoder {
    DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap())
}

/// The stored form of `raw`, as `security.users.<name>.password` takes it.
fn hash(raw: &str) -> String {
    cheap().encode(raw)
}

/// `alice` (password `wonderland`, role `USER`) and `root` (password `toor`, role `ADMIN`).
fn users_section() -> String {
    format!(
        "[security.users.alice]\npassword = \"{}\"\nroles = [\"USER\"]\n\
         [security.users.root]\npassword = \"{}\"\nroles = [\"ADMIN\"]\n",
        hash("wonderland"),
        hash("toor")
    )
}

const BASIC_ON: &str = "[security.http-basic]\nenabled = true\n";

/// The `Authorization` value for `user:password`.
fn basic(user: &str, password: &str) -> String {
    format!("Basic {}", base64(format!("{user}:{password}").as_bytes()))
}

/// Standard base64 with padding.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

struct Routes;

impl Controller for Routes {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![
            Route::new(HttpMethod::Get, "/hello", |_| async { "hello" }),
            Route::new(HttpMethod::Get, "/admin", |_| async { "admin" }),
        ]
    }
}

/// `/admin` needs `ADMIN`; everything else needs authentication.
struct Rules;

impl SecurityCustomizer for Rules {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.get("/admin")
                .has_role("ADMIN")
                .any_request()
                .authenticated()
        })
    }
}

fn wiring(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Routes)
        .provides(|c: Arc<Routes>| -> Arc<dyn Controller> { c })
        .instance(Rules)
        .provides(|c: Arc<Rules>| -> Arc<dyn SecurityCustomizer> { c })
}

async fn start(toml: String) -> RunningApp {
    TestApp::secured()
        .config_str(toml)
        .configure(wiring)
        .start()
        .await
}

async fn get(app: &RunningApp, path: &str, auth: Option<&str>) -> (StatusCode, Option<String>) {
    let mut req = app.client().get(path);
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    let res = req.send().await;
    (
        res.status(),
        res.header("www-authenticate").map(str::to_owned),
    )
}

#[tokio::test]
async fn config_users_authenticate_over_http() {
    let app = start(format!("{BASIC_ON}{}", users_section())).await;
    let alice = basic("alice", "wonderland");
    assert_eq!(get(&app, "/hello", Some(&alice)).await.0, StatusCode::OK);
    let (status, challenge) = get(&app, "/hello", Some(&basic("alice", "wrong"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(challenge.unwrap().starts_with("Basic realm=\"Oxbloom\""));
    let (status, challenge) = get(&app, "/hello", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenge.as_deref(),
        Some("Basic realm=\"Oxbloom\", charset=\"UTF-8\"")
    );
    // The right login with the wrong role.
    assert_eq!(
        get(&app, "/admin", Some(&alice)).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(&app, "/admin", Some(&basic("root", "toor"))).await.0,
        StatusCode::OK
    );
    // The name is compared exactly.
    assert_eq!(
        get(&app, "/hello", Some(&basic("Alice", "wonderland")))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    app.stop().await;
}

#[tokio::test]
async fn the_realm_comes_from_the_config() {
    let app = start(format!(
        "{BASIC_ON}realm = \"Admin area\"\n{}",
        users_section()
    ))
    .await;
    let (_, challenge) = get(&app, "/hello", None).await;
    assert_eq!(
        challenge.as_deref(),
        Some("Basic realm=\"Admin area\", charset=\"UTF-8\"")
    );
    app.stop().await;
}

#[tokio::test]
async fn basic_is_closed_without_the_enabled_key() {
    let app = start(users_section()).await;
    let (status, challenge) = get(&app, "/hello", Some(&basic("alice", "wonderland"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let challenge = challenge.unwrap_or_default();
    assert!(!challenge.starts_with("Basic"), "{challenge}");
    app.stop().await;

    // `enabled = false` written out is the same.
    let app = start(format!(
        "[security.http-basic]\nenabled = false\n{}",
        users_section()
    ))
    .await;
    let (status, _) = get(&app, "/hello", Some(&basic("alice", "wonderland"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    app.stop().await;
}

#[tokio::test]
async fn enabled_without_users_fails_startup() {
    ContextRunner::new()
        .config_str(BASIC_ON)
        .auto_configurations(auto_configurations())
        .run_failing(|failure| {
            let error = failure.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let di = error.find_cause::<DiError>().expect("a DiError cause");
            assert!(matches!(di, DiError::Creation { .. }), "{di:?}");
            assert_eq!(
                error.find_cause::<SecurityBuildError>(),
                Some(&SecurityBuildError::HttpBasicWithoutProvider)
            );
        })
        .await;
}

#[tokio::test]
async fn user_bean_overrides_config_users() {
    let mine = InMemoryUserDetailsService::new(vec![
        UserDetails::new("carol", hash("secret"), []).roles(&["ADMIN"]),
    ])
    .unwrap();
    // The configured users are not read: a broken password there does not matter.
    let app = TestApp::secured()
        .config_str(format!(
            "{BASIC_ON}[security.users.alice]\npassword = \"plain\"\n"
        ))
        .configure(wiring)
        .configure(move |b| {
            b.instance(mine)
                .provides(|s: Arc<InMemoryUserDetailsService>| -> Arc<dyn UserDetailsService> { s })
        })
        .start()
        .await;
    assert_eq!(
        get(&app, "/admin", Some(&basic("carol", "secret"))).await.0,
        StatusCode::OK
    );
    assert_eq!(
        get(&app, "/hello", Some(&basic("alice", "plain"))).await.0,
        StatusCode::UNAUTHORIZED
    );
    app.stop().await;

    ContextRunner::new()
        .config_str(users_section())
        .auto_configuration::<PasswordEncoderAutoConfiguration>()
        .configure(|b| {
            b.instance(InMemoryUserDetailsService::new(vec![]).unwrap())
                .provides(|s: Arc<InMemoryUserDetailsService>| -> Arc<dyn UserDetailsService> { s })
        })
        .run(|view| {
            // One store only, the application's: `get` fails on two.
            assert!(view.get::<InMemoryUserDetailsService>().is_ok());
            assert!(view.get::<dyn UserDetailsService>().is_ok());
            assert!(view.contains::<dyn AuthenticationProvider>());
        })
        .await;
}

/// An encoder that accepts exactly the stored text `plain:<raw>`.
struct PlainEncoder;

impl PasswordEncoder for PlainEncoder {
    fn encode(&self, raw: &str) -> String {
        format!("plain:{raw}")
    }

    fn matches(&self, raw: &str, encoded: &str) -> bool {
        encoded == format!("plain:{raw}")
    }

    fn upgrade_encoding(&self, _encoded: &str) -> bool {
        false
    }
}

#[tokio::test]
async fn encoder_bean_overrides_the_default() {
    ContextRunner::new()
        // A broken `security.password` section is not read when the encoder is the
        // application's own.
        .config_str("[security.password]\ndefault-encoder = \"md5\"\n")
        .auto_configuration::<PasswordEncoderAutoConfiguration>()
        .configure(|b| {
            b.instance(PlainEncoder)
                .provides(|e: Arc<PlainEncoder>| -> Arc<dyn PasswordEncoder> { e })
        })
        .run(|view| {
            assert!(!view.contains::<DelegatingPasswordEncoder>());
            let encoders = view.get::<dyn PasswordEncoder>().unwrap();
            assert_eq!(encoders.encode("x"), "plain:x");
        })
        .await;

    // Without the application's encoder the delegating one is registered.
    ContextRunner::new()
        .auto_configuration::<PasswordEncoderAutoConfiguration>()
        .run(|view| {
            assert!(view.contains::<DelegatingPasswordEncoder>());
            let stored = view.get::<dyn PasswordEncoder>().unwrap().encode("x");
            assert!(stored.starts_with("{argon2}$argon2id$"));
        })
        .await;
}

/// Runs the auto-configurations with `toml` and expects a `ConfigError` on `key` whose text
/// carries none of `hidden`.
async fn fails_naming(toml: String, key: &'static str, hidden: Vec<String>) {
    ContextRunner::new()
        .config_str(toml)
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
    fails_naming(
        "[security.users.alice]\npassword = \"plain-hunter2\"\n".into(),
        "security.users.alice.password",
        vec!["plain-hunter2".into()],
    )
    .await;
    fails_naming(
        "[security.password.argon2]\nmemory-kib = 2000000\n".into(),
        "security.password.argon2.memory-kib",
        vec!["2000000".into()],
    )
    .await;
    fails_naming(
        "[security.password]\nencoder = \"argon2\"\n".into(),
        "security.password",
        vec![],
    )
    .await;
    fails_naming(
        "[security.password.argon2]\nlanes = 2\n".into(),
        "security.password.argon2",
        vec![],
    )
    .await;
    fails_naming(
        format!("{BASIC_ON}realms = \"x\"\n{}", users_section()),
        "security.http-basic",
        vec![],
    )
    .await;
    let stored = hash("pw");
    fails_naming(
        format!("[security.users.alice]\npassword = \"{stored}\"\nrole = [\"A\"]\n"),
        "security.users.alice",
        vec![stored.clone()],
    )
    .await;
}

/// A TOML line for each key of the metadata, with `<name>` as `alice`.
fn sample(key: &str) -> String {
    let value = match key {
        "security.http-basic.enabled" => "true".to_owned(),
        "security.http-basic.realm" => "\"Admin\"".to_owned(),
        "security.password.argon2.iterations" => "3".to_owned(),
        "security.password.argon2.memory-kib" => "64".to_owned(),
        "security.password.argon2.parallelism" => "2".to_owned(),
        "security.password.default-encoder" => "\"argon2\"".to_owned(),
        "security.users.<name>.enabled" => "false".to_owned(),
        "security.users.<name>.locked" => "true".to_owned(),
        "security.users.<name>.password" => format!("\"{}\"", hash("pw")),
        "security.users.<name>.roles" => "[\"ADMIN\"]".to_owned(),
        other => panic!("no sample for {other}"),
    };
    format!("{} = {value}\n", key.replace("<name>", "alice"))
}

#[test]
fn metadata_lists_exactly_the_keys_read() {
    let props = PasswordEncoderAutoConfiguration::properties();
    let keys: Vec<&str> = props.iter().map(|p| p.key).collect();
    assert_eq!(
        keys,
        [
            "security.http-basic.enabled",
            "security.http-basic.realm",
            "security.password.argon2.iterations",
            "security.password.argon2.memory-kib",
            "security.password.argon2.parallelism",
            "security.password.default-encoder",
            "security.users.<name>.enabled",
            "security.users.<name>.locked",
            "security.users.<name>.password",
            "security.users.<name>.roles",
        ]
    );
    assert_eq!(
        keys.iter().collect::<BTreeSet<_>>().len(),
        keys.len(),
        "no duplicates"
    );

    // Every listed key is read: alone (with a password for the user keys), it changes a value.
    let password = sample("security.users.<name>.password");
    let read_all = |toml: &str| {
        let config = Config::from_str(toml).unwrap();
        (
            PasswordSettings::from_config(&config).unwrap(),
            HttpBasicSettings::from_config(&config).unwrap(),
            format!("{:?}", users_from_config(&config).unwrap()),
        )
    };
    let base = read_all(&password);
    for key in &keys {
        if key.ends_with(".password") {
            continue;
        }
        if *key == "security.password.default-encoder" {
            // Its only value is the default; another one is refused, so the key is read.
            let config = Config::from_str("security.password.default-encoder = \"x\"").unwrap();
            assert!(PasswordSettings::from_config(&config).is_err());
            continue;
        }
        let toml = format!("{}{password}", sample(key));
        assert_ne!(read_all(&toml), base, "{key} is not read");
    }
    assert_ne!(
        read_all(&password).2,
        read_all("").2,
        "password is not read"
    );

    // All together are accepted, and any other key in a section is rejected.
    let all: String = keys.iter().map(|k| sample(k)).collect();
    let _ = read_all(&all);
    for (extra, reader) in [
        ("security.password.bogus = 1\n", "password"),
        ("security.password.argon2.bogus = 1\n", "password"),
        ("security.http-basic.bogus = 1\n", "basic"),
        ("security.users.alice.bogus = 1\n", "users"),
    ] {
        let config = Config::from_str(&format!("{all}{extra}")).unwrap();
        let rejected = match reader {
            "password" => PasswordSettings::from_config(&config).is_err(),
            "basic" => HttpBasicSettings::from_config(&config).is_err(),
            _ => users_from_config(&config).is_err(),
        };
        assert!(rejected, "{extra}");
    }

    // The documented defaults are the real ones.
    let default_of = |key: &str| props.iter().find(|p| p.key == key).unwrap().default;
    let settings = PasswordSettings::default();
    let basic = HttpBasicSettings::default();
    assert_eq!(default_of("security.http-basic.enabled"), Some("false"));
    assert!(!basic.enabled);
    assert_eq!(
        default_of("security.http-basic.realm"),
        Some(basic.realm.as_str())
    );
    assert_eq!(
        default_of("security.password.default-encoder"),
        Some(settings.default_encoder.as_str())
    );
    assert_eq!(
        default_of("security.password.argon2.memory-kib"),
        Some(settings.argon2.memory_kib.to_string().as_str())
    );
    assert_eq!(
        default_of("security.password.argon2.iterations"),
        Some(settings.argon2.iterations.to_string().as_str())
    );
    assert_eq!(
        default_of("security.password.argon2.parallelism"),
        Some(settings.argon2.parallelism.to_string().as_str())
    );
    assert_eq!(default_of("security.users.<name>.password"), None);
    // Security lists the `security.form-login`, `security.csrf`, `security.headers` and
    // `security.remember-me` keys only (see `autoconfigure_form_login.rs`,
    // `autoconfigure_csrf.rs`, `autoconfigure_session_management.rs` and
    // `autoconfigure_remember_me.rs`).
    assert!(SecurityAutoConfiguration::properties().iter().all(|p| {
        p.key.starts_with("security.form-login.")
            || p.key.starts_with("security.csrf.")
            || p.key.starts_with("security.headers.")
            || p.key.starts_with("security.remember-me.")
    }));
}

#[tokio::test]
async fn condition_report_shows_the_skips() {
    // No users: the store and the DAO provider are skipped, the encoder is kept.
    ContextRunner::new()
        .auto_configuration::<PasswordEncoderAutoConfiguration>()
        .run(|view| {
            let records = view.conditions().records();
            let skipped = |condition: &str, needle: &str| {
                records.iter().any(|r| {
                    !r.matched() && r.condition() == condition && r.reason().contains(needle)
                })
            };
            assert!(skipped("on_property", "security.users"), "{records:?}");
            assert!(skipped("on_bean", "UserDetailsService"), "{records:?}");
            assert!(view.contains::<dyn PasswordEncoder>());
            assert!(!view.contains::<dyn UserDetailsService>());
            assert!(!view.contains::<dyn AuthenticationProvider>());
        })
        .await;

    // The application's encoder: the default one steps aside.
    ContextRunner::new()
        .auto_configuration::<PasswordEncoderAutoConfiguration>()
        .configure(|b| {
            b.instance(PlainEncoder)
                .provides(|e: Arc<PlainEncoder>| -> Arc<dyn PasswordEncoder> { e })
        })
        .run(|view| {
            let records = view.conditions().records();
            assert!(
                records.iter().any(|r| !r.matched()
                    && r.condition() == "on_missing_bean"
                    && r.reason().contains("PasswordEncoder")),
                "{records:?}"
            );
        })
        .await;
}

fn token() -> String {
    let exp = jsonwebtoken::get_current_timestamp() + 300;
    encode(
        &Header::new(Algorithm::HS256),
        &serde_json::json!({ "sub": "jwt-user", "exp": exp }),
        &EncodingKey::from_secret(SECRET.as_bytes()).expect("a valid test secret"),
    )
    .unwrap()
}

#[tokio::test]
async fn works_with_jwt_resource_server() {
    let app = start(format!(
        "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\n{BASIC_ON}{}",
        users_section()
    ))
    .await;
    assert_eq!(
        get(&app, "/hello", Some(&basic("alice", "wonderland")))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        get(&app, "/hello", Some(&format!("Bearer {}", token())))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        get(&app, "/hello", Some("Bearer not-a-jwt")).await.0,
        StatusCode::UNAUTHORIZED
    );
    // Anonymous: the Basic challenge comes first.
    let (status, challenge) = get(&app, "/hello", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(challenge.unwrap().starts_with("Basic"));
    app.stop().await;
}
