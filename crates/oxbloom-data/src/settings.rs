//! Reads the `data.*` keys into a [`DataSettings`] and validates them.
//!
//! The URL is a secret. It is parsed only as far as needed to read the scheme and the SQLite
//! options for an in-memory database, and no error or `Debug` output contains any part of it.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use oxbloom_config::{Config, ConfigError, Secret};
use serde::Deserialize;

pub(crate) const DEFAULT_SOURCE: &str = "default";
const DEFAULT_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// The database driver of a source, derived from the scheme of its URL.
///
/// `postgres://` and `postgresql://` give [`Postgres`](Self::Postgres), `mysql://` and
/// `mariadb://` give [`Mysql`](Self::Mysql), and `sqlite:` gives [`Sqlite`](Self::Sqlite).
/// All of them are recognised in every build; whether the cargo feature of the driver is on
/// is checked when a pool is built.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_data::{DataSettings, Driver};
///
/// let config = Config::from_str("[data]\nurl = \"mariadb://db.internal/app\"")?;
/// let settings = DataSettings::from_config(&config)?;
/// assert_eq!(settings.sources[0].driver, Driver::Mysql);
/// assert_eq!(Driver::Mysql.to_string(), "mysql");
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Driver {
    /// PostgreSQL.
    Postgres,
    /// MySQL and MariaDB.
    Mysql,
    /// SQLite.
    Sqlite,
}

impl fmt::Display for Driver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Sqlite => "sqlite",
        })
    }
}

/// The TLS mode of a PostgreSQL or MySQL connection, from the key `ssl-mode`.
///
/// The values are `disable`, `prefer`, `require`, `verify-ca` and `verify-full`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_data::{DataSettings, SslMode};
///
/// let config = Config::from_str("[data]\nurl = \"postgres://db/app\"\nssl-mode = \"verify-ca\"")?;
/// let settings = DataSettings::from_config(&config)?;
/// assert_eq!(settings.sources[0].ssl_mode, Some(SslMode::VerifyCa));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SslMode {
    /// No TLS.
    Disable,
    /// TLS when the server offers it.
    Prefer,
    /// TLS is required, the certificate is not checked.
    Require,
    /// TLS is required and the certificate must chain to a trusted authority.
    VerifyCa,
    /// As `VerifyCa`, and the host name must match the certificate.
    VerifyFull,
}

impl SslMode {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "disable" => Self::Disable,
            "prefer" => Self::Prefer,
            "require" => Self::Require,
            "verify-ca" => Self::VerifyCa,
            "verify-full" => Self::VerifyFull,
            _ => return None,
        })
    }
}

/// The settings of one data source: a URL and the limits of its connection pool.
///
/// The `Debug` output redacts the URL. Build it with [`DataSettings::from_config`].
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_config::Config;
/// use oxbloom_data::DataSettings;
///
/// let config = Config::from_str("[data]\nurl = \"postgres://app:pw@db/app\"")?;
/// let source = &DataSettings::from_config(&config)?.sources[0];
/// assert_eq!(source.name, "default");
/// assert_eq!(source.max_connections, 10);
/// assert_eq!(source.acquire_timeout, Duration::from_secs(30));
/// assert!(!format!("{source:?}").contains("pw"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone)]
pub struct SourceSettings {
    /// The name of the source: `default` for the unnamed `data.*` keys, otherwise the name
    /// under `data.sources`.
    pub name: String,
    /// The connection URL. A secret, never shown by `Debug`.
    pub url: Secret<String>,
    /// The driver, from the scheme of the URL.
    pub driver: Driver,
    /// The most connections the pool holds. Default 10, or 1 for an in-memory SQLite URL.
    pub max_connections: u32,
    /// The connections the pool keeps open. Default 0.
    pub min_connections: u32,
    /// How long acquiring a connection may wait. Default 30 seconds.
    pub acquire_timeout: Duration,
    /// How long a connection may sit idle before it is closed. Unset leaves the sqlx default of
    /// 10 minutes, except for an in-memory SQLite database, where unset means never: closing
    /// its last connection would destroy the database.
    pub idle_timeout: Option<Duration>,
    /// How long a connection may live. Unset leaves the sqlx default of 30 minutes, except for
    /// an in-memory SQLite database, where unset means forever, for the same reason.
    pub max_lifetime: Option<Duration>,
    /// Whether a connection is checked before it is handed out. Default true.
    pub test_before_acquire: bool,
    /// The TLS mode, not allowed for SQLite. Default none, which leaves it to the URL.
    pub ssl_mode: Option<SslMode>,
    /// The name of the SSL bundle with the trust roots or client certificate. Not allowed for
    /// SQLite. The caller resolves the name.
    pub ssl_bundle: Option<String>,
}

