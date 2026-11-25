//! [`DataSources`], the registry of the configured pools.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use std::time::Duration;

use crate::pool::{self, Pool, TlsMaterial};
use crate::settings::{DEFAULT_SOURCE, DataSettings, Driver};
use crate::{DataError, SourceError};

/// One configured source: its name, its driver and its pool.
struct Entry {
    name: String,
    driver: Driver,
    pool: Pool,
}

/// The registry of every configured data source, each with its lazy sqlx pool.
///
/// The registry holds the `default` source of the unnamed `data.*` keys and the named
/// sources of `data.sources.<name>`, in that order. The typed accessors
/// [`sqlite`](Self::sqlite), [`postgres`](Self::postgres) and [`mysql`](Self::mysql) exist
/// when the cargo feature of the driver is on. The `Debug` output shows the source names and
/// their drivers, never a URL.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_config::Config;
/// use oxbloom_data::{DataError, DataSettings, DataSources};
///
/// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
/// let settings = DataSettings::from_config(&config)?;
/// // No source here names an `ssl-bundle`, so the TLS function is never called.
/// let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
///
/// let pool = sources.sqlite("default").ok_or("no default source")?;
/// let one: i64 = oxbloom_data::sqlx::query_scalar("SELECT 1").fetch_one(pool).await?;
/// assert_eq!(one, 1);
/// assert_eq!(format!("{sources:?}"), "DataSources { default: Sqlite }");
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
pub struct DataSources {
    entries: Vec<Entry>,
}

impl fmt::Debug for DataSources {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("DataSources");
        for entry in &self.entries {
            debug.field(&entry.name, &entry.driver);
        }
        debug.finish()
    }
}

