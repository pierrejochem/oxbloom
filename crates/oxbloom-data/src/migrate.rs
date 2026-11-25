//! Schema migrations for the data sources of `oxbloom-data`, run with `sqlx::migrate`.
//!
//! This module is only available with the cargo feature `migrate`. [`run`] applies the pending
//! migrations of one source, read from the directory of [`MigrationSettings::path`] or taken
//! from an embedded [`Migrator`] (registered per source in [`EmbeddedMigrations`]), and
//! returns a [`MigrationReport`] with the versions it applied. The files and the history table
//! are those of `sqlx`, so an application can move between Oxbloom and `sqlx-cli` without a
//! conversion.
//!
//! Every [`MigrateError`] has fixed text that names the source, and the version and
//! description where a single migration is at fault. It never contains SQL, file contents, a
//! file path or the connection URL, because the text of a `sqlx` error can contain all of
//! them.
//!
//! # Examples
//!
//! ```
//! # #[cfg(feature = "sqlite")]
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use oxbloom_config::Config;
//! use oxbloom_data::sqlx::SqlSafeStr;
//! use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
//! use oxbloom_data::{DataError, DataSettings, DataSources, migrate};
//!
//! let config = Config::from_str(
//!     "[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true",
//! )?;
//! let settings = DataSettings::from_config(&config)?;
//! let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
//!
//! // An application embeds its files with `sqlx::migrate!("./migrations")`.
//! let migrator = Migrator::with_migrations(vec![Migration::new(
//!     1,
//!     "create accounts".into(),
//!     MigrationType::Simple,
//!     "CREATE TABLE accounts (id INTEGER PRIMARY KEY)".into_sql_str(),
//!     false,
//! )]);
//! let migrations = settings.sources[0].migrations.as_ref().ok_or("no migrations")?;
//! let report = migrate::run(&sources, "default", migrations, Some(&migrator)).await?;
//! assert_eq!(report.to_string(), "applied 1 migration: 1 (create accounts)");
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "sqlite"))]
//! # fn main() {}
//! ```

use std::collections::BTreeMap;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::collections::BTreeSet;
use std::fmt;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use sqlx::migrate::MigrateError as SqlxMigrateError;
use sqlx::migrate::Migrator;

use crate::settings::{DEFAULT_SOURCE, is_valid_table_name};
use crate::{DataSources, Driver, MigrationSettings};

/// The embedded migrators of the data sources, registered by an application as a bean.
///
/// Each source maps to a [`Migrator`] that lives for `'static`, which is what
/// `sqlx::migrate!("./migrations")` gives. The macro needs a direct dependency named `sqlx`
/// with the features `macros` and `migrate`; going through `oxbloom_data::sqlx` alone is not
/// enough. A source whose settings name a `path` uses that directory instead.
///
/// The `Debug` output lists the source names and the versions of their up migrations, never
/// SQL.
///
/// # Examples
///
/// ```
/// use oxbloom_data::migrate::EmbeddedMigrations;
/// use oxbloom_data::sqlx::migrate::Migrator;
///
/// // In an application: `static MIGRATOR: Migrator = sqlx::migrate!("./migrations");`
/// let migrator: &'static Migrator = Box::leak(Box::new(Migrator::with_migrations(Vec::new())));
/// let embedded = EmbeddedMigrations::new().default_source(migrator);
/// assert!(embedded.get("default").is_some());
/// assert_eq!(format!("{embedded:?}"), "EmbeddedMigrations { default: [] }");
/// ```
#[derive(Clone, Default)]
pub struct EmbeddedMigrations {
    migrators: BTreeMap<String, &'static Migrator>,
}

impl EmbeddedMigrations {
    /// An empty set: no source has an embedded migrator.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::migrate::EmbeddedMigrations;
    ///
    /// assert!(EmbeddedMigrations::new().get("default").is_none());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `migrator` for the source `default`, the one of the unnamed `data.*` keys.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::migrate::EmbeddedMigrations;
    /// use oxbloom_data::sqlx::migrate::Migrator;
    ///
    /// let migrator: &'static Migrator = Box::leak(Box::new(Migrator::with_migrations(Vec::new())));
    /// let embedded = EmbeddedMigrations::new().default_source(migrator);
    /// assert!(std::ptr::eq(embedded.get("default").unwrap(), migrator));
    /// ```
    #[must_use]
    pub fn default_source(self, migrator: &'static Migrator) -> Self {
        self.source(DEFAULT_SOURCE, migrator)
    }

