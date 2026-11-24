//! `transaction`: commit, rollback on an error, a panic and a dropped future, savepoints, the
//! options a driver cannot honor, the fixed error text and, with the feature `retry`,
//! `transaction_with_retry` and the retry of serialization failures and lock conflicts.
//!
//! The tests use SQLite file databases in the temporary directory, because a file database
//! survives the close of a pooled connection and can be opened by a second pool to provoke
//! `SQLITE_BUSY`. The Postgres and MySQL variants belong to the conformance suite.

#![cfg(feature = "sqlite")]

use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use oxbloom_data::sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use oxbloom_data::sqlx::{self, Acquire};
use oxbloom_data::{Isolation, TransactionError, TransactionOptions, transaction};

/// A business error of the work, a plain type that implements no trait of the crate.
#[derive(Debug, PartialEq)]
enum Rule {
    Violated,
    Database,
}

/// The SQLite file of one test. Dropping it removes the file and its `-journal`, `-wal` and
/// `-shm` siblings, so the tests leave nothing in the temporary directory.
struct TempDb(PathBuf);

impl TempDb {
    /// A unique path for the test `name`; an old file of the same name is removed first.
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("oxbloom-data-tx-{name}-{}.db", std::process::id()));
        let db = Self(path);
        db.remove();
        db
    }

    fn remove(&self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

impl Deref for TempDb {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        self.remove();
    }
}

/// A pool of at most `max` connections on the file `path`. The busy timeout is zero, so a
/// lock conflict fails at once with `SQLITE_BUSY` instead of waiting five seconds.
async fn file_pool(path: &PathBuf, max: u32) -> SqlitePool {
    file_pool_waiting(path, max, Duration::ZERO).await
}

/// As `file_pool`, but a lock conflict waits up to `busy_timeout` before it fails. The pools
/// that hold or release locks in the tests use it, so they never fail on a lock that the
/// connection under test holds for a moment.
async fn file_pool_waiting(path: &PathBuf, max: u32, busy_timeout: Duration) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .busy_timeout(busy_timeout);
    SqlitePoolOptions::new()
        .max_connections(max)
        .connect_with(options)
        .await
        .unwrap()
}

/// A one-connection pool on a fresh database with the table `items`.
async fn setup(name: &str) -> (SqlitePool, TempDb) {
    let path = TempDb::new(name);
    let pool = file_pool(&path, 1).await;
    sqlx::query("CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE)")
        .execute(&pool)
        .await
        .unwrap();
    (pool, path)
}

async fn count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM items")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Asserts that the next connection of `pool` is outside any transaction (a plain `BEGIN`
/// fails inside one) and that a second pool can write, so no lock is left behind.
async fn assert_no_leftover_transaction(pool: &SqlitePool, path: &PathBuf) {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
    drop(conn);

    let other = file_pool_waiting(path, 1, Duration::from_secs(5)).await;
    sqlx::query("INSERT INTO items (name) VALUES ('from another connection')")
        .execute(&other)
        .await
        .unwrap();
    sqlx::query("DELETE FROM items WHERE name = 'from another connection'")
        .execute(&other)
        .await
        .unwrap();
    other.close().await;
}

#[tokio::test]
async fn commits_on_ok() {
    let (pool, _path) = setup("commit").await;
    let id = transaction(&pool, &TransactionOptions::default(), async |tx| {
        let done = sqlx::query("INSERT INTO items (name) VALUES ('a')")
            .execute(&mut **tx)
            .await?;
        Ok::<_, sqlx::Error>(done.last_insert_rowid())
    })
    .await
    .unwrap();
    assert_eq!(id, 1);
    assert_eq!(count(&pool).await, 1);
}

