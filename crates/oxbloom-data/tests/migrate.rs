//! `migrate::run` on SQLite: the order of the applied migrations, the report, the embedded
//! migrator and the directory, the load, checksum, missing file, script, dirty history and
//! timeout errors, a custom history table, two concurrent runners on one file, and the rule
//! that no error text contains SQL, a path or the URL.
//!
//! Every test writes its migration files into a temporary directory and uses a SQLite file
//! database in another one, so nothing depends on fixture files. The Postgres and MySQL
//! variants belong to the conformance suite.

#![cfg(all(feature = "migrate", feature = "sqlite"))]

use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use oxbloom_config::Config;
use oxbloom_data::migrate::{self, EmbeddedMigrations, LoadReason, MigrateError};
use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
use oxbloom_data::sqlx::{self, SqlSafeStr, SqlitePool};
use oxbloom_data::{DataError, DataSettings, DataSources, MigrationSettings};
use tempfile::TempDir;
use tracing_subscriber::fmt::MakeWriter;

/// The three migrations most tests use. The second one needs the first, so it proves the order.
const THREE: [(&str, &str); 3] = [
    (
        "3_create_invoices.sql",
        "CREATE TABLE invoices (id INTEGER PRIMARY KEY);",
    ),
    (
        "1_create_accounts.sql",
        "CREATE TABLE accounts (id INTEGER PRIMARY KEY, name TEXT);",
    ),
    (
        "2_index_accounts.sql",
        "CREATE INDEX accounts_by_name ON accounts (name);",
    ),
];

/// A SQLite database file and a migration directory, both in temporary directories that are
/// removed on drop.
struct Fixture {
    db_dir: TempDir,
    migrations: TempDir,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let fixture = Self {
            db_dir: tempfile::Builder::new()
                .prefix("oxbloom-migrate-db")
                .tempdir()
                .unwrap(),
            migrations: tempfile::Builder::new()
                .prefix("oxbloom-migrate-files")
                .tempdir()
                .unwrap(),
        };
        for (name, sql) in files {
            fixture.write(name, sql);
        }
        fixture
    }

    fn write(&self, name: &str, sql: &str) {
        std::fs::write(self.migrations.path().join(name), sql).unwrap();
    }

    fn remove(&self, name: &str) {
        std::fs::remove_file(self.migrations.path().join(name)).unwrap();
    }

    fn db_path(&self) -> PathBuf {
        self.db_dir.path().join("app.db")
    }

    fn url(&self) -> String {
        format!("sqlite:{}?mode=rwc", self.db_path().display())
    }

    /// A fresh registry with one `default` source on the database file.
    fn sources(&self) -> DataSources {
        let config = Config::from_str(&format!("[data]\nurl = \"{}\"", self.url())).unwrap();
        let settings = DataSettings::from_config(&config).unwrap();
        DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap()
    }

    /// The settings with `path` set to the migration directory.
    fn settings(&self) -> MigrationSettings {
        settings(Some(self.migrations.path().to_path_buf()))
    }
}

fn settings(path: Option<PathBuf>) -> MigrationSettings {
    MigrationSettings {
        path,
        table: "_sqlx_migrations".to_string(),
        ignore_missing: false,
        timeout: Duration::from_secs(30),
        locking: true,
    }
}

fn pool(sources: &DataSources) -> &SqlitePool {
    sources.sqlite("default").unwrap()
}

async fn table_exists(pool: &SqlitePool, name: &str) -> bool {
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type IN ('table', 'index') AND name = ?",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    count == 1
}

async fn history(pool: &SqlitePool, table: &str) -> Vec<i64> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT version FROM {table} ORDER BY version"
    )))
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A `'static` migrator with the migrations of [`THREE`], built in code as
/// `sqlx::migrate!` would build it from the directory.
fn embedded_three() -> &'static Migrator {
    let migration = |version, description: &'static str, sql: &'static str| {
        Migration::new(
            version,
            Cow::Borrowed(description),
            MigrationType::Simple,
            sql.into_sql_str(),
            false,
        )
    };
    Box::leak(Box::new(Migrator::with_migrations(vec![
        migration(3, "create invoices", THREE[0].1),
        migration(1, "create accounts", THREE[1].1),
        migration(2, "index accounts", THREE[2].1),
    ])))
}

