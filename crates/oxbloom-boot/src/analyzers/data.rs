//! The data analyzers: an invalid `data` key or a source that cannot be built, a source that
//! is not reachable at start, and migrations that failed or cannot run.

use oxbloom_config::ConfigError;
use oxbloom_data::DataError;

#[cfg(feature = "data-migrate")]
use crate::autoconfigure::DataMigrationFailed;
#[cfg(not(feature = "data-migrate"))]
use crate::autoconfigure::MigrationsNeedFeature;
use crate::autoconfigure::{DataUnreachable, SourceBuildError};
use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises a `ConfigError::Invalid` for a key under `data` (`data.sources.` included)
/// anywhere in the cause chain, and a source the registry could not build. The report names
/// the key with a fixed text and leaves out the configuration error's message, so no value can
/// reach it. The one exception is an `ssl-bundle` key, whose messages are fixed texts that
/// list bundle names at most, so the report shows them. A source refused for
/// `ssl-mode = verify-ca` with a bundle that has a trust store is reported on its `ssl-mode`
/// key with the fixed text of the error.
pub(crate) struct DataConfigAnalyzer;

impl FailureAnalyzer for DataConfigAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        if let Some(build) = failure.find_cause::<SourceBuildError>() {
            return Some(build_failure(build));
        }
        let ConfigError::Invalid { key, message, .. } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        if key != "data" && !key.starts_with("data.") {
            return None;
        }
        if key.ends_with(".ssl-bundle") {
            return Some(FailureAnalysis::new(
                format!("The data source key `{key}` is not usable ({message})."),
                "Set it to the name of a bundle defined under `ssl.bundle.pem.<name>` or \
                 `ssl.bundle.pkcs12.<name>` (this needs the `ssl` feature), for a Postgres or \
                 MySQL source whose `ssl-mode` is not `disable`, or remove it.",
            ));
        }
        Some(FailureAnalysis::new(
            format!("The data source configuration key `{key}` is not valid."),
            "Fix the `data` section: `url` is required and starts with `postgres://`, \
             `postgresql://`, `mysql://`, `mariadb://` or `sqlite:`; `max-connections` is at \
             least 1 (1 for an in-memory SQLite URL without `cache=shared`) and not below \
             `min-connections`; `acquire-timeout`, `idle-timeout` and `max-lifetime` are \
             durations such as `5s` above zero; `ssl-mode` is `disable`, `prefer`, `require`, \
             `verify-ca` or `verify-full`, and `ssl-mode` and `ssl-bundle` are for Postgres and \
             MySQL only; a name under `data.sources` uses lowercase letters, digits and `-` \
             and is not `default`.",
        ))
    }
}

/// The report of a source the registry could not build: the source, its key and what to do,
/// by the kind of error.
fn build_failure(build: &SourceBuildError) -> FailureAnalysis {
    let name = build.error.source_name();
    let prefix = build.error.config_prefix();
    let key = format!("{prefix}.url");
    if build.error.error() == DataError::VerifyCaWithTrust {
        return FailureAnalysis::new(
            format!(
                "The data source key `{prefix}.ssl-mode` is not usable: {}.",
                build.error.error()
            ),
            format!(
                "Set `{prefix}.ssl-mode` to `verify-full`, or name in `{prefix}.ssl-bundle` a \
                 bundle without a trust store (for example one with only a client \
                 certificate)."
            ),
        );
    }
    let description = format!(
        "The data source `{name}` cannot be built: {}.",
        build.error.error()
    );
    let action = match build.error.error() {
        DataError::Unsupported => match build.driver {
            // The boot feature of a driver is `data-` and the driver's name.
            Some(driver) => format!(
                "`{key}` names the {driver} driver: enable the `data-{driver}` feature of \
                 `oxbloom-boot`, or point the source at a database of an enabled driver."
            ),
            None => format!(
                "Enable the `data-*` feature of `oxbloom-boot` for the driver that `{key}` \
                 names."
            ),
        },
        DataError::Tls => format!(
            "Check `{prefix}.ssl-mode` and `{prefix}.ssl-bundle`: the bundle needs a usable \
             trust store or client certificate."
        ),
        DataError::Pool => "Build the application on a Tokio runtime: the pools are created \
                            while the container is built and need the runtime."
            .to_owned(),
        _ => format!(
            "Check the options of `{key}`: the driver refused one of them. The URL is not shown \
             because it can contain a password."
        ),
    };
    FailureAnalysis::new(description, action)
}