    /// Registers `migrator` for the source `name`, the name under `data.sources`. A second
    /// registration for the same name replaces the first.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::migrate::EmbeddedMigrations;
    /// use oxbloom_data::sqlx::migrate::Migrator;
    ///
    /// let migrator: &'static Migrator = Box::leak(Box::new(Migrator::with_migrations(Vec::new())));
    /// let embedded = EmbeddedMigrations::new().source("reports", migrator);
    /// assert!(embedded.get("reports").is_some());
    /// assert!(embedded.get("default").is_none());
    /// ```
    #[must_use]
    pub fn source(mut self, name: &str, migrator: &'static Migrator) -> Self {
        self.migrators.insert(name.to_string(), migrator);
        self
    }

    /// The migrator registered for the source `name`, or `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::migrate::EmbeddedMigrations;
    /// use oxbloom_data::sqlx::migrate::Migrator;
    ///
    /// let migrator: &'static Migrator = Box::leak(Box::new(Migrator::with_migrations(Vec::new())));
    /// let embedded = EmbeddedMigrations::new().source("cache", migrator);
    /// assert!(embedded.get("cache").is_some());
    /// assert!(embedded.get("orders").is_none());
    /// ```
    pub fn get(&self, name: &str) -> Option<&'static Migrator> {
        self.migrators.get(name).copied()
    }
}

impl fmt::Debug for EmbeddedMigrations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("EmbeddedMigrations");
        for (name, migrator) in &self.migrators {
            debug.field(name, &up_versions(migrator));
        }
        debug.finish()
    }
}

/// The versions of the up migrations of `migrator`, in order.
fn up_versions(migrator: &Migrator) -> Vec<i64> {
    migrator
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
        .map(|m| m.version)
        .collect()
}

/// One migration that a [`run`] applied: its version and its description (the file name
/// without the version and the extension, with underscores as spaces).
///
/// # Examples
///
/// ```
/// use oxbloom_data::migrate::AppliedMigration;
///
/// let applied = AppliedMigration { version: 1, description: "create accounts".to_string() };
/// assert_eq!(applied.version, 1);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AppliedMigration {
    /// The version, the number at the start of the file name.
    pub version: i64,
    /// The description of the migration.
    pub description: String,
}

/// The migrations one [`run`] applied, in the order of their versions.
///
/// The versions are the difference between the history table before and after the run. With
/// several runners on one database a report can name a migration that another runner applied
/// at the same time. The `Debug` output shows the versions only; the `Display` text lists the
/// versions with their descriptions.
///
/// # Examples
///
/// ```
/// use oxbloom_data::migrate::MigrationReport;
///
/// let report = MigrationReport::default();
/// assert!(report.is_empty());
/// assert_eq!(report.to_string(), "applied no migrations");
/// assert_eq!(format!("{report:?}"), "MigrationReport { applied: [] }");
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    applied: Vec<AppliedMigration>,
}

impl MigrationReport {
    /// The applied migrations, in the order of their versions.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::sqlx::SqlSafeStr;
    /// use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
    /// use oxbloom_data::{DataError, DataSettings, DataSources, migrate};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
    /// let migration = |version, description: &'static str, sql: &'static str| {
    ///     Migration::new(version, description.into(), MigrationType::Simple, sql.into_sql_str(), false)
    /// };
    /// let migrator = Migrator::with_migrations(vec![
    ///     migration(2, "create orders", "CREATE TABLE orders (id INTEGER)"),
    ///     migration(1, "create accounts", "CREATE TABLE accounts (id INTEGER)"),
    /// ]);
    /// let migrations = settings.sources[0].migrations.as_ref().ok_or("no migrations")?;
    /// let report = migrate::run(&sources, "default", migrations, Some(&migrator)).await?;
    /// let versions: Vec<i64> = report.applied().iter().map(|m| m.version).collect();
    /// assert_eq!(versions, [1, 2]);
    /// assert_eq!(report.applied()[0].description, "create accounts");
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn applied(&self) -> &[AppliedMigration] {
        &self.applied
    }

