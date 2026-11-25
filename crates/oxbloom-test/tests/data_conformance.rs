//! The per-driver conformance suite of the data core: the same behaviour checked against
//! SQLite, Postgres and MySQL.
//!
//! The macro `conformance_tests!` generates the bodies that are the same for every driver; each
//! driver module hands it the way to open a `TestDatabase`, the typed pool, the accessor of
//! `DataSources` and the SQL that differs between the dialects. The tests whose setup differs
//! per driver (the transaction options, the induced serialization failure, the redaction of the
//! URL and the percent-encoded password) are written out in the driver modules under the same
//! names.
//!
//! The module `sqlite` always runs. The modules `postgres` and `mysql` need the features
//! `data-postgres` and `data-mysql`, are ignored by default, and run with
//! `cargo test -- --ignored data_conformance` against the servers in
//! `OXBLOOM_TEST_POSTGRES_URL` and `OXBLOOM_TEST_MYSQL_URL` (`make test-db` starts them). A
//! missing variable fails the test and names the variable.
//!
//! The suite lives in `oxbloom-test` rather than in `oxbloom-data`: as a dev-dependency of
//! `oxbloom-data`, `oxbloom-test` would turn the `sqlite` and `retry` features on in every test
//! build of `oxbloom-data`, so its builds without a driver would no longer be tested.

#![cfg(feature = "data")]

mod data_conformance_support;

