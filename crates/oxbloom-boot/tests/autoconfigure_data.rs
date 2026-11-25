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

/// The `migrations` keys under `data` belong to the default source, so a `migrations` table
/// without `data.url` is a default source without a URL: the boot fails naming `data.url`,
/// whether the table enables the migrations or not, and no `DataService` starts.
#[tokio::test]
async fn a_migrations_table_without_a_url_fails_naming_the_url() {
    for (toml, key) in [
        ("[data.migrations]\nenabled = false", "data.url"),
        (
            "[data]\nconnect-on-startup = false\n[data.migrations]\nenabled = false\ntable = \"history\"",
            "data.url",
        ),
        ("[data.migrations]\nenabled = true", "data.url"),
        (
            "[data.sources.orders.migrations]\nenabled = false",
            "data.sources.orders.url",
        ),
    ] {
        ContextRunner::new()
            .config_str(toml)
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_eq!(view.error().phase(), StartupPhase::Container, "{toml}");
                let config = view
                    .error()
                    .find_cause::<ConfigError>()
                    .expect("a config error");
                assert!(
                    matches!(config, ConfigError::Invalid { key: k, .. } if k == key),
                    "{toml}: {config:?}"
                );
                let report = view.error().analysis().expect("a report").to_string();
                assert!(report.contains(&format!("`{key}`")), "{report}");
            })
            .await;
    }
}

/// A source that enables migrations in a build without the feature `data-migrate` fails the
/// boot, naming the feature and the key; it is never silently left unmigrated.
#[cfg(all(feature = "data-sqlite", not(feature = "data-migrate")))]
#[tokio::test]
async fn enabled_without_the_feature_names_the_feature() {
    for (toml, key) in [
        (
            "[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true\npath = '/secret-dir'",
            "data.migrations.enabled",
        ),
        (
            "[data]\nconnect-on-startup = false\n[data.sources.orders]\nurl = \"sqlite::memory:\"\n\
             [data.sources.orders.migrations]\nenabled = true",
            "data.sources.orders.migrations.enabled",
        ),
    ] {
        ContextRunner::new()
            .config_str(toml)
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_eq!(view.error().phase(), StartupPhase::Container, "{toml}");
                let report = view.error().analysis().expect("a report").to_string();
                assert!(report.contains("`data-migrate`"), "{report}");
                assert!(report.contains(&format!("`{key}`")), "{report}");
                for text in failure_texts(view) {
                    assert!(!text.contains("secret-dir"), "{text}");
                    assert!(!text.contains("sqlite:"), "{text}");
                }
            })
            .await;
    }
    // `enabled = false` keeps working without the feature.
    ContextRunner::new()
        .config_str("[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = false")
        .auto_configuration::<DataAutoConfiguration>()
        .run(|view| assert!(view.contains::<oxbloom_boot::DataService>()))
        .await;
}