    /// Whether the run applied no migration.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::sqlx::SqlSafeStr;
    /// use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
    /// use oxbloom_data::{DataError, DataSettings, DataSources, migrate};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
    /// let migrator = Migrator::with_migrations(vec![Migration::new(
    ///     1, "create accounts".into(), MigrationType::Simple,
    ///     "CREATE TABLE accounts (id INTEGER)".into_sql_str(), false,
    /// )]);
    /// let migrations = settings.sources[0].migrations.as_ref().ok_or("no migrations")?;
    /// assert!(!migrate::run(&sources, "default", migrations, Some(&migrator)).await?.is_empty());
    /// // The second run finds everything applied.
    /// assert!(migrate::run(&sources, "default", migrations, Some(&migrator)).await?.is_empty());
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty()
    }

    /// The highest version the run applied, or `None` when it applied nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::sqlx::SqlSafeStr;
    /// use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
    /// use oxbloom_data::{DataError, DataSettings, DataSources, migrate};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
    /// let migrator = Migrator::with_migrations(vec![Migration::new(
    ///     20261125120000, "create accounts".into(), MigrationType::Simple,
    ///     "CREATE TABLE accounts (id INTEGER)".into_sql_str(), false,
    /// )]);
    /// let migrations = settings.sources[0].migrations.as_ref().ok_or("no migrations")?;
    /// let report = migrate::run(&sources, "default", migrations, Some(&migrator)).await?;
    /// assert_eq!(report.latest_version(), Some(20261125120000));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn latest_version(&self) -> Option<i64> {
        self.applied.last().map(|m| m.version)
    }
}

impl fmt::Debug for MigrationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let versions: Vec<i64> = self.applied.iter().map(|m| m.version).collect();
        f.debug_struct("MigrationReport")
            .field("applied", &versions)
            .finish()
    }
}

impl fmt::Display for MigrationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.applied.len() {
            0 => return f.write_str("applied no migrations"),
            1 => f.write_str("applied 1 migration: ")?,
            n => write!(f, "applied {n} migrations: ")?,
        }
        for (i, m) in self.applied.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{} ({})", m.version, m.description)?;
        }
        Ok(())
    }
}

/// Why the migrations of a source could not be loaded, in [`MigrateError::Load`].
///
/// # Examples
///
/// ```
/// use oxbloom_data::migrate::{LoadReason, MigrateError};
///
/// let err = MigrateError::Load { source_name: "default".into(), reason: LoadReason::Empty };
/// assert_eq!(
///     err.to_string(),
///     "data source `default`: the migration directory holds no migrations"
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LoadReason {
    /// The settings name no `path` and there is no embedded migrator for the source.
    NotConfigured,
    /// The `path` is empty, or the directory is missing or cannot be read.
    Unreadable,
    /// The directory holds no migration files.
    Empty,
    /// The name of the history table is not `[A-Za-z_][A-Za-z0-9_]*` of at most 63
    /// characters. It is placed in SQL, so it is refused before anything runs.
    InvalidTable,
}

