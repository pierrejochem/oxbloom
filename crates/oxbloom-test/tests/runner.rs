//! `ContextRunner` runs the real boot lifecycle, hands the check a view of the ready context,
//! and shuts the application down again.

use std::sync::{Arc, Mutex};

use oxbloom_boot::{StartupPhase, auto_configuration};
use oxbloom_config::Config;
use oxbloom_di::component;
use oxbloom_test::{ContextRunner, TempConfigDir};

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

#[tokio::test(flavor = "multi_thread")]
async fn run_hands_the_check_a_view_with_beans() {
    let seen = Arc::new(Mutex::new(None));
    let slot = seen.clone();
    ContextRunner::new()
        .configure(|b| b.instance(7u32))
        .run(move |view| {
            assert_eq!(*view.get::<u32>().unwrap(), 7);
            assert!(!view.contains::<String>());
            assert!(view.contains::<Config>());
            *slot.lock().unwrap() = Some(());
        })
        .await;
    assert!(seen.lock().unwrap().is_some(), "the check ran");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_property_override_reaches_the_config() {
    ContextRunner::new()
        .property("demo.name", "x")
        .run(|view| {
            assert_eq!(view.config().get::<String>("demo.name").unwrap(), "x");
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn config_str_and_profiles_load() {
    let dir = TempConfigDir::new().file("application-dev.toml", "[demo]\nname = \"dev\"\n");
    ContextRunner::new()
        .config_str("[demo]\nname = \"a\"\nother = \"base\"\n")
        .config_dir(dir.path())
        .profile("dev")
        .run(|view| {
            let config = view.config();
            assert_eq!(config.get::<String>("demo.name").unwrap(), "dev");
            assert_eq!(config.get::<String>("demo.other").unwrap(), "base");
            assert_eq!(config.active_profiles(), ["dev".to_string()]);
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn config_str_alone_loads() {
    ContextRunner::new()
        .config_str("[demo]\nname = \"a\"\n")
        .run(|view| {
            assert_eq!(view.config().get::<String>("demo.name").unwrap(), "a");
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "check failed marker")]
async fn a_failing_assertion_in_the_check_fails_the_test() {
    ContextRunner::new()
        .run(|_| panic!("check failed marker"))
        .await;
}

#[cfg(feature = "web")]
mod web {
    use super::*;
    use oxbloom_boot::WebServerAutoConfiguration;
    use oxbloom_web::{Controller, HttpMethod, Route};
    use std::time::Duration;

    struct Hello;

    impl Controller for Hello {
        fn routes(self: Arc<Self>) -> Vec<Route> {
            vec![Route::new(HttpMethod::Get, "/hello", |_| async { "hi" })]
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_panicking_check_with_a_running_server_still_shuts_down_and_rethrows() {
        let run = ContextRunner::new()
            .auto_configuration::<WebServerAutoConfiguration>()
            .configure(|b| {
                b.instance(Hello)
                    .provides(|c: Arc<Hello>| -> Arc<dyn Controller> { c })
            })
            .run(|view| {
                let server = view.get::<oxbloom_boot::WebServer>().unwrap();
                assert_ne!(server.local_addr().map(|a| a.port()), Some(8080));
                panic!("server check marker");
            });
        let joined = tokio::time::timeout(Duration::from_secs(12), tokio::spawn(run))
            .await
            .expect("the runner did not hang");
        let error = joined.expect_err("the check's panic is re-thrown");
        assert!(error.is_panic());
        assert_eq!(panic_text(error.into_panic()), "server check marker");
    }
}

/// Serializes the tests that change the process environment.
///
/// The runner does read the environment in places: `tempfile` reads `TMPDIR`, boot's output
/// reads `NO_COLOR`, and Tokio's signal setup reads it too. Changing the environment while
/// other tests run is still sound here, because the standard library serializes `set_var` and
/// `var` behind its own environment lock, and the configuration path ignores the environment
/// (`env_from([])`), so these variables change no other test's result. The mutex orders only
/// the tests that take it, not the other tests.
static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread")]
async fn the_process_environment_does_not_leak_into_the_configuration() {
    let guard = ENV.lock().await;
    // SAFETY: the variables are unique to this test, which holds the `ENV` lock while they
    // are set; other readers go through the standard library's environment lock (see `ENV`).
    unsafe {
        std::env::set_var("APP_OXBLOOM_TEST_LEAK", "1");
        std::env::set_var("APP_LEAKCHECK__SERVER__PORT", "1");
        std::env::set_var("OXBLOOM_PROFILE", "leakcheck");
    }
    let result = tokio::spawn(ContextRunner::new().run(|view| {
        let config = view.config();
        assert!(config.get::<String>("oxbloom_test_leak").is_err());
        assert!(config.get::<u16>("leakcheck.server.port").is_err());
        assert!(config.active_profiles().is_empty());
    }))
    .await;
    // SAFETY: as above.
    unsafe {
        std::env::remove_var("APP_OXBLOOM_TEST_LEAK");
        std::env::remove_var("APP_LEAKCHECK__SERVER__PORT");
        std::env::remove_var("OXBLOOM_PROFILE");
    }
    drop(guard);
    if let Err(error) = result {
        std::panic::resume_unwind(error.into_panic());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn run_failing_hands_over_the_error_and_the_report() {
    let seen = Arc::new(Mutex::new(false));
    ContextRunner::new()
        // `Config` is registered by boot, so registering it again fails the container.
        .configure(|b| b.instance(Config::from_str("").unwrap()))
        .run_failing(|failure| {
            assert_eq!(failure.error().phase(), StartupPhase::Container);
            assert!(failure.output().contains("APPLICATION FAILED TO START"));
            assert!(failure.output().contains("is registered more than once"));
            *seen.lock().unwrap() = true;
        })
        .await;
    assert!(*seen.lock().unwrap(), "the check ran");
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "startup succeeded but a failure was expected")]
async fn run_failing_panics_when_startup_succeeds() {
    ContextRunner::new().run_failing(|_| {}).await;
}

#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "startup failed")]
async fn run_panics_with_the_report_when_startup_fails() {
    ContextRunner::new()
        .configure(|b| b.instance(Config::from_str("").unwrap()))
        .run(|_| {})
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_startup_failure_panic_holds_the_captured_output() {
    let run = ContextRunner::new()
        .configure(|b| b.instance(Config::from_str("").unwrap()))
        .run(|_| {});
    let error = tokio::spawn(run).await.expect_err("startup failed");
    let text = panic_text(error.into_panic());
    assert!(text.starts_with("startup failed: "), "{text}");
    assert!(text.contains("APPLICATION FAILED TO START"), "{text}");
}

pub struct Greeter(&'static str);

#[component]
pub struct GreeterAutoConfig;

#[auto_configuration]
impl GreeterAutoConfig {
    #[bean]
    fn greeter(&self) -> Greeter {
        Greeter("default")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn exclude_removes_an_auto_configuration() {
    ContextRunner::new()
        .auto_configuration::<GreeterAutoConfig>()
        .run(|view| {
            assert_eq!(view.get::<Greeter>().unwrap().0, "default");
            let group = std::any::type_name::<GreeterAutoConfig>();
            assert!(
                view.conditions()
                    .records()
                    .iter()
                    .all(|r| r.registration() != group || r.matched()),
            );
        })
        .await;
    ContextRunner::new()
        .auto_configurations(oxbloom_boot::AutoConfigurations::new().add::<GreeterAutoConfig>())
        .exclude::<GreeterAutoConfig>()
        .run(|view| assert!(!view.contains::<Greeter>()))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_output_is_captured_and_the_banner_is_off() {
    ContextRunner::new()
        .run(|view| {
            assert!(
                !view.output().contains("Oxbloom"),
                "{}",
                view.output().contents()
            );
        })
        .await;
}

#[cfg(feature = "security")]
mod security {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use oxbloom_boot::JwtDecoderAutoConfiguration;
    use oxbloom_boot::StartupError;
    use oxbloom_security::{Algorithm, AuthenticationError, JwtDecoder};
    use oxbloom_test::ManualClock;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const SECRET: &str = "an-example-secret-of-32-bytes!!!";
    const EXP: u64 = 2_000_000_000;

    fn token() -> String {
        let claims = serde_json::json!({ "sub": "alice", "exp": EXP });
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    fn jwt_toml() -> String {
        format!(
            "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\nclock-skew-secs = 0\n"
        )
    }

    async fn decoder_at(time: SystemTime) -> Arc<dyn JwtDecoder> {
        let slot = Arc::new(Mutex::new(None));
        let seen = slot.clone();
        ContextRunner::new()
            .config_str(jwt_toml())
            .auto_configuration::<JwtDecoderAutoConfiguration>()
            .clock(Arc::new(ManualClock::at(time)))
            .run(move |view| {
                *seen.lock().unwrap() = Some(view.get::<dyn JwtDecoder>().unwrap());
            })
            .await;
        slot.lock().unwrap().take().unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_clock_override_reaches_jwt_validation() {
        let after = decoder_at(UNIX_EPOCH + Duration::from_secs(EXP + 60)).await;
        assert_eq!(
            after.decode(&token()).await.unwrap_err(),
            AuthenticationError::Expired
        );
        let before = decoder_at(UNIX_EPOCH + Duration::from_secs(EXP - 60)).await;
        assert!(before.decode(&token()).await.is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_secret_property_is_refused_like_on_the_command_line() {
        ContextRunner::new()
            .property("security.jwt.algorithms[0]", "HS256")
            .property("security.jwt.secret", SECRET)
            .auto_configuration::<JwtDecoderAutoConfiguration>()
            .run_failing(|failure| {
                let error: &StartupError = failure.error();
                assert!(error.to_string().contains("security.jwt.secret"), "{error}");
                assert!(!failure.output().contains(SECRET));
            })
            .await;
    }
}

/// A service that does nothing on start and misbehaves on stop.
struct BadStop {
    hang: bool,
}

#[oxbloom_boot::async_trait]
impl oxbloom_boot::Service for BadStop {
    async fn start(&self) -> Result<(), oxbloom_boot::ServiceError> {
        Ok(())
    }

    async fn stop(&self) {
        if self.hang {
            std::future::pending::<()>().await;
        }
        panic!("stop marker");
    }

    fn name(&self) -> &'static str {
        "BadStop"
    }
}

/// Runs a runner whose check passes and returns the text of the panic it raised.
async fn panic_of(runner: ContextRunner) -> String {
    let error = tokio::spawn(runner.run(|_| {}))
        .await
        .expect_err("the runner panicked");
    panic_text(error.into_panic())
}

fn bad_stop(hang: bool) -> ContextRunner {
    ContextRunner::new().configure(move |b| {
        b.instance(BadStop { hang })
            .provides(|s: Arc<BadStop>| -> Arc<dyn oxbloom_boot::Service> { s })
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_whose_stop_panics_fails_the_run() {
    let text = panic_of(bad_stop(false)).await;
    assert!(text.starts_with("shutdown failed"), "{text}");
    assert!(
        text.contains("service BadStop panicked while stopping"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_service_whose_stop_never_returns_fails_the_run_within_the_bound() {
    let began = std::time::Instant::now();
    let text = panic_of(bad_stop(true)).await;
    assert!(
        began.elapsed() < std::time::Duration::from_secs(8),
        "{:?}",
        began.elapsed()
    );
    assert!(text.starts_with("shutdown failed"), "{text}");
    assert!(
        text.contains("service BadStop did not stop in time"),
        "{text}"
    );
}

struct Fragile;

#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_pre_destroy_fails_the_run() {
    let runner = ContextRunner::new().configure(|b| {
        b.instance(Fragile)
            .pre_destroy(|_: &Fragile| panic!("pre_destroy marker"))
    });
    let text = panic_of(runner).await;
    assert!(text.starts_with("shutdown failed"), "{text}");
    assert!(text.contains("panicked during shutdown"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_panic_wins_over_a_shutdown_failure() {
    let error = tokio::spawn(bad_stop(false).run(|_| panic!("check first marker")))
        .await
        .expect_err("the runner panicked");
    assert_eq!(panic_text(error.into_panic()), "check first marker");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn config_str_with_a_config_dir_skips_symbolic_links() {
    let dir = TempConfigDir::new().file("application-dev.toml", "[demo]\nname = \"dev\"\n");
    // A link back to the directory itself would loop forever if links were followed.
    std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
    ContextRunner::new()
        .config_str("[demo]\nname = \"a\"\n")
        .config_dir(dir.path())
        .profile("dev")
        .run(|view| {
            assert_eq!(view.config().get::<String>("demo.name").unwrap(), "dev");
        })
        .await;
}

pub struct Feature;

#[tokio::test(flavor = "multi_thread")]
async fn a_conditional_bean_follows_its_property() {
    let wiring = |b: oxbloom_di::ContainerBuilder| b.instance(Feature).on_property("feature.on");
    ContextRunner::new()
        .property("feature.on", "true")
        .configure(wiring)
        .run(|view| {
            assert!(view.contains::<Feature>());
            let record = &view.conditions().records()[0];
            assert_eq!(record.condition(), "on_property");
            assert!(record.matched());
        })
        .await;
    ContextRunner::new()
        .configure(wiring)
        .run(|view| {
            assert!(!view.contains::<Feature>());
            let record = &view.conditions().records()[0];
            assert_eq!(record.condition(), "on_property");
            assert!(!record.matched());
            assert!(
                record.reason().contains("feature.on"),
                "{}",
                record.reason()
            );
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_port_property_wins_over_the_default() {
    ContextRunner::new()
        .run(|view| assert_eq!(view.config().get::<u16>("server.port").unwrap(), 0))
        .await;
    ContextRunner::new()
        .property("server.port", "8123")
        .run(|view| assert_eq!(view.config().get::<u16>("server.port").unwrap(), 8123))
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_logging_installation_is_off_by_property() {
    ContextRunner::new()
        .run(|view| {
            assert!(
                !view
                    .config()
                    .get::<bool>("oxbloom.logging.enabled")
                    .unwrap()
            )
        })
        .await;
}