fn versions(report: &migrate::MigrationReport) -> Vec<i64> {
    report.applied().iter().map(|m| m.version).collect()
}

#[tokio::test]
async fn pending_migrations_are_applied_in_version_order() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();

    let report = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    assert_eq!(versions(&report), [1, 2, 3]);
    let descriptions: Vec<&str> = report
        .applied()
        .iter()
        .map(|m| m.description.as_str())
        .collect();
    assert_eq!(
        descriptions,
        ["create accounts", "index accounts", "create invoices"]
    );
    assert_eq!(report.latest_version(), Some(3));
    assert!(!report.is_empty());
    for table in ["accounts", "accounts_by_name", "invoices"] {
        assert!(table_exists(pool(&sources), table).await, "{table}");
    }
    assert_eq!(history(pool(&sources), "_sqlx_migrations").await, [1, 2, 3]);
}

#[tokio::test]
async fn a_second_run_applies_nothing() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    let report = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    assert!(report.is_empty());
    assert_eq!(report.latest_version(), None);
    assert_eq!(history(pool(&sources), "_sqlx_migrations").await, [1, 2, 3]);
}

#[tokio::test]
async fn an_embedded_migrator_gives_the_same_result_as_the_directory() {
    let from_dir = Fixture::new(&THREE);
    let from_code = Fixture::new(&[]);
    let (dir_sources, code_sources) = (from_dir.sources(), from_code.sources());

    let dir_report = migrate::run(&dir_sources, "default", &from_dir.settings(), None)
        .await
        .unwrap();
    let code_report = migrate::run(
        &code_sources,
        "default",
        &settings(None),
        Some(embedded_three()),
    )
    .await
    .unwrap();

    assert_eq!(dir_report.applied(), code_report.applied());
    for table in ["accounts", "accounts_by_name", "invoices"] {
        assert!(table_exists(pool(&code_sources), table).await, "{table}");
    }
    // The checksums agree as well: the directory accepts what the embedded migrator applied.
    let again = migrate::run(&code_sources, "default", &from_dir.settings(), None)
        .await
        .unwrap();
    assert!(again.is_empty());
}

#[tokio::test]
async fn path_wins_over_the_embedded_migrator() {
    let fixture = Fixture::new(&[("1_from_dir.sql", "CREATE TABLE from_dir (id INTEGER);")]);
    let sources = fixture.sources();
    let embedded = Migrator::with_migrations(vec![Migration::new(
        1,
        Cow::Borrowed("from code"),
        MigrationType::Simple,
        "CREATE TABLE from_code (id INTEGER);".into_sql_str(),
        false,
    )]);

    let report = migrate::run(&sources, "default", &fixture.settings(), Some(&embedded))
        .await
        .unwrap();

    assert_eq!(report.applied()[0].description, "from dir");
    assert!(table_exists(pool(&sources), "from_dir").await);
    assert!(!table_exists(pool(&sources), "from_code").await);
}

#[tokio::test]
async fn a_missing_directory_is_a_load_error_without_the_path() {
    let fixture = Fixture::new(&[]);
    let sources = fixture.sources();
    let missing = fixture.migrations.path().join("no-such-oxbloom-migrations");

    let err = migrate::run(&sources, "default", &settings(Some(missing)), None)
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            MigrateError::Load { source_name, reason: LoadReason::Unreadable } if source_name == "default"
        ),
        "{err:?}"
    );
    let text = err.to_string();
    assert!(!text.contains("no-such-oxbloom-migrations"), "{text}");
    assert!(
        !text.contains(&*fixture.migrations.path().to_string_lossy()),
        "{text}"
    );
    assert!(!format!("{err:?}").contains("no-such-oxbloom-migrations"));
    assert!(std::error::Error::source(&err).is_none());
}