/// The failure of a [`run`].
///
/// The `Display` text is fixed per variant apart from the source name, the version and the
/// description; it never contains SQL, file contents, a file path or the URL. The `Debug`
/// output shows the same fields and leaves the database error out. The database error of
/// [`Script`](Self::Script) and [`Database`](Self::Database) stays available through
/// [`source`](std::error::Error::source); its text can contain SQL object names or
/// statement fragments, so do not print it into a log or a report.
///
/// # Examples
///
/// ```
/// use oxbloom_data::migrate::MigrateError;
///
/// let err = MigrateError::ChecksumMismatch {
///     source_name: "default".into(),
///     version: 2,
///     description: "index accounts".into(),
/// };
/// assert_eq!(
///     err.to_string(),
///     "data source `default`: migration 2 (index accounts) was applied, but its file has changed since"
/// );
/// assert_eq!(err.version(), Some(2));
/// ```
#[non_exhaustive]
pub enum MigrateError {
    /// The migrations could not be loaded; nothing ran.
    Load {
        /// The source.
        source_name: String,
        /// Why loading failed.
        reason: LoadReason,
    },
    /// The history records a migration that started and did not finish (the `sqlx` dirty
    /// state). On MySQL a script failed part way; elsewhere a transaction was interrupted, for
    /// example by a crash of the server. The database needs a repair first.
    Dirty {
        /// The source.
        source_name: String,
        /// The version of the unfinished migration.
        version: i64,
        /// Its description, when the migrations still contain that version.
        description: Option<String>,
    },
    /// An applied migration's file has changed since it was applied.
    ChecksumMismatch {
        /// The source.
        source_name: String,
        /// The version of the changed migration.
        version: i64,
        /// Its description.
        description: String,
    },
    /// A migration is recorded as applied but has no file (and `ignore-missing` is false).
    /// The history keeps no description that `sqlx` reads, so there is none here.
    MissingFile {
        /// The source.
        source_name: String,
        /// The version of the missing migration.
        version: i64,
    },
    /// A migration script failed. On Postgres and SQLite it was rolled back; on MySQL it may
    /// be partly applied.
    Script {
        /// The source.
        source_name: String,
        /// The version of the failed migration.
        version: i64,
        /// Its description.
        description: String,
        /// The database error, kept for [`source`](std::error::Error::source) only.
        cause: sqlx::Error,
    },
    /// The run did not finish within `timeout`.
    Timeout {
        /// The source.
        source_name: String,
    },
    /// There is no source of that name.
    UnknownSource {
        /// The name that was asked for.
        source_name: String,
    },
    /// The driver feature of the source is off in this build.
    DriverDisabled {
        /// The source.
        source_name: String,
    },
    /// The database failed outside a migration script, for example while connecting or while
    /// reading or writing the history table.
    Database {
        /// The source.
        source_name: String,
        /// The database error, when there is one, kept for
        /// [`source`](std::error::Error::source) only.
        cause: Option<sqlx::Error>,
    },
}

impl MigrateError {
    /// The name of the source the error concerns.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::migrate::MigrateError;
    ///
    /// let err = MigrateError::Timeout { source_name: "reports".into() };
    /// assert_eq!(err.source_name(), "reports");
    /// ```
    pub fn source_name(&self) -> &str {
        match self {
            Self::Load { source_name, .. }
            | Self::Dirty { source_name, .. }
            | Self::ChecksumMismatch { source_name, .. }
            | Self::MissingFile { source_name, .. }
            | Self::Script { source_name, .. }
            | Self::Timeout { source_name }
            | Self::UnknownSource { source_name }
            | Self::DriverDisabled { source_name }
            | Self::Database { source_name, .. } => source_name,
        }
    }

    /// The version of the migration the error concerns, or `None` when it concerns no single
    /// migration.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::migrate::MigrateError;
    ///
    /// let err = MigrateError::MissingFile { source_name: "default".into(), version: 3 };
    /// assert_eq!(err.version(), Some(3));
    /// assert_eq!(MigrateError::Timeout { source_name: "default".into() }.version(), None);
    /// ```
    pub fn version(&self) -> Option<i64> {
        match self {
            Self::Dirty { version, .. }
            | Self::ChecksumMismatch { version, .. }
            | Self::MissingFile { version, .. }
            | Self::Script { version, .. } => Some(*version),
            Self::Load { .. }
            | Self::Timeout { .. }
            | Self::UnknownSource { .. }
            | Self::DriverDisabled { .. }
            | Self::Database { .. } => None,
        }
    }
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSource { source_name } => {
                return write!(f, "data source `{source_name}` does not exist");
            }
            _ => write!(f, "data source `{}`: ", self.source_name())?,
        }
        match self {
            Self::Load { reason, .. } => f.write_str(match reason {
                LoadReason::NotConfigured => {
                    "there are no migrations to run; set `migrations.path` or register \
                     `EmbeddedMigrations` for the source"
                }
                LoadReason::Unreadable => "the migration directory could not be read",
                LoadReason::Empty => "the migration directory holds no migrations",
                LoadReason::InvalidTable => "the name of the migration history table is invalid",
            }),
            Self::Dirty {
                version,
                description,
                ..
            } => {
                write!(f, "migration {version}")?;
                if let Some(description) = description {
                    write!(f, " ({description})")?;
                }
                f.write_str(
                    " is partially applied; repair the database and remove its row from the \
                     history table",
                )
            }
            Self::ChecksumMismatch {
                version,
                description,
                ..
            } => write!(
                f,
                "migration {version} ({description}) was applied, but its file has changed since"
            ),
            Self::MissingFile { version, .. } => {
                write!(
                    f,
                    "migration {version} was applied, but its file is missing"
                )
            }
            Self::Script {
                version,
                description,
                ..
            } => write!(f, "migration {version} ({description}) failed"),
            Self::Timeout { .. } => f.write_str("the migrations did not finish within the timeout"),
            Self::DriverDisabled { .. } => {
                f.write_str("the database driver is not available in this build")
            }
            Self::Database { .. } => f.write_str("the database failed while the migrations ran"),
            Self::UnknownSource { .. } => Ok(()),
        }
    }
}

