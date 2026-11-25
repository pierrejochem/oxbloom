//! Helpers shared by the migration conformance suites `data_migrate_conformance.rs` and
//! `data_migrate_conformance_boot.rs`: a source with migration settings built from the snippet
//! of a `TestDatabase`, the versions of a report, the texts that must stay free of secrets, and
//! an in-memory capture of the `tracing` events.

#![allow(dead_code)]

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use oxbloom_config::Config;
use oxbloom_data::migrate::{self, MigrateError, MigrationReport};
use oxbloom_data::sqlx::migrate::Migrator;
use oxbloom_data::{DataError, DataSettings, DataSources, MigrationSettings};
use oxbloom_test::TestDatabase;
use tracing_subscriber::fmt::MakeWriter;

/// The marker in the SQL of the log hygiene tests. It is compared without regard to case,
/// because Postgres folds unquoted names to lower case in its messages.
pub const SQL_MARKER: &str = "SECRET_MARKER_SQL";

/// The marker in the password of the unreachable source of the log hygiene tests.
pub const PASSWORD_MARKER: &str = "SECRET_MARKER_PW";

/// A registry with one source, `default`, whose migrations are enabled and read from a
/// directory, built from a TOML document as the boot does.
pub struct MigratingSource {
    settings: DataSettings,
    sources: Arc<DataSources>,
}

impl MigratingSource {
    /// The source of the test database `db` with the migrations in `dir`.
    pub fn new(db: &TestDatabase, dir: &Path) -> Self {
        Self::from_toml(&db.config_snippet_with_migrations(dir))
    }

    /// A source whose URL is `url`, with the migrations in `dir`. Building the registry does not
    /// connect, so an unreachable URL is fine.
    pub fn with_url(url: &str, dir: &Path) -> Self {
        Self::from_toml(&format!(
            "[data]\nurl = {}\nacquire-timeout = \"2s\"\n[data.migrations]\nenabled = true\n\
             path = {}\n",
            toml_str(url),
            toml_str(&dir.to_string_lossy())
        ))
    }

    fn from_toml(toml: &str) -> Self {
        let config = Config::from_str(toml).expect("the test configuration is valid TOML");
        let settings =
            DataSettings::from_config(&config).expect("the test data settings are valid");
        let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))
            .expect("the test registry builds");
        assert!(
            settings.sources[0].migrations.is_some(),
            "the test settings enable the migrations"
        );
        Self {
            settings,
            sources: Arc::new(sources),
        }
    }

    /// Changes the migration settings of the source.
    pub fn with(mut self, change: impl FnOnce(&mut MigrationSettings)) -> Self {
        change(
            self.settings.sources[0]
                .migrations
                .as_mut()
                .expect("the migrations are enabled"),
        );
        self
    }

    /// The migration settings of the source.
    pub fn migrations(&self) -> &MigrationSettings {
        self.settings.sources[0]
            .migrations
            .as_ref()
            .expect("the migrations are enabled")
    }

    /// The registry.
    pub fn sources(&self) -> &Arc<DataSources> {
        &self.sources
    }

    /// Applies the pending migrations of the directory.
    pub async fn run(&self) -> Result<MigrationReport, MigrateError> {
        migrate::run(&self.sources, "default", self.migrations(), None).await
    }

    /// Applies the pending migrations of `embedded`; the settings must name no `path`.
    pub async fn run_embedded(&self, embedded: &Migrator) -> Result<MigrationReport, MigrateError> {
        migrate::run(&self.sources, "default", self.migrations(), Some(embedded)).await
    }

    /// Reverts the migrations above `target`.
    pub async fn revert(&self, target: i64) -> Result<MigrationReport, MigrateError> {
        migrate::revert(&self.sources, "default", self.migrations(), None, target).await
    }

    /// Closes the pools of the registry.
    pub async fn close(&self) {
        self.sources.close_all().await;
    }
}

/// The text as a TOML basic string, with the quotes.
pub fn toml_str(text: &str) -> String {
    let mut out = String::from('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The versions of a report, in its order.
pub fn versions(report: &MigrationReport) -> Vec<i64> {
    report.applied().iter().map(|m| m.version).collect()
}

/// The parts of a database URL that must never show: the whole URL and, when it has one, the
/// password with its delimiters (`:password@`). The password alone can be a common word that
/// also appears in names, so it is not checked on its own.
pub fn url_secrets(url: &str) -> Vec<String> {
    let mut secrets = vec![url.to_owned()];
    if let Some((_, rest)) = url.split_once("://")
        && let Some((user_info, _)) = rest.split_once('@')
        && let Some((_, password)) = user_info.split_once(':')
    {
        secrets.push(format!(":{password}@"));
    }
    secrets
}

/// Asserts that no text contains any of the secrets, compared without regard to case. The
/// message names the secret and the label of the text, never the text, which would show what
/// leaked.
pub fn assert_free_of(texts: &[(&str, String)], secrets: &[&str]) {
    for (label, text) in texts {
        let text = text.to_lowercase();
        for (index, secret) in secrets.iter().enumerate() {
            assert!(
                !text.contains(&secret.to_lowercase()),
                "secret number {index} of the list leaked into the {label}"
            );
        }
    }
}

/// The `Display` and `Debug` text of a migration error, labelled. The `source` of the error is
/// left out on purpose: it is the database error, documented to hold SQL fragments.
pub fn error_texts(error: &MigrateError) -> Vec<(&'static str, String)> {
    vec![
        ("error display", error.to_string()),
        ("error debug", format!("{error:?}")),
    ]
}

/// An in-memory sink for the `tracing` events of the test binary.
#[derive(Clone, Default)]
pub struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl LogBuffer {
    /// Everything written so far.
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

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

/// Installs, once per test binary, a global subscriber that keeps every event at `debug` and
/// above in memory, with its fields. The events of the `sqlx` worker threads need a global
/// subscriber.
pub fn logs() -> LogBuffer {
    static LOGS: OnceLock<LogBuffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .expect("no other global subscriber in this test binary");
        logs
    })
    .clone()
}
