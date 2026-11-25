//! `DataAutoConfiguration` with the feature `data-migrate`: the `DataService` applies the
//! migrations of every source that enables them while the application starts, and a failed run
//! stops the boot with a report that names the source and the version, run in process through
//! `ContextRunner` with SQLite files in temporary directories.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use oxbloom_boot::{
    AutoConfiguration, DataAutoConfiguration, DataService, Service, ServiceError, StartupPhase,
    async_trait,
};
use oxbloom_config::ConfigError;
use oxbloom_data::migrate::{EmbeddedMigrations, MigrateError};
use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
use oxbloom_data::sqlx::{self, SqlSafeStr, SqlitePool};
use oxbloom_data::{DataError, DataSettings, DataSources};
use oxbloom_test::ContextRunner;
use tempfile::TempDir;
use tracing_subscriber::fmt::MakeWriter;

/// A text in every SQL script of these tests, so a report or a log line that shows SQL is
/// caught.
const SQL_MARKER: &str = "SECRET_SQL_MARKER";

/// A temporary directory with SQLite database files and migration directories.
struct Scratch(TempDir);

impl Scratch {
    fn new() -> Self {
        Scratch(tempfile::tempdir().unwrap())
    }

    /// The URL of the database file `name`, created on first use.
    fn url(&self, name: &str) -> String {
        format!(
            "sqlite:{}?mode=rwc",
            self.0.path().join(format!("{name}.db")).display()
        )
    }

    /// A migration directory `name` with `files` (file name, content).
    fn migrations(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = self.0.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, content) in files {
            std::fs::write(dir.join(file), content).unwrap();
        }
        dir
    }

    /// The tables of the database file `name`, sorted, without the history table.
    async fn tables(&self, name: &str) -> Vec<String> {
        let pool = SqlitePool::connect(&self.url(name)).await.unwrap();
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE '\\_sqlx%' ESCAPE '\\' AND name NOT LIKE 'sqlite%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        pool.close().await;
        tables
    }

    /// Runs `sql` on the database file `name`.
    async fn execute(&self, name: &str, sql: &'static str) {
        let pool = SqlitePool::connect(&self.url(name)).await.unwrap();
        sqlx::query(sql).execute(&pool).await.unwrap();
        pool.close().await;
    }
}

/// The TOML path value of `dir`.
fn path(dir: &Path) -> String {
    dir.display().to_string()
}

/// An embedded migrator that lives for `'static`, as `sqlx::migrate!` gives.
fn migrator(migrations: &[(i64, &'static str, &'static str)]) -> &'static Migrator {
    let migrations = migrations
        .iter()
        .map(|&(version, description, sql)| {
            Migration::new(
                version,
                description.into(),
                MigrationType::Simple,
                sql.into_sql_str(),
                false,
            )
        })
        .collect();
    Box::leak(Box::new(Migrator::with_migrations(migrations)))
}

/// The texts of a failed run that must never contain a value: the error, its analysis and the
/// captured output.
fn failure_texts(view: &oxbloom_test::FailureView<'_>) -> Vec<String> {
    vec![
        view.error().to_string(),
        format!("{:?}", view.error().find_cause::<MigrateError>()),
        view.error()
            .analysis()
            .map(ToString::to_string)
            .unwrap_or_default(),
        view.output().contents(),
    ]
}

/// Asserts that no failure text holds SQL, the directory or a migration file name.
fn assert_no_secrets(view: &oxbloom_test::FailureView<'_>, scratch: &Scratch, files: &[&str]) {
    let dir = scratch.0.path().display().to_string();
    for text in failure_texts(view) {
        assert!(!text.contains(SQL_MARKER), "{text}");
        assert!(!text.contains(&dir), "{text}");
        assert!(!text.contains(".sql"), "{text}");
        for file in files {
            assert!(!text.contains(file), "{text}");
        }
    }
}

/// A service started after the `DataService` that reads the migrated table in its `start`.
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