/// Written by hand so that the database error never shows.
impl fmt::Debug for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load {
                source_name,
                reason,
            } => f
                .debug_struct("Load")
                .field("source_name", source_name)
                .field("reason", reason)
                .finish(),
            Self::Dirty {
                source_name,
                version,
                description,
            } => f
                .debug_struct("Dirty")
                .field("source_name", source_name)
                .field("version", version)
                .field("description", description)
                .finish(),
            Self::ChecksumMismatch {
                source_name,
                version,
                description,
            } => f
                .debug_struct("ChecksumMismatch")
                .field("source_name", source_name)
                .field("version", version)
                .field("description", description)
                .finish(),
            Self::MissingFile {
                source_name,
                version,
            } => f
                .debug_struct("MissingFile")
                .field("source_name", source_name)
                .field("version", version)
                .finish(),
            Self::Script {
                source_name,
                version,
                description,
                ..
            } => f
                .debug_struct("Script")
                .field("source_name", source_name)
                .field("version", version)
                .field("description", description)
                .finish_non_exhaustive(),
            Self::Timeout { source_name } => f
                .debug_struct("Timeout")
                .field("source_name", source_name)
                .finish(),
            Self::UnknownSource { source_name } => f
                .debug_struct("UnknownSource")
                .field("source_name", source_name)
                .finish(),
            Self::DriverDisabled { source_name } => f
                .debug_struct("DriverDisabled")
                .field("source_name", source_name)
                .finish(),
            Self::Database { source_name, .. } => f
                .debug_struct("Database")
                .field("source_name", source_name)
                .finish_non_exhaustive(),
        }
    }
}

impl std::error::Error for MigrateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Script { cause, .. } => Some(cause),
            Self::Database {
                cause: Some(cause), ..
            } => Some(cause),
            _ => None,
        }
    }
}

