//! `DataAutoConfiguration`: the `DataSources` registry, the default source's pool bean, the
//! connect and close service, and the failure reports of a broken `data` section, run in
//! process through `ContextRunner` with SQLite (and Postgres where the feature is on).

#![cfg(any(
    feature = "data-sqlite",
    feature = "data-postgres",
    feature = "data-mysql"
))]

use std::sync::{Arc, Mutex};

use oxbloom_boot::{AutoConfiguration, DataAutoConfiguration, StartupPhase};
use oxbloom_config::ConfigError;
use oxbloom_data::DataError;
use oxbloom_test::ContextRunner;

/// A path in a directory that does not exist, so SQLite cannot open the file.
#[cfg(feature = "data-sqlite")]
const MISSING_DIR_URL: &str = "sqlite:/no-such-oxbloom-data-dir/secret-file-name.db";

/// The texts of a failed run that must never contain a value: the error, its analysis and the
/// captured output.
fn failure_texts(view: &oxbloom_test::FailureView<'_>) -> Vec<String> {
    vec![
        view.error().to_string(),
        view.error()
            .analysis()
            .map(ToString::to_string)
            .unwrap_or_default(),
        view.output().contents(),
    ]
}

#[tokio::test]
async fn the_property_metadata_lists_every_key() {
    let keys: Vec<_> = DataAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .collect();
    let expected: Vec<_> = oxbloom_data::PROPERTIES.iter().map(|p| p.0).collect();
    assert_eq!(keys, expected);
    assert!(keys.contains(&"data.sources.<name>.ssl-bundle"));
}

#[tokio::test]
async fn a_bad_url_scheme_fails_the_boot_with_the_key_and_no_value() {
    ContextRunner::new()
        .config_str("[data]\nurl = \"redis://secret-host.internal/0\"")
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            assert_eq!(view.error().phase(), StartupPhase::Container);
            let config = view
                .error()
                .find_cause::<ConfigError>()
                .expect("a config error");
            assert!(
                matches!(config, ConfigError::Invalid { key, .. } if key == "data.url"),
                "{config:?}"
            );
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`data.url`"), "{report}");
            for text in failure_texts(view) {
                assert!(!text.contains("secret-host"), "{text}");
                assert!(!text.contains("redis"), "{text}");
            }
        })
        .await;
}

#[cfg(not(feature = "data-postgres"))]
#[tokio::test]
async fn a_postgres_source_without_the_feature_fails_naming_the_feature() {
    ContextRunner::new()
        .config_str("[data]\nurl = \"postgres://app:hunter2@db.internal/app\"")
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            assert_eq!(view.error().phase(), StartupPhase::Container);
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`default`"), "{report}");
            assert!(report.contains("data-postgres"), "{report}");
            assert!(report.contains("`data.url`"), "{report}");
            for text in failure_texts(view) {
                assert!(!text.contains("hunter2"), "{text}");
                assert!(!text.contains("db.internal"), "{text}");
            }
        })
        .await;
}

/// Without the `ssl` feature a named bundle is an error naming the key.
#[cfg(all(not(feature = "ssl"), feature = "data-postgres"))]
#[tokio::test]
async fn a_bundle_without_the_ssl_feature_fails_naming_the_key() {
    ContextRunner::new()
        .config_str("[data]\nurl = \"postgres://db.internal/app\"\nssl-bundle = \"db\"")
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            let config = view.error().find_cause::<ConfigError>().unwrap();
            assert!(
                matches!(config, ConfigError::Invalid { key, .. } if key == "data.ssl-bundle"),
                "{config:?}"
            );
        })
        .await;
}

#[cfg(feature = "data-sqlite")]
mod sqlite {
    use super::*;
    use oxbloom_data::DataSources;
    use oxbloom_data::sqlx::SqlitePool;

    type Slot<T> = Arc<Mutex<Option<T>>>;

    fn slot<T>() -> Slot<T> {
        Arc::new(Mutex::new(None))
    }

