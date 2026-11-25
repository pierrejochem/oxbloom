//! The boot-level part of the data conformance suite: the startup failure report of an
//! unreachable source, the graceful close of the pools when the application stops, and the
//! `db` component of the health endpoint, per driver.
//!
//! As in `data_conformance.rs`, the module `sqlite` always runs and the modules `postgres` and
//! `mysql` (features `data-postgres` and `data-mysql`) are ignored by default and run with
//! `cargo test -- --ignored data_conformance` against the servers that `make test-db` starts.
//! The Postgres and MySQL modules also boot a context whose `data.url` carries a
//! percent-encoded password.

#![cfg(all(feature = "data", feature = "web"))]

mod data_conformance_support;

/// Generates the boot-level tests that have the same body for every driver.
///
/// - `attrs`: extra attributes of every test, such as the `ignore` of a server driver.
/// - `open`: an async function that returns a fresh `TestDatabase`.
/// - `pool_type`: the type of the pool bean of the default source.
/// - `typed`: the accessor of `DataSources` for this driver.
/// - `driver`: the driver name in the health details.
macro_rules! boot_conformance_tests {
    (
        attrs: [$(#[$attr:meta])*],
        open: $open:path,
        pool_type: $pool_type:ty,
        typed: $typed:ident,
        driver: $driver:literal $(,)?
    ) => {
        use std::sync::{Arc, Mutex};

        use oxbloom_boot::{ActuatorAutoConfiguration, DataAutoConfiguration, StartupPhase};
        use oxbloom_data::{DataError, DataSources};
        use oxbloom_test::{ContextRunner, TestApp, TestDatabase};
        use oxbloom_web::{Controller, HttpMethod, Route};
        use serde_json::Value;

        /// The texts of a failed boot that must never contain a part of the URL.
        fn failure_texts(view: &oxbloom_test::FailureView<'_>) -> Vec<String> {
            vec![
                view.error().to_string(),
                format!("{:?}", view.error()),
                view.error()
                    .analysis()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                view.output().contents(),
            ]
        }

        struct Ping;

        impl Controller for Ping {
            fn routes(self: Arc<Self>) -> Vec<Route> {
                vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
            }
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_graceful_close() {
            let db = $open().await;
            let seen: Arc<Mutex<Option<(Arc<$pool_type>, Arc<DataSources>)>>> =
                Arc::default();
            let out = seen.clone();
            ContextRunner::new()
                .config_str(db.config_snippet())
                .auto_configuration::<DataAutoConfiguration>()
                .run(move |view| {
                    let pool = view.get::<$pool_type>().expect("the pool bean");
                    let sources = view.get::<DataSources>().expect("the registry");
                    assert!(!pool.is_closed());
                    // The start-up check connected the pool.
                    assert!(sources.pool_counts("default").unwrap().0 >= 1);
                    *out.lock().unwrap() = Some((pool, sources));
                })
                .await;
            let (pool, sources) = seen.lock().unwrap().take().unwrap();
            assert!(pool.is_closed());
            assert!(sources.$typed("default").unwrap().is_closed());
            assert_eq!(
                sources
                    .ping("default", std::time::Duration::from_secs(2))
                    .await,
                Err(DataError::Pool)
            );
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_health_endpoint_reports_db_up() {
            let db = $open().await;
            let app = TestApp::web()
                .config_str(format!(
                    "[management.health]\nshow-details = \"always\"\n{}",
                    db.config_snippet()
                ))
                .auto_configuration::<DataAutoConfiguration>()
                .auto_configuration::<ActuatorAutoConfiguration>()
                .configure(|b| {
                    b.instance(Ping)
                        .provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c })
                })
                .start()
                .await;
            let res = app.client().get("/actuator/health").send().await;
            let text = res.text();
            assert_eq!(res.status(), 200, "{text}");
            let body: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(body["status"], "UP", "{body}");
            let db_health = &body["components"]["db"];
            assert_eq!(db_health["status"], "UP", "{body}");
            assert_eq!(db_health["details"]["driver"], $driver);
            assert!(db_health["details"]["size"].as_u64().unwrap() >= 1, "{body}");
            assert!(!text.contains(db.url()), "{text}");
            app.stop().await;
            db.close().await;
        }
    };
}

mod sqlite {
    async fn open() -> TestDatabase {
        TestDatabase::sqlite()
    }

    boot_conformance_tests! {
        attrs: [],
        open: open,
        pool_type: oxbloom_data::sqlx::SqlitePool,
        typed: sqlite,
        driver: "sqlite",
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_startup_failure_report_for_an_unreachable_source() {
        let marker = "secret-marker-5c1d";
        ContextRunner::new()
            .config_str(format!(
                "[data.sources.orders]\nurl = \"sqlite:/no-such-oxbloom-dir-{marker}/{marker}.db\""
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_eq!(view.error().phase(), StartupPhase::Service);
                assert_eq!(
                    view.error().find_cause::<DataError>(),
                    Some(&DataError::Connect)
                );
                let report = view.error().analysis().expect("a report").to_string();
                assert!(report.contains("`orders`"), "{report}");
                assert!(report.contains("`data.sources.orders.url`"), "{report}");
                for text in failure_texts(view) {
                    assert!(!text.contains(marker), "{text}");
                    assert!(!text.contains("no-such-oxbloom-dir"), "{text}");
                }
            })
            .await;
    }
}

/// The boot-level tests of a server driver that are not in the macro: the report of a server
/// that refuses connections, and a booted context whose `data.url` has a percent-encoded
/// password.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
macro_rules! server_boot_tests {
    (
        ignore: $ignore:literal,
        scheme: $scheme:literal,
        pool_type: $pool_type:ty,
        create_user: $create_user:path,
        drop_user: $drop_user:path,
        pool: $pool:ident,
        current_user: $current_user:literal,
        user_suffix: $user_suffix:literal $(,)?
    ) => {
        use super::data_conformance_support::{
            SPECIAL_PASSWORD, SPECIAL_PASSWORD_ENCODED, closed_port, data_section, url_with_user,
        };

        #[ignore = $ignore]
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_startup_failure_report_for_an_unreachable_network_source() {
            let password = "pw-that-must-never-leak";
            let user = "leaky-user";
            let host = format!("127.0.0.1:{}", closed_port());
            let url = format!("{}://{user}:{password}@{host}/app", $scheme);
            ContextRunner::new()
                .config_str(format!(
                    "[data.sources.orders]\nurl = \"{url}\"\nacquire-timeout = \"1s\"\n"
                ))
                .auto_configuration::<DataAutoConfiguration>()
                .run_failing(|view| {
                    assert_eq!(view.error().phase(), StartupPhase::Service);
                    assert!(
                        matches!(
                            view.error().find_cause::<DataError>(),
                            Some(DataError::Pool | DataError::Connect)
                        ),
                        "{:?}",
                        view.error()
                    );
                    let report = view.error().analysis().expect("a report").to_string();
                    assert!(
                        report.contains("The data source `orders` is not reachable"),
                        "{report}"
                    );
                    assert!(report.contains("`data.sources.orders.url`"), "{report}");
                    for text in failure_texts(view) {
                        assert!(!text.contains(password), "{text}");
                        assert!(!text.contains(user), "{text}");
                        assert!(!text.contains(&host), "{text}");
                    }
                })
                .await;
        }

        #[ignore = $ignore]
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_percent_encoded_password_connects() {
            let db = open().await;
            let user = $create_user(db.$pool()).await;
            let url = url_with_user(db.url(), &user, SPECIAL_PASSWORD_ENCODED);
            assert!(!url.contains(SPECIAL_PASSWORD));
            // The check stores its outcome instead of panicking, so the user is dropped
            // whatever happens.
            let seen: Arc<Mutex<Option<Result<String, String>>>> = Arc::default();
            let out = seen.clone();
            ContextRunner::new()
                .config_str(data_section(&url, ""))
                .auto_configuration::<DataAutoConfiguration>()
                .run(move |view| {
                    let outcome = view
                        .get::<$pool_type>()
                        .map_err(|_| "no pool bean".to_string())
                        .and_then(|pool| {
                            let handle = tokio::runtime::Handle::current();
                            tokio::task::block_in_place(|| {
                                handle.block_on(
                                    oxbloom_data::sqlx::query_scalar::<_, String>($current_user)
                                        .fetch_one(&*pool),
                                )
                            })
                            .map_err(|_| "the query failed".to_string())
                        });
                    *out.lock().unwrap() = Some(outcome);
                })
                .await;
            $drop_user(db.$pool(), &user).await;
            db.close().await;
            let outcome = seen.lock().unwrap().take().expect("the context booted");
            let expected = format!("{user}{}", $user_suffix);
            assert_eq!(outcome, Ok(expected));
        }
    };
}

#[cfg(feature = "data-postgres")]
mod postgres {
    use super::data_conformance_support::{create_postgres_role, drop_postgres_role};

    async fn open() -> TestDatabase {
        TestDatabase::postgres().await
    }

    boot_conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]],
        open: open,
        pool_type: oxbloom_data::sqlx::PgPool,
        typed: postgres,
        driver: "postgres",
    }

    server_boot_tests! {
        ignore: "needs OXBLOOM_TEST_POSTGRES_URL",
        scheme: "postgres",
        pool_type: oxbloom_data::sqlx::PgPool,
        create_user: create_postgres_role,
        drop_user: drop_postgres_role,
        pool: postgres_pool,
        current_user: "SELECT current_user::text",
        user_suffix: "",
    }
}

#[cfg(feature = "data-mysql")]
mod mysql {
    use super::data_conformance_support::{create_mysql_user, drop_mysql_user};

    async fn open() -> TestDatabase {
        TestDatabase::mysql().await
    }

    boot_conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]],
        open: open,
        pool_type: oxbloom_data::sqlx::MySqlPool,
        typed: mysql,
        driver: "mysql",
    }

    server_boot_tests! {
        ignore: "needs OXBLOOM_TEST_MYSQL_URL",
        scheme: "mysql",
        pool_type: oxbloom_data::sqlx::MySqlPool,
        create_user: create_mysql_user,
        drop_user: drop_mysql_user,
        pool: mysql_pool,
        current_user: "SELECT CURRENT_USER()",
        user_suffix: "@%",
    }
}