/// Applies the pending migrations of the source `name` and reports which ones it applied.
///
/// The migrations come from the directory `settings.path` when it is set, and otherwise from
/// `embedded`; a source with both uses the directory. The settings' history `table`,
/// `ignore_missing` and `locking` are applied to the migrator (an embedded migrator's own
/// settings are not used), and the whole run, including loading the files and the wait for
/// the database lock, is bounded by `settings.timeout`. A migration that is recorded as
/// applied is checked against its file; a changed file always fails the run.
///
/// The `sqlx` lock is a no-op on SQLite, so two runners on one SQLite file can race, and the
/// loser fails, for example with "table already exists". On a SQLite source the run is
/// therefore repeated once after any failure other than a dirty history or a changed file;
/// the second run sees what the winner applied, and a script that really fails fails again.
/// On Postgres and MySQL the run uses a connection of its own, which is closed afterwards, so
/// a database lock that `sqlx` leaves behind after a failure is released with it.
///
/// # Errors
///
/// - [`MigrateError::UnknownSource`] when there is no source `name`, and
///   [`MigrateError::DriverDisabled`] when its driver feature is off.
/// - [`MigrateError::Load`] when the table name is invalid, `path` is empty, missing,
///   unreadable or holds no migrations, or there is neither a `path` nor `embedded`.
/// - [`MigrateError::Dirty`], [`MigrateError::ChecksumMismatch`],
///   [`MigrateError::MissingFile`] and [`MigrateError::Script`] for the migration at fault.
/// - [`MigrateError::Timeout`] when the run takes longer than `settings.timeout`.
/// - [`MigrateError::Database`] for any other database failure.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_config::Config;
/// use oxbloom_data::migrate::{self, LoadReason, MigrateError};
/// use oxbloom_data::sqlx::{self, SqlSafeStr};
/// use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
/// use oxbloom_data::{DataError, DataSettings, DataSources};
///
/// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true")?;
/// let settings = DataSettings::from_config(&config)?;
/// let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
/// let migrations = settings.sources[0].migrations.as_ref().ok_or("no migrations")?;
///
/// let migrator = Migrator::with_migrations(vec![Migration::new(
///     1, "create accounts".into(), MigrationType::Simple,
///     "CREATE TABLE accounts (id INTEGER PRIMARY KEY)".into_sql_str(), false,
/// )]);
/// let report = migrate::run(&sources, "default", migrations, Some(&migrator)).await?;
/// assert_eq!(report.latest_version(), Some(1));
/// let pool = sources.sqlite("default").ok_or("no default")?;
/// let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts").fetch_one(pool).await?;
/// assert_eq!(rows, 0);
///
/// // Without a `path` and without an embedded migrator there is nothing to run.
/// let err = migrate::run(&sources, "default", migrations, None).await.unwrap_err();
/// assert!(matches!(err, MigrateError::Load { reason: LoadReason::NotConfigured, .. }));
/// let err = migrate::run(&sources, "reports", migrations, Some(&migrator)).await.unwrap_err();
/// assert_eq!(err.to_string(), "data source `reports` does not exist");
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
pub async fn run(
    sources: &DataSources,
    name: &str,
    settings: &MigrationSettings,
    embedded: Option<&Migrator>,
) -> Result<MigrationReport, MigrateError> {
    let Some(driver) = sources.driver_of(name) else {
        return Err(MigrateError::UnknownSource {
            source_name: name.to_string(),
        });
    };
    if !is_valid_table_name(&settings.table) {
        return Err(load_error(name, LoadReason::InvalidTable));
    }
    let work = async {
        let migrator = build_migrator(name, settings, embedded).await?;
        dispatch(sources, name, driver, &migrator, settings).await
    };
    tokio::time::timeout(settings.timeout, work)
        .await
        .unwrap_or_else(|_| {
            Err(MigrateError::Timeout {
                source_name: name.to_string(),
            })
        })
}

fn load_error(name: &str, reason: LoadReason) -> MigrateError {
    MigrateError::Load {
        source_name: name.to_string(),
        reason,
    }
}

/// The migrator of the source: from the directory when `path` is set, else a copy of the
/// embedded one, with the settings applied.
async fn build_migrator(
    name: &str,
    settings: &MigrationSettings,
    embedded: Option<&Migrator>,
) -> Result<Migrator, MigrateError> {
    let mut migrator = match (&settings.path, embedded) {
        (Some(path), _) => {
            if path.as_os_str().is_empty() {
                return Err(load_error(name, LoadReason::Unreadable));
            }
            // The error of sqlx names the path and an OS error, so it is dropped here.
            let migrator = Migrator::new(path.as_path())
                .await
                .map_err(|_| load_error(name, LoadReason::Unreadable))?;
            if migrator.iter().next().is_none() {
                return Err(load_error(name, LoadReason::Empty));
            }
            migrator
        }
        (None, Some(embedded)) => Migrator::with_migrations(embedded.iter().cloned().collect()),
        (None, None) => return Err(load_error(name, LoadReason::NotConfigured)),
    };
    migrator.dangerous_set_table_name(settings.table.clone());
    migrator
        .set_ignore_missing(settings.ignore_missing)
        .set_locking(settings.locking);
    Ok(migrator)
}

/// Runs `migrator` on the pool of the source's driver.
async fn dispatch(
    sources: &DataSources,
    name: &str,
    driver: Driver,
    migrator: &Migrator,
    settings: &MigrationSettings,
) -> Result<MigrationReport, MigrateError> {
    let unknown = || MigrateError::UnknownSource {
        source_name: name.to_string(),
    };
    match driver {
        #[cfg(feature = "postgres")]
        Driver::Postgres => {
            let pool = sources.postgres(name).ok_or_else(unknown)?;
            migrate_pool(pool, name, migrator, settings).await
        }
        #[cfg(feature = "mysql")]
        Driver::Mysql => {
            let pool = sources.mysql(name).ok_or_else(unknown)?;
            migrate_pool(pool, name, migrator, settings).await
        }
        #[cfg(feature = "sqlite")]
        Driver::Sqlite => {
            let pool = sources.sqlite(name).ok_or_else(unknown)?;
            migrate_pool(pool, name, migrator, settings).await
        }
        #[allow(unreachable_patterns)]
        _ => {
            let _ = (sources, migrator, settings, unknown);
            Err(MigrateError::DriverDisabled {
                source_name: name.to_string(),
            })
        }
    }
}