#[tokio::test]
async fn rolls_back_on_err_and_returns_the_inner_error() {
    let (pool, path) = setup("rollback-err").await;
    let result: Result<(), _> = transaction(&pool, &TransactionOptions::default(), async |tx| {
        sqlx::query("INSERT INTO items (name) VALUES ('a')")
            .execute(&mut **tx)
            .await
            .map_err(|_| Rule::Database)?;
        Err(Rule::Violated)
    })
    .await;
    let err = result.unwrap_err();
    assert!(matches!(err, TransactionError::Inner(Rule::Violated)));
    assert_eq!(err.into_inner(), Some(Rule::Violated));
    assert_eq!(count(&pool).await, 0);
    assert_no_leftover_transaction(&pool, &path).await;
}

#[tokio::test]
async fn rolls_back_on_a_panic() {
    let (pool, path) = setup("rollback-panic").await;
    let task_pool = pool.clone();
    let handle = tokio::spawn(async move {
        transaction(&task_pool, &TransactionOptions::default(), async |tx| {
            sqlx::query("INSERT INTO items (name) VALUES ('a')")
                .execute(&mut **tx)
                .await?;
            let explode = std::hint::black_box(true);
            if explode {
                panic!("the work panics inside the transaction");
            }
            Ok::<_, sqlx::Error>(())
        })
        .await
    });
    let join = handle.await.unwrap_err();
    assert!(join.is_panic());
    assert_eq!(count(&pool).await, 0);
    assert_no_leftover_transaction(&pool, &path).await;
}

#[tokio::test]
async fn rolls_back_when_the_future_is_dropped() {
    let (pool, path) = setup("rollback-drop").await;
    let (inserted_tx, inserted_rx) = tokio::sync::oneshot::channel::<()>();
    let task_pool = pool.clone();
    let handle = tokio::spawn(async move {
        let mut inserted_tx = Some(inserted_tx);
        let name = String::from("a");
        // A spawned task needs a `Send` future: the closure owns what it uses (`async move`),
        // because a closure that borrows from the task is not `Send` (rust-lang/rust#110338).
        transaction(
            &task_pool,
            &TransactionOptions::default(),
            async move |tx| {
                sqlx::query("INSERT INTO items (name) VALUES (?)")
                    .bind(&name)
                    .execute(&mut **tx)
                    .await?;
                if let Some(sender) = inserted_tx.take() {
                    let _ = sender.send(());
                }
                std::future::pending::<()>().await;
                Ok::<_, sqlx::Error>(())
            },
        )
        .await
    });
    inserted_rx.await.unwrap();
    handle.abort();
    assert!(handle.await.unwrap_err().is_cancelled());
    assert_eq!(count(&pool).await, 0);
    assert_no_leftover_transaction(&pool, &path).await;
}

#[tokio::test]
async fn a_nested_begin_makes_a_savepoint() {
    let (pool, _path) = setup("savepoint").await;
    transaction(&pool, &TransactionOptions::default(), async |tx| {
        sqlx::query("INSERT INTO items (name) VALUES ('outer')")
            .execute(&mut **tx)
            .await?;
        let mut inner = tx.begin().await?;
        sqlx::query("INSERT INTO items (name) VALUES ('inner')")
            .execute(&mut *inner)
            .await?;
        inner.rollback().await?;
        Ok::<_, sqlx::Error>(())
    })
    .await
    .unwrap();
    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM items")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, ["outer"]);
}

#[tokio::test]
async fn the_next_acquire_sees_no_leftover_transaction() {
    let (pool, path) = setup("leftover").await;
    for _ in 0..3 {
        let _ = transaction(&pool, &TransactionOptions::default(), async |tx| {
            sqlx::query("INSERT INTO items (name) VALUES ('a')")
                .execute(&mut **tx)
                .await?;
            Err::<(), _>(sqlx::Error::RowNotFound)
        })
        .await;
        assert_no_leftover_transaction(&pool, &path).await;
    }
    transaction(&pool, &TransactionOptions::default(), async |tx| {
        sqlx::query("INSERT INTO items (name) VALUES ('kept')")
            .execute(&mut **tx)
            .await?;
        Ok::<_, sqlx::Error>(())
    })
    .await
    .unwrap();
    assert_no_leftover_transaction(&pool, &path).await;
    assert_eq!(count(&pool).await, 1);
}

