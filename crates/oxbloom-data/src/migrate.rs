//! Schema migrations for the data sources of `oxbloom-data`, run with `sqlx::migrate`.
//!
//! This module is only available with the cargo feature `migrate`. [`run`] applies the pending
//! migrations of one source, read from the directory of [`MigrationSettings::path`] or taken
//! from an embedded [`Migrator`] (registered per source in [`EmbeddedMigrations`]), and
//! returns a [`MigrationReport`] with the versions it applied. [`revert`] goes back to a target
//! version with the down files. The files and the history table
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

/// The migrations one [`run`] applied, in the order of their versions, or the migrations one
/// [`revert`] reverted, highest version first.
///
/// The versions are the difference between the history table before and after the run. With
/// several runners on one database a report can name a migration that another runner applied
/// at the same time. The `Debug` output shows the versions only; the `Display` text lists the
/// versions with their descriptions and starts with `applied` or `reverted`.
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
    /// Whether the report is the result of a revert.
    reverted: bool,
}

impl MigrationReport {
    /// The applied migrations, in the order of their versions; for the report of a
    /// [`revert`], the reverted migrations, highest version first.
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

    /// The highest version the run applied or the revert reverted, or `None` when it changed
    /// nothing.
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
        self.applied.iter().map(|m| m.version).max()
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
        let verb = if self.reverted { "reverted" } else { "applied" };
        match self.applied.len() {
            0 => return write!(f, "{verb} no migrations"),
            1 => write!(f, "{verb} 1 migration: ")?,
            n => write!(f, "{verb} {n} migrations: ")?,
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

/// The failure of a [`run`] or a [`revert`].
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
    /// A migration that has to be reverted has no down file, so [`revert`] reverted nothing.
    NotReversible {
        /// The source.
        source_name: String,
        /// The version of the migration without a down file; the highest one when several
        /// have none.
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
    /// The source is an in-memory SQLite database with a private cache. The migrations run on a
    /// connection of their own, which would see a different, empty database, so nothing ran.
    PrivateMemory {
        /// The source.
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
            | Self::NotReversible { source_name, .. }
            | Self::Script { source_name, .. }
            | Self::Timeout { source_name }
            | Self::PrivateMemory { source_name }
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
            | Self::NotReversible { version, .. }
            | Self::Script { version, .. } => Some(*version),
            Self::Load { .. }
            | Self::Timeout { .. }
            | Self::PrivateMemory { .. }
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
            Self::NotReversible { version, .. } => write!(
                f,
                "migration {version} has no down file, so it cannot be reverted"
            ),
            Self::Script {
                version,
                description,
                ..
            } => write!(f, "migration {version} ({description}) failed"),
            Self::Timeout { .. } => f.write_str("the migrations did not finish within the timeout"),
            Self::PrivateMemory { .. } => f.write_str(
                "an in-memory SQLite database with a private cache cannot be migrated; use \
                 `cache=shared` or a database file",
            ),
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
            Self::NotReversible {
                source_name,
                version,
            } => f
                .debug_struct("NotReversible")
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
            Self::PrivateMemory { source_name } => f
                .debug_struct("PrivateMemory")
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
///
/// The work runs on a connection of its own, opened with the connect options of the pool and
/// with `sqlx` statement logging off, so no migration SQL reaches the `tracing` output through
/// the `sqlx::query` events. Because the connection is not one of the pool, an in-memory SQLite
/// database with a private cache (`cache=private`) is refused with
/// [`MigrateError::PrivateMemory`]: that connection would see another, empty database.
/// `:memory:`, a named `mode=memory` or `cache=shared` database and database files work; an unnamed `mode=memory` one is private as well. On Postgres and MySQL
/// the connection is closed afterwards, so a database lock that `sqlx` leaves behind after a
/// failure is released with it.
///
/// A long migration on SQLite holds the write lock for its whole run, so a second instance that
/// starts meanwhile can hit the busy timeout of SQLite and fail.
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
    execute(sources, name, settings, embedded, Op::Run).await
}

/// Reverts the migrations of the source `name` that are above `target_version`, newest first,
/// with their down files, and reports the migrations it reverted, highest version first.
///
/// The source of the migrations, the history `table`, `ignore_missing`, `locking` and the
/// `timeout` work as they do for [`run`]. A missing file, a dirty history and a failing script
/// fail the revert with the same errors; a changed file does not, because `sqlx` checks no
/// checksums when it undoes. A migration is only reversible when
/// the migrations hold a down file for it (`N_name.down.sql` next to `N_name.up.sql`, or a
/// [`MigrationType::ReversibleDown`](sqlx::migrate::MigrationType::ReversibleDown) migration in
/// an embedded migrator). The check runs before the first script: when an applied migration
/// above `target_version` has no down file, nothing is reverted and the error names the
/// highest such version. A `target_version` at or above the newest applied version reverts
/// nothing, and `0` reverts everything.
///
/// On SQLite the revert holds the write lock of the database for its whole run, as [`run`]
/// does, and a failure is not repeated. It uses a connection of its own on every driver, with
/// the same consequences as [`run`], including the refusal of a private-cache in-memory SQLite
/// database ([`MigrateError::PrivateMemory`]).
///
/// # Errors
///
/// The errors of [`run`], and [`MigrateError::NotReversible`] for an applied migration above
/// `target_version` that has no down file.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_config::Config;
/// use oxbloom_data::migrate::{self, MigrateError};
/// use oxbloom_data::sqlx::SqlSafeStr;
/// use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
/// use oxbloom_data::{DataError, DataSettings, DataSources};
///
/// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"\n[data.migrations]\nenabled = true")?;
/// let settings = DataSettings::from_config(&config)?;
/// let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
/// let migrations = settings.sources[0].migrations.as_ref().ok_or("no migrations")?;
///
/// let migration = |version, kind, sql: &'static str| {
///     Migration::new(version, "create accounts".into(), kind, sql.into_sql_str(), false)
/// };
/// let migrator = Migrator::with_migrations(vec![
///     migration(1, MigrationType::ReversibleUp, "CREATE TABLE accounts (id INTEGER)"),
///     migration(1, MigrationType::ReversibleDown, "DROP TABLE accounts"),
///     migration(2, MigrationType::Simple, "CREATE TABLE orders (id INTEGER)"),
/// ]);
/// migrate::run(&sources, "default", migrations, Some(&migrator)).await?;
///
/// // Migration 2 has no down file, so nothing is reverted.
/// let err = migrate::revert(&sources, "default", migrations, Some(&migrator), 0).await.unwrap_err();
/// assert!(matches!(err, MigrateError::NotReversible { version: 2, .. }));
///
/// // Going back to version 2 only needs the down files above it, and there are none.
/// let report = migrate::revert(&sources, "default", migrations, Some(&migrator), 2).await?;
/// assert!(report.is_empty());
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
pub async fn revert(
    sources: &DataSources,
    name: &str,
    settings: &MigrationSettings,
    embedded: Option<&Migrator>,
    target_version: i64,
) -> Result<MigrationReport, MigrateError> {
    execute(
        sources,
        name,
        settings,
        embedded,
        Op::Revert(target_version),
    )
    .await
}

/// What a call does to the history. Without a driver feature nothing reads the target.
#[derive(Clone, Copy)]
#[cfg_attr(
    not(any(feature = "postgres", feature = "mysql", feature = "sqlite")),
    allow(dead_code)
)]
enum Op {
    Run,
    Revert(i64),
}

/// The shared path of [`run`] and [`revert`]: the checks that need no database, the migrator,
/// and the work under the timeout.
async fn execute(
    sources: &DataSources,
    name: &str,
    settings: &MigrationSettings,
    embedded: Option<&Migrator>,
    op: Op,
) -> Result<MigrationReport, MigrateError> {
    let Some(driver) = sources.driver_of(name) else {
        return Err(MigrateError::UnknownSource {
            source_name: name.to_string(),
        });
    };
    if !is_valid_table_name(&settings.table) {
        return Err(load_error(name, LoadReason::InvalidTable));
    }
    if sources.is_private_memory(name) {
        return Err(MigrateError::PrivateMemory {
            source_name: name.to_string(),
        });
    }
    // Before the directory is read, so a disabled driver is not hidden by a load error.
    if !driver_enabled(driver) {
        return Err(MigrateError::DriverDisabled {
            source_name: name.to_string(),
        });
    }
    let work = async {
        let migrator = build_migrator(name, settings, embedded).await?;
        dispatch(sources, name, driver, &migrator, settings, op).await
    };
    tokio::time::timeout(settings.timeout, work)
        .await
        .unwrap_or_else(|_| {
            Err(MigrateError::Timeout {
                source_name: name.to_string(),
            })
        })
}

/// Whether the cargo feature of `driver` is on in this build.
fn driver_enabled(driver: Driver) -> bool {
    (cfg!(feature = "postgres") && driver == Driver::Postgres)
        || (cfg!(feature = "mysql") && driver == Driver::Mysql)
        || (cfg!(feature = "sqlite") && driver == Driver::Sqlite)
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
    op: Op,
) -> Result<MigrationReport, MigrateError> {
    let unknown = || MigrateError::UnknownSource {
        source_name: name.to_string(),
    };
    match driver {
        #[cfg(feature = "postgres")]
        Driver::Postgres => {
            let pool = sources.postgres(name).ok_or_else(unknown)?;
            migrate_pool(pool, name, migrator, settings, op).await
        }
        #[cfg(feature = "mysql")]
        Driver::Mysql => {
            let pool = sources.mysql(name).ok_or_else(unknown)?;
            migrate_pool(pool, name, migrator, settings, op).await
        }
        #[cfg(feature = "sqlite")]
        Driver::Sqlite => {
            let pool = sources.sqlite(name).ok_or_else(unknown)?;
            migrate_pool(pool, name, migrator, settings, op).await
        }
        #[allow(unreachable_patterns)]
        _ => {
            let _ = (sources, migrator, settings, op, unknown);
            Err(MigrateError::DriverDisabled {
                source_name: name.to_string(),
            })
        }
    }
}

/// Runs `op` on `pool`, a run once more on SQLite after a failure that a second run can
/// resolve, and maps the outcome. The history before the work is read once, by the first
/// attempt that gets that far, so the report of a repeated run keeps what the first attempt
/// applied.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
async fn migrate_pool<DB: private::MigrateDatabase>(
    pool: &sqlx::Pool<DB>,
    name: &str,
    migrator: &Migrator,
    settings: &MigrationSettings,
    op: Op,
) -> Result<MigrationReport, MigrateError>
where
    DB::Connection: sqlx::migrate::Migrate,
{
    let mut before = None;
    let outcome = match attempt(pool, migrator, settings, op, &mut before).await {
        Err(Fail::Sqlx(error))
            if DB::RUN_TWICE_ON_FAILURE
                && matches!(op, Op::Run)
                && !matches!(
                    error,
                    SqlxMigrateError::Dirty(_) | SqlxMigrateError::VersionMismatch(_)
                ) =>
        {
            attempt(pool, migrator, settings, op, &mut before).await
        }
        other => other,
    };
    match outcome {
        Ok(after) => Ok(report(migrator, &before.unwrap_or_default(), &after, op)),
        Err(fail) => Err(map_error(name, migrator, fail)),
    }
}

/// Why one attempt failed.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
enum Fail {
    Sqlx(SqlxMigrateError),
    /// The applied migration with this version has no down file.
    NotReversible(i64),
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
impl From<SqlxMigrateError> for Fail {
    fn from(error: SqlxMigrateError) -> Self {
        Self::Sqlx(error)
    }
}

/// One attempt on a connection of its own, opened with the connect options of `pool` and with
/// statement logging off; returns the versions in the history afterwards.
///
/// `sqlx` logs every statement it runs at `debug` (and a slow one at `warn`) through `tracing`,
/// and that text would put the SQL of a migration into the application's logs. The pool's
/// connections log by default, so the work does not use them. A connection of its own also
/// ends its session when it is dropped, even after a failure or a cancelled run, so a
/// database lock that sqlx did not release (it does not after a failure) goes with it. On
/// SQLite a pool connection is held meanwhile, because closing the last connection of an
/// in-memory database drops it and the connection of its own would otherwise be the last.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
async fn attempt<DB: private::MigrateDatabase>(
    pool: &sqlx::Pool<DB>,
    migrator: &Migrator,
    settings: &MigrationSettings,
    op: Op,
    before: &mut Option<BTreeSet<i64>>,
) -> Result<BTreeSet<i64>, Fail>
where
    DB::Connection: sqlx::migrate::Migrate,
{
    use sqlx::{ConnectOptions, Connection};

    let keep_alive = if DB::HOLD_POOL_CONNECTION {
        Some(pool.acquire().await.map_err(SqlxMigrateError::from)?)
    } else {
        None
    };
    let options = (*pool.connect_options())
        .clone()
        .disable_statement_logging();
    let mut conn = options.connect().await.map_err(SqlxMigrateError::from)?;
    let result = DB::attempt_on(
        migrator,
        &mut conn,
        &settings.table,
        settings.locking,
        op,
        before,
    )
    .await;
    // A failed close changes nothing about the outcome of the work.
    let _ = conn.close().await;
    drop(keep_alive);
    result
}

/// Does `op` on `conn` as `Migrator::run` or `Migrator::undo` does, and returns the versions
/// that are in the history afterwards. `before` is filled with the versions in the history
/// before the work, unless an earlier attempt filled it.
///
/// `Migrator::run` on a connection does not compile as a `Send` future ("implementation of
/// `Acquire` is not general enough"), so a run calls `run_direct`, the public method (hidden
/// from the docs) that `sqlx` keeps for that case and that `run` itself calls with
/// `(None, conn, false)`. `Migrator::undo` has the same problem and no `undo_direct`, so a
/// revert goes through [`undo_direct`].
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
async fn apply_op<C: sqlx::migrate::Migrate>(
    migrator: &Migrator,
    conn: &mut C,
    table: &str,
    op: Op,
    before: &mut Option<BTreeSet<i64>>,
) -> Result<BTreeSet<i64>, Fail> {
    if before.is_none() {
        // The table does not exist before the first run; that reads as an empty history.
        *before = Some(
            conn.list_applied_migrations(table)
                .await
                .map(|applied| applied.iter().map(|m| m.version).collect())
                .unwrap_or_default(),
        );
    }
    match op {
        Op::Run => migrator.run_direct(None, conn, false).await?,
        Op::Revert(target) => {
            undo_direct(migrator, conn, table, target).await?;
        }
    }
    let after = conn.list_applied_migrations(table).await?;
    Ok(after.iter().map(|m| m.version).collect())
}

/// `Migrator::undo` on an open connection, step by step as `sqlx` does it: the lock, the
/// history table, the dirty check, the check that every applied migration has a file (unless
/// `ignore_missing`), and the down migrations above `target`, newest first. Unlike `sqlx`, it
/// first checks that every applied migration above `target` has a down file. Like `sqlx`, it
/// does not release the lock after a failure; the connection of the work is closed instead.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
async fn undo_direct<C: sqlx::migrate::Migrate>(
    migrator: &Migrator,
    conn: &mut C,
    table: &str,
    target: i64,
) -> Result<(), Fail> {
    if migrator.locking {
        conn.lock().await?;
    }
    conn.ensure_migrations_table(table).await?;
    if let Some(version) = conn.dirty_version(table).await? {
        return Err(Fail::Sqlx(SqlxMigrateError::Dirty(version)));
    }
    let applied = conn.list_applied_migrations(table).await?;
    if !migrator.ignore_missing {
        let known: BTreeSet<i64> = migrator.iter().map(|m| m.version).collect();
        if let Some(missing) = applied.iter().find(|m| !known.contains(&m.version)) {
            return Err(Fail::Sqlx(SqlxMigrateError::VersionMissing(
                missing.version,
            )));
        }
    }
    let applied: BTreeSet<i64> = applied.iter().map(|m| m.version).collect();
    // `undo` skips an applied migration that has no down file without a word. That would leave
    // the database half way, so the check is made before the first down script, and the
    // highest version without a down file is named.
    if let Some(version) = applied
        .iter()
        .rev()
        .find(|v| **v > target && !has_down(migrator, **v))
    {
        return Err(Fail::NotReversible(*version));
    }
    for migration in migrator
        .iter()
        .rev()
        .filter(|m| m.migration_type.is_down_migration())
        .filter(|m| applied.contains(&m.version))
        .filter(|m| m.version > target)
    {
        conn.revert(table, migration).await?;
    }
    if migrator.locking {
        conn.unlock().await?;
    }
    Ok(())
}

/// Whether the migrator has a down migration for `version`.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn has_down(migrator: &Migrator, version: i64) -> bool {
    migrator
        .iter()
        .any(|m| m.version == version && m.migration_type.is_down_migration())
}

/// The SQLite attempt. The `sqlx` lock is a no-op on SQLite, so with `locking` the work takes
/// SQLite's write lock itself: it runs inside a `BEGIN IMMEDIATE` transaction, and each
/// migration's own transaction becomes a savepoint in it. A second runner waits for the lock
/// (up to the busy timeout of the connection) and then finds everything applied. The
/// transaction is committed after a failure as well, so the migrations before the failing one
/// stay applied (or reverted), as they do without it. A migration marked `-- no-transaction`
/// cannot run inside a transaction, so a migrator that has one runs without this lock.
#[cfg(feature = "sqlite")]
async fn sqlite_attempt(
    migrator: &Migrator,
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    locking: bool,
    op: Op,
    before: &mut Option<BTreeSet<i64>>,
) -> Result<BTreeSet<i64>, Fail> {
    use sqlx::Connection;

    if !locking || migrator.iter().any(|m| m.no_tx) {
        return apply_op(migrator, conn, table, op, before).await;
    }
    let mut tx = conn
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(SqlxMigrateError::from)?;
    let result = apply_op(migrator, &mut *tx, table, op, before).await;
    let committed = tx.commit().await;
    let after = result?;
    committed.map_err(SqlxMigrateError::from)?;
    Ok(after)
}

/// The report of the work: the migrations that entered the history for a run, and those that
/// left it (highest version first) for a revert, with the descriptions of the migrator.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn report(
    migrator: &Migrator,
    before: &BTreeSet<i64>,
    after: &BTreeSet<i64>,
    op: Op,
) -> MigrationReport {
    let reverted = matches!(op, Op::Revert(_));
    let versions: BTreeSet<i64> = if reverted {
        before.difference(after).copied().collect()
    } else {
        after.difference(before).copied().collect()
    };
    let mut applied: Vec<AppliedMigration> = migrator
        .iter()
        .filter(|m| {
            m.migration_type.is_down_migration() == reverted && versions.contains(&m.version)
        })
        .map(|m| AppliedMigration {
            version: m.version,
            description: m.description.to_string(),
        })
        .collect();
    applied.sort_by_key(|m| m.version);
    if reverted {
        applied.reverse();
    }
    MigrationReport { applied, reverted }
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
fn map_error(name: &str, migrator: &Migrator, fail: Fail) -> MigrateError {
    let source_name = name.to_string();
    let error = match fail {
        Fail::Sqlx(error) => error,
        Fail::NotReversible(version) => {
            return MigrateError::NotReversible {
                source_name,
                version,
            };
        }
    };
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

    use super::{Fail, Migrator, Op};

    /// The future of one attempt: the versions in the history afterwards.
    pub type Attempt<'c> = Pin<Box<dyn Future<Output = Result<BTreeSet<i64>, Fail>> + Send + 'c>>;

    /// The per-driver part of the work, implemented for the enabled drivers only. On Postgres
    /// and MySQL `locking` is applied by `sqlx` through the migrator.
    pub trait MigrateDatabase: sqlx::Database
    where
        <Self as sqlx::Database>::Connection: sqlx::migrate::Migrate,
    {
        /// Whether a failed run is repeated once (SQLite, whose sqlx lock is a no-op).
        const RUN_TWICE_ON_FAILURE: bool;
        /// Whether a pool connection is held while the work runs on its own connection (SQLite,
        /// where the last connection of an in-memory database keeps the database alive).
        const HOLD_POOL_CONNECTION: bool;

        /// One attempt of `op` on `conn`; returns the versions in the history afterwards.
        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut Self::Connection,
            table: &'c str,
            locking: bool,
            op: Op,
            before: &'c mut Option<BTreeSet<i64>>,
        ) -> Attempt<'c>;
    }

    #[cfg(feature = "postgres")]
    impl MigrateDatabase for sqlx::Postgres {
        const RUN_TWICE_ON_FAILURE: bool = false;
        const HOLD_POOL_CONNECTION: bool = false;

        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut sqlx::PgConnection,
            table: &'c str,
            _locking: bool,
            op: Op,
            before: &'c mut Option<BTreeSet<i64>>,
        ) -> Attempt<'c> {
            Box::pin(super::apply_op(migrator, conn, table, op, before))
        }
    }

    #[cfg(feature = "mysql")]
    impl MigrateDatabase for sqlx::MySql {
        const RUN_TWICE_ON_FAILURE: bool = false;
        const HOLD_POOL_CONNECTION: bool = false;

        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut sqlx::MySqlConnection,
            table: &'c str,
            _locking: bool,
            op: Op,
            before: &'c mut Option<BTreeSet<i64>>,
        ) -> Attempt<'c> {
            Box::pin(super::apply_op(migrator, conn, table, op, before))
        }
    }

    #[cfg(feature = "sqlite")]
    impl MigrateDatabase for sqlx::Sqlite {
        const RUN_TWICE_ON_FAILURE: bool = true;
        const HOLD_POOL_CONNECTION: bool = true;

        fn attempt_on<'c>(
            migrator: &'c Migrator,
            conn: &'c mut sqlx::SqliteConnection,
            table: &'c str,
            locking: bool,
            op: Op,
            before: &'c mut Option<BTreeSet<i64>>,
        ) -> Attempt<'c> {
            Box::pin(super::sqlite_attempt(
                migrator, conn, table, locking, op, before,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source of a disabled driver cannot be built (`DataSources::build` refuses it), so the
    /// check in `execute` is out of reach of an integration test; this pins what it tests.
    #[test]
    fn a_driver_is_enabled_exactly_when_its_feature_is() {
        assert_eq!(driver_enabled(Driver::Postgres), cfg!(feature = "postgres"));
        assert_eq!(driver_enabled(Driver::Mysql), cfg!(feature = "mysql"));
        assert_eq!(driver_enabled(Driver::Sqlite), cfg!(feature = "sqlite"));
    }
}