/// Runs the migrator on `pool`, once more on SQLite after a failure that a second run can
/// resolve, and maps the outcome.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
async fn migrate_pool<DB: private::MigrateDatabase>(
    pool: &sqlx::Pool<DB>,
    name: &str,
    migrator: &Migrator,
    settings: &MigrationSettings,
) -> Result<MigrationReport, MigrateError> {
    let applied = match attempt(pool, migrator, settings).await {
        Err(error)
            if DB::RUN_TWICE_ON_FAILURE
                && !matches!(
                    error,
                    SqlxMigrateError::Dirty(_) | SqlxMigrateError::VersionMismatch(_)
                ) =>
        {
            attempt(pool, migrator, settings).await
        }
        other => other,
    };
    match applied {
        Ok(versions) => Ok(report(migrator, &versions)),
        Err(error) => Err(map_error(name, migrator, error)),
    }
}

/// One run on one connection of `pool`.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
async fn attempt<DB: private::MigrateDatabase>(
    pool: &sqlx::Pool<DB>,
    migrator: &Migrator,
    settings: &MigrationSettings,
) -> Result<BTreeSet<i64>, SqlxMigrateError> {
    let mut conn = pool.acquire().await?;
    if DB::CLOSE_AFTER_RUN {
        // Ends the session after the run, even when it fails or is cancelled, so a database
        // lock that sqlx did not release (it does not after a failure) goes with it.
        conn.close_on_drop();
    }
    DB::attempt_on(migrator, &mut *conn, &settings.table, settings.locking).await
}

/// Runs `migrator` on `conn` as `Migrator::run` does, and returns the versions that are in
/// the history afterwards and were not before.
///
/// `Migrator::run` on a connection does not compile as a `Send` future ("implementation of
/// `Acquire` is not general enough"), so this calls `run_direct`, the public method (hidden
/// from the docs) that `sqlx` keeps for that case and that `run` itself calls with
/// `(None, conn, false)`.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
async fn newly_applied<C: sqlx::migrate::Migrate>(
    migrator: &Migrator,
    conn: &mut C,
    table: &str,
) -> Result<BTreeSet<i64>, SqlxMigrateError> {
    // The table does not exist before the first run; that reads as an empty history.
    let before: BTreeSet<i64> = conn
        .list_applied_migrations(table)
        .await
        .map(|applied| applied.iter().map(|m| m.version).collect())
        .unwrap_or_default();
    migrator.run_direct(None, conn, false).await?;
    let after = conn.list_applied_migrations(table).await?;
    Ok(after
        .iter()
        .map(|m| m.version)
        .filter(|v| !before.contains(v))
        .collect())
}

/// The SQLite run. The `sqlx` lock is a no-op on SQLite, so with `locking` the run takes
/// SQLite's write lock itself: it runs inside a `BEGIN IMMEDIATE` transaction, and each
/// migration's own transaction becomes a savepoint in it. A second runner waits for the lock
/// (up to the busy timeout of the connection) and then finds everything applied. The
/// transaction is committed after a failure as well, so the migrations before the failing one
/// stay applied, as they do without it. A migration marked `-- no-transaction` cannot run
/// inside a transaction, so a migrator that has one runs without this lock.
#[cfg(feature = "sqlite")]
async fn sqlite_attempt(
    migrator: &Migrator,
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    locking: bool,
) -> Result<BTreeSet<i64>, SqlxMigrateError> {
    use sqlx::Connection;

    if !locking || migrator.iter().any(|m| m.no_tx) {
        return newly_applied(migrator, conn, table).await;
    }
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    let result = newly_applied(migrator, &mut *tx, table).await;
    let committed = tx.commit().await;
    let applied = result?;
    committed?;
    Ok(applied)
}