/// Recognises a source that failed the start-up check. [`DataError::Connect`] and
/// [`DataError::Pool`] get the same report: sqlx retries a refused connection until the
/// acquire timeout, so an unreachable server can end in either. The report names the source
/// and its key and says to check the URL, the network and the credentials; it never shows the
/// URL or the password.
pub(crate) struct DataUnreachableAnalyzer;

impl FailureAnalyzer for DataUnreachableAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let unreachable = &failure.find_cause::<DataUnreachable>()?.0;
        let name = unreachable.source_name();
        let description = match unreachable.error() {
            DataError::Connect | DataError::Pool => format!(
                "The data source `{name}` is not reachable: no connection could be made during \
                 the start-up check."
            ),
            other => format!("The data source `{name}` failed the start-up check: {other}."),
        };
        Some(FailureAnalysis::new(
            description,
            format!(
                "Check the URL in `{}.url` (host, port, database name or file path), the \
                 network path to the database and the credentials. Set \
                 `data.connect-on-startup = false` to start without connecting.",
                unreachable.config_prefix()
            ),
        ))
    }
}

/// Recognises the migrations of a source that failed while the `DataService` started
/// (feature `data-migrate`), and a source that enables migrations in a build without that
/// feature. The text is fixed per kind of error; it names the source, the version of the
/// migration at fault (with its description where the error has one) and the key to change.
/// It never shows SQL, a path, a file name or the URL. The report for a dirty history says what
/// the dirty state means for the driver of the source: on MySQL a script did not finish, on
/// the others a transaction was interrupted.
pub(crate) struct DataMigrationAnalyzer;

impl FailureAnalyzer for DataMigrationAnalyzer {
    #[cfg(feature = "data-migrate")]
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let failed = failure.find_cause::<DataMigrationFailed>()?;
        Some(migration_failure(&failed.error, failed.driver))
    }

    #[cfg(not(feature = "data-migrate"))]
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let off = failure.find_cause::<MigrationsNeedFeature>()?;
        Some(FailureAnalysis::new(
            format!(
                "The data source `{}` enables migrations in `{}`, but this build has no \
                 migration support.",
                off.source_name, off.key
            ),
            format!(
                "Enable the `data-migrate` feature of `oxbloom-boot`, or set `{} = false`.",
                off.key
            ),
        ))
    }
}

/// The configuration prefix of the source `name`: `data` for the default source, and
/// `data.sources.<name>` for a named one.
#[cfg(feature = "data-migrate")]
fn prefix_of(name: &str) -> String {
    if name == "default" {
        "data".to_owned()
    } else {
        format!("data.sources.{name}")
    }
}