// The path without the `ssl` feature (a named bundle is an error on its `ssl-bundle` key) has
// no test here: a dev-dependency of this crate turns `ssl` on in every test build. The
// non-`ssl` `bundle_material` is compiled by `cargo build -p oxbloom-boot --no-default-features
// --features data-postgres`.

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
        // A shared-cache in-memory database, so a table written through one connection is
        // visible through any other connection of the same pool.
        ContextRunner::new()
            .config_str(
                "[data]\nurl = \"sqlite:file:oxbloom-boot-same-pool?mode=memory&cache=shared\"",
            )
            .auto_configuration::<DataAutoConfiguration>()
            .run(move |view| {
                let sources = view.get::<DataSources>().unwrap();
                let pool = view.get::<SqlitePool>().unwrap();
                assert_eq!(sources.names(), ["default"]);
                assert_eq!(sources.default_name(), Some("default"));
                assert!(pool.size() >= 1, "connect-on-startup connects");
                *out.lock().unwrap() = Some((sources, pool));
            })
            .await;
        let (sources, pool) = seen.lock().unwrap().take().unwrap();
        // The run is over and the pools are closed; the same pool is closed through both
        // handles.
        assert!(pool.is_closed());
        assert!(sources.sqlite("default").unwrap().is_closed());
    }

    /// The bean is the registry's pool: a row written through the bean is read back through
    /// `sources.sqlite("default")`.
    #[tokio::test]
    async fn the_pool_bean_is_the_registry_pool() {
        use oxbloom_boot::{ApplicationArguments, ApplicationRunner, RunnerError, async_trait};

        struct WriteRead(Arc<SqlitePool>, Arc<DataSources>, Slot<String>);

        #[async_trait]
        impl ApplicationRunner for WriteRead {
            async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
                use oxbloom_data::sqlx;
                sqlx::query("CREATE TABLE notes (body TEXT NOT NULL)")
                    .execute(&*self.0)
                    .await
                    .map_err(RunnerError::new)?;
                sqlx::query("INSERT INTO notes (body) VALUES ('through the bean')")
                    .execute(&*self.0)
                    .await
                    .map_err(RunnerError::new)?;
                let registry = self
                    .1
                    .sqlite("default")
                    .ok_or_else(|| RunnerError::new("none"))?;
                let body: String = sqlx::query_scalar("SELECT body FROM notes")
                    .fetch_one(registry)
                    .await
                    .map_err(RunnerError::new)?;
                *self.2.lock().unwrap() = Some(body);
                Ok(())
            }
        }

        let seen: Slot<String> = slot();
        let out = seen.clone();
        ContextRunner::new()
            .config_str(
                "[data]\nurl = \"sqlite:file:oxbloom-boot-bean-pool?mode=memory&cache=shared\"\n\
                 max-connections = 2",
            )
            .auto_configuration::<DataAutoConfiguration>()
            .configure(move |b| {
                b.bean(move |pool: Arc<SqlitePool>, sources: Arc<DataSources>| {
                    WriteRead(pool, sources, out.clone())
                })
                .provides(|r: Arc<WriteRead>| -> Arc<dyn ApplicationRunner> { r })
            })
            .run(|_| {})
            .await;
        assert_eq!(seen.lock().unwrap().as_deref(), Some("through the bean"));
    }

    /// The application registers its own `DataSources`: boot does not build another one, and
    /// the default pool bean, the service and the start-up check use the application's.
    #[tokio::test]
    async fn an_application_registry_wins() {
        use oxbloom_config::Config;
        use oxbloom_data::DataSettings;

        fn own_registry() -> DataSources {
            let config = Config::from_str(
                "[data]\nurl = \"sqlite::memory:\"\n[data.sources.own]\nurl = \"sqlite::memory:\"",
            )
            .unwrap();
            let settings = DataSettings::from_config(&config).unwrap();
            DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap()
        }

        let seen: Slot<Arc<DataSources>> = slot();
        let out = seen.clone();
        ContextRunner::new()
            .config_str("[data]\nurl = \"sqlite::memory:\"")
            .auto_configuration::<DataAutoConfiguration>()
            .configure(|b| b.bean(own_registry))
            .run(move |view| {
                let sources = view.get::<DataSources>().unwrap();
                assert_eq!(sources.names(), ["default", "own"]);
                let pool = view.get::<SqlitePool>().unwrap();
                // The start-up check pinged the application's sources, and the pool bean is
                // the application's default pool.
                assert!(pool.size() >= 1);
                assert!(sources.sqlite("own").unwrap().size() >= 1);
                let service = view.get::<oxbloom_boot::DataService>().unwrap();
                assert!(Arc::ptr_eq(service.sources(), &sources));
                let report = view.conditions().to_string();
                assert!(report.contains("on_missing_bean"), "{report}");
                *out.lock().unwrap() = Some(sources);
            })
            .await;
        // The service closed the application's pools at shutdown.
        let own = seen.lock().unwrap().take().unwrap();
        assert!(own.sqlite("own").unwrap().is_closed());
    }

    /// A run-to-completion application: a runner does the work and no service keeps running,
    /// so the run ends after the runner without a shutdown signal. The watchdog fails the test
    /// instead of letting a regression hang.
    #[tokio::test]
    async fn a_runner_only_application_ends_without_a_signal() {
        use oxbloom_boot::{
            ApplicationArguments, ApplicationRunner, Banner, OxbloomApplication, RunnerError,
            async_trait,
        };
        use oxbloom_config::Config;

        struct Work(Arc<SqlitePool>);

        #[async_trait]
        impl ApplicationRunner for Work {
            async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
                oxbloom_data::sqlx::query("SELECT 1")
                    .execute(&*self.0)
                    .await
                    .map_err(RunnerError::new)?;
                Ok(())
            }
        }

        for extra in ["", "connect-on-startup = false\n"] {
            let seen: Slot<Arc<SqlitePool>> = slot();
            let out = seen.clone();
            let run = OxbloomApplication::builder()
                .with_config(
                    Config::from_str(&format!("[data]\nurl = \"sqlite::memory:\"\n{extra}"))
                        .unwrap(),
                )
                .banner(Banner::off())
                .output(std::io::sink())
                .auto_configuration::<DataAutoConfiguration>()
                .configure(move |b| {
                    let out = out.clone();
                    b.bean(move |pool: Arc<SqlitePool>| {
                        *out.lock().unwrap() = Some(pool.clone());
                        Work(pool)
                    })
                    .provides(|r: Arc<Work>| -> Arc<dyn ApplicationRunner> { r })
                })
                .run();
            let status = tokio::time::timeout(std::time::Duration::from_secs(20), run)
                .await
                .expect("the run ends without a shutdown signal")
                .unwrap();
            assert_eq!(status.code(), 0);
            let pool = seen.lock().unwrap().take().unwrap();
            assert!(pool.is_closed(), "the data service still stopped: {extra}");
        }
    }

    /// A `data` section without a source registers no `DataService`.
    #[tokio::test]
    async fn a_data_section_without_sources_registers_no_service() {
        ContextRunner::new()
            .config_str("[data]\nconnect-on-startup = false")
            .auto_configuration::<DataAutoConfiguration>()
            .run(|view| {
                assert!(view.get::<DataSources>().unwrap().is_empty());
                assert!(!view.contains::<oxbloom_boot::DataService>());
                let report = view.conditions().to_string();
                assert!(
                    report.contains("the data section configures no source"),
                    "{report}"
                );
            })
            .await;
        // A named source alone registers it.
        ContextRunner::new()
            .config_str("[data.sources.cache]\nurl = \"sqlite::memory:\"")
            .auto_configuration::<DataAutoConfiguration>()
            .run(|view| assert!(view.contains::<oxbloom_boot::DataService>()))
            .await;
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