#[tokio::test]
async fn an_empty_path_is_a_load_error() {
    let fixture = Fixture::new(&[]);
    let sources = fixture.sources();

    let err = migrate::run(&sources, "default", &settings(Some(PathBuf::new())), None)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            MigrateError::Load {
                reason: LoadReason::Unreadable,
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn an_empty_directory_is_a_load_error_but_an_empty_embedded_migrator_is_not() {
    let fixture = Fixture::new(&[]);
    let sources = fixture.sources();

    let err = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            MigrateError::Load {
                reason: LoadReason::Empty,
                ..
            }
        ),
        "{err:?}"
    );

    let empty = Migrator::with_migrations(Vec::new());
    let report = migrate::run(&sources, "default", &settings(None), Some(&empty))
        .await
        .unwrap();
    assert!(report.is_empty());
}

#[tokio::test]
async fn neither_a_path_nor_an_embedded_migrator_is_a_load_error_that_says_what_to_set() {
    let fixture = Fixture::new(&[]);
    let sources = fixture.sources();

    let err = migrate::run(&sources, "default", &settings(None), None)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            MigrateError::Load {
                reason: LoadReason::NotConfigured,
                ..
            }
        ),
        "{err:?}"
    );
    let text = err.to_string();
    assert!(text.contains("`default`"), "{text}");
    assert!(text.contains("path"), "{text}");
    assert!(text.contains("EmbeddedMigrations"), "{text}");
}

#[tokio::test]
async fn an_invalid_table_name_is_refused_before_it_reaches_sql() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();
    let mut bad = fixture.settings();
    bad.table = "x; DROP TABLE accounts".to_string();

    let err = migrate::run(&sources, "default", &bad, None)
        .await
        .unwrap_err();

    assert!(
        matches!(
            err,
            MigrateError::Load {
                reason: LoadReason::InvalidTable,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!err.to_string().contains("DROP"));
    assert!(!table_exists(pool(&sources), "accounts").await);
}

#[tokio::test]
async fn a_changed_applied_file_fails_with_the_version() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();
    fixture.write(
        "2_index_accounts.sql",
        "CREATE INDEX accounts_by_id ON accounts (id);",
    );

    let err = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            MigrateError::ChecksumMismatch { source_name, version: 2, description }
                if source_name == "default" && description == "index accounts"
        ),
        "{err:?}"
    );
    assert_eq!(err.version(), Some(2));
    assert_eq!(err.source_name(), "default");
}

#[tokio::test]
async fn a_missing_file_fails_unless_ignore_missing() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();
    fixture.remove("3_create_invoices.sql");

    let err = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MigrateError::MissingFile { version: 3, .. }),
        "{err:?}"
    );

    let mut lenient = fixture.settings();
    lenient.ignore_missing = true;
    let report = migrate::run(&sources, "default", &lenient, None)
        .await
        .unwrap();
    assert!(report.is_empty());
}

#[tokio::test]
async fn a_failing_script_names_version_and_description_and_leaves_no_table() {
    let fixture = Fixture::new(&[
        ("1_create_accounts.sql", THREE[1].1),
        (
            "2_add_ledger.sql",
            "CREATE TABLE ledger_secret (id INTEGER);\nINSERT INTO nosuch_secret_table VALUES (1);",
        ),
    ]);
    let sources = fixture.sources();

    let err = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            MigrateError::Script { source_name, version: 2, description, .. }
                if source_name == "default" && description == "add ledger"
        ),
        "{err:?}"
    );
    // SQLite runs the script and the history row in one transaction.
    assert!(table_exists(pool(&sources), "accounts").await);
    assert!(!table_exists(pool(&sources), "ledger_secret").await);
    assert_eq!(history(pool(&sources), "_sqlx_migrations").await, [1]);
    // The database error is kept as the source, but no text of this crate prints it.
    assert!(std::error::Error::source(&err).is_some());
    for text in [err.to_string(), format!("{err:?}")] {
        for secret in ["ledger_secret", "nosuch_secret_table", "INSERT", "CREATE"] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

#[tokio::test]
async fn a_dirty_history_fails_with_the_version() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET success = false WHERE version = 2")
        .execute(pool(&sources))
        .await
        .unwrap();

    let err = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            MigrateError::Dirty { source_name, version: 2, .. } if source_name == "default"
        ),
        "{err:?}"
    );
    assert!(!err.to_string().contains("_sqlx_migrations"));
}