impl SourceSettings {
    /// The configuration key of this source: `data` for the default source and
    /// `data.sources.<name>` for a named one. Failure reports use it to say where to fix a
    /// source.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_data::DataSettings;
    ///
    /// let config = Config::from_str(
    ///     "[data]\nurl = \"postgres://db/app\"\n[data.sources.rep]\nurl = \"mysql://db/rep\"",
    /// )?;
    /// let settings = DataSettings::from_config(&config)?;
    /// assert_eq!(settings.sources[0].config_prefix(), "data");
    /// assert_eq!(settings.sources[1].config_prefix(), "data.sources.rep");
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn config_prefix(&self) -> String {
        if self.name == DEFAULT_SOURCE {
            "data".to_string()
        } else {
            format!("data.sources.{}", self.name)
        }
    }
}

/// The settings of the data core: whether the pools open at startup and the sources.
///
/// Source names with a dash are best set in the configuration file: an environment variable
/// override such as `..._DATA__SOURCES__REPORTING_DB__URL` spells the name `reporting_db`,
/// which is refused.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_data::DataSettings;
///
/// let config = Config::from_str(
///     "[data]\nurl = \"postgres://db/app\"\n\
///      [data.sources.reporting]\nurl = \"mysql://db/reports\"",
/// )?;
/// let settings = DataSettings::from_config(&config)?;
/// let names: Vec<_> = settings.sources.iter().map(|s| s.name.as_str()).collect();
/// assert_eq!(names, ["default", "reporting"]);
///
/// // Without a `data` section there are no sources.
/// assert!(DataSettings::from_config(&Config::from_str("")?)?.sources.is_empty());
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone)]
pub struct DataSettings {
    /// Whether the pools connect when the application starts. Default true.
    pub connect_on_startup: bool,
    /// The sources: `default` first when configured, then the named ones in name order.
    pub sources: Vec<SourceSettings>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawData {
    connect_on_startup: Option<bool>,
    sources: BTreeMap<String, RawSource>,
    url: Option<Secret<String>>,
    max_connections: Option<u32>,
    min_connections: Option<u32>,
    acquire_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    max_lifetime: Option<Duration>,
    test_before_acquire: Option<bool>,
    ssl_mode: Option<String>,
    ssl_bundle: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawSource {
    url: Option<Secret<String>>,
    max_connections: Option<u32>,
    min_connections: Option<u32>,
    acquire_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    max_lifetime: Option<Duration>,
    test_before_acquire: Option<bool>,
    ssl_mode: Option<String>,
    ssl_bundle: Option<String>,
}

impl RawSource {
    fn is_empty(&self) -> bool {
        self.url.is_none()
            && self.max_connections.is_none()
            && self.min_connections.is_none()
            && self.acquire_timeout.is_none()
            && self.idle_timeout.is_none()
            && self.max_lifetime.is_none()
            && self.test_before_acquire.is_none()
            && self.ssl_mode.is_none()
            && self.ssl_bundle.is_none()
    }
}

impl DataSettings {
    /// Reads the settings from the `data` section of `config`. A missing section gives no
    /// sources and `connect_on_startup` true.
    ///
    /// The unnamed keys (`url`, `max-connections`, `min-connections`, `acquire-timeout`,
    /// `idle-timeout`, `max-lifetime`, `test-before-acquire`, `ssl-mode`, `ssl-bundle`) form the
    /// `default` source; `data.sources.<name>.*` holds the others with the same keys.
    /// `connect-on-startup` is global. Durations take the usual form, such as `500ms` or `30s`.
    ///
    /// # Errors
    ///
    /// Fails with a [`ConfigError::Invalid`] naming the full key, and never the value, for an
    /// unknown key, a value of the wrong type, a bad duration, a source without `url`, a URL
    /// whose scheme is not `postgres`, `postgresql`, `mysql`, `mariadb` or `sqlite`, a zero
    /// `max-connections` or duration, `min-connections` above `max-connections`, an unknown
    /// `ssl-mode`, `ssl-mode` or `ssl-bundle` on a SQLite source, a source name outside
    /// `[a-z0-9-]+` or equal to `default`, and an in-memory SQLite URL with `max-connections`
    /// above 1 and no `cache=shared`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_data::DataSettings;
    ///
    /// // An in-memory SQLite database gets one connection unless told otherwise.
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// assert_eq!(DataSettings::from_config(&config)?.sources[0].max_connections, 1);
    ///
    /// let config = Config::from_str("[data]\nurl = \"postgres://u:secret@db/app\"\nmax-connections = 0")?;
    /// let err = DataSettings::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("data.max-connections"));
    /// assert!(!err.to_string().contains("secret"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let raw = match config.get::<RawData>("data") {
            Ok(raw) => raw,
            Err(ConfigError::Missing { .. }) => RawData::default(),
            Err(e) => return Err(e),
        };
        let RawData {
            connect_on_startup,
            sources: named,
            url,
            max_connections,
            min_connections,
            acquire_timeout,
            idle_timeout,
            max_lifetime,
            test_before_acquire,
            ssl_mode,
            ssl_bundle,
        } = raw;
        let default = RawSource {
            url,
            max_connections,
            min_connections,
            acquire_timeout,
            idle_timeout,
            max_lifetime,
            test_before_acquire,
            ssl_mode,
            ssl_bundle,
        };
        let mut sources = Vec::new();
        if !default.is_empty() {
            sources.push(source(DEFAULT_SOURCE, "data", default)?);
        }
        for (name, raw) in named {
            let key = format!("data.sources.{name}");
            if !is_valid_name(&name) {
                return Err(ConfigError::invalid(
                    key,
                    "the source name must consist of lowercase letters, digits and `-`",
                ));
            }
            if name == DEFAULT_SOURCE {
                return Err(ConfigError::invalid(
                    key,
                    "`default` is the source of the unnamed `data` keys; use another name",
                ));
            }
            sources.push(source(&name, &key, raw)?);
        }
        Ok(Self {
            connect_on_startup: connect_on_startup.unwrap_or(true),
            sources,
        })
    }
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The driver of the URL. The scheme is matched case-sensitively, in the lowercase forms the
/// drivers themselves accept, and the network schemes need `://`.
fn driver_of(url: &str) -> Option<Driver> {
    let (scheme, rest) = url.split_once(':')?;
    let slashes = rest.starts_with("//");
    match scheme {
        "postgres" | "postgresql" if slashes => Some(Driver::Postgres),
        "mysql" | "mariadb" if slashes => Some(Driver::Mysql),
        "sqlite" => Some(Driver::Sqlite),
        _ => None,
    }
}

/// What the options of a SQLite URL say about an in-memory database.
pub(crate) struct SqliteMemory {
    pub(crate) in_memory: bool,
    shared_cache: bool,
}

// sqlx already enables the shared cache for `:memory:`, so the one-connection rule is
// conservative. The pool module uses `in_memory` to keep the connections of such a database
// open. It never shows a sqlx connect-option parse error, because that error echoes query
// values; it maps it to a fixed `DataError`.
pub(crate) fn sqlite_memory(url: &str) -> SqliteMemory {
    let rest = url.split_once(':').map_or("", |(_, rest)| rest);
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let has = |pair: &str| query.split('&').any(|p| p == pair);
    SqliteMemory {
        in_memory: path == ":memory:" || has("mode=memory"),
        shared_cache: has("cache=shared"),
    }
}

fn source(name: &str, prefix: &str, raw: RawSource) -> Result<SourceSettings, ConfigError> {
    let key = |field: &str| format!("{prefix}.{field}");
    let Some(url) = raw.url else {
        return Err(ConfigError::invalid(key("url"), "is required"));
    };
    let Some(driver) = driver_of(url.expose()) else {
        return Err(ConfigError::invalid(
            key("url"),
            "must start with `postgres://`, `postgresql://`, `mysql://` or `mariadb://` (lowercase, followed by `://`), or with `sqlite:`",
        ));
    };
    let memory = if driver == Driver::Sqlite {
        sqlite_memory(url.expose())
    } else {
        SqliteMemory {
            in_memory: false,
            shared_cache: false,
        }
    };
    if raw.max_connections == Some(0) {
        return Err(ConfigError::invalid(
            key("max-connections"),
            "must be at least 1",
        ));
    }
    let single = memory.in_memory && !memory.shared_cache;
    if single && raw.max_connections.is_some_and(|n| n > 1) {
        return Err(ConfigError::invalid(
            key("max-connections"),
            "an in-memory SQLite database is private to a connection; use 1 or add `cache=shared` to the URL",
        ));
    }
    let max_connections =
        raw.max_connections
            .unwrap_or(if single { 1 } else { DEFAULT_MAX_CONNECTIONS });
    let min_connections = raw.min_connections.unwrap_or(0);
    if min_connections > max_connections {
        return Err(ConfigError::invalid(
            key("min-connections"),
            "must not be above the effective max-connections (an in-memory SQLite source has an implicit maximum of 1)",
        ));
    }
    for (field, value) in [
        ("acquire-timeout", raw.acquire_timeout),
        ("idle-timeout", raw.idle_timeout),
        ("max-lifetime", raw.max_lifetime),
    ] {
        if value.is_some_and(|d| d.is_zero()) {
            return Err(ConfigError::invalid(key(field), "must be above zero"));
        }
    }
    if raw
        .ssl_bundle
        .as_deref()
        .is_some_and(|b| b.trim().is_empty())
    {
        return Err(ConfigError::invalid(key("ssl-bundle"), "must not be empty"));
    }
    let ssl_mode = match raw.ssl_mode {
        Some(value) => Some(SslMode::parse(&value).ok_or_else(|| {
            ConfigError::invalid(
                key("ssl-mode"),
                "must be `disable`, `prefer`, `require`, `verify-ca` or `verify-full`",
            )
        })?),
        None => None,
    };
    if driver == Driver::Sqlite {
        if ssl_mode.is_some() {
            return Err(ConfigError::invalid(
                key("ssl-mode"),
                "is not available for a SQLite source",
            ));
        }
        if raw.ssl_bundle.is_some() {
            return Err(ConfigError::invalid(
                key("ssl-bundle"),
                "is not available for a SQLite source",
            ));
        }
    }
    Ok(SourceSettings {
        name: name.to_string(),
        url,
        driver,
        max_connections,
        min_connections,
        acquire_timeout: raw.acquire_timeout.unwrap_or(DEFAULT_ACQUIRE_TIMEOUT),
        idle_timeout: raw.idle_timeout,
        max_lifetime: raw.max_lifetime,
        test_before_acquire: raw.test_before_acquire.unwrap_or(true),
        ssl_mode,
        ssl_bundle: raw.ssl_bundle,
    })
}

/// Every configuration key this crate reads, as `(key, type, default, description)`, in the
/// shape of `oxbloom_ssl::PROPERTIES`. The keys of a named source are listed once with
/// `<name>` in place of the source name; they take the same values as the unnamed keys.
///
/// # Examples
///
/// ```
/// use oxbloom_data::PROPERTIES;
///
/// let (key, ty, default, _) = PROPERTIES.iter().find(|p| p.0 == "data.max-connections").unwrap();
/// assert_eq!((*key, *ty, *default), ("data.max-connections", "u32", Some("10")));
/// assert!(PROPERTIES.iter().any(|p| p.0 == "data.sources.<name>.url"));
/// ```
pub const PROPERTIES: &[(&str, &str, Option<&str>, &str)] = &[
    (
        "data.connect-on-startup",
        "bool",
        Some("true"),
        "Whether the pools connect when the application starts.",
    ),
    (
        "data.url",
        "String (secret)",
        None,
        "The connection URL of the default source; the scheme picks the driver (`postgres`, `postgresql`, `mysql`, `mariadb`, `sqlite`).",
    ),
    (
        "data.max-connections",
        "u32",
        Some("10"),
        "The most connections of the pool of the default source; 1 for an in-memory SQLite URL.",
    ),
    (
        "data.min-connections",
        "u32",
        Some("0"),
        "The connections the pool keeps open; not above max-connections.",
    ),
    (
        "data.acquire-timeout",
        "Duration",
        Some("30s"),
        "How long acquiring a connection may wait; above zero.",
    ),
    (
        "data.idle-timeout",
        "Duration",
        None,
        "How long a connection may sit idle before it is closed; above zero.",
    ),
    (
        "data.max-lifetime",
        "Duration",
        None,
        "How long a connection may live; above zero.",
    ),
    (
        "data.test-before-acquire",
        "bool",
        Some("true"),
        "Whether a connection is checked before it is handed out.",
    ),
    (
        "data.ssl-mode",
        "String",
        None,
        "`disable`, `prefer`, `require`, `verify-ca` or `verify-full`; not for SQLite.",
    ),
    (
        "data.ssl-bundle",
        "String",
        None,
        "The name of the SSL bundle with the trust roots or client certificate; not for SQLite.",
    ),
    (
        "data.sources.<name>.url",
        "String (secret)",
        None,
        "The connection URL of a named source; the name uses `[a-z0-9-]+` and is not `default`.",
    ),
    (
        "data.sources.<name>.max-connections",
        "u32",
        Some("10"),
        "The most connections of the pool of a named source; 1 for an in-memory SQLite URL.",
    ),
    (
        "data.sources.<name>.min-connections",
        "u32",
        Some("0"),
        "The connections the pool of a named source keeps open.",
    ),
    (
        "data.sources.<name>.acquire-timeout",
        "Duration",
        Some("30s"),
        "How long acquiring a connection of a named source may wait.",
    ),
    (
        "data.sources.<name>.idle-timeout",
        "Duration",
        None,
        "How long a connection of a named source may sit idle.",
    ),
    (
        "data.sources.<name>.max-lifetime",
        "Duration",
        None,
        "How long a connection of a named source may live.",
    ),
    (
        "data.sources.<name>.test-before-acquire",
        "bool",
        Some("true"),
        "Whether a connection of a named source is checked before it is handed out.",
    ),
    (
        "data.sources.<name>.ssl-mode",
        "String",
        None,
        "The TLS mode of a named source; not for SQLite.",
    ),
    (
        "data.sources.<name>.ssl-bundle",
        "String",
        None,
        "The SSL bundle of a named source; not for SQLite.",
    ),
];
