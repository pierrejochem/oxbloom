//! The per-driver conformance suite of the data migrations: `migrate::run` and
//! `migrate::revert` checked against SQLite, Postgres and MySQL.
//!
//! The macro `migrate_conformance_tests!` generates the bodies that are the same for every
//! driver; each driver module hands it the way to open a `TestDatabase`, its pool, the SQL that
//! differs between the dialects (a slow script, the lookup of a table) and whether a failed
//! script is rolled back as a whole. The one rule that differs per driver, what a failing
//! script leaves behind, is chosen inside the body by that flag: Postgres and SQLite run a
//! script in a transaction, MySQL commits each schema change at once.
//!
//! The module `sqlite` always runs. The modules `postgres` and `mysql` need the features
//! `data-postgres` and `data-mysql`, are ignored by default, and run with
//! `cargo test -- --ignored data_migrate_conformance` against the servers in
//! `OXBLOOM_TEST_POSTGRES_URL` and `OXBLOOM_TEST_MYSQL_URL` (`make test-db` starts them). A
//! missing variable fails the test and names the variable.
//!
//! No assertion message prints SQL, a path, a URL or a password: a leak is reported by the
//! number of the secret and the label of the text it leaked into.

#![cfg(feature = "data")]

mod data_conformance_support;
mod data_migrate_conformance_support;