    #[tokio::test]
    async fn a_sqlite_default_source_registers_the_registry_and_the_pool_bean() {
        let seen: Slot<(Arc<DataSources>, Arc<SqlitePool>)> = slot();
        let out = seen.clone();
        ContextRunner::new()
            .config_str("[data]\nurl = \"sqlite::memory:\"")
            .auto_configuration::<DataAutoConfiguration>()
            .run(move |view| {
                let sources = view.get::<DataSources>().unwrap();
                let pool = view.get::<SqlitePool>().unwrap();
                assert_eq!(sources.names(), ["default"]);
                assert_eq!(sources.default_name(), Some("default"));
                // The start-up ping opened a connection of the registry's pool, which is the
                // same pool as the bean.
                assert_eq!(pool.size(), sources.sqlite("default").unwrap().size());
                assert!(pool.size() >= 1, "connect-on-startup connects");
                *out.lock().unwrap() = Some((sources, pool));
            })
            .await;
        assert!(seen.lock().unwrap().is_some());
    }

    /// The registry is built while the container is built, inside the application's runtime:
    /// a runner of the run uses the pool bean on that runtime.
    #[tokio::test]
    async fn the_pool_is_built_on_the_application_runtime() {
        use oxbloom_boot::{ApplicationArguments, ApplicationRunner, RunnerError, async_trait};

        struct UsePool(Arc<SqlitePool>, Slot<i64>);

        #[async_trait]
        impl ApplicationRunner for UsePool {
            async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
                let one: i64 = oxbloom_data::sqlx::query_scalar("SELECT 1")
                    .fetch_one(&*self.0)
                    .await
                    .map_err(RunnerError::new)?;
                *self.1.lock().unwrap() = Some(one);
                Ok(())
            }
        }