/// The report of the newly applied `versions`, with the descriptions of the migrator.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn report(migrator: &Migrator, versions: &BTreeSet<i64>) -> MigrationReport {
    let applied = migrator
        .iter()
        .filter(|m| !m.migration_type.is_down_migration() && versions.contains(&m.version))
        .map(|m| AppliedMigration {
            version: m.version,
            description: m.description.to_string(),
        })
        .collect();
    MigrationReport { applied }
}

/// The description of the up migration `version`, if the migrator has it.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn description(migrator: &Migrator, version: i64) -> Option<String> {
    migrator
        .iter()
        .find(|m| m.version == version && !m.migration_type.is_down_migration())
        .map(|m| m.description.to_string())
}

/// Maps an error of sqlx to the fixed-text [`MigrateError`]; the text of sqlx is never kept
/// for display.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn map_error(name: &str, migrator: &Migrator, error: SqlxMigrateError) -> MigrateError {
    let source_name = name.to_string();
    match error {
        SqlxMigrateError::Dirty(version) => MigrateError::Dirty {
            source_name,
            version,
            description: description(migrator, version),
        },
        SqlxMigrateError::VersionMismatch(version) => MigrateError::ChecksumMismatch {
            source_name,
            version,
            description: description(migrator, version).unwrap_or_default(),
        },
        SqlxMigrateError::VersionMissing(version) => MigrateError::MissingFile {
            source_name,
            version,
        },
        SqlxMigrateError::ExecuteMigration(cause, version) => MigrateError::Script {
            source_name,
            version,
            description: description(migrator, version).unwrap_or_default(),
            cause,
        },
        SqlxMigrateError::Execute(cause) => MigrateError::Database {
            source_name,
            cause: Some(cause),
        },
        _ => MigrateError::Database {
            source_name,
            cause: None,
        },
    }
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
mod private {
    use std::collections::BTreeSet;
    use std::future::Future;
    use std::pin::Pin;

    use super::{Migrator, SqlxMigrateError};

    /// The future of one run: the newly applied versions.
    pub type Attempt<'c> =
        Pin<Box<dyn Future<Output = Result<BTreeSet<i64>, SqlxMigrateError>> + Send + 'c>>;

    /// The per-driver part of a run, implemented for the enabled drivers only. On Postgres
    /// and MySQL `locking` is applied by `sqlx` through the migrator.
    pub trait MigrateDatabase: sqlx::Database {
        /// Whether a failed run is repeated once (SQLite, whose sqlx lock is a no-op).
        const RUN_TWICE_ON_FAILURE: bool;
        /// Whether the connection of a run is closed afterwards instead of returned to the
        /// pool. Not on SQLite, where closing the last connection of an in-memory database
        /// would drop the database.
        const CLOSE_AFTER_RUN: bool;

        /// One run of `migrator` on `conn`; returns the newly applied versions.
        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut Self::Connection,
            table: &'c str,
            locking: bool,
        ) -> Attempt<'c>;
    }

    #[cfg(feature = "postgres")]
    impl MigrateDatabase for sqlx::Postgres {
        const RUN_TWICE_ON_FAILURE: bool = false;
        const CLOSE_AFTER_RUN: bool = true;

        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut sqlx::PgConnection,
            table: &'c str,
            _locking: bool,
        ) -> Attempt<'c> {
            Box::pin(super::newly_applied(migrator, conn, table))
        }
    }

    #[cfg(feature = "mysql")]
    impl MigrateDatabase for sqlx::MySql {
        const RUN_TWICE_ON_FAILURE: bool = false;
        const CLOSE_AFTER_RUN: bool = true;

        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut sqlx::MySqlConnection,
            table: &'c str,
            _locking: bool,
        ) -> Attempt<'c> {
            Box::pin(super::newly_applied(migrator, conn, table))
        }
    }

    #[cfg(feature = "sqlite")]
    impl MigrateDatabase for sqlx::Sqlite {
        const RUN_TWICE_ON_FAILURE: bool = true;
        const CLOSE_AFTER_RUN: bool = false;

        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut sqlx::SqliteConnection,
            table: &'c str,
            locking: bool,
        ) -> Attempt<'c> {
            Box::pin(super::sqlite_attempt(migrator, conn, table, locking))
        }
    }
}