/// Generates the tests that have the same body for every driver.
///
/// - `attrs`: extra attributes of every test, such as the `ignore` of a server driver.
/// - `open`: an async function that returns a fresh `TestDatabase`.
/// - `pool`: the method of `TestDatabase` that returns the typed pool.
/// - `typed`: the accessor of `DataSources` for this driver.
/// - `driver`: the driver name in the health details.
/// - `create_items`: creates the table `items` with an `id` and a unique `name`.
/// - `insert_item`: inserts one `name`, bound as the only parameter.
macro_rules! conformance_tests {
    (
        attrs: [$(#[$attr:meta])*],
        open: $open:path,
        pool: $pool:ident,
        typed: $typed:ident,
        driver: $driver:literal,
        create_items: $create_items:literal,
        insert_item: $insert_item:literal $(,)?
    ) => {
        use std::sync::Arc;
        use std::time::Duration;

        use oxbloom_actuator::{DataHealthIndicator, HealthIndicator, Status};
        use oxbloom_data::sqlx::{self, Acquire};
        use oxbloom_data::{DataError, TransactionError, TransactionOptions, transaction};
        use oxbloom_test::TestDatabase;

        use super::{assert_free_of, chain_texts};
        use super::data_conformance_support::{data_section, sources};

        /// A fresh database with the table `items`.
        async fn items_db() -> TestDatabase {
            let db = $open().await;
            sqlx::query($create_items).execute(db.$pool()).await.unwrap();
            db
        }

        async fn count(db: &TestDatabase) -> i64 {
            sqlx::query_scalar("SELECT COUNT(*) FROM items")
                .fetch_one(db.$pool())
                .await
                .unwrap()
        }

        /// The rolled-back row is gone and no lock of it is left: the same unique name can be
        /// inserted again at once on whichever connection the pool hands out.
        async fn assert_nothing_left(db: &TestDatabase) {
            assert_eq!(count(db).await, 0);
            let insert = sqlx::query($insert_item).bind("a").execute(db.$pool());
            tokio::time::timeout(Duration::from_secs(10), insert)
                .await
                .expect("no lock of the rolled-back transaction is left")
                .unwrap();
            sqlx::query("DELETE FROM items")
                .execute(db.$pool())
                .await
                .unwrap();
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_pool_settings_are_applied() {
            let db = $open().await;
            let registry = sources(&data_section(
                db.url(),
                "max-connections = 3\nmin-connections = 1\nacquire-timeout = \"7s\"\n\
                 idle-timeout = \"45s\"\nmax-lifetime = \"10m\"\ntest-before-acquire = false\n",
            ));
            let pool = registry.$typed("default").expect("the typed pool");
            let options = pool.options();
            assert_eq!(options.get_max_connections(), 3);
            assert_eq!(options.get_min_connections(), 1);
            assert_eq!(options.get_acquire_timeout(), Duration::from_secs(7));
            assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(45)));
            assert_eq!(options.get_max_lifetime(), Some(Duration::from_secs(600)));
            assert!(!options.get_test_before_acquire());
            // The pool really connects with these settings.
            sqlx::query("SELECT 1").execute(pool).await.unwrap();
            registry.close_all().await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_commit() {
            let db = items_db().await;
            transaction(db.$pool(), &TransactionOptions::default(), async |tx| {
                sqlx::query($insert_item).bind("a").execute(&mut **tx).await?;
                sqlx::query($insert_item).bind("b").execute(&mut **tx).await?;
                Ok::<_, sqlx::Error>(())
            })
            .await
            .unwrap();
            assert_eq!(count(&db).await, 2);
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_rollback_on_err() {
            #[derive(Debug, PartialEq)]
            struct OutOfStock;

            let db = items_db().await;
            let result: Result<(), _> =
                transaction(db.$pool(), &TransactionOptions::default(), async |tx| {
                    sqlx::query($insert_item)
                        .bind("a")
                        .execute(&mut **tx)
                        .await
                        .unwrap();
                    Err(OutOfStock)
                })
                .await;
            assert_eq!(result.unwrap_err().into_inner(), Some(OutOfStock));
            assert_nothing_left(&db).await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_rollback_on_panic() {
            let db = items_db().await;
            let pool = db.$pool().clone();
            let handle = tokio::spawn(async move {
                transaction(&pool, &TransactionOptions::default(), async |tx| {
                    sqlx::query($insert_item).bind("a").execute(&mut **tx).await?;
                    if std::hint::black_box(true) {
                        panic!("the work panics inside the transaction");
                    }
                    Ok::<_, sqlx::Error>(())
                })
                .await
            });
            assert!(handle.await.unwrap_err().is_panic());
            assert_nothing_left(&db).await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_rollback_on_dropped_future() {
            let db = items_db().await;
            let (inserted_tx, inserted_rx) = tokio::sync::oneshot::channel::<()>();
            let mut inserted_tx = Some(inserted_tx);
            let options = TransactionOptions::default();
            let work = transaction(db.$pool(), &options, async |tx| {
                sqlx::query($insert_item).bind("a").execute(&mut **tx).await?;
                if let Some(sender) = inserted_tx.take() {
                    let _ = sender.send(());
                }
                std::future::pending::<()>().await;
                Ok::<_, sqlx::Error>(())
            });
            // The future is dropped by `select!` as soon as the row is inserted.
            tokio::select! {
                _ = work => panic!("the work never ends on its own"),
                received = inserted_rx => received.unwrap(),
            }
            assert_nothing_left(&db).await;
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_nested_savepoint() {
            let db = items_db().await;
            transaction(db.$pool(), &TransactionOptions::default(), async |tx| {
                sqlx::query($insert_item).bind("outer").execute(&mut **tx).await?;
                let mut inner = tx.begin().await?;
                sqlx::query($insert_item).bind("inner").execute(&mut *inner).await?;
                inner.rollback().await?;
                sqlx::query($insert_item).bind("after").execute(&mut **tx).await?;
                Ok::<_, sqlx::Error>(())
            })
            .await
            .unwrap();
            let mut names: Vec<String> = sqlx::query_scalar("SELECT name FROM items")
                .fetch_all(db.$pool())
                .await
                .unwrap();
            names.sort();
            assert_eq!(names, ["after", "outer"]);
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_ping_and_close() {
            let db = $open().await;
            let registry = sources(&data_section(db.url(), ""));
            let timeout = Duration::from_secs(5);
            assert_eq!(registry.ping("default", timeout).await, Ok(()));
            assert_eq!(registry.ping("reports", timeout).await, Err(DataError::Config));
            assert_eq!(
                registry.ping_all(timeout).await,
                [("default".to_string(), Ok(()))]
            );
            let (size, idle) = registry.pool_counts("default").unwrap();
            assert!(size >= 1 && idle <= size, "size {size}, idle {idle}");

            registry.close_all().await;
            let pool = registry.$typed("default").unwrap();
            assert!(pool.is_closed());
            assert_eq!(registry.ping("default", timeout).await, Err(DataError::Pool));
            assert_eq!(
                registry.ping_all(timeout).await,
                [("default".to_string(), Err(DataError::Pool))]
            );
            let begin = transaction(pool, &TransactionOptions::default(), async |_tx| {
                Ok::<_, sqlx::Error>(())
            })
            .await;
            assert!(matches!(begin, Err(TransactionError::Begin)));
            db.close().await;
        }

        $(#[$attr])*
        #[tokio::test(flavor = "multi_thread")]
        async fn data_conformance_health_up_and_down() {
            let db = $open().await;
            let registry = Arc::new(sources(&data_section(db.url(), "")));
            let indicator = DataHealthIndicator::new(registry.clone(), "default");
            assert_eq!(indicator.name(), "db");

            let up = indicator.health().await;
            assert_eq!(up.status, Status::Up, "{up:?}");
            assert_eq!(up.details["driver"], $driver);
            let size = up.details["size"].as_u64().unwrap();
            let idle = up.details["idle"].as_u64().unwrap();
            assert!(size >= 1 && idle <= size, "{up:?}");

            registry.close_all().await;
            let down = indicator.health().await;
            assert_eq!(down.status, Status::Down, "{down:?}");
            assert_eq!(down.details["error"], "connection failed");
            assert_eq!(down.details["driver"], $driver);
            assert!(!down.details.contains_key("size"), "{down:?}");
            db.close().await;
        }
    };
}

/// The `Display` and `Debug` text of `error` and of every error in its
/// [`source`](std::error::Error::source) chain.
fn chain_texts(error: &(dyn std::error::Error + 'static)) -> Vec<String> {
    let mut texts = Vec::new();
    let mut next = Some(error);
    while let Some(error) = next {
        texts.push(format!("{error} {error:?}"));
        next = error.source();
    }
    texts
}

/// Asserts that none of `texts` contains any of `secrets`.
fn assert_free_of(texts: &[String], secrets: &[&str]) {
    for text in texts {
        for secret in secrets {
            assert!(!text.contains(secret), "`{secret}` leaked into: {text}");
        }
    }
}

mod sqlite {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use oxbloom_data::sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use oxbloom_data::{Isolation, transaction_with_retry};

    use super::data_conformance_support::{policy, settings, unique};

    async fn open() -> TestDatabase {
        TestDatabase::sqlite()
    }

    conformance_tests! {
        attrs: [],
        open: open,
        pool: sqlite_pool,
        typed: sqlite,
        driver: "sqlite",
        create_items: "CREATE TABLE items (id INTEGER PRIMARY KEY, name VARCHAR(64) NOT NULL UNIQUE)",
        insert_item: "INSERT INTO items (name) VALUES (?)",
    }

    /// A SQLite file in the temporary directory, removed with its journal files on drop.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("oxbloom-conformance-{}.db", unique())))
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            for suffix in ["", "-journal", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_isolation_and_read_only_options_or_their_refusal() {
        let db = items_db().await;
        // Serializable is what SQLite always does, so it is accepted.
        let serializable = TransactionOptions::default().isolation(Isolation::Serializable);
        transaction(db.sqlite_pool(), &serializable, async |tx| {
            sqlx::query("INSERT INTO items (name) VALUES ('a')")
                .execute(&mut **tx)
                .await?;
            Ok::<_, sqlx::Error>(())
        })
        .await
        .unwrap();
        assert_eq!(count(&db).await, 1);

        // The refusal comes before any connection is taken: even a closed pool answers with
        // `Unsupported`, not with `Begin`.
        let pool = db.sqlite_pool().clone();
        db.close().await;
        assert!(pool.is_closed());
        for options in [
            TransactionOptions::default().read_only(true),
            TransactionOptions::default().isolation(Isolation::ReadCommitted),
            TransactionOptions::default()
                .isolation(Isolation::Serializable)
                .read_only(true),
        ] {
            let mut ran = false;
            let result = transaction(&pool, &options, async |_tx| {
                ran = true;
                Ok::<_, sqlx::Error>(())
            })
            .await;
            assert!(
                matches!(result, Err(TransactionError::Unsupported)),
                "{options:?}"
            );
            assert!(!ran);
        }
    }

    /// `SQLITE_BUSY`: a second connection holds the write lock of a file database, and the
    /// work reads before it writes, so its write fails at once instead of waiting in the busy
    /// handler. The retry runs it again until the holder commits.
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_retry_on_an_induced_serialization_failure() {
        let file = TempFile::new();
        let url = format!("sqlite:{}?mode=rwc", file.0.display());
        let registry = sources(&data_section(&url, ""));
        let pool = registry.sqlite("default").unwrap();
        sqlx::query("CREATE TABLE counters (id INTEGER PRIMARY KEY, n BIGINT NOT NULL)")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO counters (id, n) VALUES (1, 0)")
            .execute(pool)
            .await
            .unwrap();

        let holder = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&file.0)
                    .busy_timeout(Duration::from_secs(10)),
            )
            .await
            .unwrap();
        let mut held = holder.begin().await.unwrap();
        sqlx::query("UPDATE counters SET n = n + 100 WHERE id = 1")
            .execute(&mut *held)
            .await
            .unwrap();

        let runs = AtomicU32::new(0);
        // The SQLite result code of the first failed write.
        let first_failure: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
        // The holder commits only once the first run has failed; no timing is assumed.
        let failed = tokio::sync::Notify::new();
        let options = TransactionOptions::default();
        let retry = policy(100, 20);
        let release = async {
            failed.notified().await;
            held.commit().await.unwrap();
        };
        let work = transaction_with_retry(pool, &options, &retry, async |tx| {
            runs.fetch_add(1, Ordering::SeqCst);
            let n: i64 = sqlx::query_scalar("SELECT n FROM counters WHERE id = 1")
                .fetch_one(&mut **tx)
                .await?;
            let updated = sqlx::query("UPDATE counters SET n = ? WHERE id = 1")
                .bind(n + 1)
                .execute(&mut **tx)
                .await;
            if let Err(error) = &updated {
                let mut first = first_failure.lock().unwrap();
                if first.is_none() {
                    *first = Some(
                        error
                            .as_database_error()
                            .and_then(|e| e.code())
                            .map(|code| code.into_owned())
                            .unwrap_or_default(),
                    );
                    failed.notify_one();
                }
            }
            updated.map(|_| ())
        });
        let ((), result) = tokio::time::timeout(Duration::from_secs(60), async {
            tokio::join!(release, work)
        })
        .await
        .expect("the retried work finishes");
        result.unwrap();
        assert!(runs.load(Ordering::SeqCst) >= 2, "the busy work ran again");
        // `SQLITE_BUSY` is 5; an extended code keeps it in its low byte.
        let code = first_failure
            .lock()
            .unwrap()
            .clone()
            .expect("a write failed");
        assert_eq!(code.parse::<i32>().map(|c| c & 0xff), Ok(5), "code {code}");
        let n: i64 = sqlx::query_scalar("SELECT n FROM counters WHERE id = 1")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(n, 101);
        holder.close().await;
        registry.close_all().await;
    }

    /// A path with a marker never shows in `Debug`, in an error (its source chain included) or
    /// in the health report. No logs are checked here: `oxbloom-data` logs nothing, and the
    /// boot tests check the logs of a failed start.
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_no_password_in_debug_or_errors() {
        let marker = "secret-marker-7f3a";
        let url = format!("sqlite:/no-such-oxbloom-dir-{marker}/{marker}.db");
        let toml = data_section(&url, "");
        let data_settings = settings(&toml);
        let registry = Arc::new(sources(&toml));
        let ping = registry
            .ping("default", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert_eq!(ping, DataError::Connect);
        let health = DataHealthIndicator::new(registry.clone(), "default")
            .health()
            .await;
        assert_eq!(health.status, Status::Down);
        assert_eq!(health.details["error"], "connection failed");

        // A URL that sqlx refuses gives a `SourceError`.
        let bad = settings(&data_section(&format!("{url}?mode=fast"), ""));
        let source_error =
            oxbloom_data::DataSources::build(&bad, &|_| Err(DataError::Tls)).unwrap_err();
        assert_eq!(source_error.error(), DataError::Config);

        let mut texts = vec![
            format!("{data_settings:?}"),
            format!("{registry:?}"),
            format!("{health:?}"),
        ];
        texts.extend(chain_texts(&ping));
        texts.extend(chain_texts(&source_error));
        assert_free_of(&texts, &[marker, "no-such-oxbloom-dir"]);
    }
}

#[cfg(feature = "data-postgres")]
mod postgres {
    use std::sync::atomic::{AtomicU32, Ordering};

    use oxbloom_data::sqlx::postgres::PgDatabaseError;
    use oxbloom_data::{Isolation, RetryableError, transaction_with_retry};
    use tokio::sync::Barrier;

    use super::data_conformance_support::{
        SPECIAL_PASSWORD, SPECIAL_PASSWORD_ENCODED, closed_port, create_postgres_role,
        drop_postgres_role, policy, settings, url_with_user,
    };

    async fn open() -> TestDatabase {
        TestDatabase::postgres().await
    }

    conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]],
        open: open,
        pool: postgres_pool,
        typed: postgres,
        driver: "postgres",
        create_items: "CREATE TABLE items (id BIGSERIAL PRIMARY KEY, name VARCHAR(64) NOT NULL UNIQUE)",
        insert_item: "INSERT INTO items (name) VALUES ($1)",
    }

    fn sqlstate(error: &sqlx::Error) -> Option<String> {
        match error {
            sqlx::Error::Database(db) => db
                .try_downcast_ref::<PgDatabaseError>()
                .map(|e| e.code().to_owned()),
            _ => None,
        }
    }

    #[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_isolation_and_read_only_options_or_their_refusal() {
        let db = items_db().await;
        let options = TransactionOptions::default()
            .isolation(Isolation::Serializable)
            .read_only(true);
        let (level, read_only) = transaction(db.postgres_pool(), &options, async |tx| {
            let level: String = sqlx::query_scalar("SHOW transaction_isolation")
                .fetch_one(&mut **tx)
                .await?;
            let read_only: String = sqlx::query_scalar("SHOW transaction_read_only")
                .fetch_one(&mut **tx)
                .await?;
            Ok::<_, sqlx::Error>((level, read_only))
        })
        .await
        .unwrap();
        assert_eq!((level.as_str(), read_only.as_str()), ("serializable", "on"));

        // A write in a read-only transaction fails with `read_only_sql_transaction`.
        let read_only = TransactionOptions::default().read_only(true);
        let result = transaction(db.postgres_pool(), &read_only, async |tx| {
            sqlx::query("INSERT INTO items (name) VALUES ('a')")
                .execute(&mut **tx)
                .await?;
            Ok::<_, sqlx::Error>(())
        })
        .await;
        let error = result.unwrap_err().into_inner().expect("the work's error");
        assert_eq!(sqlstate(&error).as_deref(), Some("25006"));
        assert_eq!(count(&db).await, 0);
        db.close().await;
    }

    /// Two serializable transactions read and then update the same row: the later update
    /// fails with `40001` and succeeds when it runs again.
    #[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_retry_on_an_induced_serialization_failure() {
        let db = TestDatabase::postgres().await;
        let pool = db.postgres_pool();
        sqlx::query("CREATE TABLE counters (id INTEGER PRIMARY KEY, n BIGINT NOT NULL)")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO counters (id, n) VALUES (1, 0)")
            .execute(pool)
            .await
            .unwrap();
        let options = TransactionOptions::default().isolation(Isolation::Serializable);
        let both_read = Barrier::new(2);
        let runs = AtomicU32::new(0);
        let failures = AtomicU32::new(0);
        let increment = || {
            let mut first = true;
            let (both_read, runs, failures) = (&both_read, &runs, &failures);
            async move |tx: &mut sqlx::Transaction<'_, sqlx::Postgres>| {
                runs.fetch_add(1, Ordering::SeqCst);
                let n: i64 = sqlx::query_scalar("SELECT n FROM counters WHERE id = 1")
                    .fetch_one(&mut **tx)
                    .await?;
                if std::mem::take(&mut first) {
                    both_read.wait().await;
                }
                let updated = sqlx::query("UPDATE counters SET n = $1 WHERE id = 1")
                    .bind(n + 1)
                    .execute(&mut **tx)
                    .await;
                if let Err(error) = &updated {
                    assert_eq!(sqlstate(error).as_deref(), Some("40001"));
                    assert!(error.is_retryable());
                    failures.fetch_add(1, Ordering::SeqCst);
                }
                updated.map(|_| ())
            }
        };
        let retry = policy(5, 20);
        let (a, b) = tokio::join!(
            transaction_with_retry(pool, &options, &retry, increment()),
            transaction_with_retry(pool, &options, &retry, increment()),
        );
        a.unwrap();
        b.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT n FROM counters WHERE id = 1")
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(n, 2);
        assert!(failures.load(Ordering::SeqCst) >= 1, "a 40001 was induced");
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2 + failures.load(Ordering::SeqCst)
        );
        db.close().await;
    }

    /// A URL with a password to a closed port: neither `Debug`, an error (its source chain
    /// included) nor the health report shows the password, the user or the host. No logs are
    /// checked here: `oxbloom-data` logs nothing, and the boot tests check the logs of a failed
    /// start.
    #[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_no_password_in_debug_or_errors() {
        let password = "pw-that-must-never-leak";
        let user = "leaky-user";
        let host = format!("127.0.0.1:{}", closed_port());
        let url = format!("postgres://{user}:{password}@{host}/app");
        let toml = data_section(&url, "acquire-timeout = \"1s\"\n");
        let data_settings = settings(&toml);
        let registry = Arc::new(sources(&toml));
        let ping = registry
            .ping("default", Duration::from_secs(5))
            .await
            .unwrap_err();
        let health = DataHealthIndicator::new(registry.clone(), "default")
            .health()
            .await;
        assert_eq!(health.status, Status::Down);
        assert_eq!(health.details["error"], "connection failed");

        let bad = settings(&data_section(&format!("{url}?sslmode=bogus"), ""));
        let source_error =
            oxbloom_data::DataSources::build(&bad, &|_| Err(DataError::Tls)).unwrap_err();
        assert_eq!(source_error.error(), DataError::Config);

        let mut texts = vec![
            format!("{data_settings:?}"),
            format!("{registry:?}"),
            format!("{health:?}"),
        ];
        texts.extend(chain_texts(&ping));
        texts.extend(chain_texts(&source_error));
        assert_free_of(&texts, &[password, user, &host]);
    }

    /// A role whose password needs percent-encoding connects through `data.url`.
    #[ignore = "needs OXBLOOM_TEST_POSTGRES_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_percent_encoded_password_connects() {
        let db = TestDatabase::postgres().await;
        let role = create_postgres_role(db.postgres_pool()).await;
        let url = url_with_user(db.url(), &role, SPECIAL_PASSWORD_ENCODED);
        // Every step that can panic runs in a task, so the role is dropped whatever happens.
        let outcome = tokio::spawn(async move {
            assert!(!url.contains(SPECIAL_PASSWORD));
            let registry = sources(&data_section(&url, ""));
            let one: Result<i64, _> = sqlx::query_scalar("SELECT 1::int8")
                .fetch_one(registry.postgres("default").unwrap())
                .await;
            let user: Result<String, _> = sqlx::query_scalar("SELECT current_user::text")
                .fetch_one(registry.postgres("default").unwrap())
                .await;
            registry.close_all().await;
            (one.map_err(|e| sqlstate(&e)), user.ok())
        })
        .await;
        drop_postgres_role(db.postgres_pool(), &role).await;
        db.close().await;
        let outcome = outcome.unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
        assert_eq!(outcome.0, Ok(1));
        assert_eq!(outcome.1.as_deref(), Some(role.as_str()));
    }
}