#[tokio::test]
async fn a_custom_table_is_used() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();
    let mut custom = fixture.settings();
    custom.table = "app_history".to_string();

    let report = migrate::run(&sources, "default", &custom, None)
        .await
        .unwrap();

    assert_eq!(versions(&report), [1, 2, 3]);
    assert_eq!(history(pool(&sources), "app_history").await, [1, 2, 3]);
    assert!(!table_exists(pool(&sources), "_sqlx_migrations").await);
}

#[tokio::test]
async fn two_concurrent_runs_on_one_sqlite_file_both_end_up_migrated() {
    const ROUNDS: usize = 25;
    // The sqlx lock is a no-op on SQLite. Without a lock of its own a runner can lose the race
    // with "table already exists" (in about one round in fifty even after one re-run, as
    // measured with 200 rounds). The runner takes SQLite's write lock for the run and runs
    // once more after a failure, so both runners succeed and each migration is applied once.
    for round in 0..ROUNDS {
        let fixture = Fixture::new(&THREE);
        let (first, second) = (fixture.sources(), fixture.sources());
        let settings = fixture.settings();

        let (a, b) = tokio::join!(
            migrate::run(&first, "default", &settings, None),
            migrate::run(&second, "default", &settings, None),
        );

        for result in [&a, &b] {
            assert!(result.is_ok(), "round {round}: {result:?}");
        }
        assert_eq!(history(pool(&first), "_sqlx_migrations").await, [1, 2, 3]);
        for table in ["accounts", "accounts_by_name", "invoices"] {
            assert!(table_exists(pool(&first), table).await, "{table}");
        }
    }
}