#[tokio::test]
async fn migrations_from_a_directory_run_before_a_later_service_starts() {
    let scratch = Scratch::new();
    let dir = scratch.migrations(
        "migrations",
        &[
            (
                "1_create_accounts.sql",
                "CREATE TABLE accounts (id INTEGER PRIMARY KEY); -- SECRET_SQL_MARKER",
            ),
            (
                "2_seed_accounts.sql",
                "INSERT INTO accounts (id) VALUES (1), (2);",
            ),
        ],
    );
    let seen = Arc::new(Mutex::new(None));
    let out = seen.clone();
    ContextRunner::new()
        .config_str(format!(
            "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = true\npath = '{}'",
            scratch.url("app"),
            path(&dir)
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .configure(move |b| {
            b.bean(move |pool: Arc<SqlitePool>| ReadsAccounts(pool, out.clone()))
                .provides(|s: Arc<ReadsAccounts>| -> Arc<dyn Service> { s })
        })
        .run(|view| assert!(view.contains::<DataService>()))
        .await;
    assert_eq!(*seen.lock().unwrap(), Some(2));
    assert_eq!(scratch.tables("app").await, ["accounts"]);
}

#[tokio::test]
async fn migrations_from_an_embedded_bean_run() {
    let scratch = Scratch::new();
    let embedded = EmbeddedMigrations::new()
        .default_source(migrator(&[(
            1,
            "create accounts",
            "CREATE TABLE accounts (id INTEGER)",
        )]))
        .source(
            "reports",
            migrator(&[(1, "create totals", "CREATE TABLE totals (id INTEGER)")]),
        );
    ContextRunner::new()
        .config_str(format!(
            "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = true\n\
             [data.sources.reports]\nurl = \"{}\"\n[data.sources.reports.migrations]\nenabled = true",
            scratch.url("app"),
            scratch.url("reports")
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .configure(move |b| b.instance(embedded))
        .run(|_| {})
        .await;
    assert_eq!(scratch.tables("app").await, ["accounts"]);
    assert_eq!(scratch.tables("reports").await, ["totals"]);
}

#[tokio::test]
async fn a_source_without_migrations_enabled_is_untouched() {
    let scratch = Scratch::new();
    // An embedded migrator exists, but no source enables migrations.
    let embedded = EmbeddedMigrations::new().default_source(migrator(&[(
        1,
        "create accounts",
        "CREATE TABLE accounts (id INTEGER)",
    )]));
    ContextRunner::new()
        .config_str(format!("[data]\nurl = \"{}\"", scratch.url("app")))
        .auto_configuration::<DataAutoConfiguration>()
        .configure(move |b| b.instance(embedded))
        .run(|_| {})
        .await;
    assert!(scratch.tables("app").await.is_empty());
    let pool = SqlitePool::connect(&scratch.url("app")).await.unwrap();
    let history: Option<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE name = '_sqlx_migrations'")
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(history, None, "no history table is created");
}

#[tokio::test]
async fn enabled_false_with_other_keys_is_untouched() {
    let scratch = Scratch::new();
    let dir = scratch.migrations(
        "migrations",
        &[(
            "1_create_accounts.sql",
            "CREATE TABLE accounts (id INTEGER);",
        )],
    );
    ContextRunner::new()
        .config_str(format!(
            "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = false\npath = '{}'\n\
             table = \"history\"\ntimeout = \"5s\"",
            scratch.url("app"),
            path(&dir)
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .run(|_| {})
        .await;
    assert!(scratch.tables("app").await.is_empty());
}

#[tokio::test]
async fn a_failing_migration_fails_the_boot_with_source_and_version() {
    let scratch = Scratch::new();
    let dir = scratch.migrations(
        "migrations",
        &[
            (
                "1_create_accounts.sql",
                "CREATE TABLE accounts (id INTEGER);",
            ),
            (
                "20261125_break_things.sql",
                "SELECT SECRET_SQL_MARKER FROM no_such_table;",
            ),
        ],
    );
    ContextRunner::new()
        .config_str(format!(
            "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = true\npath = '{}'",
            scratch.url("app"),
            path(&dir)
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            assert_eq!(view.error().phase(), StartupPhase::Service);
            let error = view
                .error()
                .find_cause::<MigrateError>()
                .expect("a migrate error");
            assert!(
                matches!(
                    error,
                    MigrateError::Script {
                        version: 20261125,
                        ..
                    }
                ),
                "{error:?}"
            );
            assert!(
                view.error()
                    .to_string()
                    .contains("data source `default`: the migrations failed"),
                "{}",
                view.error()
            );
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`default`"), "{report}");
            assert!(report.contains("20261125"), "{report}");
            assert_no_secrets(view, &scratch, &["20261125_break_things", "break_things"]);
        })
        .await;
    // SQLite rolls the failed script back; the first migration stays applied.
    assert_eq!(scratch.tables("app").await, ["accounts"]);
}

#[tokio::test]
async fn a_changed_file_fails_the_second_boot() {
    let scratch = Scratch::new();
    let dir = scratch.migrations(
        "migrations",
        &[(
            "1_create_accounts.sql",
            "CREATE TABLE accounts (id INTEGER);",
        )],
    );
    let config = format!(
        "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = true\npath = '{}'",
        scratch.url("app"),
        path(&dir)
    );
    ContextRunner::new()
        .config_str(config.clone())
        .auto_configuration::<DataAutoConfiguration>()
        .run(|_| {})
        .await;
    std::fs::write(
        dir.join("1_create_accounts.sql"),
        "CREATE TABLE accounts (id INTEGER, SECRET_SQL_MARKER TEXT);",
    )
    .unwrap();
    ContextRunner::new()
        .config_str(config)
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            let error = view
                .error()
                .find_cause::<MigrateError>()
                .expect("a migrate error");
            assert!(
                matches!(error, MigrateError::ChecksumMismatch { version: 1, .. }),
                "{error:?}"
            );
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`default`"), "{report}");
            assert!(report.contains("migration 1"), "{report}");
            assert!(report.contains("changed"), "{report}");
            assert_no_secrets(view, &scratch, &["1_create_accounts"]);
        })
        .await;
}

#[tokio::test]
async fn a_dirty_history_report_says_what_it_means_and_names_the_version() {
    let scratch = Scratch::new();
    let dir = scratch.migrations(
        "migrations",
        &[(
            "7_create_accounts.sql",
            "CREATE TABLE accounts (id INTEGER);",
        )],
    );
    let config = format!(
        "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = true\npath = '{}'",
        scratch.url("app"),
        path(&dir)
    );
    ContextRunner::new()
        .config_str(config.clone())
        .auto_configuration::<DataAutoConfiguration>()
        .run(|_| {})
        .await;
    // What a crash in the middle of migration 7 leaves behind.
    scratch
        .execute(
            "app",
            "UPDATE _sqlx_migrations SET success = false WHERE version = 7",
        )
        .await;
    ContextRunner::new()
        .config_str(config)
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            let error = view
                .error()
                .find_cause::<MigrateError>()
                .expect("a migrate error");
            assert!(
                matches!(error, MigrateError::Dirty { version: 7, .. }),
                "{error:?}"
            );
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`default`"), "{report}");
            assert!(report.contains("migration 7"), "{report}");
            // SQLite runs a script in a transaction: the dirty row means an interrupted one.
            assert!(report.contains("transaction"), "{report}");
            assert!(report.contains("interrupted"), "{report}");
            assert!(report.contains("history table"), "{report}");
            assert_no_secrets(view, &scratch, &["7_create_accounts"]);
        })
        .await;
}

#[tokio::test]
async fn enabled_without_a_path_or_an_embedded_bean_names_the_source_and_both_remedies() {
    let scratch = Scratch::new();
    // Once without any `EmbeddedMigrations` bean, once with a bean that lacks the source.
    for with_bean in [false, true] {
        ContextRunner::new()
            .config_str(format!(
                "[data.sources.orders]\nurl = \"{}\"\n[data.sources.orders.migrations]\nenabled = true",
                scratch.url("orders")
            ))
            .auto_configuration::<DataAutoConfiguration>()
            .configure(move |b| {
                if with_bean {
                    b.instance(EmbeddedMigrations::new().default_source(migrator(&[])))
                } else {
                    b
                }
            })
            .run_failing(|view| {
                assert_eq!(view.error().phase(), StartupPhase::Service);
                let report = view.error().analysis().expect("a report").to_string();
                assert!(report.contains("`orders`"), "{report}");
                assert!(
                    report.contains("`data.sources.orders.migrations.path`"),
                    "{report}"
                );
                assert!(report.contains("`EmbeddedMigrations`"), "{report}");
            })
            .await;
    }
    assert!(scratch.tables("orders").await.is_empty());
}

#[tokio::test]
async fn an_unknown_source_under_migrations_fails_with_the_key() {
    // A `migrations` table under a name that has no `url` is a source without a URL.
    ContextRunner::new()
        .config_str("[data.sources.ghost.migrations]\nenabled = true")
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            assert_eq!(view.error().phase(), StartupPhase::Container);
            let config = view.error().find_cause::<ConfigError>().expect("a config error");
            assert!(
                matches!(config, ConfigError::Invalid { key, .. } if key == "data.sources.ghost.url"),
                "{config:?}"
            );
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`data.sources.ghost.url`"), "{report}");
        })
        .await;

    // The application's own registry has no source of the name the settings migrate.
    fn own_registry() -> DataSources {
        let config = oxbloom_config::Config::from_str("[data]\nurl = \"sqlite::memory:\"").unwrap();
        let settings = DataSettings::from_config(&config).unwrap();
        DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap()
    }
    let scratch = Scratch::new();
    ContextRunner::new()
        .config_str(format!(
            "[data.sources.ghost]\nurl = \"{}\"\n[data.sources.ghost.migrations]\nenabled = true",
            scratch.url("ghost")
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .configure(|b| b.bean(own_registry))
        .run_failing(|view| {
            assert_eq!(view.error().phase(), StartupPhase::Service);
            let error = view
                .error()
                .find_cause::<MigrateError>()
                .expect("a migrate error");
            assert!(
                matches!(error, MigrateError::UnknownSource { .. }),
                "{error:?}"
            );
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`ghost`"), "{report}");
            assert!(
                report.contains("`data.sources.ghost.migrations.enabled`"),
                "{report}"
            );
            assert_no_secrets(view, &scratch, &[]);
        })
        .await;
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

impl LogBuffer {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// A subscriber for the whole test binary that keeps every event at `info` and below in
/// memory.
fn logs() -> LogBuffer {
    static LOGS: OnceLock<LogBuffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        logs
    })
    .clone()
}

#[tokio::test]
async fn a_source_with_both_path_and_embedded_uses_the_path() {
    let logs = logs();
    let scratch = Scratch::new();
    let dir = scratch.migrations(
        "migrations",
        &[("1_from_dir.sql", "CREATE TABLE from_dir (id INTEGER);")],
    );
    let embedded = EmbeddedMigrations::new().source(
        "pathwins",
        migrator(&[(
            1,
            "from embedded",
            "CREATE TABLE from_embedded (id INTEGER)",
        )]),
    );
    ContextRunner::new()
        .config_str(format!(
            "[data.sources.pathwins]\nurl = \"{}\"\n[data.sources.pathwins.migrations]\n\
             enabled = true\npath = '{}'",
            scratch.url("pathwins"),
            path(&dir)
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .configure(move |b| b.instance(embedded))
        .run(|_| {})
        .await;
    assert_eq!(scratch.tables("pathwins").await, ["from_dir"]);
    let text = logs.contents();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("pathwins") && l.contains("embedded"))
        .collect();
    assert_eq!(lines.len(), 1, "{text}");
    assert!(lines[0].contains("INFO"), "{}", lines[0]);
    let dir_text = scratch.0.path().display().to_string();
    assert!(!text.contains(&dir_text), "{text}");
    assert!(!text.contains(SQL_MARKER), "{text}");
}

#[tokio::test]
async fn connect_on_startup_false_still_migrates_an_enabled_source_and_keeps_the_others_lazy() {
    let scratch = Scratch::new();
    let dir = scratch.migrations(
        "migrations",
        &[(
            "1_create_accounts.sql",
            "CREATE TABLE accounts (id INTEGER);",
        )],
    );
    ContextRunner::new()
        .config_str(format!(
            "[data]\nconnect-on-startup = false\nurl = \"{}\"\n[data.migrations]\nenabled = true\n\
             path = '{}'\n[data.sources.lazy]\nurl = \"{}\"",
            scratch.url("app"),
            path(&dir),
            scratch.url("lazy")
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .run(|view| {
            let sources = view.get::<DataSources>().unwrap();
            assert_eq!(sources.pool_counts("lazy"), Some((0, 0)));
        })
        .await;
    assert_eq!(scratch.tables("app").await, ["accounts"]);
}

#[tokio::test]
async fn data_service_is_registered_for_a_migrating_source_with_connect_on_startup_false() {
    let scratch = Scratch::new();
    let embedded = EmbeddedMigrations::new().default_source(migrator(&[(
        1,
        "create accounts",
        "CREATE TABLE accounts (id INTEGER)",
    )]));
    ContextRunner::new()
        .config_str(format!(
            "[data]\nconnect-on-startup = false\nurl = \"{}\"\n[data.migrations]\nenabled = true",
            scratch.url("app")
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .configure(move |b| b.instance(embedded))
        .run(|view| {
            assert!(view.contains::<DataService>());
            let report = view.conditions().to_string();
            assert!(
                !report.contains("the data section configures no source"),
                "{report}"
            );
        })
        .await;
    assert_eq!(scratch.tables("app").await, ["accounts"]);
}

#[tokio::test]
async fn several_sources_migrate_in_registry_order_and_a_failure_stops_the_rest() {
    let scratch = Scratch::new();
    let good = scratch.migrations(
        "good",
        &[(
            "1_create_accounts.sql",
            "CREATE TABLE accounts (id INTEGER);",
        )],
    );
    let bad = scratch.migrations(
        "bad",
        &[
            ("1_create_totals.sql", "CREATE TABLE totals (id INTEGER);"),
            ("2_broken.sql", "SELECT SECRET_SQL_MARKER FROM nowhere;"),
        ],
    );
    let zeta = scratch.migrations(
        "zeta",
        &[("1_create_zeta.sql", "CREATE TABLE zeta (id INTEGER);")],
    );
    ContextRunner::new()
        .config_str(format!(
            "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = true\npath = '{}'\n\
             [data.sources.reports]\nurl = \"{}\"\n[data.sources.reports.migrations]\n\
             enabled = true\npath = '{}'\n\
             [data.sources.zeta]\nurl = \"{}\"\n[data.sources.zeta.migrations]\n\
             enabled = true\npath = '{}'",
            scratch.url("app"),
            path(&good),
            scratch.url("reports"),
            path(&bad),
            scratch.url("zeta"),
            path(&zeta)
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .run_failing(|view| {
            let error = view
                .error()
                .find_cause::<MigrateError>()
                .expect("a migrate error");
            assert_eq!(error.source_name(), "reports");
            assert_eq!(error.version(), Some(2));
            let report = view.error().analysis().expect("a report").to_string();
            assert!(report.contains("`reports`"), "{report}");
            assert!(report.contains("migration 2"), "{report}");
            assert_no_secrets(view, &scratch, &["2_broken"]);
        })
        .await;
    // The default source came first and was migrated; the failure stopped the run before
    // `zeta`.
    assert_eq!(scratch.tables("app").await, ["accounts"]);
    assert_eq!(scratch.tables("reports").await, ["totals"]);
    assert!(scratch.tables("zeta").await.is_empty());
}

#[tokio::test]
async fn the_data_service_runs_the_migrations_itself() {
    let scratch = Scratch::new();
    let config = oxbloom_config::Config::from_str(&format!(
        "[data]\nurl = \"{}\"\n[data.migrations]\nenabled = true",
        scratch.url("app")
    ))
    .unwrap();
    let settings = Arc::new(DataSettings::from_config(&config).unwrap());
    let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap());
    let embedded = Arc::new(EmbeddedMigrations::new().default_source(migrator(&[(
        1,
        "create accounts",
        "CREATE TABLE accounts (id INTEGER)",
    )])));
    let service = DataService::with_migrations(sources.clone(), settings, Some(embedded), false);
    service.start().await.unwrap();
    service.stop().await;
    assert_eq!(scratch.tables("app").await, ["accounts"]);
    // `new` keeps its behavior: no migrations.
    let _ = DataService::new(sources, true);
}

#[tokio::test]
async fn the_debug_of_the_data_service_hides_the_migration_path() {
    let config = oxbloom_config::Config::from_str(
        "[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true\n\
         path = \"/srv/oxbloom-secret-dir/migrations\"",
    )
    .unwrap();
    let settings = Arc::new(DataSettings::from_config(&config).unwrap());
    let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap());
    let service = DataService::with_migrations(sources, settings, None, false);
    let text = format!("{service:?}");
    assert!(text.contains("<redacted>"), "{text}");
    assert!(!text.contains("oxbloom-secret-dir"), "{text}");
}

#[test]
fn the_property_metadata_lists_the_migration_keys() {
    let keys: Vec<_> = DataAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .collect();
    assert!(keys.contains(&"data.migrations.enabled"));
    assert!(keys.contains(&"data.sources.<name>.migrations.path"));
}