#[cfg(feature = "data-mysql")]
mod mysql {
    use std::sync::atomic::{AtomicU32, Ordering};

    use oxbloom_data::sqlx::mysql::MySqlDatabaseError;
    use oxbloom_data::{Isolation, RetryableError, transaction_with_retry};
    use tokio::sync::Barrier;

    use super::data_conformance_support::{
        SPECIAL_PASSWORD, SPECIAL_PASSWORD_ENCODED, closed_port, create_mysql_user,
        drop_mysql_user, policy, settings, url_with_user,
    };

    async fn open() -> TestDatabase {
        TestDatabase::mysql().await
    }

    conformance_tests! {
        attrs: [#[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]],
        open: open,
        pool: mysql_pool,
        typed: mysql,
        driver: "mysql",
        create_items: "CREATE TABLE items (id BIGINT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(64) NOT NULL UNIQUE) ENGINE=InnoDB",
        insert_item: "INSERT INTO items (name) VALUES (?)",
    }

    fn error_number(error: &sqlx::Error) -> Option<u16> {
        match error {
            sqlx::Error::Database(db) => db
                .try_downcast_ref::<MySqlDatabaseError>()
                .map(MySqlDatabaseError::number),
            _ => None,
        }
    }

    #[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_isolation_and_read_only_options_or_their_refusal() {
        let db = items_db().await;
        // InnoDB lists a transaction once it has touched a table; the isolation level of the
        // listed transaction is the one the begin statement set.
        let serializable = TransactionOptions::default().isolation(Isolation::Serializable);
        let level = transaction(db.mysql_pool(), &serializable, async |tx| {
            sqlx::query("INSERT INTO items (name) VALUES ('a')")
                .execute(&mut **tx)
                .await?;
            let level: String = sqlx::query_scalar(
                "SELECT trx_isolation_level FROM information_schema.innodb_trx \
                 WHERE trx_mysql_thread_id = CONNECTION_ID()",
            )
            .fetch_one(&mut **tx)
            .await?;
            Ok::<_, sqlx::Error>(level)
        })
        .await
        .unwrap();
        assert_eq!(level, "SERIALIZABLE");
        assert_eq!(count(&db).await, 1);

        // Serializable and read-only together begin; a read works and a write fails with
        // `ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION` (1792).
        let read_only = TransactionOptions::default()
            .isolation(Isolation::Serializable)
            .read_only(true);
        let result = transaction(db.mysql_pool(), &read_only, async |tx| {
            let seen: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
                .fetch_one(&mut **tx)
                .await?;
            assert_eq!(seen, 1);
            sqlx::query("INSERT INTO items (name) VALUES ('b')")
                .execute(&mut **tx)
                .await?;
            Ok::<_, sqlx::Error>(())
        })
        .await;
        let error = result.unwrap_err().into_inner().expect("the work's error");
        assert_eq!(error_number(&error), Some(1792));
        assert_eq!(count(&db).await, 1);
        db.close().await;
    }