/// The report of a failed migration run, fixed per kind of [`MigrateError`].
#[cfg(feature = "data-migrate")]
fn migration_failure(
    error: &oxbloom_data::MigrateError,
    driver: Option<oxbloom_data::Driver>,
) -> FailureAnalysis {
    use oxbloom_data::Driver;
    use oxbloom_data::MigrateError as E;
    use oxbloom_data::migrate::LoadReason;

    let name = error.source_name();
    let prefix = prefix_of(name);
    let key = |field: &str| format!("`{prefix}.migrations.{field}`");
    let migration = |version: i64, description: Option<&str>| match description {
        Some(description) => format!("migration {version} ({description})"),
        None => format!("migration {version}"),
    };
    let mysql = driver == Some(Driver::Mysql);
    let (description, action) = match error {
        E::Load {
            reason: LoadReason::NotConfigured,
            ..
        } => (
            format!("The data source `{name}` enables migrations, but has none to run."),
            format!(
                "Set {} to the directory of the migration files, or register an \
                 `EmbeddedMigrations` bean with an entry for the source `{name}`.",
                key("path")
            ),
        ),
        E::Load {
            reason: LoadReason::Unreadable,
            ..
        } => (
            format!("The migration directory of the data source `{name}` could not be read."),
            format!(
                "Check that {} names an existing directory the application can read. The \
                 path is not shown.",
                key("path")
            ),
        ),
        E::Load {
            reason: LoadReason::Empty,
            ..
        } => (
            format!("The migration directory of the data source `{name}` holds no migrations."),
            format!(
                "Put the migration files (`<version>_<description>.sql`) into the directory of \
                 {}, or point it at the right directory.",
                key("path")
            ),
        ),
        E::Load {
            reason: LoadReason::InvalidTable,
            ..
        } => (
            format!("The migration history table name of the data source `{name}` is invalid."),
            format!(
                "Set {} to a name that matches `[A-Za-z_][A-Za-z0-9_]*` and has at most 63 \
                 characters.",
                key("table")
            ),
        ),
        E::Dirty {
            version,
            description,
            ..
        } => {
            let meaning = if mysql {
                "On MySQL this means a migration script did not finish: it failed part way, and \
                 the statements before the failure stay applied, because MySQL commits each \
                 schema change at once."
            } else {
                "This means the transaction of the migration was interrupted, for example by a \
                 crash of the database server, before it was recorded as finished."
            };
            (
                format!(
                    "The migration history of the data source `{name}` is dirty: {} started and \
                     did not finish. {meaning}",
                    migration(*version, description.as_deref())
                ),
                format!(
                    "Repair the database by hand: finish or undo what migration {version} \
                     changed, then delete its row from the history table ({}). The next start \
                     applies it again.",
                    key("table")
                ),
            )
        }
        E::ChecksumMismatch {
            version,
            description,
            ..
        } => (
            format!(
                "{} of the data source `{name}` was applied, but its file has changed since.",
                capitalize(&migration(*version, Some(description)))
            ),
            format!(
                "Restore migration {version} as it was applied, and put the change into a new \
                 migration with a higher version."
            ),
        ),
        E::MissingFile { version, .. } => (
            format!(
                "Migration {version} of the data source `{name}` was applied, but its file is \
                 missing."
            ),
            format!(
                "Restore the file of migration {version}, or set {} to accept applied \
                 migrations without a file.",
                key("ignore-missing")
            ),
        ),
        E::NotReversible { version, .. } => (
            format!(
                "Migration {version} of the data source `{name}` has no down file, so it \
                 cannot be reverted."
            ),
            format!("Add a down file for migration {version}."),
        ),
        E::Script {
            version,
            description,
            ..
        } => (
            format!(
                "{} of the data source `{name}` failed.",
                capitalize(&migration(*version, Some(description)))
            ),
            if mysql {
                format!(
                    "Fix migration {version} and start again. MySQL commits each schema change \
                     at once, so the statements before the failure may stay applied and the \
                     next start may report a dirty history that needs a repair first. The SQL \
                     and the database message are not shown."
                )
            } else {
                format!(
                    "Fix migration {version} and start again; its transaction was rolled back, \
                     so it left no changes, unless the migration is marked \
                     `-- no-transaction`: then the statements before the failure may stay \
                     applied and run again at the next start, so make them safe to repeat or \
                     undo them first. The SQL and the database message are not shown."
                )
            },
        ),
        E::Timeout { .. } => (
            format!(
                "The migrations of the data source `{name}` did not finish within the timeout."
            ),
            format!(
                "Raise {} or make the migrations faster; the time includes the wait for the \
                 migration lock that another instance may hold.",
                key("timeout")
            ),
        ),
        E::UnknownSource { .. } => (
            format!("The data source `{name}` enables migrations, but it does not exist."),
            format!(
                "Configure the source `{name}` (with `{prefix}.url`), register it in the \
                 application's `DataSources`, or set {} to false.",
                key("enabled")
            ),
        ),
        E::PrivateMemory { .. } => (
            format!(
                "The data source `{name}` is an in-memory SQLite database with a private \
                 cache, which cannot be migrated."
            ),
            format!(
                "Add `cache=shared` to the URL in `{prefix}.url`, or use a database file. The \
                 URL is not shown."
            ),
        ),
        E::DriverDisabled { .. } => (
            format!("The database driver of the data source `{name}` is not in this build."),
            match driver {
                Some(driver) => format!("Enable the `data-{driver}` feature of `oxbloom-boot`."),
                None => "Enable the `data-*` feature of `oxbloom-boot` for the driver of the \
                         source."
                    .to_owned(),
            },
        ),
        E::Database { .. } => (
            format!("The database failed while the migrations of the data source `{name}` ran."),
            format!(
                "Check that the database is reachable through `{prefix}.url` and that its user \
                 may create and write the history table ({}). The database message is not \
                 shown.",
                key("table")
            ),
        ),
        _ => (
            format!("The migrations of the data source `{name}` failed."),
            "Check the migration settings and files of the source.".to_owned(),
        ),
    };
    FailureAnalysis::new(description, action)
}