impl DataSources {
    /// Builds a lazy pool for every source of `settings`, applying all its pool settings.
    ///
    /// Building opens no connection and needs no database; the first use of a pool connects.
    /// `tls` resolves an SSL bundle name to its PEM material and is called only for a source
    /// with an `ssl-bundle`, with the bundle name. A source with a bundle and no `ssl-mode`
    /// connects with `verify-full`. The drivers add the bundle's trust certificates to the
    /// platform roots instead of replacing them. An in-memory SQLite database keeps its connections open
    /// unless `idle-timeout` or `max-lifetime` is set, because closing the last connection
    /// destroys the database. `min-connections` is honoured only loosely by a lazy pool: sqlx
    /// opens and refills those connections in the background.
    ///
    /// Call it inside a Tokio runtime, and on the runtime the application keeps running, not a
    /// temporary one: sqlx ties the maintenance task of a pool to the runtime that is current
    /// when the pool is built.
    ///
    /// The pools come from sqlx, and sqlx's own `Debug` of the connect options (reachable
    /// through `connect_options()` of a pool) contains the password, so never log it.
    ///
    /// # Errors
    ///
    /// The first failing source stops the build. The [`SourceError`] names it, and
    /// [`SourceError::config_prefix`] gives its configuration key. Its error is:
    ///
    /// - [`DataError::Unsupported`] for a source whose driver feature is off.
    /// - [`DataError::Config`] for a URL the driver refuses, for example an unknown option
    ///   value. The message never contains the URL.
    /// - Any error of `tls`, such as [`DataError::Tls`].
    /// - [`DataError::VerifyCaWithTrust`] for a source with `ssl-mode = verify-ca` whose bundle
    ///   has a trust store: `verify-ca` does not check the host name, and the bundle's trust is
    ///   added to the platform roots, so any publicly trusted certificate would pass. Use
    ///   `verify-full`, or a bundle without a trust store.
    /// - [`DataError::Pool`] when it is called outside a Tokio runtime and there is a source.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "sqlite", not(feature = "postgres")))]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str(
    ///     "[data]\nurl = \"sqlite::memory:\"\nmax-connections = 1\nacquire-timeout = \"5s\"",
    /// )?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// let options = sources.sqlite("default").ok_or("no default")?.options();
    /// assert_eq!(options.get_acquire_timeout(), std::time::Duration::from_secs(5));
    ///
    /// // A Postgres source in a build without the `postgres` feature.
    /// let config = Config::from_str("[data]\nurl = \"postgres://app:pw@db/app\"")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// let err = DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap_err();
    /// assert_eq!(err.error(), DataError::Unsupported);
    /// assert_eq!(err.config_prefix(), "data");
    /// assert_eq!(
    ///     err.to_string(),
    ///     "data source `default`: the database driver is not available in this build"
    /// );
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "sqlite", not(feature = "postgres"))))]
    /// # fn main() {}
    /// ```
    pub fn build(
        settings: &DataSettings,
        tls: &dyn Fn(&str) -> Result<TlsMaterial, DataError>,
    ) -> Result<DataSources, SourceError> {
        let entries = settings
            .sources
            .iter()
            .map(|source| {
                Ok(Entry {
                    name: source.name.clone(),
                    driver: source.driver,
                    pool: pool::build(source, tls)
                        .map_err(|error| SourceError::new(&source.name, error))?,
                })
            })
            .collect::<Result<_, SourceError>>()?;
        Ok(DataSources { entries })
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The name of the default source, `default`, when the unnamed `data.*` keys configure
    /// one.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let build = |toml: &str| -> Result<DataSources, Box<dyn std::error::Error>> {
    ///     let settings = DataSettings::from_config(&Config::from_str(toml)?)?;
    ///     Ok(DataSources::build(&settings, &|_| Err(DataError::Tls))?)
    /// };
    /// assert_eq!(build("[data]\nurl = \"sqlite::memory:\"")?.default_name(), Some("default"));
    /// // Only a named source: there is no default.
    /// let named = build("[data.sources.cache]\nurl = \"sqlite::memory:\"")?;
    /// assert_eq!(named.default_name(), None);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn default_name(&self) -> Option<&str> {
        self.entry(DEFAULT_SOURCE).map(|e| e.name.as_str())
    }

    /// The names of all sources: `default` first when configured, then the named ones in name
    /// order.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str(
    ///     "[data]\nurl = \"sqlite::memory:\"\n[data.sources.cache]\nurl = \"sqlite::memory:\"",
    /// )?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// assert_eq!(sources.names(), ["default", "cache"]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// The driver of the source `name`, or `None` for an unknown name.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources, Driver};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// assert_eq!(sources.driver_of("default"), Some(Driver::Sqlite));
    /// assert_eq!(sources.driver_of("reports"), None);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn driver_of(&self, name: &str) -> Option<Driver> {
        self.entry(name).map(|e| e.driver)
    }

    /// The number of sources.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// assert_eq!(sources.len(), 1);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no sources, which is the case without a `data` section.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let settings = DataSettings::from_config(&Config::from_str("")?)?;
    /// let sources = DataSources::build(&settings, &|_| Err(DataError::Tls))?;
    /// assert!(sources.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The Postgres pool of the source `name`, or `None` when the name is unknown or the
    /// source uses another driver.
    ///
    /// Never log `connect_options()` of the pool: sqlx's `Debug` of it contains the password.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// // The pool is lazy: building it needs no server.
    /// let config = Config::from_str("[data]\nurl = \"postgres://app@db.internal/app\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// let pool = sources.postgres("default").ok_or("no default")?;
    /// assert_eq!(pool.size(), 0);
    /// assert!(sources.postgres("reports").is_none());
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "postgres")]
    pub fn postgres(&self, name: &str) -> Option<&sqlx::PgPool> {
        match &self.entry(name)?.pool {
            Pool::Postgres(pool) => Some(pool),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The MySQL or MariaDB pool of the source `name`, or `None` when the name is unknown or
    /// the source uses another driver.
    ///
    /// Never log `connect_options()` of the pool: sqlx's `Debug` of it contains the password.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// // The pool is lazy: building it needs no server.
    /// let config = Config::from_str("[data]\nurl = \"mariadb://app@db.internal/app\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// let pool = sources.mysql("default").ok_or("no default")?;
    /// assert_eq!(pool.size(), 0);
    /// assert!(sources.mysql("reports").is_none());
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "mysql")]
    pub fn mysql(&self, name: &str) -> Option<&sqlx::MySqlPool> {
        match &self.entry(name)?.pool {
            Pool::Mysql(pool) => Some(pool),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// The SQLite pool of the source `name`, or `None` when the name is unknown or the source
    /// uses another driver.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// let pool = sources.sqlite("default").ok_or("no default")?;
    /// oxbloom_data::sqlx::query("CREATE TABLE notes (body TEXT)").execute(pool).await?;
    /// assert!(sources.sqlite("reports").is_none());
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "sqlite")]
    pub fn sqlite(&self, name: &str) -> Option<&sqlx::SqlitePool> {
        match &self.entry(name)?.pool {
            Pool::Sqlite(pool) => Some(pool),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    /// Runs `SELECT 1` on the source `name` within `timeout`, whatever its driver.
    ///
    /// A ping takes a connection from the pool and opens one when none is idle.
    ///
    /// # Errors
    ///
    /// - [`DataError::Config`] for an unknown name.
    /// - [`DataError::Connect`] when the connection or the query fails.
    /// - [`DataError::Pool`] when `timeout` runs out (for example because every connection is
    ///   in use), the pool's acquire timeout runs out, or the pool is closed.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// sources.ping("default", Duration::from_secs(2)).await?;
    /// assert_eq!(sources.ping("reports", Duration::from_secs(2)).await, Err(DataError::Config));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub async fn ping(&self, name: &str, timeout: Duration) -> Result<(), DataError> {
        let entry = self.entry(name).ok_or(DataError::Config)?;
        tokio::time::timeout(timeout, entry.pool.ping())
            .await
            .unwrap_or(Err(DataError::Pool))
    }

    /// Runs [`ping`](Self::ping) on every source, one after the other and each within
    /// `timeout`, and returns the result per source name in the order of
    /// [`names`](Self::names).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// // The named source points into a directory that does not exist.
    /// let config = Config::from_str(
    ///     "[data]\nurl = \"sqlite::memory:\"\n\
    ///      [data.sources.broken]\nurl = \"sqlite:/no-such-oxbloom-dir/app.db\"",
    /// )?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// let results = sources.ping_all(Duration::from_secs(2)).await;
    /// assert_eq!(results[0], ("default".to_string(), Ok(())));
    /// assert_eq!(results[1], ("broken".to_string(), Err(DataError::Connect)));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub async fn ping_all(&self, timeout: Duration) -> Vec<(String, Result<(), DataError>)> {
        let mut results = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            results.push((entry.name.clone(), self.ping(&entry.name, timeout).await));
        }
        results
    }

    /// The open connections of the pool of `name` and how many of them are idle, as
    /// `(size, idle)`, or `None` for an unknown name.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// assert_eq!(sources.pool_counts("default"), Some((0, 0)));
    /// sources.ping("default", Duration::from_secs(2)).await?;
    /// // One connection is open; it is idle again once sqlx has taken it back.
    /// let (size, idle) = sources.pool_counts("default").ok_or("no default")?;
    /// assert_eq!(size, 1);
    /// assert!(idle <= 1);
    /// assert_eq!(sources.pool_counts("reports"), None);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn pool_counts(&self, name: &str) -> Option<(u32, u32)> {
        self.entry(name).map(|e| e.pool.counts())
    }

    /// Closes every pool and waits until each one has closed.
    ///
    /// Every pool is marked closed first, so all of them refuse new connections at once with
    /// [`DataError::Pool`], and then the closes run concurrently. A close waits until the
    /// connections in use are returned, so one connection that is never returned keeps this
    /// future pending; a caller at shutdown uses
    /// [`close_all_within`](Self::close_all_within) instead.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// sources.close_all().await;
    /// assert!(sources.sqlite("default").ok_or("no default")?.is_closed());
    /// assert_eq!(sources.ping("default", Duration::from_secs(2)).await, Err(DataError::Pool));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub async fn close_all(&self) {
        let mut pending = self.start_closing();
        join_closes(&mut pending).await;
    }

    /// Closes every pool like [`close_all`](Self::close_all), but waits at most `timeout`
    /// for all of them together, and returns the names of the sources whose close did not
    /// finish in that time, in the order of [`names`](Self::names). An empty list means that
    /// every pool closed.
    ///
    /// Every pool is marked closed before the wait begins, so a source that is still listed
    /// refuses new connections all the same; only its connections in use stay open until
    /// they are returned. Closing a closed registry again returns an empty list.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str(
    ///     "[data]\nurl = \"sqlite::memory:\"\n[data.sources.cache]\nurl = \"sqlite::memory:\"",
    /// )?;
    /// let sources = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))?;
    /// // A connection of `cache` is checked out and not returned within the time.
    /// let held = sources.sqlite("cache").ok_or("no cache")?.acquire().await?;
    /// let unfinished = sources.close_all_within(Duration::from_millis(100)).await;
    /// assert_eq!(unfinished, ["cache"]);
    /// assert!(sources.sqlite("default").ok_or("no default")?.is_closed());
    /// assert!(sources.sqlite("cache").ok_or("no cache")?.is_closed());
    /// drop(held);
    /// assert!(sources.close_all_within(Duration::from_secs(5)).await.is_empty());
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub async fn close_all_within(&self, timeout: Duration) -> Vec<String> {
        let mut pending = self.start_closing();
        // The timeout drops only the joining future; `pending` keeps what did not finish.
        let _ = tokio::time::timeout(timeout, join_closes(&mut pending)).await;
        pending
            .into_iter()
            .filter(|(_, close)| close.is_some())
            .map(|(name, _)| name.to_owned())
            .collect()
    }

    /// Marks every pool closed, in the order of the sources, and returns the futures that wait
    /// for the closes.
    fn start_closing(&self) -> Vec<(&str, Option<Close<'_>>)> {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), Some(entry.pool.close())))
            .collect()
    }
}

/// The pending close of one pool.
type Close<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Drives every pending close concurrently until all have finished; a finished close is set to
/// `None`, so after a timeout the remaining `Some` entries are the closes that did not finish.
async fn join_closes(pending: &mut [(&str, Option<Close<'_>>)]) {
    std::future::poll_fn(|cx| {
        let mut done = true;
        for (_, slot) in pending.iter_mut() {
            if let Some(close) = slot {
                if close.as_mut().poll(cx).is_ready() {
                    *slot = None;
                } else {
                    done = false;
                }
            }
        }
        if done { Poll::Ready(()) } else { Poll::Pending }
    })
    .await;
}
