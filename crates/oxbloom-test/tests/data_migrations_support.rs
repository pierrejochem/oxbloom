//! `TestDatabase::with_migrations` and `MigrationFiles` against SQLite.

#![cfg(feature = "data")]

use oxbloom_data::sqlx;
use oxbloom_test::{MigrationFiles, TestDatabase};

#[tokio::test]
async fn with_migrations_applies_to_a_sqlite_database() {
    let files = MigrationFiles::new()
        .up(
            1,
            "create accounts",
            "CREATE TABLE accounts (id INTEGER PRIMARY KEY)",
        )
        .reversible(
            2,
            "create notes",
            "CREATE TABLE notes (body TEXT)",
            "DROP TABLE notes",
        );
    let db = TestDatabase::sqlite()
        .with_migrations(files.path())
        .await
        .unwrap();
    let (tables,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sqlite_master WHERE name IN ('accounts', 'notes', '_sqlx_migrations')",
    )
    .fetch_one(db.sqlite_pool())
    .await
    .unwrap();
    assert_eq!(tables, 3);
    let (versions,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(db.sqlite_pool())
        .await
        .unwrap();
    assert_eq!(versions, 2);
    // The pool of the test database is still open, and a second run changes nothing.
    assert!(!db.sqlite_pool().is_closed());
    let db = db.with_migrations(files.path()).await.unwrap();
    let (versions,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(db.sqlite_pool())
        .await
        .unwrap();
    assert_eq!(versions, 2);
    db.close().await;
}

#[tokio::test]
async fn the_shared_cache_memory_database_is_not_a_private_one() {
    let db = TestDatabase::sqlite();
    assert!(db.url().contains("mode=memory&cache=shared"));
    let files = MigrationFiles::new().up(1, "t", "CREATE TABLE t (x INTEGER)");
    // A private in-memory database would fail with `PrivateMemory`.
    let db = db.with_migrations(files.path()).await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn with_migrations_on_a_failing_script_returns_a_value_free_error() {
    let files = MigrationFiles::new().up(
        1,
        "broken",
        "CREATE TABLE SECRET_MARKER_TABLE (id INTEGER); SELECT * FROM SECRET_MARKER_MISSING;",
    );
    let dir = files.path().to_str().unwrap().to_owned();
    let error = TestDatabase::sqlite()
        .with_migrations(files.path())
        .await
        .unwrap_err();
    for text in [error.to_string(), format!("{error:?}")] {
        assert!(!text.contains("SECRET_MARKER"), "{text}");
        assert!(!text.contains(&dir), "{text}");
    }
    assert_eq!(error.version(), Some(1));
}

#[tokio::test]
async fn a_directory_that_mixes_plain_and_reversible_files_is_applied_in_version_order() {
    let files = MigrationFiles::new()
        .plain(1, "first", "CREATE TABLE a (x INTEGER)")
        .reversible(2, "second", "CREATE TABLE b (x INTEGER)", "DROP TABLE b")
        .up(3, "third", "CREATE TABLE c (x INTEGER)");
    let db = TestDatabase::sqlite()
        .with_migrations(files.path())
        .await
        .unwrap();
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT version, description FROM _sqlx_migrations ORDER BY version")
            .fetch_all(db.sqlite_pool())
            .await
            .unwrap();
    assert_eq!(
        rows,
        [
            (1, "first".to_owned()),
            (2, "second".to_owned()),
            (3, "third".to_owned())
        ]
    );
    db.close().await;
}