        let seen: Slot<i64> = slot();
        let out = seen.clone();
        ContextRunner::new()
            .config_str("[data]\nurl = \"sqlite::memory:\"\nconnect-on-startup = false")
            .auto_configuration::<DataAutoConfiguration>()
            .configure(move |b| {
                b.bean(move |pool: Arc<SqlitePool>| UsePool(pool, out.clone()))
                    .provides(|r: Arc<UsePool>| -> Arc<dyn ApplicationRunner> { r })
            })
            .run(|_| {})
            .await;
        assert_eq!(*seen.lock().unwrap(), Some(1));
    }

    #[tokio::test]
    async fn named_sources_are_in_the_registry_only() {
        ContextRunner::new()
            .config_str(
                "[data.sources.cache]\nurl = \"sqlite::memory:\"\n\
                 [data.sources.reports]\nurl = \"sqlite::memory:\"",
            )
            .auto_configuration::<DataAutoConfiguration>()
            .run(|view| {
                let sources = view.get::<DataSources>().unwrap();
                assert_eq!(sources.names(), ["cache", "reports"]);
                assert_eq!(sources.default_name(), None);
                assert!(sources.sqlite("cache").is_some());
                assert!(!view.contains::<SqlitePool>());
            })
            .await;
    }

    #[tokio::test]
    async fn an_application_pool_bean_wins() {
        let own = oxbloom_data::sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_lazy("sqlite::memory:")
            .unwrap();
        let own_bean = own.clone();
        ContextRunner::new()
            .config_str("[data]\nurl = \"sqlite::memory:\"")
            .auto_configuration::<DataAutoConfiguration>()
            .configure(move |b| b.instance(own_bean.clone()))
            .run(move |view| {
                let pool = view.get::<SqlitePool>().unwrap();
                let sources = view.get::<DataSources>().unwrap();
                // The application's pool was never used; the registry's pool connected.
                assert_eq!(pool.size(), 0);
                assert!(sources.sqlite("default").unwrap().size() >= 1);
                let report = view.conditions().to_string();
                assert!(report.contains("on_missing_bean"), "{report}");
            })
            .await;
        assert!(
            !own.is_closed(),
            "the application's pool is not closed by boot"
        );
    }

    #[tokio::test]
    async fn a_sqlite_source_with_ssl_mode_fails_with_the_key() {
        ContextRunner::new()
            .config_str("[data.sources.cache]\nurl = \"sqlite::memory:\"\nssl-mode = \"require\"")
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_eq!(view.error().phase(), StartupPhase::Container);
                let config = view.error().find_cause::<ConfigError>().unwrap();
                assert!(
                    matches!(config, ConfigError::Invalid { key, .. }
                        if key == "data.sources.cache.ssl-mode"),
                    "{config:?}"
                );
                let report = view.error().analysis().expect("a report").to_string();
                assert!(report.contains("`data.sources.cache.ssl-mode`"), "{report}");
            })
            .await;
    }

    #[tokio::test]
    async fn an_unreachable_sqlite_file_fails_the_boot_naming_the_source() {
        ContextRunner::new()
            .config_str(format!("[data]\nurl = \"{MISSING_DIR_URL}\""))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_eq!(view.error().phase(), StartupPhase::Service);
                assert_eq!(
                    view.error().find_cause::<DataError>(),
                    Some(&DataError::Connect)
                );
                let report = view.error().analysis().expect("a report").to_string();
                assert!(report.contains("`default`"), "{report}");
                assert!(report.contains("URL"), "{report}");
                for text in failure_texts(view) {
                    assert!(!text.contains("no-such-oxbloom-data-dir"), "{text}");
                    assert!(!text.contains("secret-file-name"), "{text}");
                    assert!(!text.contains("sqlite:"), "{text}");
                }
            })
            .await;
    }

    #[tokio::test]
    async fn connect_on_startup_false_boots_without_connecting() {
        ContextRunner::new()
            .config_str(format!(
                "[data]\nurl = \"{MISSING_DIR_URL}\"\nconnect-on-startup = false"
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .run(|view| {
                let sources = view.get::<DataSources>().unwrap();
                assert_eq!(sources.pool_counts("default"), Some((0, 0)));
            })
            .await;
    }

    #[tokio::test]
    async fn shutdown_closes_the_pools() {
        let seen: Slot<(Arc<SqlitePool>, Arc<DataSources>)> = slot();
        let out = seen.clone();
        ContextRunner::new()
            .config_str(
                "[data]\nurl = \"sqlite::memory:\"\n[data.sources.cache]\nurl = \"sqlite::memory:\"",
            )
            .auto_configuration::<DataAutoConfiguration>()
            .run(move |view| {
                let pool = view.get::<SqlitePool>().unwrap();
                assert!(!pool.is_closed());
                *out.lock().unwrap() = Some((pool, view.get::<DataSources>().unwrap()));
            })
            .await;
        let (pool, sources) = seen.lock().unwrap().take().unwrap();
        assert!(pool.is_closed());
        assert!(sources.sqlite("cache").unwrap().is_closed());
    }

    #[cfg(feature = "ssl")]
    #[tokio::test]
    async fn an_unknown_ssl_bundle_lists_the_bundles() {
        use oxbloom_boot::SslAutoConfiguration;

        let ca = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .unwrap()
            .cert
            .pem();
        let bundle = |name: &str| format!("[ssl.bundle.pem.{name}]\ntrust = '''\n{ca}'''\n");
        // A Postgres URL needs the driver feature to get as far as the bundle; without it the
        // bundle is still resolved first, so the error is the same.
        let toml = format!(
            "{}{}[data.sources.orders]\nurl = \"postgres://db.internal/app\"\nssl-bundle = \"nope\"\n",
            bundle("idp"),
            bundle("api"),
        );
        ContextRunner::new()
            .config_str(toml)
            .auto_configuration::<SslAutoConfiguration>()
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_eq!(view.error().phase(), StartupPhase::Container);
                let config = view.error().find_cause::<ConfigError>().unwrap();
                assert!(
                    matches!(config, ConfigError::Invalid { key, .. }
                        if key == "data.sources.orders.ssl-bundle"),
                    "{config:?}"
                );
                let report = view.error().analysis().expect("a report").to_string();
                assert!(
                    report.contains("data.sources.orders.ssl-bundle"),
                    "{report}"
                );
                assert!(report.contains("available bundles: api, idp"), "{report}");
                for text in failure_texts(view) {
                    assert!(!text.contains("BEGIN"), "{text}");
                    assert!(!text.contains("db.internal"), "{text}");
                }
            })
            .await;
    }
}