    /// Two transactions lock two rows in opposite order: InnoDB picks one as the deadlock
    /// victim (`1213`), and the retry runs it again after the other commits.
    #[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_retry_on_an_induced_serialization_failure() {
        let db = TestDatabase::mysql().await;
        let pool = db.mysql_pool();
        sqlx::query(
            "CREATE TABLE counters (id INTEGER PRIMARY KEY, n BIGINT NOT NULL) ENGINE=InnoDB",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO counters (id, n) VALUES (1, 0), (2, 0)")
            .execute(pool)
            .await
            .unwrap();
        let options = TransactionOptions::default();
        let both_locked = Barrier::new(2);
        let runs = AtomicU32::new(0);
        let deadlocks = AtomicU32::new(0);
        let update = "UPDATE counters SET n = n + 1 WHERE id = ?";
        let lock_in_order = |first_id: i32, second_id: i32| {
            let mut first = true;
            let (both_locked, runs, deadlocks) = (&both_locked, &runs, &deadlocks);
            async move |tx: &mut sqlx::Transaction<'_, sqlx::MySql>| {
                runs.fetch_add(1, Ordering::SeqCst);
                sqlx::query(update)
                    .bind(first_id)
                    .execute(&mut **tx)
                    .await?;
                if std::mem::take(&mut first) {
                    both_locked.wait().await;
                }
                let second = sqlx::query(update).bind(second_id).execute(&mut **tx).await;
                if let Err(error) = &second {
                    assert_eq!(error_number(error), Some(1213));
                    assert!(error.is_retryable());
                    deadlocks.fetch_add(1, Ordering::SeqCst);
                }
                second.map(|_| ())
            }
        };
        let retry = policy(5, 20);
        let (a, b) = tokio::join!(
            transaction_with_retry(pool, &options, &retry, lock_in_order(1, 2)),
            transaction_with_retry(pool, &options, &retry, lock_in_order(2, 1)),
        );
        a.unwrap();
        b.unwrap();
        let rows: Vec<(i32, i64)> = sqlx::query_as("SELECT id, n FROM counters ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap();
        assert_eq!(rows, [(1, 2), (2, 2)]);
        assert!(
            deadlocks.load(Ordering::SeqCst) >= 1,
            "a deadlock was induced"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            2 + deadlocks.load(Ordering::SeqCst)
        );
        db.close().await;
    }

