//! `TestDatabase`: isolated SQLite databases with one kept-open connection, the `[data]` snippet
//! for `ContextRunner`, and the environment variables of the server-backed databases.

#![cfg(feature = "data")]

use oxbloom_boot::DataAutoConfiguration;
use oxbloom_data::sqlx::{self, SqlitePool};
use oxbloom_test::{ContextRunner, TestDatabase};

#[tokio::test]
async fn sqlite_databases_are_isolated_from_each_other() {
    let first = TestDatabase::sqlite();
    let second = TestDatabase::sqlite();
    assert_ne!(first.url(), second.url());
    sqlx::query("CREATE TABLE t (id INTEGER)")
        .execute(first.sqlite_pool())
        .await
        .unwrap();
    // The table exists only in the first database.
    let missing = sqlx::query("SELECT id FROM t")
        .fetch_all(second.sqlite_pool())
        .await;
    assert!(missing.is_err());
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn all_pool_connections_see_the_same_in_memory_data() {
    let db = TestDatabase::sqlite();
    let pool = db.sqlite_pool();
    sqlx::query("CREATE TABLE t (id INTEGER)")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO t (id) VALUES (1), (2)")
        .execute(pool)
        .await
        .unwrap();
    // Two connections at the same time: one is held while the other reads.
    let mut held = pool.acquire().await.unwrap();
    let mut other = pool.acquire().await.unwrap();
    sqlx::query("INSERT INTO t (id) VALUES (3)")
        .execute(&mut *held)
        .await
        .unwrap();
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM t")
        .fetch_one(&mut *other)
        .await
        .unwrap();
    assert_eq!(count, 3);
    drop(held);
    drop(other);
    db.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn config_snippet_boots_a_context_with_a_pool_bean() {
    let db = TestDatabase::sqlite();
    sqlx::query("CREATE TABLE t (id INTEGER)")
        .execute(db.sqlite_pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO t (id) VALUES (7)")
        .execute(db.sqlite_pool())
        .await
        .unwrap();
    let snippet = db.config_snippet();
    assert!(snippet.starts_with("[data]\n"));
    assert!(!snippet.contains("sqlite_pool"));
    ContextRunner::new()
        .config_str(snippet)
        .auto_configuration::<DataAutoConfiguration>()
        .run(|view| {
            let pool = view.get::<SqlitePool>().expect("the pool bean");
            let handle = tokio::runtime::Handle::current();
            let (id,): (i64,) = tokio::task::block_in_place(|| {
                handle.block_on(sqlx::query_as("SELECT id FROM t").fetch_one(&*pool))
            })
            .unwrap();
            assert_eq!(id, 7);
        })
        .await;
    db.close().await;
}

#[tokio::test]
async fn close_makes_the_pool_unusable() {
    let db = TestDatabase::sqlite();
    let pool = db.sqlite_pool().clone();
    assert!(!pool.is_closed());
    db.close().await;
    assert!(pool.is_closed());
    assert!(pool.acquire().await.is_err());
}