/// A Postgres server that refuses connections: sqlx retries until the acquire timeout, so the
/// check ends in `DataError::Pool`, and the report is the same as for a refused connection.
#[cfg(feature = "data-postgres")]
#[tokio::test]
async fn a_pool_timeout_at_startup_is_reported_as_unreachable() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    ContextRunner::new()
        .config_str(format!(
            "[data.sources.orders]\nurl = \"postgres://app@127.0.0.1:{port}/app\"\n\
             acquire-timeout = \"1s\""
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            assert_eq!(view.error().phase(), StartupPhase::Service);
            assert_eq!(
                view.error().find_cause::<DataError>(),
                Some(&DataError::Pool)
            );
            let report = view.error().analysis().expect("a report").to_string();
            assert!(
                report.contains("The data source `orders` is not reachable"),
                "{report}"
            );
            assert!(report.contains("`data.sources.orders.url`"), "{report}");
            assert!(report.contains("credentials"), "{report}");
            for text in failure_texts(view) {
                assert!(!text.contains("127.0.0.1"), "{text}");
            }
        })
        .await;
}

/// A Postgres URL with a password to a closed port: the boot fails at the start-up ping, and
/// neither the report nor the debug logs contain the password, the user or the URL.
#[cfg(feature = "data-postgres")]
#[tokio::test]
async fn no_password_in_the_report_or_logs() {
    use oxbloom_boot::{Banner, OxbloomApplication};
    use oxbloom_config::Config;
    use oxbloom_test::Captured;

    // A port that was free a moment ago: connecting is refused at once.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let password = "pw-that-must-never-leak";
    let mut toml = format!(
        "[data]\nurl = \"postgres://leaky-user:{password}@127.0.0.1:{port}/app\"\n\
         acquire-timeout = \"1s\"\n"
    );
    if cfg!(feature = "logging") {
        toml.push_str("[logging.console]\ncolor = \"never\"\n[logging.level]\nroot = \"debug\"\n");
    }
    let output = Captured::new();
    let logs = Captured::new();
    let builder = OxbloomApplication::builder()
        .with_config(Config::from_str(&toml).unwrap())
        .banner(Banner::off())
        .output(output.clone())
        .auto_configuration::<DataAutoConfiguration>();
    #[cfg(feature = "logging")]
    let builder = {
        let sink = logs.clone();
        builder.logging_writer(move || sink.clone())
    };
    let error = tokio::time::timeout(std::time::Duration::from_secs(20), builder.run())
        .await
        .expect("the run ends")
        .unwrap_err();
    assert_eq!(error.phase(), StartupPhase::Service);
    let report = error.analysis().expect("a report").to_string();
    assert!(report.contains("`default`"), "{report}");
    let texts = [
        report,
        error.to_string(),
        format!("{error:?}"),
        output.contents(),
        logs.contents(),
    ];
    // The subscriber is global, so the logs also hold the events of the other tests of this
    // binary (one of them is the fixed text `must start with `postgres://` ...`); the checks
    // look for this test's own URL parts only.
    let host = format!("127.0.0.1:{port}");
    for text in &texts {
        assert!(!text.contains(password), "{text}");
        assert!(!text.contains("leaky-user"), "{text}");
        assert!(!text.contains(&host), "{text}");
    }
    if cfg!(feature = "logging") {
        assert!(!logs.contents().is_empty(), "the debug logs were captured");
    }
}