    /// A URL with a password to a closed port: neither `Debug`, an error (its source chain
    /// included) nor the health report shows the password, the user or the host. No logs are
    /// checked here: `oxbloom-data` logs nothing, and the boot tests check the logs of a failed
    /// start.
    #[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_no_password_in_debug_or_errors() {
        let password = "pw-that-must-never-leak";
        let user = "leaky-user";
        let host = format!("127.0.0.1:{}", closed_port());
        let url = format!("mysql://{user}:{password}@{host}/app");
        let toml = data_section(&url, "acquire-timeout = \"1s\"\n");
        let data_settings = settings(&toml);
        let registry = Arc::new(sources(&toml));
        let ping = registry
            .ping("default", Duration::from_secs(5))
            .await
            .unwrap_err();
        let health = DataHealthIndicator::new(registry.clone(), "default")
            .health()
            .await;
        assert_eq!(health.status, Status::Down);
        assert_eq!(health.details["error"], "connection failed");

        let bad = settings(&data_section(&format!("{url}?ssl-mode=bogus"), ""));
        let source_error =
            oxbloom_data::DataSources::build(&bad, &|_| Err(DataError::Tls)).unwrap_err();
        assert_eq!(source_error.error(), DataError::Config);

        let mut texts = vec![
            format!("{data_settings:?}"),
            format!("{registry:?}"),
            format!("{health:?}"),
        ];
        texts.extend(chain_texts(&ping));
        texts.extend(chain_texts(&source_error));
        assert_free_of(&texts, &[password, user, &host]);
    }

    /// A user whose password needs percent-encoding connects through `data.url`.
    #[ignore = "needs OXBLOOM_TEST_MYSQL_URL"]
    #[tokio::test(flavor = "multi_thread")]
    async fn data_conformance_percent_encoded_password_connects() {
        let db = TestDatabase::mysql().await;
        let user = create_mysql_user(db.mysql_pool()).await;
        let url = url_with_user(db.url(), &user, SPECIAL_PASSWORD_ENCODED);
        // Every step that can panic runs in a task, so the user is dropped whatever happens.
        let outcome = tokio::spawn(async move {
            assert!(!url.contains(SPECIAL_PASSWORD));
            let registry = sources(&data_section(&url, ""));
            let one: Result<i64, _> = sqlx::query_scalar("SELECT 1")
                .fetch_one(registry.mysql("default").unwrap())
                .await;
            let current: Result<String, _> = sqlx::query_scalar("SELECT CURRENT_USER()")
                .fetch_one(registry.mysql("default").unwrap())
                .await;
            registry.close_all().await;
            (one.map_err(|e| error_number(&e)), current.ok())
        })
        .await;
        drop_mysql_user(db.mysql_pool(), &user).await;
        db.close().await;
        let outcome = outcome.unwrap_or_else(|e| std::panic::resume_unwind(e.into_panic()));
        assert_eq!(outcome.0, Ok(1));
        assert_eq!(outcome.1, Some(format!("{user}@%")));
    }
}