/// `text` with its first letter in upper case.
#[cfg(feature = "data-migrate")]
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ServiceError, StartupPhase};
    use oxbloom_data::SourceError;
    use oxbloom_di::DiError;

    fn analyze(
        analyzer: &dyn FailureAnalyzer,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Option<FailureAnalysis> {
        analyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    #[test]
    fn an_invalid_data_key_is_named_and_the_message_is_not() {
        let di: DiError = ConfigError::invalid("data.sources.cache.url", "hunter2").into();
        let a = analyze(&DataConfigAnalyzer, di).unwrap();
        assert_eq!(
            a.description(),
            "The data source configuration key `data.sources.cache.url` is not valid."
        );
        assert!(!a.to_string().contains("hunter2"));
    }

    #[test]
    fn an_ssl_bundle_key_shows_the_message() {
        let di: DiError = ConfigError::invalid(
            "data.ssl-bundle",
            "names an SSL bundle that does not exist; available bundles: api, idp",
        )
        .into();
        let a = analyze(&DataConfigAnalyzer, di).unwrap();
        assert_eq!(
            a.description(),
            "The data source key `data.ssl-bundle` is not usable (names an SSL bundle that \
             does not exist; available bundles: api, idp)."
        );
    }

    #[test]
    fn verify_ca_with_a_trust_store_is_reported_on_the_ssl_mode_key() {
        let build = SourceBuildError {
            error: SourceError::new("orders", DataError::VerifyCaWithTrust),
            driver: Some(oxbloom_data::Driver::Postgres),
        };
        let di = DiError::creation_from::<oxbloom_data::DataSources>(build);
        let a = analyze(&DataConfigAnalyzer, di).unwrap();
        assert_eq!(
            a.description(),
            "The data source key `data.sources.orders.ssl-mode` is not usable: ssl-mode \
             verify-ca cannot be combined with an ssl-bundle that has a trust store because the \
             database drivers add the bundle's trust to the platform roots; use verify-full."
        );
        let action = a.action().unwrap();
        assert!(
            action.contains("`data.sources.orders.ssl-mode`"),
            "{action}"
        );
        assert!(action.contains("`verify-full`"), "{action}");
    }

    #[test]
    fn other_keys_and_errors_are_none() {
        assert!(
            analyze(
                &DataConfigAnalyzer,
                ConfigError::invalid("database.url", "x")
            )
            .is_none()
        );
        assert!(
            analyze(
                &DataConfigAnalyzer,
                ConfigError::invalid("http.client", "x")
            )
            .is_none()
        );
        assert!(
            analyze(
                &DataConfigAnalyzer,
                ConfigError::Missing {
                    key: "data.url".into()
                }
            )
            .is_none()
        );
        assert!(analyze(&DataUnreachableAnalyzer, DataError::Connect).is_none());
    }

    #[test]
    fn an_unreachable_source_is_named_with_its_key() {
        // A refused connection and a pool timeout get the same report.
        for error in [DataError::Connect, DataError::Pool] {
            let error = ServiceError::new(DataUnreachable(SourceError::new("orders", error)));
            let a = analyze(&DataUnreachableAnalyzer, error).unwrap();
            assert_eq!(
                a.description(),
                "The data source `orders` is not reachable: no connection could be made during \
                 the start-up check."
            );
            assert!(a.action().unwrap().contains("`data.sources.orders.url`"));
            assert!(a.action().unwrap().contains("credentials"));
        }
        let tls = ServiceError::new(DataUnreachable(SourceError::new("default", DataError::Tls)));
        let a = analyze(&DataUnreachableAnalyzer, tls).unwrap();
        assert_eq!(
            a.description(),
            "The data source `default` failed the start-up check: the TLS configuration of the \
             database is unusable."
        );
    }

    #[cfg(feature = "data-migrate")]
    fn migration_report(
        error: oxbloom_data::MigrateError,
        driver: Option<oxbloom_data::Driver>,
    ) -> FailureAnalysis {
        let failed = ServiceError::new(DataMigrationFailed { error, driver });
        analyze(&DataMigrationAnalyzer, failed).unwrap()
    }

    #[cfg(feature = "data-migrate")]
    #[test]
    fn a_dirty_history_says_what_it_means_for_the_driver() {
        use oxbloom_data::{Driver, MigrateError};
        let dirty = || MigrateError::Dirty {
            source_name: "orders".into(),
            version: 20261125,
            description: Some("add totals".into()),
        };
        let mysql = migration_report(dirty(), Some(Driver::Mysql)).to_string();
        assert!(mysql.contains("`orders`"), "{mysql}");
        assert!(mysql.contains("migration 20261125 (add totals)"), "{mysql}");
        assert!(mysql.contains("script did not finish"), "{mysql}");
        assert!(!mysql.contains("transaction"), "{mysql}");
        assert!(
            mysql.contains("`data.sources.orders.migrations.table`"),
            "{mysql}"
        );
        for driver in [Driver::Postgres, Driver::Sqlite] {
            let other = migration_report(dirty(), Some(driver)).to_string();
            assert!(other.contains("transaction"), "{other}");
            assert!(other.contains("interrupted"), "{other}");
            assert!(other.contains("migration 20261125"), "{other}");
        }
    }

    #[cfg(feature = "data-migrate")]
    #[test]
    fn every_migration_error_names_the_source_and_its_key_or_version() {
        use oxbloom_data::migrate::LoadReason;
        use oxbloom_data::{Driver, MigrateError as E};
        let s = || "default".to_string();
        let cases: Vec<(E, &[&str])> = vec![
            (
                E::Load {
                    source_name: s(),
                    reason: LoadReason::NotConfigured,
                },
                &["`data.migrations.path`", "`EmbeddedMigrations`"],
            ),
            (
                E::Load {
                    source_name: s(),
                    reason: LoadReason::Unreadable,
                },
                &["`data.migrations.path`"],
            ),
            (
                E::Load {
                    source_name: s(),
                    reason: LoadReason::Empty,
                },
                &["`data.migrations.path`"],
            ),
            (
                E::Load {
                    source_name: s(),
                    reason: LoadReason::InvalidTable,
                },
                &["`data.migrations.table`"],
            ),
            (
                E::ChecksumMismatch {
                    source_name: s(),
                    version: 3,
                    description: "x".into(),
                },
                &["Migration 3 (x)", "changed", "migration 3"],
            ),
            (
                E::MissingFile {
                    source_name: s(),
                    version: 4,
                },
                &["Migration 4", "`data.migrations.ignore-missing`"],
            ),
            (
                E::NotReversible {
                    source_name: s(),
                    version: 5,
                },
                &["Migration 5", "down file"],
            ),
            (
                E::Timeout { source_name: s() },
                &["`data.migrations.timeout`"],
            ),
            (
                E::UnknownSource { source_name: s() },
                &["does not exist", "`data.migrations.enabled`"],
            ),
            (
                E::PrivateMemory { source_name: s() },
                &["`data.url`", "cache=shared"],
            ),
            (E::DriverDisabled { source_name: s() }, &["`data-sqlite`"]),
            (
                E::Database {
                    source_name: s(),
                    cause: None,
                },
                &["`data.url`"],
            ),
        ];
        for (error, expected) in cases {
            let text = migration_report(error, Some(Driver::Sqlite)).to_string();
            assert!(text.contains("`default`"), "{text}");
            for part in expected {
                assert!(text.contains(part), "{part}: {text}");
            }
        }
    }

    #[cfg(feature = "data-migrate")]
    #[test]
    fn a_failed_script_says_whether_it_was_rolled_back() {
        use oxbloom_data::{Driver, MigrateError};
        let script = || MigrateError::Script {
            source_name: "default".into(),
            version: 2,
            description: "seed".into(),
            cause: oxbloom_data::sqlx::Error::RowNotFound,
        };
        let sqlite = migration_report(script(), Some(Driver::Sqlite)).to_string();
        assert!(sqlite.contains("Migration 2 (seed)"), "{sqlite}");
        assert!(sqlite.contains("rolled back"), "{sqlite}");
        // A `-- no-transaction` script runs outside a transaction, so it may stay partly
        // applied; the advice says so for every driver that otherwise rolls back.
        for driver in [Driver::Sqlite, Driver::Postgres] {
            let other = migration_report(script(), Some(driver)).to_string();
            assert!(other.contains("`-- no-transaction`"), "{other}");
            assert!(other.contains("run again at the next start"), "{other}");
        }
        let mysql = migration_report(script(), Some(Driver::Mysql)).to_string();
        assert!(mysql.contains("dirty history"), "{mysql}");
        assert!(!mysql.contains("rolled back"), "{mysql}");
        // The display of the wrapper is the fixed text.
        let failed = DataMigrationFailed {
            error: script(),
            driver: None,
        };
        assert_eq!(
            failed.to_string(),
            "data source `default`: the migrations failed"
        );
    }

    #[cfg(not(feature = "data-migrate"))]
    #[test]
    fn migrations_without_the_feature_name_the_feature_and_the_key() {
        let off = MigrationsNeedFeature {
            source_name: "orders".into(),
            key: "data.sources.orders.migrations.enabled".into(),
        };
        let di = DiError::creation_from::<oxbloom_data::DataSettings>(off);
        let a = analyze(&DataMigrationAnalyzer, di).unwrap().to_string();
        assert!(a.contains("`orders`"), "{a}");
        assert!(a.contains("`data-migrate`"), "{a}");
        assert!(
            a.contains("`data.sources.orders.migrations.enabled = false`"),
            "{a}"
        );
    }
}
