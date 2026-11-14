//! The security auto-configurations: `JwtDecoderAutoConfiguration` and
//! `SecurityAutoConfiguration`, run in process through the boot builder.

#![cfg(feature = "security")]

mod common;

use std::any::type_name;
use std::collections::BTreeSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jsonwebtoken::{EncodingKey, Header, encode};
use oxbloom_boot::{
    AutoConfiguration, Banner, JwtDecoderAutoConfiguration, OxbloomApplication,
    OxbloomApplicationBuilder, RunListener, SecurityAutoConfiguration, SecurityCustomizer,
    StartupPhase, auto_configurations,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{Container, DiError};
use oxbloom_security::{
    Algorithm, AuthenticationEntryPoint, AuthenticationError, DefaultJwtDecoder, HttpSecurity, Jwt,
    JwtDecoder, SecurityBuildError, SecurityFilterChain, jwt_decoder_from_config,
};
use oxbloom_test::{Captured, ContextRunner, Log};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

fn jwt_section() -> String {
    format!("[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\n")
}

/// One condition record: registration, condition, matched, reason.
type Record = (String, String, bool, String);

/// What the container held when it was ready.
#[derive(Default)]
struct Snapshot {
    decoders: Vec<Arc<dyn JwtDecoder>>,
    default_decoder: bool,
    chains: Vec<Arc<SecurityFilterChain>>,
    records: Vec<Record>,
}

/// Captures a [`Snapshot`] at `context_ready`.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Option<Snapshot>>>);

impl RunListener for Capture {
    fn context_ready(&self, c: &Container) {
        let snapshot = Snapshot {
            decoders: c.get_all::<dyn JwtDecoder>().unwrap(),
            default_decoder: c.get_opt::<DefaultJwtDecoder>().unwrap().is_some(),
            chains: c.get_all::<SecurityFilterChain>().unwrap(),
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

fn app(config: &str, out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str(config).unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .failure_report(true)
        .auto_configurations(auto_configurations())
}

/// Runs `builder` (which has no service, so it returns after the runners) and captures the
/// container.
async fn snapshot(builder: OxbloomApplicationBuilder) -> Snapshot {
    let capture = Capture::default();
    within(builder.listener(capture.clone()).run())
        .await
        .unwrap();
    capture.take()
}

/// The single chain of `s`, as an app with a few routes.
fn web(s: &Snapshot) -> App {
    assert_eq!(s.chains.len(), 1, "exactly one chain");
    App::new()
        .filter(Arc::clone(&s.chains[0]))
        .route(Route::new(HttpMethod::Get, "/data", |_| async { "data" }))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/open/x", |_| async { "open" }))
        .unwrap()
}

async fn status(app: &App, path: &str, token: Option<&str>) -> u16 {
    let mut req = Request::new(HttpMethod::Get, path);
    if let Some(token) = token {
        req.headers
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    app.handle(req).await.status().as_u16()
}

fn token() -> String {
    let exp = jsonwebtoken::get_current_timestamp() + 300;
    encode(
        &Header::new(Algorithm::HS256),
        &serde_json::json!({ "sub": "alice", "exp": exp }),
        &EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

#[tokio::test]
async fn jwt_decoder_is_built_when_section_present() {
    let out = Captured::new();
    // With the conditions report on, so the printed output is checked for the secret too.
    let toml = format!(
        "oxbloom.main.conditions-report = \"console\"\n{}",
        jwt_section()
    );
    let s = snapshot(app(&toml, &out)).await;
    let printed = out.contents();
    assert!(
        printed.contains("CONDITIONS EVALUATION REPORT"),
        "{printed}"
    );
    assert!(printed.contains("security.jwt"), "{printed}");
    assert!(!printed.contains(SECRET), "the secret leaked: {printed}");
    assert_eq!(s.decoders.len(), 1);
    assert!(s.default_decoder);
    let group = type_name::<JwtDecoderAutoConfiguration>();
    assert!(
        s.records
            .iter()
            .any(|(r, c, m, _)| r == group && c == "on_property" && *m),
        "{:?}",
        s.records
    );
    // The chain uses the decoder: a valid token is accepted.
    let app = web(&s);
    assert_eq!(status(&app, "/data", Some(&token())).await, 200);
    assert_eq!(status(&app, "/data", Some("not-a-jwt")).await, 401);
    assert_eq!(status(&app, "/data", None).await, 401);
}

#[tokio::test]
async fn jwt_decoder_absent_without_section() {
    let out = Captured::new();
    let s = snapshot(app("", &out)).await;
    assert!(s.decoders.is_empty());
    assert!(!s.default_decoder);
    let group = type_name::<JwtDecoderAutoConfiguration>();
    let record = s
        .records
        .iter()
        .find(|(r, _, _, _)| r == group)
        .expect("a group record");
    assert_eq!(record.1, "on_property");
    assert!(!record.2);
    assert!(record.3.contains("security.jwt"), "{}", record.3);
    // Without a decoder a bearer token is not accepted.
    assert_eq!(status(&web(&s), "/data", Some(&token())).await, 401);
}

struct RejectAll;

#[oxbloom_boot::async_trait]
impl JwtDecoder for RejectAll {
    async fn decode(&self, _token: &str) -> Result<Jwt, AuthenticationError> {
        Err(AuthenticationError::InvalidToken)
    }
}

#[tokio::test]
async fn user_decoder_wins() {
    let out = Captured::new();
    let s = snapshot(app(&jwt_section(), &out).configure(|b| {
        b.instance(RejectAll)
            .provides(|d: Arc<RejectAll>| -> Arc<dyn JwtDecoder> { d })
    }))
    .await;
    assert_eq!(s.decoders.len(), 1);
    assert!(!s.default_decoder, "the auto decoder is skipped");
    assert!(
        s.records
            .iter()
            .any(|(_, c, m, reason)| c == "on_missing_bean"
                && !*m
                && reason.contains("JwtDecoder")),
        "{:?}",
        s.records
    );
    // The chain uses the user's decoder, which rejects every token.
    assert_eq!(status(&web(&s), "/data", Some(&token())).await, 401);
}

#[tokio::test]
async fn default_chain_requires_authentication() {
    let out = Captured::new();
    let s = snapshot(app("", &out)).await;
    let app = web(&s);
    assert_eq!(status(&app, "/data", None).await, 401);
    assert_eq!(status(&app, "/open/x", None).await, 401);
}

/// An entry point that answers with a fixed status.
struct Status(u16);

impl AuthenticationEntryPoint for Status {
    fn commence(&self, _req: &Request, _error: Option<&AuthenticationError>) -> Response {
        let mut res = Response::new(Default::default());
        *res.status_mut() = http::StatusCode::from_u16(self.0).unwrap();
        res
    }
}

/// Opens `/open/**` and requires authentication elsewhere; its entry point answers 403.
struct OpenRules(Log);

impl SecurityCustomizer for OpenRules {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        self.0.push("rules");
        http.entry_point(Arc::new(Status(403))).authorize(|a| {
            a.path("/open/**")
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}

/// Replaces the entry point with one that answers 418.
struct Teapot(Log);

impl SecurityCustomizer for Teapot {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        self.0.push("teapot");
        http.entry_point(Arc::new(Status(418)))
    }
}

#[tokio::test]
async fn customizers_apply_in_order() {
    let out = Captured::new();
    let log = Log::new();
    let (first, second) = (log.clone(), log.clone());
    // Registered in reverse: `order` decides, not registration order.
    let s = snapshot(app("", &out).configure(move |b| {
        b.instance(Teapot(second))
            .provides(|c: Arc<Teapot>| -> Arc<dyn SecurityCustomizer> { c })
            .order(2)
            .instance(OpenRules(first))
            .provides(|c: Arc<OpenRules>| -> Arc<dyn SecurityCustomizer> { c })
            .order(1)
    }))
    .await;
    assert_eq!(log.entries(), ["rules", "teapot"]);
    let app = web(&s);
    // The first customizer's rules...
    assert_eq!(status(&app, "/open/x", None).await, 200);
    // ...and the second's entry point, which replaced the first's.
    assert_eq!(status(&app, "/data", None).await, 418);
}

#[tokio::test]
async fn user_chain_wins() {
    let out = Captured::new();
    let s = snapshot(app("", &out).configure(|b| {
        b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
            Ok(HttpSecurity::new()
                .authorize(|a| a.any_request().permit_all())
                .build()?)
        })
    }))
    .await;
    assert_eq!(s.chains.len(), 1);
    assert_eq!(status(&web(&s), "/data", None).await, 200);
    assert!(
        s.records
            .iter()
            .any(|(_, c, m, reason)| c == "on_missing_bean"
                && !*m
                && reason.contains("SecurityFilterChain")),
        "{:?}",
        s.records
    );
}

/// A customizer that never calls `authorize`.
struct NoRules;

impl SecurityCustomizer for NoRules {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http
    }
}

#[tokio::test]
async fn customizer_without_authorize_fails_the_build() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .configure(|b| {
                b.instance(NoRules)
                    .provides(|c: Arc<NoRules>| -> Arc<dyn SecurityCustomizer> { c })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    assert_eq!(
        err.find_cause::<SecurityBuildError>(),
        Some(&SecurityBuildError::NoAuthorizeSection)
    );
    assert!(
        err.to_string().contains(type_name::<SecurityFilterChain>()),
        "the failure names the bean: {err}"
    );
}

#[tokio::test]
async fn broken_jwt_section_fails_with_the_key() {
    let out = Captured::new();
    let err = within(app("[security.jwt]\nalgorithms = [\"HS256\"]\n", &out).run())
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    let cause = err.find_cause::<ConfigError>().expect("a typed source");
    assert!(matches!(cause, ConfigError::Invalid { key, .. } if key == "security.jwt.secret"));
    assert!(
        err.to_string().contains(type_name::<DefaultJwtDecoder>()),
        "the failure names the bean: {err}"
    );
    let analysis = err.analysis().expect("an analysis");
    assert!(
        analysis.description().contains("security.jwt.secret"),
        "{}",
        analysis.description()
    );
    assert!(out.contents().contains("security.jwt.secret"));
}

#[tokio::test]
async fn secret_on_the_command_line_fails_without_showing_it() {
    let marker = "cli-secret-marker-value-32-bytes!!";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_owned();
    let out = Captured::new();
    let err = within(
        OxbloomApplication::builder()
            .config(move |b| b.dir(path).env_from([]))
            .args([
                "--security.jwt.algorithms=HS256".to_owned(),
                format!("--security.jwt.secret={marker}"),
            ])
            .banner(Banner::off())
            .output(out.clone())
            .failure_report(true)
            .auto_configurations(auto_configurations())
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    let analysis = err.analysis().expect("an analysis");
    assert!(analysis.description().contains("security.jwt.secret"));
    assert!(
        analysis.description().contains("command line"),
        "{}",
        analysis.description()
    );
    for text in [
        err.to_string(),
        format!("{err:?}"),
        out.contents(),
        analysis.description().to_owned(),
        analysis.action().unwrap_or_default().to_owned(),
    ] {
        assert!(!text.contains(marker), "the secret leaked: {text}");
    }
    assert!(out.contents().contains("security.jwt.secret"));
}

/// The keys `jwt_decoder_from_config` reads, written down by hand.
const JWT_KEYS: [&str; 8] = [
    "security.jwt.algorithms",
    "security.jwt.audience",
    "security.jwt.clock-skew-secs",
    "security.jwt.issuer",
    "security.jwt.jwk-set-uri",
    "security.jwt.jwks-ssl-bundle",
    "security.jwt.public-key-pem",
    "security.jwt.secret",
];

/// A TOML line for `key` (the part after `security.jwt.`) with a sample value of its type.
fn sample(field: &str) -> String {
    let value = match field {
        "algorithms" => "[\"HS256\"]".to_owned(),
        "audience" => "[\"api\"]".to_owned(),
        "clock-skew-secs" => "5".to_owned(),
        "jwk-set-uri" => "\"https://issuer.example/jwks.json\"".to_owned(),
        "secret" => format!("\"{SECRET}\""),
        _ => "\"sample\"".to_owned(),
    };
    format!("{field} = {value}\n")
}

/// Whether `error` is the rejection of an unknown key in the section.
fn unknown_key(result: &Result<DefaultJwtDecoder, ConfigError>) -> bool {
    matches!(result, Err(ConfigError::Invalid { key, .. }) if key == "security.jwt")
}

#[test]
fn metadata_keys_match_config() {
    let props = JwtDecoderAutoConfiguration::properties();
    let keys: BTreeSet<&str> = props.iter().map(|p| p.key).collect();
    assert_eq!(keys, JWT_KEYS.into_iter().collect::<BTreeSet<_>>());
    assert_eq!(keys.len(), props.len(), "no duplicates");

    // Every declared key is one the section accepts.
    let mut toml = String::from("[security.jwt]\n");
    for p in props {
        let field = p
            .key
            .strip_prefix("security.jwt.")
            .expect("under security.jwt");
        toml.push_str(&sample(field));
    }
    let config = Config::from_str(&toml).unwrap();
    assert!(!unknown_key(&jwt_decoder_from_config(&config, None)));
    // The check is real: an undeclared key is rejected.
    let config = Config::from_str(&format!("{toml}bogus = 1\n")).unwrap();
    assert!(unknown_key(&jwt_decoder_from_config(&config, None)));

    // The secret is described, never given a value.
    let secret = props
        .iter()
        .find(|p| p.key == "security.jwt.secret")
        .unwrap();
    assert_eq!(secret.default, None);
    assert!(secret.description.contains("never printed"));
    assert!(SecurityAutoConfiguration::properties().is_empty());
}

#[test]
fn auto_configurations_lists_both_and_metadata_follows_exclusion() {
    // `web` adds `RequestLogAutoConfiguration` and `WebServerAutoConfiguration`, `ssl` adds
    // `SslAutoConfiguration`, `actuator` adds `ActuatorAutoConfiguration`, `tasks` adds
    // `TasksAutoConfiguration` and `RetryAutoConfiguration`.
    let web = 2 * usize::from(cfg!(feature = "web"));
    let ssl = usize::from(cfg!(feature = "ssl"));
    let actuator = usize::from(cfg!(feature = "actuator"));
    let tasks = 2 * usize::from(cfg!(feature = "tasks"));
    assert_eq!(
        auto_configurations().len(),
        2 + web + ssl + actuator + tasks
    );
    let with = OxbloomApplication::builder()
        .auto_configurations(auto_configurations())
        .property_metadata();
    assert!(with.iter().any(|p| p.key == "security.jwt.secret"));
    let without = OxbloomApplication::builder()
        .auto_configurations(auto_configurations())
        .exclude::<JwtDecoderAutoConfiguration>()
        .property_metadata();
    assert!(!without.iter().any(|p| p.key.starts_with("security.jwt")));
}

#[tokio::test]
async fn excluded_security_auto_configuration_registers_no_chain() {
    ContextRunner::new()
        .auto_configurations(auto_configurations())
        .exclude::<SecurityAutoConfiguration>()
        .run(|view| {
            assert!(matches!(
                view.get::<SecurityFilterChain>(),
                Err(DiError::Missing { .. })
            ));
        })
        .await;
}