/// Generates the tests that have the same body for every driver.
///
/// - `attrs`: extra attributes of every test, such as the `ignore` of a server driver.
/// - `open`: an async function that returns a fresh `TestDatabase`.
/// - `pool`: the method of `TestDatabase` that returns the typed pool.
/// - `transactional`: whether a failed script is rolled back with its history row.
/// - `slow_script`: a migration script that takes a few seconds, far longer than the timeout
///   of the timeout test.
/// - `table_count`: counts the tables of the database named by the only bound parameter.
/// - `unreachable_url`: a function that returns the URL of a source that cannot be reached,
///   with [`PASSWORD_MARKER`](data_migrate_conformance_support::PASSWORD_MARKER) in it.
/// - `concurrency_test`: attributes of the concurrency test; SQLite passes `#[cfg(any())]` and
///   writes its own version on a database file, since two instances cannot share an in-memory
///   database.
macro_rules! migrate_conformance_tests {
    (
        attrs: [$(#[$attr:meta])*],
        open: $open:path,
        pool: $pool:ident,
        transactional: $transactional:literal,
        slow_script: $slow_script:literal,
        table_count: $table_count:literal,
        unreachable_url: $unreachable_url:path,
        concurrency_test: [$(#[$concurrency:meta])*] $(,)?
    ) => {
        use std::collections::BTreeSet;
        use std::time::{Duration, Instant};

        use oxbloom_actuator::{DataHealthIndicator, HealthIndicator, Status};
        use oxbloom_data::migrate::MigrateError;
        use oxbloom_data::sqlx::{self, AssertSqlSafe};
        use oxbloom_data::sqlx::migrate::Migrator;
        use oxbloom_test::{MigrationFiles, TestDatabase};
        use tokio::sync::Barrier;

        use super::data_migrate_conformance_support::{
            MigratingSource, PASSWORD_MARKER, SQL_MARKER, assert_free_of, error_texts, logs,
            url_secrets, versions,
        };

        /// The longest a run may take that must not wait for a lock left behind by an earlier
        /// run. The runs themselves have the default timeout of 300 seconds, so a lock that is
        /// still held shows as a run that does not finish within this bound.
        const NO_WAIT: Duration = Duration::from_secs(10);

        async fn table_exists(db: &TestDatabase, table: &str) -> bool {
            let count: i64 = sqlx::query_scalar($table_count)
                .bind(table)
                .fetch_one(db.$pool())
                .await
                .unwrap();
            count == 1
        }

        /// The versions in the history table, finished or not, in order.
        async fn history(db: &TestDatabase, table: &str) -> Vec<i64> {
            // The table names of these tests are constants of `[a-z_]`.
            sqlx::query_scalar(AssertSqlSafe(format!(
                "SELECT version FROM {table} ORDER BY version"
            )))
            .fetch_all(db.$pool())
            .await
            .unwrap()
        }

        /// The versions of the history table that are recorded as not finished.
        async fn dirty_rows(db: &TestDatabase) -> Vec<i64> {
            sqlx::query_scalar(
                "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version",
            )
            .fetch_all(db.$pool())
            .await
            .unwrap()
        }

        /// Runs `source` and fails the test when the run waits longer than [`NO_WAIT`].
        async fn run_without_waiting(
            source: &MigratingSource,
        ) -> Result<oxbloom_data::migrate::MigrationReport, MigrateError> {
            tokio::time::timeout(NO_WAIT, source.run())
                .await
                .expect("the run waited for a lock that an earlier run left behind")
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_pending_migrations_apply_in_order_and_a_rerun_applies_nothing() {
            let db = $open().await;
            // Written out of order: the versions decide the order, not the files.
            let files = MigrationFiles::new()
                .plain(3, "create invoices", "CREATE TABLE invoices (id INTEGER)")
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER PRIMARY KEY)")
                .plain(2, "index accounts", "CREATE INDEX accounts_by_id ON accounts (id)");
            let source = MigratingSource::new(&db, files.path());

            let report = source.run().await.unwrap();
            assert_eq!(versions(&report), [1, 2, 3]);
            assert_eq!(
                report.to_string(),
                "applied 3 migrations: 1 (create accounts), 2 (index accounts), 3 (create invoices)"
            );
            assert!(table_exists(&db, "accounts").await);
            assert!(table_exists(&db, "invoices").await);
            assert_eq!(history(&db, "_sqlx_migrations").await, [1, 2, 3]);

            let again = source.run().await.unwrap();
            assert!(again.is_empty(), "{again:?}");
            assert_eq!(history(&db, "_sqlx_migrations").await, [1, 2, 3]);
            source.close().await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_a_changed_file_fails_naming_the_version() {
            let db = $open().await;
            let applied = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
                .plain(2, "create orders", "CREATE TABLE orders (id INTEGER)");
            let changed = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
                .plain(2, "create orders", "CREATE TABLE orders (id INTEGER, note INTEGER)");
            let first = MigratingSource::new(&db, applied.path());
            first.run().await.unwrap();

            let second = MigratingSource::new(&db, changed.path());
            let err = second.run().await.unwrap_err();
            assert!(
                matches!(
                    &err,
                    MigrateError::ChecksumMismatch { version: 2, description, .. }
                        if description == "create orders"
                ),
                "{err:?}"
            );
            assert_eq!(err.version(), Some(2));
            assert!(err.to_string().contains("migration 2 (create orders)"), "{err:?}");
            first.close().await;
            second.close().await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_a_missing_file_fails_unless_ignored() {
            let db = $open().await;
            let both = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
                .plain(2, "create orders", "CREATE TABLE orders (id INTEGER)");
            let first_only = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)");
            let full = MigratingSource::new(&db, both.path());
            full.run().await.unwrap();

            let strict = MigratingSource::new(&db, first_only.path());
            let err = strict.run().await.unwrap_err();
            assert!(matches!(err, MigrateError::MissingFile { version: 2, .. }), "{err:?}");
            assert!(err.to_string().contains("migration 2"), "{err:?}");

            let lenient = MigratingSource::new(&db, first_only.path())
                .with(|m| m.ignore_missing = true);
            let report = lenient.run().await.unwrap();
            assert!(report.is_empty(), "{report:?}");
            assert_eq!(history(&db, "_sqlx_migrations").await, [1, 2]);
            for source in [full, strict, lenient] {
                source.close().await;
            }
            db.close().await;
        }

        /// The failing script creates a table and then fails. Postgres and SQLite roll both the
        /// table and the history row back. MySQL commits the table at once, which also commits
        /// the history row that `sqlx` writes as unfinished first, so the history stays dirty
        /// and the next run fails naming the version. Either way the next run does not wait for
        /// a lock: the connection of the failed run was closed.
        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_a_failing_script_fails_naming_the_version() {
            let db = $open().await;
            let files = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
                .plain(
                    2,
                    "break things",
                    "CREATE TABLE partial_work (id INTEGER); SELECT missing FROM no_such_table;",
                );
            let source = MigratingSource::new(&db, files.path());

            let err = source.run().await.unwrap_err();
            assert!(
                matches!(
                    &err,
                    MigrateError::Script { version: 2, description, .. }
                        if description == "break things"
                ),
                "{err:?}"
            );
            assert!(err.to_string().contains("migration 2 (break things)"), "{err:?}");
            assert!(table_exists(&db, "accounts").await);

            let next = run_without_waiting(&source).await.unwrap_err();
            if $transactional {
                assert!(!table_exists(&db, "partial_work").await);
                assert_eq!(history(&db, "_sqlx_migrations").await, [1]);
                assert!(dirty_rows(&db).await.is_empty());
                // Nothing of the failed script is left, so it simply fails again.
                assert!(matches!(next, MigrateError::Script { version: 2, .. }), "{next:?}");
            } else {
                assert!(table_exists(&db, "partial_work").await);
                assert_eq!(dirty_rows(&db).await, [2]);
                assert!(
                    matches!(
                        &next,
                        MigrateError::Dirty { version: 2, description: Some(description), .. }
                            if description == "break things"
                    ),
                    "{next:?}"
                );
                assert!(next.to_string().contains("migration 2 (break things)"), "{next:?}");
            }
            source.close().await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_a_custom_table_is_used() {
            let db = $open().await;
            let files = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)");
            let source = MigratingSource::new(&db, files.path())
                .with(|m| m.table = "app_schema_history".to_owned());

            assert_eq!(versions(&source.run().await.unwrap()), [1]);
            assert_eq!(history(&db, "app_schema_history").await, [1]);
            assert!(!table_exists(&db, "_sqlx_migrations").await);
            assert!(source.run().await.unwrap().is_empty());
            source.close().await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_revert_goes_back_to_a_version() {
            let db = $open().await;
            let files = MigrationFiles::new()
                .reversible(1, "create accounts", "CREATE TABLE accounts (id INTEGER)", "DROP TABLE accounts")
                .reversible(2, "create orders", "CREATE TABLE orders (id INTEGER)", "DROP TABLE orders")
                .reversible(3, "create invoices", "CREATE TABLE invoices (id INTEGER)", "DROP TABLE invoices");
            let source = MigratingSource::new(&db, files.path());
            source.run().await.unwrap();

            let report = source.revert(1).await.unwrap();
            assert_eq!(versions(&report), [3, 2]);
            assert_eq!(
                report.to_string(),
                "reverted 2 migrations: 3 (create invoices), 2 (create orders)"
            );
            assert!(table_exists(&db, "accounts").await);
            assert!(!table_exists(&db, "orders").await);
            assert!(!table_exists(&db, "invoices").await);
            assert_eq!(history(&db, "_sqlx_migrations").await, [1]);
            assert!(source.revert(1).await.unwrap().is_empty());

            // A migration without a down file stops the revert before any script runs.
            let one_way = MigrationFiles::new()
                .reversible(1, "create accounts", "CREATE TABLE accounts (id INTEGER)", "DROP TABLE accounts")
                .up(2, "create orders", "CREATE TABLE orders (id INTEGER)");
            let one_way_source = MigratingSource::new(&db, one_way.path());
            assert_eq!(versions(&one_way_source.run().await.unwrap()), [2]);
            let err = one_way_source.revert(0).await.unwrap_err();
            assert!(matches!(err, MigrateError::NotReversible { version: 2, .. }), "{err:?}");
            assert!(err.to_string().contains("migration 2"), "{err:?}");
            assert_eq!(history(&db, "_sqlx_migrations").await, [1, 2]);
            assert!(table_exists(&db, "accounts").await);
            assert!(table_exists(&db, "orders").await);

            // A failing down script fails the revert naming its version, on every driver.
            let broken_down = MigrationFiles::new()
                .reversible(1, "create accounts", "CREATE TABLE accounts (id INTEGER)", "DROP TABLE accounts")
                .reversible(2, "create orders", "CREATE TABLE orders (id INTEGER)", "DROP TABLE no_such_table");
            let broken_source = MigratingSource::new(&db, broken_down.path());
            let err = broken_source.revert(0).await.unwrap_err();
            assert!(
                matches!(
                    &err,
                    MigrateError::Script { version: 2, description, .. }
                        if description == "create orders"
                ),
                "{err:?}"
            );
            assert!(table_exists(&db, "accounts").await);
            source.close().await;
            one_way_source.close().await;
            broken_source.close().await;
            db.close().await;
        }

        /// A slow script makes the run return `Timeout` long before the script would end, and
        /// the database becomes usable again afterwards: a following run eventually applies its
        /// migration, exactly once.
        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_the_timeout_cancels_a_slow_script() {
            let db = $open().await;
            let slow = MigrationFiles::new().plain(1, "slow", $slow_script);
            let source = MigratingSource::new(&db, slow.path())
                .with(|m| m.timeout = Duration::from_millis(300));

            let started = Instant::now();
            let err = source.run().await.unwrap_err();
            let took = started.elapsed();
            assert!(
                matches!(&err, MigrateError::Timeout { source_name } if source_name == "default"),
                "{err:?}"
            );
            assert!(took < Duration::from_secs(2), "the timed-out run took {took:?}");

            // The timeout stops the wait and closes the connection of the run, but it does not
            // cancel the statement on the server: Postgres and MySQL may keep it running with
            // its lock, and SQLite keeps it running on the worker thread, until the script ends.
            // So the following run is retried until the database is usable again; on SQLite it
            // can fail with `Database` meanwhile, and on the servers it can wait for the lock.
            let quick = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)");
            let next = MigratingSource::new(&db, quick.path())
                .with(|m| m.timeout = Duration::from_secs(5));
            let deadline = Instant::now() + Duration::from_secs(30);
            let report = loop {
                match next.run().await {
                    Ok(report) => break report,
                    Err(err) if Instant::now() < deadline => {
                        assert!(
                            matches!(err, MigrateError::Database { .. } | MigrateError::Timeout { .. }),
                            "{err:?}"
                        );
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                    Err(err) => panic!("the database did not become usable again: {err:?}"),
                }
            };
            assert_eq!(versions(&report), [1]);
            assert!(table_exists(&db, "accounts").await);
            assert_eq!(history(&db, "_sqlx_migrations").await, [1]);
            assert!(dirty_rows(&db).await.is_empty());
            assert!(next.run().await.unwrap().is_empty());
            source.close().await;
            next.close().await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_the_embedded_migrator_matches_the_directory() {
            let from_dir = $open().await;
            let embedded_db = $open().await;
            let files = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
                .reversible(2, "create orders", "CREATE TABLE orders (id INTEGER)", "DROP TABLE orders");
            let directory = MigratingSource::new(&from_dir, files.path());
            let dir_report = directory.run().await.unwrap();

            let migrator: &'static Migrator =
                Box::leak(Box::new(Migrator::new(files.path()).await.unwrap()));
            let embedded = MigratingSource::new(&embedded_db, files.path()).with(|m| m.path = None);
            let embedded_report = embedded.run_embedded(migrator).await.unwrap();

            assert_eq!(dir_report, embedded_report);
            async fn rows(db: &TestDatabase) -> Vec<(i64, String, Vec<u8>)> {
                sqlx::query_as(
                    "SELECT version, description, checksum FROM _sqlx_migrations ORDER BY version",
                )
                .fetch_all(db.$pool())
                .await
                .unwrap()
            }
            let dir_rows = rows(&from_dir).await;
            assert_eq!(dir_rows.len(), 2);
            assert!(dir_rows == rows(&embedded_db).await, "the history rows differ");
            assert!(table_exists(&embedded_db, "orders").await);
            directory.close().await;
            embedded.close().await;
            from_dir.close().await;
            embedded_db.close().await;
        }

        /// The health detail reads `MAX(version)` of the rows with `success = true` from the
        /// quoted history table: a finished row counts, an unfinished one does not.
        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_the_health_detail_shows_the_version() {
            let files = MigrationFiles::new()
                .plain(1, "create accounts", "CREATE TABLE accounts (id INTEGER)")
                .plain(20261125, "create orders", "CREATE TABLE orders (id INTEGER)");
            for table in ["_sqlx_migrations", "app_schema_history", "AppSchemaHistory"] {
                let db = $open().await;
                let source = MigratingSource::new(&db, files.path())
                    .with(|m| m.table = table.to_owned());
                source.run().await.unwrap();
                let indicator = DataHealthIndicator::new(source.sources().clone(), "default")
                    .with_migrations_table(table);
                let health = indicator.health().await;
                assert_eq!(health.status, Status::Up);
                assert_eq!(health.details["migrations"], 20261125, "table {table}");

                sqlx::query(AssertSqlSafe(format!(
                    "UPDATE {table} SET success = false WHERE version = 20261125"
                )))
                .execute(db.$pool())
                .await
                .unwrap();
                let health = indicator.health().await;
                assert_eq!(health.details["migrations"], 1, "table {table}");
                let report = format!("{health:?}");
                assert!(!report.contains("create orders"), "a description is in the health report");
                source.close().await;
                db.close().await;
            }
        }

        /// Two runners that start together on one database, each with a registry of its own as
        /// two application instances would have, both end without an error and apply every
        /// migration exactly once. Every migration appends its version to `applied_log`, so a
        /// double apply would show there even where the history could hide it.
        $(#[$concurrency])*
        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_two_concurrent_runs_apply_each_migration_once() {
            let files = MigrationFiles::new()
                .plain(
                    1,
                    "create log",
                    "CREATE TABLE applied_log (version INTEGER); \
                     INSERT INTO applied_log (version) VALUES (1);",
                )
                .plain(2, "log two", "INSERT INTO applied_log (version) VALUES (2);")
                .plain(3, "log three", "INSERT INTO applied_log (version) VALUES (3);");
            for round in 0..5 {
                let db = $open().await;
                let first = MigratingSource::new(&db, files.path());
                let second = MigratingSource::new(&db, files.path());
                let barrier = Barrier::new(2);
                let (a, b) = tokio::join!(
                    async {
                        barrier.wait().await;
                        first.run().await
                    },
                    async {
                        barrier.wait().await;
                        second.run().await
                    },
                );
                let a = versions(&a.unwrap_or_else(|e| panic!("round {round}: {e:?}")));
                let b = versions(&b.unwrap_or_else(|e| panic!("round {round}: {e:?}")));
                let log: Vec<i32> =
                    sqlx::query_scalar("SELECT version FROM applied_log ORDER BY version")
                        .fetch_all(db.$pool())
                        .await
                        .unwrap();
                assert_eq!(log, [1, 2, 3], "round {round}");
                assert_eq!(history(&db, "_sqlx_migrations").await, [1, 2, 3], "round {round}");
                assert!(dirty_rows(&db).await.is_empty(), "round {round}");
                let union: BTreeSet<i64> = a.iter().chain(&b).copied().collect();
                assert_eq!(union, BTreeSet::from([1, 2, 3]), "round {round}");
                // Each report names migrations of the directory only. The history before a run
                // is read before the lock of `sqlx`, so on Postgres and MySQL both reports may
                // name the same migration (the documented behavior of `MigrationReport`); on
                // SQLite the runner reads it inside its write lock.
                for report in [&a, &b] {
                    assert!(report.iter().all(|v| [1, 2, 3].contains(v)), "round {round}");
                }
                first.close().await;
                second.close().await;
                db.close().await;
            }
        }

        /// A script with the SQL marker, a migration directory and a database URL never show
        /// in the `Display` or `Debug` of an error, in a `tracing` event at `debug` or above
        /// (message and fields), or in the health report, during a run, a failed run, a revert
        /// and a failed down script. A source whose URL holds a password that cannot be reached
        /// fails without showing it.
        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_migrate_conformance_no_sql_path_or_password_appears_in_errors_logs_or_the_health_report() {
            let logs = logs();
            let db = $open().await;
            let files = MigrationFiles::new()
                .reversible(
                    1,
                    "create accounts",
                    "CREATE TABLE accounts (id INTEGER); SELECT 'SECRET_MARKER_SQL_up';",
                    "DROP TABLE accounts; SELECT 'SECRET_MARKER_SQL_down';",
                )
                .reversible(
                    2,
                    "broken",
                    "SELECT SECRET_MARKER_SQL FROM no_such_table_SECRET_MARKER_SQL;",
                    "DROP TABLE no_such_table_SECRET_MARKER_SQL;",
                );
            let source = MigratingSource::new(&db, files.path());
            let mut texts = Vec::new();

            let failed = source.run().await.unwrap_err();
            assert!(matches!(failed, MigrateError::Script { version: 2, .. }), "{failed:?}");
            texts.extend(error_texts(&failed));
            let reverted = source.revert(0).await.unwrap();
            assert_eq!(versions(&reverted), [1]);
            texts.push(("revert report", format!("{reverted} {reverted:?}")));

            // A failing down script: with an up script that works, migration 2 is applied, and
            // the revert runs its down script first, which fails.
            let fixed = MigrationFiles::new()
                .reversible(
                    1,
                    "create accounts",
                    "CREATE TABLE accounts (id INTEGER); SELECT 'SECRET_MARKER_SQL_up';",
                    "DROP TABLE accounts; SELECT 'SECRET_MARKER_SQL_down';",
                )
                .reversible(
                    2,
                    "broken",
                    "CREATE TABLE fine (id INTEGER);",
                    "DROP TABLE no_such_table_SECRET_MARKER_SQL;",
                );
            let fixed_source = MigratingSource::new(&db, fixed.path());
            let applied = fixed_source.run().await.unwrap();
            texts.push(("run report", format!("{applied} {applied:?}")));
            let failed_down = fixed_source.revert(0).await.unwrap_err();
            assert!(
                matches!(failed_down, MigrateError::Script { version: 2, .. }),
                "{failed_down:?}"
            );
            texts.extend(error_texts(&failed_down));

            let health = DataHealthIndicator::new(fixed_source.sources().clone(), "default")
                .with_migrations_table("_sqlx_migrations")
                .health()
                .await;
            assert_eq!(health.status, Status::Up);
            texts.push(("health report", format!("{health:?}")));

            // The unreachable source with a password.
            let url = $unreachable_url();
            let unreachable = MigratingSource::with_url(&url, fixed.path())
                .with(|m| m.timeout = Duration::from_secs(10));
            let refused = unreachable.run().await.unwrap_err();
            assert!(
                matches!(refused, MigrateError::Database { .. } | MigrateError::Timeout { .. }),
                "{refused:?}"
            );
            texts.extend(error_texts(&refused));
            let refused_revert = unreachable.revert(0).await.unwrap_err();
            texts.extend(error_texts(&refused_revert));
            let down = DataHealthIndicator::new(unreachable.sources().clone(), "default")
                .with_migrations_table("_sqlx_migrations")
                .health()
                .await;
            assert_eq!(down.status, Status::Down);
            texts.push(("health report of the unreachable source", format!("{down:?}")));

            // A statement on the pool of the test database, which logs its statements, proves
            // that the capture sees the `sqlx` events.
            sqlx::query("SELECT 1").execute(db.$pool()).await.unwrap();
            texts.push(("tracing output", logs.contents()));
            assert!(
                logs.contents().contains("sqlx"),
                "the capture saw no sqlx event, so this test proves nothing"
            );

            let dir = files.path().to_string_lossy().into_owned();
            let fixed_dir = fixed.path().to_string_lossy().into_owned();
            let mut secrets = vec![SQL_MARKER.to_owned(), PASSWORD_MARKER.to_owned(), dir, fixed_dir];
            secrets.extend(url_secrets(db.url()));
            secrets.extend(url_secrets(&url));
            let secrets: Vec<&str> = secrets.iter().map(String::as_str).collect();
            assert_free_of(&texts, &secrets);
            source.close().await;
            fixed_source.close().await;
            unreachable.close().await;
            db.close().await;
        }
    };
}

mod sqlite {

    async fn open() -> TestDatabase {
        TestDatabase::sqlite()
    }

    /// A file in a directory that does not exist; SQLite URLs have no password, so the marker
    /// is in the path.
    fn unreachable_url() -> String {
        format!("sqlite:/no-such-oxbloom-dir-{PASSWORD_MARKER}/{PASSWORD_MARKER}.db")
    }

    migrate_conformance_tests! {
        attrs: [],
        open: open,
        pool: sqlite_pool,
        transactional: true,
        slow_script: "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 3000000) \
                      SELECT count(*) FROM n;",
        table_count: "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        unreachable_url: unreachable_url,
        concurrency_test: [#[cfg(any())]],
    }

    /// The concurrency test on a SQLite file, which two application instances can share. The
    /// `sqlx` lock is a no-op on SQLite; the runner takes SQLite's write lock instead and runs
    /// once more after a failure, so both runners end without an error and every migration is
    /// applied exactly once. The reports are disjoint here, since the history before a run is
    /// read inside the write lock, but that is not part of the contract and is not asserted.
    #[tokio::test(flavor = "multi_thread")]
    async fn data_migrate_conformance_two_concurrent_runs_apply_each_migration_once() {
        use oxbloom_data::sqlx::SqlitePool;

        let files = MigrationFiles::new()
            .plain(
                1,
                "create log",
                "CREATE TABLE applied_log (version INTEGER); \
                 INSERT INTO applied_log (version) VALUES (1);",
            )
            .plain(
                2,
                "log two",
                "INSERT INTO applied_log (version) VALUES (2);",
            )
            .plain(
                3,
                "log three",
                "INSERT INTO applied_log (version) VALUES (3);",
            );
        for round in 0..10 {
            let scratch = tempfile::tempdir().unwrap();
            let url = format!(
                "sqlite:{}?mode=rwc",
                scratch.path().join("app.db").display()
            );
            let first = MigratingSource::with_url(&url, files.path());
            let second = MigratingSource::with_url(&url, files.path());
            let barrier = Barrier::new(2);
            let (a, b) = tokio::join!(
                async {
                    barrier.wait().await;
                    first.run().await
                },
                async {
                    barrier.wait().await;
                    second.run().await
                },
            );
            let a = versions(&a.unwrap_or_else(|e| panic!("round {round}: {e:?}")));
            let b = versions(&b.unwrap_or_else(|e| panic!("round {round}: {e:?}")));
            let pool = SqlitePool::connect(&url).await.unwrap();
            let log: Vec<i32> =
                sqlx::query_scalar("SELECT version FROM applied_log ORDER BY version")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(log, [1, 2, 3], "round {round}");
            let history: Vec<i64> =
                sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(history, [1, 2, 3], "round {round}");
            let union: BTreeSet<i64> = a.iter().chain(&b).copied().collect();
            assert_eq!(union, BTreeSet::from([1, 2, 3]), "round {round}");
            pool.close().await;
            first.close().await;
            second.close().await;
        }
    }
}

#[cfg(feature = "data-postgres")]
mod postgres {
    use super::data_conformance_support::closed_port;

    async fn open() -> TestDatabase {
        TestDatabase::postgres().await
    }

    fn unreachable_url() -> String {
        format!(
            "postgres://leaky-user:pw-{PASSWORD_MARKER}@127.0.0.1:{}/app",
            closed_port()
        )
    }

    migrate_conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]],
        open: open,
        pool: postgres_pool,
        transactional: true,
        slow_script: "SELECT pg_sleep(3);",
        table_count: "SELECT COUNT(*) FROM information_schema.tables \
                      WHERE table_schema = current_schema() AND table_name = $1",
        unreachable_url: unreachable_url,
        concurrency_test: [],
    }
}

#[cfg(feature = "data-mysql")]
mod mysql {
    use super::data_conformance_support::closed_port;

    async fn open() -> TestDatabase {
        TestDatabase::mysql().await
    }

    fn unreachable_url() -> String {
        format!(
            "mysql://leaky-user:pw-{PASSWORD_MARKER}@127.0.0.1:{}/app",
            closed_port()
        )
    }

    migrate_conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]],
        open: open,
        pool: mysql_pool,
        transactional: false,
        slow_script: "SELECT SLEEP(3);",
        table_count: "SELECT COUNT(*) FROM information_schema.tables \
                      WHERE table_schema = DATABASE() AND table_name = ?",
        unreachable_url: unreachable_url,
        concurrency_test: [],
    }
}