#[tokio::test]
async fn the_timeout_cancels_a_slow_run() {
    let fixture = Fixture::new(&[(
        "1_slow.sql",
        "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 30000000) \
         SELECT count(*) FROM n;",
    )]);
    let sources = fixture.sources();
    let mut quick = fixture.settings();
    quick.timeout = Duration::from_millis(50);

    let started = std::time::Instant::now();
    let err = migrate::run(&sources, "default", &quick, None)
        .await
        .unwrap_err();

    assert!(
        matches!(&err, MigrateError::Timeout { source_name } if source_name == "default"),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn an_unknown_source_is_reported() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();

    let err = migrate::run(&sources, "reports", &fixture.settings(), None)
        .await
        .unwrap_err();

    assert!(
        matches!(&err, MigrateError::UnknownSource { source_name } if source_name == "reports"),
        "{err:?}"
    );
    assert_eq!(err.to_string(), "data source `reports` does not exist");
}

#[tokio::test]
async fn no_error_display_contains_sql_a_path_or_the_url() {
    let mut errors = Vec::new();

    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();
    let missing = fixture.migrations.path().join("gone");
    errors.push(
        migrate::run(&sources, "default", &settings(Some(missing)), None)
            .await
            .unwrap_err(),
    );
    errors.push(
        migrate::run(&sources, "default", &settings(None), None)
            .await
            .unwrap_err(),
    );
    errors.push(
        migrate::run(&sources, "nope", &fixture.settings(), None)
            .await
            .unwrap_err(),
    );
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();
    fixture.write("2_index_accounts.sql", "CREATE INDEX x ON accounts (id);");
    errors.push(
        migrate::run(&sources, "default", &fixture.settings(), None)
            .await
            .unwrap_err(),
    );
    fixture.remove("3_create_invoices.sql");
    errors.push(
        migrate::run(&sources, "default", &fixture.settings(), None)
            .await
            .unwrap_err(),
    );

    let failing = Fixture::new(&[("1_broken.sql", "SELECT * FROM missing_secret_table;")]);
    errors.push(
        migrate::run(&failing.sources(), "default", &failing.settings(), None)
            .await
            .unwrap_err(),
    );

    let paths = [
        fixture.migrations.path().to_string_lossy().into_owned(),
        fixture.db_dir.path().to_string_lossy().into_owned(),
        failing.migrations.path().to_string_lossy().into_owned(),
        failing.db_dir.path().to_string_lossy().into_owned(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    ];
    let urls = [fixture.url(), failing.url()];
    for err in &errors {
        for text in [err.to_string(), format!("{err:?}")] {
            for word in [
                "SELECT",
                "CREATE",
                "INSERT",
                "missing_secret_table",
                "app.db",
            ] {
                assert!(!text.contains(word), "{text}");
            }
            for secret in paths.iter().chain(urls.iter()) {
                assert!(!text.contains(secret.as_str()), "{text}");
            }
            assert!(!text.contains("sqlite:"), "{text}");
        }
    }
}

#[test]
fn debug_output_shows_names_and_versions_only() {
    let embedded = EmbeddedMigrations::new()
        .default_source(embedded_three())
        .source("cache", embedded_three());
    let text = format!("{embedded:?}");
    assert_eq!(
        text,
        "EmbeddedMigrations { cache: [1, 2, 3], default: [1, 2, 3] }"
    );
    assert!(embedded.get("default").is_some());
    assert!(embedded.get("cache").is_some());
    assert!(embedded.get("reports").is_none());
    assert!(!text.contains("CREATE"));
}

#[tokio::test]
async fn the_report_debug_shows_versions_and_its_display_lists_descriptions() {
    let fixture = Fixture::new(&THREE);
    let sources = fixture.sources();

    let report = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    assert_eq!(
        format!("{report:?}"),
        "MigrationReport { applied: [1, 2, 3] }"
    );
    assert_eq!(
        report.to_string(),
        "applied 3 migrations: 1 (create accounts), 2 (index accounts), 3 (create invoices)"
    );
}

/// Reversible pairs 1 to 3: each up file creates a table and each down file drops it.
const REVERSIBLE: [(&str, &str); 6] = [
    (
        "1_create_accounts.up.sql",
        "CREATE TABLE accounts (id INTEGER PRIMARY KEY);",
    ),
    ("1_create_accounts.down.sql", "DROP TABLE accounts;"),
    (
        "2_create_orders.up.sql",
        "CREATE TABLE orders (id INTEGER PRIMARY KEY);",
    ),
    ("2_create_orders.down.sql", "DROP TABLE orders;"),
    (
        "3_create_invoices.up.sql",
        "CREATE TABLE invoices (id INTEGER PRIMARY KEY);",
    ),
    ("3_create_invoices.down.sql", "DROP TABLE invoices;"),
];

#[tokio::test]
async fn revert_goes_back_to_the_target_version() {
    let fixture = Fixture::new(&REVERSIBLE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    let report = migrate::revert(&sources, "default", &fixture.settings(), None, 1)
        .await
        .unwrap();

    assert_eq!(versions(&report), [3, 2]);
    assert_eq!(report.latest_version(), Some(3));
    assert_eq!(
        report.to_string(),
        "reverted 2 migrations: 3 (create invoices), 2 (create orders)"
    );
    assert!(table_exists(pool(&sources), "accounts").await);
    assert!(!table_exists(pool(&sources), "orders").await);
    assert!(!table_exists(pool(&sources), "invoices").await);
    assert_eq!(history(pool(&sources), "_sqlx_migrations").await, [1]);

    // A later run applies the reverted migrations again.
    let again = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();
    assert_eq!(versions(&again), [2, 3]);
}

#[tokio::test]
async fn revert_to_the_current_version_does_nothing() {
    let fixture = Fixture::new(&REVERSIBLE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    for target in [3, 4, i64::MAX] {
        let report = migrate::revert(&sources, "default", &fixture.settings(), None, target)
            .await
            .unwrap();
        assert!(report.is_empty(), "target {target}");
        assert_eq!(report.latest_version(), None);
    }
    assert_eq!(history(pool(&sources), "_sqlx_migrations").await, [1, 2, 3]);
    assert!(table_exists(pool(&sources), "invoices").await);

    // An empty database has nothing to revert either.
    let empty = Fixture::new(&REVERSIBLE);
    let report = migrate::revert(&empty.sources(), "default", &empty.settings(), None, 0)
        .await
        .unwrap();
    assert!(report.is_empty());
}

#[tokio::test]
async fn revert_to_zero_reverts_everything() {
    let fixture = Fixture::new(&REVERSIBLE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    let report = migrate::revert(&sources, "default", &fixture.settings(), None, 0)
        .await
        .unwrap();

    assert_eq!(versions(&report), [3, 2, 1]);
    assert!(history(pool(&sources), "_sqlx_migrations").await.is_empty());
}

#[tokio::test]
async fn a_migration_without_a_down_file_is_not_reversible_and_named_by_version() {
    let fixture = Fixture::new(&REVERSIBLE);
    fixture.remove("2_create_orders.down.sql");
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    let err = migrate::revert(&sources, "default", &fixture.settings(), None, 0)
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            MigrateError::NotReversible { source_name, version: 2 } if source_name == "default"
        ),
        "{err:?}"
    );
    assert_eq!(err.version(), Some(2));
    assert_eq!(err.source_name(), "default");
    assert_eq!(
        err.to_string(),
        "data source `default`: migration 2 has no down file, so it cannot be reverted"
    );
    assert!(std::error::Error::source(&err).is_none());
    // Nothing was reverted, not even the reversible migration 3 above it.
    assert_eq!(history(pool(&sources), "_sqlx_migrations").await, [1, 2, 3]);
    assert!(table_exists(pool(&sources), "invoices").await);

    // Reverting only as far as the reversible migration 3 still works.
    let report = migrate::revert(&sources, "default", &fixture.settings(), None, 2)
        .await
        .unwrap();
    assert_eq!(versions(&report), [3]);
}

#[tokio::test]
async fn an_embedded_migrator_is_reverted_like_a_directory() {
    let migration = |version, kind, description: &'static str, sql: &'static str| {
        Migration::new(
            version,
            Cow::Borrowed(description),
            kind,
            sql.into_sql_str(),
            false,
        )
    };
    let embedded = Migrator::with_migrations(vec![
        migration(
            1,
            MigrationType::ReversibleUp,
            "create accounts",
            "CREATE TABLE accounts (id INTEGER);",
        ),
        migration(
            1,
            MigrationType::ReversibleDown,
            "create accounts",
            "DROP TABLE accounts;",
        ),
    ]);
    let fixture = Fixture::new(&[]);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &settings(None), Some(&embedded))
        .await
        .unwrap();

    let report = migrate::revert(&sources, "default", &settings(None), Some(&embedded), 0)
        .await
        .unwrap();

    assert_eq!(versions(&report), [1]);
    assert!(!table_exists(pool(&sources), "accounts").await);
}

#[tokio::test]
async fn revert_fails_on_a_missing_file_and_a_dirty_history_but_not_on_a_changed_file() {
    let fixture = Fixture::new(&REVERSIBLE);
    let sources = fixture.sources();
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    // A changed file does not fail a revert: `sqlx` checks no checksums when it undoes.
    fixture.write(
        "2_create_orders.up.sql",
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, changed INTEGER);",
    );
    let report = migrate::revert(&sources, "default", &fixture.settings(), None, 2)
        .await
        .unwrap();
    assert_eq!(versions(&report), [3]);
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap_err();
    fixture.write(
        "2_create_orders.up.sql",
        "CREATE TABLE orders (id INTEGER PRIMARY KEY);",
    );
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    // A missing file fails unless `ignore_missing` is set.
    fixture.remove("3_create_invoices.up.sql");
    fixture.remove("3_create_invoices.down.sql");
    let err = migrate::revert(&sources, "default", &fixture.settings(), None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MigrateError::MissingFile { version: 3, .. }),
        "{err:?}"
    );
    let mut lenient = fixture.settings();
    lenient.ignore_missing = true;
    // Version 3 is applied but has no down file at all, so it cannot be reverted.
    let err = migrate::revert(&sources, "default", &lenient, None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MigrateError::NotReversible { version: 3, .. }),
        "{err:?}"
    );

    // A dirty history is named before anything is reverted.
    sqlx::query("UPDATE _sqlx_migrations SET success = 0 WHERE version = 2")
        .execute(pool(&sources))
        .await
        .unwrap();
    let err = migrate::revert(&sources, "default", &lenient, None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MigrateError::Dirty { version: 2, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn revert_uses_the_same_timeout_and_table() {
    let fixture = Fixture::new(&[
        ("1_slow.up.sql", "CREATE TABLE slow (id INTEGER);"),
        (
            "1_slow.down.sql",
            "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n WHERE x < 30000000) \
             SELECT count(*) FROM n;",
        ),
        ("2_fast.up.sql", "CREATE TABLE fast (id INTEGER);"),
        ("2_fast.down.sql", "DROP TABLE fast;"),
    ]);
    let sources = fixture.sources();
    let mut custom = fixture.settings();
    custom.table = "app_history".to_string();
    migrate::run(&sources, "default", &custom, None)
        .await
        .unwrap();
    assert!(!table_exists(pool(&sources), "_sqlx_migrations").await);

    // The custom table is the one that shrinks.
    let report = migrate::revert(&sources, "default", &custom, None, 1)
        .await
        .unwrap();
    assert_eq!(versions(&report), [2]);
    assert_eq!(history(pool(&sources), "app_history").await, [1]);
    assert!(!table_exists(pool(&sources), "_sqlx_migrations").await);

    // The timeout bounds the revert as it bounds the run.
    custom.timeout = Duration::from_millis(50);
    let started = std::time::Instant::now();
    let err = migrate::revert(&sources, "default", &custom, None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, MigrateError::Timeout { source_name } if source_name == "default"),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn revert_reports_load_unknown_source_and_table_errors_like_run() {
    let fixture = Fixture::new(&REVERSIBLE);
    let sources = fixture.sources();

    let err = migrate::revert(&sources, "default", &settings(None), None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            MigrateError::Load {
                reason: LoadReason::NotConfigured,
                ..
            }
        ),
        "{err:?}"
    );
    let err = migrate::revert(&sources, "nope", &fixture.settings(), None, 0)
        .await
        .unwrap_err();
    assert!(matches!(err, MigrateError::UnknownSource { .. }), "{err:?}");
    let mut bad = fixture.settings();
    bad.table = "history; DROP TABLE accounts".to_string();
    let err = migrate::revert(&sources, "default", &bad, None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            MigrateError::Load {
                reason: LoadReason::InvalidTable,
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_retry_keeps_the_versions_applied_by_the_first_attempt() {
    // Migration 2 fails the first time and succeeds the second time: it is a `no-transaction`
    // migration, so the first statement stays when the second one fails, and the second
    // statement only passes when it finds the row the first attempt left behind.
    let fixture = Fixture::new(&[
        (
            "1_create_accounts.sql",
            "CREATE TABLE accounts (id INTEGER);",
        ),
        (
            "2_flaky.sql",
            "-- no-transaction\n\
             CREATE TABLE IF NOT EXISTS seen (x INTEGER);\n\
             CREATE TABLE IF NOT EXISTS probe (x INTEGER NOT NULL);\n\
             INSERT INTO seen VALUES (1);\n\
             INSERT INTO probe VALUES (CASE WHEN (SELECT count(*) FROM seen) > 1 THEN 1 END);",
        ),
    ]);
    let sources = fixture.sources();

    let report = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();

    assert_eq!(history(pool(&sources), "_sqlx_migrations").await, [1, 2]);
    assert_eq!(versions(&report), [1, 2]);
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Installs a subscriber for the whole test binary that keeps every event at `debug` and
/// below in memory. The events of the `sqlx` worker threads need a global subscriber.
fn logs() -> LogBuffer {
    static LOGS: OnceLock<LogBuffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        logs
    })
    .clone()
}

#[tokio::test]
async fn run_and_revert_emit_no_sql_in_tracing_output() {
    let logs = logs();
    let fixture = Fixture::new(&[
        (
            "1_create_accounts.up.sql",
            "CREATE TABLE accounts (SECRET_MARKER_SQL_ok INTEGER);",
        ),
        (
            "1_create_accounts.down.sql",
            "DROP TABLE accounts; -- SECRET_MARKER_SQL_down",
        ),
        (
            "2_broken.up.sql",
            "SELECT SECRET_MARKER_SQL FROM no_such_table;",
        ),
        ("2_broken.down.sql", "DROP TABLE SECRET_MARKER_SQL_gone;"),
    ]);
    let sources = fixture.sources();

    let err = migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MigrateError::Script { version: 2, .. }),
        "{err:?}"
    );
    // The history now holds migration 1, so the revert runs its down script, and a revert of
    // the never applied migration 2 is not attempted.
    migrate::revert(&sources, "default", &fixture.settings(), None, 0)
        .await
        .unwrap();
    // A failing down script is covered as well.
    fixture.remove("2_broken.up.sql");
    fixture.write("2_broken.up.sql", "CREATE TABLE fine (id INTEGER);");
    migrate::run(&sources, "default", &fixture.settings(), None)
        .await
        .unwrap();
    let err = migrate::revert(&sources, "default", &fixture.settings(), None, 0)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MigrateError::Script { version: 2, .. }),
        "{err:?}"
    );

    let captured = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        captured.contains("sqlx"),
        "the capture saw no sqlx event, so this test proves nothing: {captured}"
    );
    assert!(
        !captured.contains("SECRET_MARKER_SQL"),
        "a statement of a migration reached the tracing output: {captured}"
    );
}