#[tokio::test]
async fn read_only_is_refused_on_sqlite() {
    let (pool, _path) = setup("read-only").await;
    // A closed pool proves the refusal comes before any connection is acquired: an acquire
    // would fail with `Begin`.
    pool.close().await;
    let calls = AtomicU32::new(0);
    for options in [
        TransactionOptions::default().read_only(true),
        TransactionOptions::default().isolation(Isolation::ReadCommitted),
        TransactionOptions::default().isolation(Isolation::ReadUncommitted),
        TransactionOptions::default().isolation(Isolation::RepeatableRead),
        TransactionOptions::default()
            .isolation(Isolation::Serializable)
            .read_only(true),
    ] {
        let err = transaction(&pool, &options, async |_tx| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, sqlx::Error>(())
        })
        .await
        .unwrap_err();
        assert!(matches!(err, TransactionError::Unsupported), "{options:?}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn serializable_is_accepted_on_sqlite() {
    let (pool, _path) = setup("serializable").await;
    let options = TransactionOptions::default()
        .isolation(Isolation::Serializable)
        .read_only(false);
    transaction(&pool, &options, async |tx| {
        sqlx::query("INSERT INTO items (name) VALUES ('a')")
            .execute(&mut **tx)
            .await?;
        Ok::<_, sqlx::Error>(())
    })
    .await
    .unwrap();
    assert_eq!(count(&pool).await, 1);
}

#[tokio::test]
async fn a_failed_begin_is_reported_as_begin() {
    let (pool, _path) = setup("begin-fails").await;
    pool.close().await;
    let err = transaction(&pool, &TransactionOptions::default(), async |_tx| {
        Ok::<_, sqlx::Error>(())
    })
    .await
    .unwrap_err();
    assert!(matches!(err, TransactionError::Begin));
    assert_eq!(format!("{err:?}"), "Begin");
    assert_eq!(err.to_string(), "could not begin the database transaction");
    assert!(err.into_inner().is_none());
}

#[tokio::test]
async fn error_display_is_fixed_text() {
    let (pool, _path) = setup("display").await;
    let err = transaction(&pool, &TransactionOptions::default(), async |tx| {
        sqlx::query("SELECT hunter2_secret FROM items")
            .execute(&mut **tx)
            .await?;
        Ok::<_, sqlx::Error>(())
    })
    .await
    .unwrap_err();
    let shown = format!("{err} {err:?} {err:#?}");
    assert!(!shown.contains("hunter2_secret"), "{shown}");
    assert!(!shown.contains("SELECT"), "{shown}");
    assert_eq!(
        err.to_string(),
        "the work of the database transaction failed and it was rolled back"
    );
    assert_eq!(format!("{err:?}"), "Inner(..)");
    let source = std::error::Error::source(&err).unwrap().to_string();
    assert!(
        source.contains("hunter2_secret"),
        "the inner error is kept: {source}"
    );
    let inner = err.into_inner().unwrap();
    assert!(inner.to_string().contains("hunter2_secret"));

    for (err, text) in [
        (
            TransactionError::<sqlx::Error>::Commit,
            "could not commit the database transaction",
        ),
        (
            TransactionError::Unsupported,
            "the database driver does not support these transaction options",
        ),
    ] {
        assert_eq!(err.to_string(), text);
        assert!(std::error::Error::source(&err).is_none());
    }
    assert_eq!(
        format!("{:?}", TransactionError::<sqlx::Error>::Commit),
        "Commit"
    );
}

/// `transaction` has no bound on the work's error type in any configuration, so a plain
/// `&str` error compiles with and without the cargo feature `retry`.
#[tokio::test]
async fn transaction_accepts_any_error_type() {
    let (pool, _path) = setup("any-error").await;
    let err = transaction(&pool, &TransactionOptions::default(), async |tx| {
        sqlx::query("INSERT INTO items (name) VALUES ('a')")
            .execute(&mut **tx)
            .await
            .map_err(|_| "insert failed")?;
        Err::<(), &'static str>("not today")
    })
    .await
    .unwrap_err();
    assert_eq!(err.into_inner(), Some("not today"));
    let value = transaction(&pool, &TransactionOptions::default(), async |_tx| {
        Ok::<_, String>(5)
    })
    .await
    .unwrap();
    assert_eq!(value, 5);
    assert_eq!(count(&pool).await, 0);
}

#[cfg(feature = "retry")]
mod retry {
    use super::*;
    use oxbloom_data::{RetryableError, transaction_with_retry};
    use oxbloom_tasks::{Backoff, Jitter, RetryPolicy};

    fn policy(max_attempts: u32, delay_ms: u64) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            backoff: Backoff::Fixed,
            initial: Duration::from_millis(delay_ms),
            jitter: Jitter::None,
            ..RetryPolicy::default()
        }
    }

    /// A stub error whose classification the test chooses.
    #[derive(Debug, PartialEq)]
    struct Stub(bool);

    impl RetryableError for Stub {
        fn is_retryable(&self) -> bool {
            self.0
        }
    }

    /// Opens a write transaction on a second pool of the same file and inserts a row, so the
    /// file stays locked for writers until the returned transaction ends.
    async fn lock_for_writers(
        path: &PathBuf,
    ) -> (SqlitePool, sqlx::Transaction<'static, sqlx::Sqlite>) {
        // The holder waits for locks, so its commit never fails on the shared lock that a
        // failed attempt of the retrying writer holds until its rollback.
        let holder = file_pool_waiting(path, 1, Duration::from_secs(5)).await;
        let mut tx = holder.begin().await.unwrap();
        sqlx::query("INSERT INTO items (name) VALUES ('holder')")
            .execute(&mut *tx)
            .await
            .unwrap();
        (holder, tx)
    }

    #[tokio::test]
    async fn retry_runs_the_closure_again_only_for_retryable_errors() {
        let (pool, path) = setup("retry-stub").await;
        let options = TransactionOptions::default();
        let retry = policy(5, 1);

        // The stub classifier: retryable errors run again, until the work succeeds.
        let calls = AtomicU32::new(0);
        let value = transaction_with_retry(&pool, &options, &retry, async |_tx| {
            if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                Err(Stub(true))
            } else {
                Ok(7)
            }
        })
        .await
        .unwrap();
        assert_eq!((value, calls.load(Ordering::SeqCst)), (7, 3));

        // A non-retryable error runs once.
        let calls = AtomicU32::new(0);
        let err = transaction_with_retry(&pool, &options, &retry, async |_tx| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(Stub(false))
        })
        .await
        .unwrap_err();
        assert_eq!(err.into_inner(), Some(Stub(false)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // `transaction` itself never retries.
        let calls = AtomicU32::new(0);
        let _ = transaction(&pool, &TransactionOptions::default(), async |_tx| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(Stub(true))
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // An induced `SQLITE_BUSY`: the second writer retries until the first one commits.
        let (holder, held) = lock_for_writers(&path).await;
        let calls = AtomicU32::new(0);
        let release = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            held.commit().await.unwrap();
        };
        let retry = policy(50, 20);
        let work = transaction_with_retry(&pool, &options, &retry, {
            async |tx| {
                calls.fetch_add(1, Ordering::SeqCst);
                sqlx::query("INSERT INTO items (name) VALUES ('retried')")
                    .execute(&mut **tx)
                    .await?;
                Ok::<_, sqlx::Error>(())
            }
        });
        let ((), result) = tokio::join!(release, work);
        result.unwrap();
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "the busy writer was retried"
        );
        assert_eq!(count(&pool).await, 2);
        holder.close().await;
    }

    #[tokio::test]
    async fn retry_stops_after_max_attempts() {
        let (pool, path) = setup("retry-max").await;
        let (holder, held) = lock_for_writers(&path).await;
        let calls = AtomicU32::new(0);
        let options = TransactionOptions::default();
        let err = transaction_with_retry(&pool, &options, &policy(3, 5), {
            async |tx| {
                calls.fetch_add(1, Ordering::SeqCst);
                sqlx::query("INSERT INTO items (name) VALUES ('never')")
                    .execute(&mut **tx)
                    .await?;
                Ok::<_, sqlx::Error>(())
            }
        })
        .await
        .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(err.is_retryable(), "the last error is the busy error");
        let inner = err.into_inner().unwrap();
        assert!(inner.is_retryable());
        // A boxed error is classified through its source chain.
        let wrapped = Wrapped(inner);
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(Wrapped(wrapped.0));
        assert!(
            boxed.is_retryable(),
            "a wrapped sqlx error is classified too"
        );
        let plain: Box<dyn std::error::Error> = boxed;
        assert!(plain.is_retryable());
        let not_sqlx: Box<dyn std::error::Error> = Box::new(std::io::Error::other("busy"));
        assert!(!not_sqlx.is_retryable());
        held.rollback().await.unwrap();
        holder.close().await;
        assert_no_leftover_transaction(&pool, &path).await;
        assert_eq!(count(&pool).await, 0);
    }

    #[tokio::test]
    async fn a_constraint_violation_is_never_retried() {
        let (pool, _path) = setup("retry-constraint").await;
        sqlx::query("INSERT INTO items (name) VALUES ('taken')")
            .execute(&pool)
            .await
            .unwrap();
        let calls = AtomicU32::new(0);
        let options = TransactionOptions::default();
        let err = transaction_with_retry(&pool, &options, &policy(5, 1), {
            async |tx| {
                calls.fetch_add(1, Ordering::SeqCst);
                sqlx::query("INSERT INTO items (name) VALUES ('taken')")
                    .execute(&mut **tx)
                    .await?;
                Ok::<_, sqlx::Error>(())
            }
        })
        .await
        .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!err.is_retryable());
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(err.into_inner().unwrap());
        assert!(!boxed.is_retryable());
        assert!(!sqlx::Error::PoolTimedOut.is_retryable());
    }

    /// An application error that wraps a sqlx error and exposes it as its source.
    #[derive(Debug)]
    struct Wrapped(sqlx::Error);

    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the order could not be stored")
        }
    }

    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[tokio::test]
    async fn a_retryable_commit_failure_is_retried() {
        let (pool, path) = setup("retry-commit").await;
        // An open read transaction on another connection holds a shared lock, so the commit of
        // the writer, which needs an exclusive lock, fails with `SQLITE_BUSY`.
        let reader_pool = file_pool_waiting(&path, 1, Duration::from_secs(5)).await;
        let mut reader = reader_pool.acquire().await.unwrap();
        sqlx::query("BEGIN").execute(&mut *reader).await.unwrap();
        let seen: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items")
            .fetch_one(&mut *reader)
            .await
            .unwrap();
        assert_eq!(seen, 0);

        let calls = AtomicU32::new(0);
        let release = async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            sqlx::query("COMMIT").execute(&mut *reader).await.unwrap();
        };
        let options = TransactionOptions::default();
        let retry = policy(50, 20);
        let work = transaction_with_retry(&pool, &options, &retry, async |tx| {
            calls.fetch_add(1, Ordering::SeqCst);
            // The insert takes only the reserved lock, which the reader does not block.
            sqlx::query("INSERT INTO items (name) VALUES ('committed')")
                .execute(&mut **tx)
                .await?;
            Ok::<_, sqlx::Error>(())
        });
        let ((), result) = tokio::join!(release, work);
        result.unwrap();
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "the busy commit was retried"
        );
        assert_eq!(count(&pool).await, 1);
        drop(reader);
        reader_pool.close().await;
    }
}
