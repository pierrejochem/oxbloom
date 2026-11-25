//! The boot-level part of the migration conformance suite: the startup failure reports of the
//! `DataService` when the migrations of a source fail, run in process through `ContextRunner`
//! on the snippet of a `TestDatabase` (`config_snippet_with_migrations`).
//!
//! The macro `boot_migrate_conformance_tests!` checks, for every driver, that a failed script
//! stops the boot with a report that names the source and the version, that says what the
//! failure left behind on the driver in use, and that holds no SQL, path or URL. The module
//! `sqlite` adds the report of every other `MigrateError` that SQLite can produce at startup
//! and the ordering check. The reports of a failed script, a changed file, a dirty history, a
//! source without migrations and an unknown source are covered with SQLite files by
//! `oxbloom-boot/tests/autoconfigure_data_migrate.rs` and are not repeated here.
//!
//! As in `data_migrate_conformance.rs`, the module `sqlite` always runs and the modules
//! `postgres` and `mysql` (features `data-postgres` and `data-mysql`) are ignored by default and
//! run with `cargo test -- --ignored data_migrate_conformance` against the servers that
//! `make test-db` starts.

#![cfg(feature = "data")]

mod data_migrate_conformance_support;

/// Generates the boot-level tests that have the same body for every driver.
///
/// - `attrs`: extra attributes of every test, such as the `ignore` of a server driver.
/// - `open`: an async function that returns a fresh `TestDatabase`.
/// - `transactional`: whether a failed script is rolled back with its history row.
macro_rules! boot_migrate_conformance_tests {
    (
        attrs: [$(#[$attr:meta])*],
        open: $open:path,
        transactional: $transactional:literal $(,)?
    ) => {
        use oxbloom_boot::{DataAutoConfiguration, StartupPhase};
        use oxbloom_data::migrate::MigrateError;
        use oxbloom_test::{ContextRunner, MigrationFiles, TestDatabase};

        use super::data_migrate_conformance_support::{assert_free_of, url_secrets};

        /// The texts of a failed boot that must never hold a secret: the error, the migration
        /// error, the report and the captured output.
        fn failure_texts(view: &oxbloom_test::FailureView<'_>) -> Vec<(&'static str, String)> {
            vec![
                ("startup error", view.error().to_string()),
                (
                    "migration error",
                    format!("{:?}", view.error().find_cause::<MigrateError>()),
                ),
                (
                    "failure report",
                    view.error()
                        .analysis()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                ),
                ("captured output", view.output().contents()),
            ]
        }

        /// Asserts that the failure texts hold no SQL marker, no part of the migration
        /// directory, no file name of a failing script and no part of the URL. A bare `.sql` is
        /// not checked: the report of an empty directory shows the pattern of a file name.
        fn assert_no_secrets(
            view: &oxbloom_test::FailureView<'_>,
            db: &TestDatabase,
            files: &MigrationFiles,
        ) {
            let mut secrets = vec![
                "SECRET_MARKER_SQL".to_owned(),
                files.path().to_string_lossy().into_owned(),
                "2_break_things".to_owned(),
            ];
            secrets.extend(url_secrets(db.url()));
            let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
            assert_free_of(&failure_texts(view), &secrets);
        }

        /// A failing script stops the boot with a report that names the source and the
        /// version and says what the failure left behind. The next boot fails again: with the
        /// same script on Postgres and SQLite, which left no trace, and with the dirty history
        /// on MySQL, whose report says what a dirty history means there.
        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_a_failed_script_stops_the_boot_with_a_value_free_report() {
            let db = $open().await;
            let files = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
                .plain(
                    2,
                    "break things",
                    "CREATE TABLE partial_work (id INTEGER); \
                     SELECT 'SECRET_MARKER_SQL' FROM no_such_table;",
                );
            let config = db.config_snippet_with_migrations(files.path());
            ContextRunner::new()
                .config_str(config.clone())
                .auto_configuration::<DataAutoConfiguration>()
                .run_failing(|view| {
                    assert_no_secrets(view, &db, &files);
                    assert_eq!(view.error().phase(), StartupPhase::Service);
                    let error = view.error().find_cause::<MigrateError>().expect("a migrate error");
                    assert!(matches!(error, MigrateError::Script { version: 2, .. }), "{error:?}");
                    assert!(view
                        .error()
                        .to_string()
                        .contains("data source `default`: the migrations failed"));
                    let report = view.error().analysis().expect("a report").to_string();
                    assert!(report.contains("`default`"), "{report}");
                    assert!(report.contains("Migration 2 (break things)"), "{report}");
                    if $transactional {
                        assert!(report.contains("rolled back"), "{report}");
                    } else {
                        assert!(report.contains("dirty history"), "{report}");
                    }
                })
                .await;
            ContextRunner::new()
                .config_str(config)
                .auto_configuration::<DataAutoConfiguration>()
                .run_failing(|view| {
                    assert_no_secrets(view, &db, &files);
                    let error = view.error().find_cause::<MigrateError>().expect("a migrate error");
                    let report = view.error().analysis().expect("a report").to_string();
                    if $transactional {
                        assert!(
                            matches!(error, MigrateError::Script { version: 2, .. }),
                            "{error:?}"
                        );
                    } else {
                        assert!(
                            matches!(error, MigrateError::Dirty { version: 2, .. }),
                            "{error:?}"
                        );
                        assert!(report.contains("migration 2 (break things)"), "{report}");
                        assert!(report.contains("On MySQL"), "{report}");
                        assert!(report.contains("history table"), "{report}");
                    }
                })
                .await;
            db.close().await;
        }
    };
}

mod sqlite {
    use std::sync::{Arc, Mutex};

    use oxbloom_boot::{DataService, Service, ServiceError, async_trait};
    use oxbloom_data::sqlx::{self, SqlitePool};

    async fn open() -> TestDatabase {
        TestDatabase::sqlite()
    }

    boot_migrate_conformance_tests! {
        attrs: [],
        open: open,
        transactional: true,
    }

    /// The report of a failed boot, after the secret checks.
    fn report_of(view: &oxbloom_test::FailureView<'_>) -> String {
        view.error().analysis().expect("a report").to_string()
    }

    /// The migration error of a failed boot.
    fn error_of<'a>(view: &'a oxbloom_test::FailureView<'_>) -> &'a MigrateError {
        view.error()
            .find_cause::<MigrateError>()
            .expect("a migrate error")
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_the_report_of_a_missing_file_names_the_version_and_the_key() {
        let db = TestDatabase::sqlite();
        let both = MigrationFiles::new()
            .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
            .plain(2, "create orders", "CREATE TABLE orders (id INTEGER)");
        let first_only =
            MigrationFiles::new().plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)");
        ContextRunner::new()
            .config_str(db.config_snippet_with_migrations(both.path()))
            .auto_configuration::<DataAutoConfiguration>()
            .run(|_| {})
            .await;
        ContextRunner::new()
            .config_str(db.config_snippet_with_migrations(first_only.path()))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_no_secrets(view, &db, &first_only);
                let error = error_of(view);
                assert!(
                    matches!(error, MigrateError::MissingFile { version: 2, .. }),
                    "{error:?}"
                );
                let report = report_of(view);
                assert!(
                    report.contains("Migration 2 of the data source `default`"),
                    "{report}"
                );
                assert!(report.contains("its file is missing"), "{report}");
                assert!(
                    report.contains("`data.migrations.ignore-missing`"),
                    "{report}"
                );
            })
            .await;
        // With `ignore-missing` the same boot succeeds.
        ContextRunner::new()
            .config_str(format!(
                "{}ignore-missing = true\n",
                db.config_snippet_with_migrations(first_only.path())
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .run(|view| assert!(view.contains::<DataService>()))
            .await;
        db.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_the_report_of_a_missing_directory_names_the_key_not_the_path()
    {
        let db = TestDatabase::sqlite();
        let files = MigrationFiles::new();
        let missing = files.path().join("no-such-migrations");
        ContextRunner::new()
            .config_str(db.config_snippet_with_migrations(&missing))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_no_secrets(view, &db, &files);
                let error = error_of(view);
                assert!(
                    matches!(
                        error,
                        MigrateError::Load {
                            reason: oxbloom_data::migrate::LoadReason::Unreadable,
                            ..
                        }
                    ),
                    "{error:?}"
                );
                let report = report_of(view);
                assert!(report.contains("could not be read"), "{report}");
                assert!(report.contains("`data.migrations.path`"), "{report}");
                assert!(report.contains("The path is not shown"), "{report}");
            })
            .await;
        db.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_the_report_of_an_empty_directory_names_the_key() {
        let db = TestDatabase::sqlite();
        let files = MigrationFiles::new();
        ContextRunner::new()
            .config_str(db.config_snippet_with_migrations(files.path()))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_no_secrets(view, &db, &files);
                let error = error_of(view);
                assert!(
                    matches!(
                        error,
                        MigrateError::Load {
                            reason: oxbloom_data::migrate::LoadReason::Empty,
                            ..
                        }
                    ),
                    "{error:?}"
                );
                let report = report_of(view);
                assert!(report.contains("holds no migrations"), "{report}");
                assert!(report.contains("`data.migrations.path`"), "{report}");
            })
            .await;
        db.close().await;
    }

    /// A bad `table` is refused while the settings are read, before any service starts, and
    /// the configuration report names the key.
    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_a_bad_table_name_is_refused_naming_the_key() {
        let db = TestDatabase::sqlite();
        let files =
            MigrationFiles::new().plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)");
        ContextRunner::new()
            .config_str(format!(
                "{}table = \"history; DROP TABLE accounts\"\n",
                db.config_snippet_with_migrations(files.path())
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_no_secrets(view, &db, &files);
                assert!(view.error().find_cause::<MigrateError>().is_none());
                let report = report_of(view);
                assert!(report.contains("data.migrations.table"), "{report}");
                assert!(
                    !report.contains("DROP TABLE"),
                    "the bad value is in the report"
                );
            })
            .await;
        db.close().await;
    }

    /// The slow script keeps the SQLite worker thread busy after the boot gave up on it, so it
    /// is short enough not to slow the rest of the binary down.
    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_the_report_of_a_timeout_names_the_key() {
        let db = TestDatabase::sqlite();
        let files = MigrationFiles::new().plain(
            1,
            "slow",
            "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 3000000) \
             SELECT count(*) FROM n;",
        );
        ContextRunner::new()
            .config_str(format!(
                "{}timeout = \"50ms\"\n",
                db.config_snippet_with_migrations(files.path())
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_no_secrets(view, &db, &files);
                let error = error_of(view);
                assert!(matches!(error, MigrateError::Timeout { .. }), "{error:?}");
                let report = report_of(view);
                assert!(
                    report.contains("did not finish within the timeout"),
                    "{report}"
                );
                assert!(report.contains("`data.migrations.timeout`"), "{report}");
            })
            .await;
        db.close().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_the_report_of_a_private_memory_database_names_the_url_key() {
        let files =
            MigrationFiles::new().plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)");
        let url = "sqlite:file:oxbloom-private?mode=memory&cache=private";
        ContextRunner::new()
            .config_str(format!(
                "[data]\nurl = \"{url}\"\n[data.migrations]\nenabled = true\npath = {}\n",
                super::data_migrate_conformance_support::toml_str(&files.path().to_string_lossy())
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                let mut secrets = vec![files.path().to_string_lossy().into_owned()];
                secrets.extend(url_secrets(url));
                let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
                assert_free_of(&failure_texts(view), &secrets);
                let error = error_of(view);
                assert!(
                    matches!(error, MigrateError::PrivateMemory { .. }),
                    "{error:?}"
                );
                let report = report_of(view);
                assert!(report.contains("private cache"), "{report}");
                assert!(report.contains("`cache=shared`"), "{report}");
                assert!(report.contains("`data.url`"), "{report}");
            })
            .await;
    }

    /// A history table name that is taken by a table of the application: `sqlx` keeps the
    /// existing table and its reads fail, which is a database failure outside a script.
    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_the_report_of_a_database_failure_names_the_url_and_table_keys()
     {
        let db = TestDatabase::sqlite();
        sqlx::query("CREATE TABLE accounts (id INTEGER)")
            .execute(db.sqlite_pool())
            .await
            .unwrap();
        let files =
            MigrationFiles::new().plain(1, "create orders", "CREATE TABLE orders (id INTEGER)");
        ContextRunner::new()
            .config_str(format!(
                "{}table = \"accounts\"\n",
                db.config_snippet_with_migrations(files.path())
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .run_failing(|view| {
                assert_no_secrets(view, &db, &files);
                let error = error_of(view);
                assert!(matches!(error, MigrateError::Database { .. }), "{error:?}");
                let report = report_of(view);
                assert!(report.contains("The database failed"), "{report}");
                assert!(report.contains("`data.url`"), "{report}");
                assert!(report.contains("`data.migrations.table`"), "{report}");
                assert!(
                    report.contains("The database message is not shown"),
                    "{report}"
                );
            })
            .await;
        db.close().await;
    }

    /// A service that starts after the `DataService` and reads the migrated table in its
    /// `start`.
    struct ReadsAccounts(Arc<SqlitePool>, Arc<Mutex<Option<i64>>>);

    #[async_trait]
    impl Service for ReadsAccounts {
        async fn start(&self) -> Result<(), ServiceError> {
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
                .fetch_one(&*self.0)
                .await
                .map_err(ServiceError::new)?;
            *self.1.lock().unwrap() = Some(count);
            Ok(())
        }

        async fn stop(&self) {}

        fn name(&self) -> &'static str {
            "ReadsAccounts"
        }
    }

    /// The ordering check on the snippet of the test support: a service registered by the
    /// application starts after the `DataService` and sees the migrated schema and data.
    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_a_later_service_sees_the_new_schema() {
        let db = TestDatabase::sqlite();
        let files = MigrationFiles::new()
            .plain(
                1,
                "create accounts",
                "CREATE TABLE accounts (id INTEGER PRIMARY KEY)",
            )
            .plain(
                2,
                "seed accounts",
                "INSERT INTO accounts (id) VALUES (1), (2), (3)",
            );
        let seen = Arc::new(Mutex::new(None));
        let out = seen.clone();
        ContextRunner::new()
            .config_str(db.config_snippet_with_migrations(files.path()))
            .auto_configuration::<DataAutoConfiguration>()
            .configure(move |b| {
                b.bean(move |pool: Arc<SqlitePool>| ReadsAccounts(pool, out.clone()))
                    .provides(|s: Arc<ReadsAccounts>| -> Arc<dyn Service> { s })
            })
            .run(|view| assert!(view.contains::<DataService>()))
            .await;
        assert_eq!(*seen.lock().unwrap(), Some(3));
        db.close().await;
    }
}

#[cfg(feature = "data-postgres")]
mod postgres {
    async fn open() -> TestDatabase {
        TestDatabase::postgres().await
    }

    boot_migrate_conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]],
        open: open,
        transactional: true,
    }
}

#[cfg(feature = "data-mysql")]
mod mysql {
    async fn open() -> TestDatabase {
        TestDatabase::mysql().await
    }

    boot_migrate_conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]],
        open: open,
        transactional: false,
    }
}