fn memory_sources(url: &str) -> DataSources {
    let config =
        Config::from_str(&format!("[data]\nurl = \"{url}\"\nmax-connections = 1")).unwrap();
    let settings = DataSettings::from_config(&config).unwrap();
    DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap()
}

#[tokio::test]
async fn a_private_cache_memory_database_is_refused_by_run_and_revert() {
    let fixture = Fixture::new(&REVERSIBLE);
    for url in [
        "sqlite://?mode=memory&cache=private",
        "sqlite://?mode=memory",
        "sqlite:file:oxbloom-private?mode=memory&cache=private",
    ] {
        let sources = memory_sources(url);
        let run = migrate::run(&sources, "default", &fixture.settings(), None)
            .await
            .unwrap_err();
        let revert = migrate::revert(&sources, "default", &fixture.settings(), None, 0)
            .await
            .unwrap_err();
        for err in [run, revert] {
            assert!(
                matches!(&err, MigrateError::PrivateMemory { source_name } if source_name == "default"),
                "{err:?}"
            );
            for text in [err.to_string(), format!("{err:?}")] {
                assert!(!text.contains("oxbloom-private"), "{text}");
                assert!(!text.contains("sqlite:"), "{text}");
                assert!(!text.contains("cache=private"), "{text}");
            }
            assert_eq!(
                err.to_string(),
                "data source `default`: an in-memory SQLite database with a private cache cannot \
                 be migrated; use `cache=shared` or a database file"
            );
        }
        // Nothing was migrated: the pool's own database has no history table.
        assert!(!table_exists(pool(&sources), "_sqlx_migrations").await);
    }
}

#[tokio::test]
async fn shared_cache_memory_databases_still_migrate() {
    let fixture = Fixture::new(&REVERSIBLE);
    for url in [
        "sqlite::memory:",
        "sqlite://:memory:",
        "sqlite:file:oxbloom-shared-one?mode=memory&cache=shared",
    ] {
        let sources = memory_sources(url);
        let report = migrate::run(&sources, "default", &fixture.settings(), None)
            .await
            .unwrap();
        assert_eq!(versions(&report), [1, 2, 3], "{url}");
        // The pool sees what the run did.
        assert!(table_exists(pool(&sources), "invoices").await, "{url}");
        let report = migrate::revert(&sources, "default", &fixture.settings(), None, 1)
            .await
            .unwrap();
        assert_eq!(versions(&report), [3, 2], "{url}");
        assert!(!table_exists(pool(&sources), "invoices").await, "{url}");
    }
}
