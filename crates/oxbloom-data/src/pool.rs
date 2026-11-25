//! Builds one lazy sqlx pool from a [`SourceSettings`].
//!
//! The URL is read only inside the functions that build the connect options of a driver, and
//! every sqlx error is mapped to a [`DataError`] with fixed text: sqlx parse errors echo the
//! values of URL options, and its configuration and database errors can contain a host, a
//! user name or SQL text.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use crate::DataError;
#[cfg(any(feature = "postgres", feature = "mysql"))]
use crate::settings::SslMode;
use crate::settings::{Driver, SourceSettings};

/// The TLS material of an SSL bundle, as PEM, for the sources that name the bundle in
/// `ssl-bundle`.
///
/// The caller resolves the bundle and hands the material to
/// [`DataSources::build`](crate::DataSources::build). `trust_pem` holds the trusted
/// certificates. The database drivers add them to the platform roots (unlike the other users
/// of an SSL bundle, where the bundle's trust replaces the platform roots), so a source with
/// a trust store and `ssl-mode = verify-ca` is refused; without `trust_pem` the platform
/// roots alone stay in use. `identity` holds the client
/// certificate chain and its private key. The `Debug` output shows only which parts are
/// present. A pool keeps the material it was built with and does not follow a reload of the
/// bundle.
///
/// # Examples
///
/// ```
/// use oxbloom_data::TlsMaterial;
///
/// let material = TlsMaterial {
///     trust_pem: Some(b"-----BEGIN CERTIFICATE-----\n...".to_vec()),
///     identity: None,
/// };
/// assert_eq!(
///     format!("{material:?}"),
///     "TlsMaterial { trust_pem: Some(\"<redacted>\"), identity: None }"
/// );
/// ```
#[derive(Clone, Default)]
pub struct TlsMaterial {
    /// The trusted certificates as PEM, or `None` to keep the platform roots.
    pub trust_pem: Option<Vec<u8>>,
    /// The client certificate chain and its private key, both as PEM.
    pub identity: Option<(Vec<u8>, Vec<u8>)>,
}

impl fmt::Debug for TlsMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const REDACTED: &str = "<redacted>";
        f.debug_struct("TlsMaterial")
            .field("trust_pem", &self.trust_pem.as_ref().map(|_| REDACTED))
            .field(
                "identity",
                &self.identity.as_ref().map(|_| (REDACTED, REDACTED)),
            )
            .finish()
    }
}

/// The function that resolves an SSL bundle name to its [`TlsMaterial`].
pub(crate) type TlsResolver<'a> = &'a dyn Fn(&str) -> Result<TlsMaterial, DataError>;

/// The pool of one source, of whichever driver its URL names. Without any driver feature
/// this enum has no variants and no value of it can exist.
pub(crate) enum Pool {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    #[cfg(feature = "mysql")]
    Mysql(sqlx::MySqlPool),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::SqlitePool),
}

/// Runs `$body` with `$pool` bound to the sqlx pool of `$self`, whatever its driver.
macro_rules! each_pool {
    ($self:expr, $pool:ident => $body:expr) => {
        match *$self {
            #[cfg(feature = "postgres")]
            Pool::Postgres(ref $pool) => $body,
            #[cfg(feature = "mysql")]
            Pool::Mysql(ref $pool) => $body,
            #[cfg(feature = "sqlite")]
            Pool::Sqlite(ref $pool) => $body,
        }
    };
}

impl Pool {
    /// Runs `SELECT 1` on a connection of the pool.
    pub(crate) async fn ping(&self) -> Result<(), DataError> {
        each_pool!(self, pool => sqlx::query("SELECT 1")
            .execute(pool)
            .await
            .map(|_| ())
            .map_err(|e| map_error(&e)))
    }

    /// Reads the highest `version` with `success = true` of the history table `table`, which
    /// the caller has already checked as a plain identifier, which is why the text is safe to
    /// run. The statement only reads.
    ///
    /// `sqlx` creates and reads the history table with its name unquoted, and Postgres folds an
    /// unquoted name to lower case, so on Postgres the quoted name is the lower-case one; a
    /// name with capitals would otherwise name a table that does not exist.
    #[cfg_attr(
        not(any(feature = "postgres", feature = "mysql", feature = "sqlite")),
        allow(unused_variables)
    )]
    pub(crate) async fn latest_migration_version(
        &self,
        table: &str,
    ) -> Result<Option<i64>, DataError> {
        match *self {
            #[cfg(feature = "postgres")]
            Pool::Postgres(ref pool) => {
                let table = table.to_ascii_lowercase();
                let sql = format!("SELECT MAX(version) FROM \"{table}\" WHERE success = true");
                sqlx::query_scalar::<_, Option<i64>>(sqlx::AssertSqlSafe(sql))
                    .fetch_one(pool)
                    .await
                    .map_err(|e| map_error(&e))
            }
            #[cfg(feature = "mysql")]
            Pool::Mysql(ref pool) => {
                let sql = format!("SELECT MAX(version) FROM `{table}` WHERE success = true");
                sqlx::query_scalar::<_, Option<i64>>(sqlx::AssertSqlSafe(sql))
                    .fetch_one(pool)
                    .await
                    .map_err(|e| map_error(&e))
            }
            #[cfg(feature = "sqlite")]
            Pool::Sqlite(ref pool) => {
                let sql = format!("SELECT MAX(version) FROM \"{table}\" WHERE success = true");
                sqlx::query_scalar::<_, Option<i64>>(sqlx::AssertSqlSafe(sql))
                    .fetch_one(pool)
                    .await
                    .map_err(|e| map_error(&e))
            }
        }
    }

    /// Marks the pool closed at once, so it refuses new connections, and returns the future
    /// that waits until its connections are returned and closed. sqlx marks the pool closed
    /// when `close` is called, not when the future is first polled, so the caller can mark
    /// every pool closed before it waits for any of them.
    pub(crate) fn close(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        each_pool!(self, pool => Box::pin(pool.close()))
    }

    /// The open connections and the idle ones among them.
    pub(crate) fn counts(&self) -> (u32, u32) {
        each_pool!(self, pool => (
            pool.size(),
            u32::try_from(pool.num_idle()).unwrap_or(u32::MAX),
        ))
    }
}

impl fmt::Debug for Pool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Pool").field(&self.driver()).finish()
    }
}

impl Pool {
    fn driver(&self) -> Driver {
        match *self {
            #[cfg(feature = "postgres")]
            Pool::Postgres(_) => Driver::Postgres,
            #[cfg(feature = "mysql")]
            Pool::Mysql(_) => Driver::Mysql,
            #[cfg(feature = "sqlite")]
            Pool::Sqlite(_) => Driver::Sqlite,
        }
    }
}

/// Maps a sqlx error to fixed text: a timed-out or closed pool is [`DataError::Pool`],
/// everything else is a failed connection or query, [`DataError::Connect`]. The sqlx error
/// itself is never shown.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn map_error(error: &sqlx::Error) -> DataError {
    match error {
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed => DataError::Pool,
        _ => DataError::Connect,
    }
}

/// Builds the lazy pool of `source`. `tls` is called with the bundle name only when the source
/// has an `ssl-bundle`.
pub(crate) fn build(source: &SourceSettings, tls: TlsResolver<'_>) -> Result<Pool, DataError> {
    match source.driver {
        Driver::Postgres => postgres(source, tls),
        Driver::Mysql => mysql(source, tls),
        Driver::Sqlite => sqlite(source),
    }
}

/// sqlx spawns the maintenance tasks of a pool on the current Tokio runtime and panics
/// without one, so building outside a runtime is refused instead.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn ensure_runtime() -> Result<(), DataError> {
    tokio::runtime::Handle::try_current()
        .map(|_| ())
        .map_err(|_| DataError::Pool)
}

/// The TLS mode to set on the connect options: the configured `ssl-mode`, or `verify-full`
/// when the source has an `ssl-bundle` and no mode, so the bundle's trust roots are really
/// checked. Postgres and MySQL accept any certificate in `prefer` and `require`, and `prefer`
/// falls back to plain text, so a weaker mode from the URL or the environment must not
/// apply. `None` leaves the mode of the URL and the driver.
#[cfg(any(feature = "postgres", feature = "mysql"))]
fn effective_ssl_mode(source: &SourceSettings) -> Option<SslMode> {
    source
        .ssl_mode
        .or(source.ssl_bundle.is_some().then_some(SslMode::VerifyFull))
}

/// The bundle's trust certificates are added to the platform roots by sqlx, and `verify-ca`
/// does not check the host name, so `verify-ca` with a bundle that has a trust store would
/// accept any publicly trusted certificate. That pair is refused.
#[cfg(any(feature = "postgres", feature = "mysql"))]
fn refuse_verify_ca_with_trust(
    source: &SourceSettings,
    material: &TlsMaterial,
) -> Result<(), DataError> {
    if source.ssl_mode == Some(SslMode::VerifyCa) && material.trust_pem.is_some() {
        return Err(DataError::VerifyCaWithTrust);
    }
    Ok(())
}

/// The pool options of `source`. For an in-memory SQLite database an unset idle timeout or
/// lifetime means never, because closing its last connection destroys the database; for any
/// other source the sqlx defaults stay.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn pool_options<DB: sqlx::Database>(
    source: &SourceSettings,
    in_memory: bool,
) -> sqlx::pool::PoolOptions<DB> {
    let mut options = sqlx::pool::PoolOptions::<DB>::new()
        .max_connections(source.max_connections)
        .min_connections(source.min_connections)
        .acquire_timeout(source.acquire_timeout)
        .test_before_acquire(source.test_before_acquire);
    if source.idle_timeout.is_some() || in_memory {
        options = options.idle_timeout(source.idle_timeout);
    }
    if source.max_lifetime.is_some() || in_memory {
        options = options.max_lifetime(source.max_lifetime);
    }
    options
}

#[cfg(feature = "postgres")]
fn postgres(source: &SourceSettings, tls: TlsResolver<'_>) -> Result<Pool, DataError> {
    let options = postgres_connect_options(source, tls)?;
    ensure_runtime()?;
    Ok(Pool::Postgres(
        pool_options(source, false).connect_lazy_with(options),
    ))
}

#[cfg(feature = "postgres")]
fn postgres_connect_options(
    source: &SourceSettings,
    tls: TlsResolver<'_>,
) -> Result<sqlx::postgres::PgConnectOptions, DataError> {
    use sqlx::postgres::{PgConnectOptions, PgSslMode};
    use std::str::FromStr;

    let mut options =
        PgConnectOptions::from_str(source.url.expose()).map_err(|_| DataError::Config)?;
    if let Some(mode) = effective_ssl_mode(source) {
        options = options.ssl_mode(match mode {
            SslMode::Disable => PgSslMode::Disable,
            SslMode::Prefer => PgSslMode::Prefer,
            SslMode::Require => PgSslMode::Require,
            SslMode::VerifyCa => PgSslMode::VerifyCa,
            SslMode::VerifyFull => PgSslMode::VerifyFull,
        });
    }
    if let Some(bundle) = &source.ssl_bundle {
        let material = tls(bundle)?;
        refuse_verify_ca_with_trust(source, &material)?;
        let TlsMaterial {
            trust_pem,
            identity,
        } = material;
        if let Some(pem) = trust_pem {
            options = options.ssl_root_cert_from_pem(pem);
        }
        if let Some((chain, key)) = identity {
            options = options
                .ssl_client_cert_from_pem(chain)
                .ssl_client_key_from_pem(key);
        }
    }
    Ok(options)
}

#[cfg(not(feature = "postgres"))]
fn postgres(_: &SourceSettings, _: TlsResolver<'_>) -> Result<Pool, DataError> {
    Err(DataError::Unsupported)
}

#[cfg(feature = "mysql")]
fn mysql(source: &SourceSettings, tls: TlsResolver<'_>) -> Result<Pool, DataError> {
    let options = mysql_connect_options(source, tls)?;
    ensure_runtime()?;
    Ok(Pool::Mysql(
        pool_options(source, false).connect_lazy_with(options),
    ))
}

#[cfg(feature = "mysql")]
fn mysql_connect_options(
    source: &SourceSettings,
    tls: TlsResolver<'_>,
) -> Result<sqlx::mysql::MySqlConnectOptions, DataError> {
    use sqlx::mysql::{MySqlConnectOptions, MySqlSslMode};
    use std::str::FromStr;

    let mut options =
        MySqlConnectOptions::from_str(source.url.expose()).map_err(|_| DataError::Config)?;
    if let Some(mode) = effective_ssl_mode(source) {
        options = options.ssl_mode(match mode {
            SslMode::Disable => MySqlSslMode::Disabled,
            SslMode::Prefer => MySqlSslMode::Preferred,
            SslMode::Require => MySqlSslMode::Required,
            SslMode::VerifyCa => MySqlSslMode::VerifyCa,
            SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
        });
    }
    if let Some(bundle) = &source.ssl_bundle {
        let material = tls(bundle)?;
        refuse_verify_ca_with_trust(source, &material)?;
        let TlsMaterial {
            trust_pem,
            identity,
        } = material;
        if let Some(pem) = trust_pem {
            options = options.ssl_ca_from_pem(pem);
        }
        if let Some((chain, key)) = identity {
            options = options
                .ssl_client_cert_from_pem(chain)
                .ssl_client_key_from_pem(key);
        }
    }
    Ok(options)
}

#[cfg(not(feature = "mysql"))]
fn mysql(_: &SourceSettings, _: TlsResolver<'_>) -> Result<Pool, DataError> {
    Err(DataError::Unsupported)
}

#[cfg(feature = "sqlite")]
fn sqlite(source: &SourceSettings) -> Result<Pool, DataError> {
    let (options, in_memory) = sqlite_connect_options(source)?;
    ensure_runtime()?;
    Ok(Pool::Sqlite(
        pool_options(source, in_memory).connect_lazy_with(options),
    ))
}

/// The connect options and whether the URL names an in-memory database. SQLite has no TLS;
/// the settings refuse `ssl-mode` and `ssl-bundle` on a SQLite source.
#[cfg(feature = "sqlite")]
fn sqlite_connect_options(
    source: &SourceSettings,
) -> Result<(sqlx::sqlite::SqliteConnectOptions, bool), DataError> {
    use std::str::FromStr;

    let url = source.url.expose();
    let options =
        sqlx::sqlite::SqliteConnectOptions::from_str(url).map_err(|_| DataError::Config)?;
    Ok((options, crate::settings::sqlite_memory(url).in_memory))
}

#[cfg(not(feature = "sqlite"))]
fn sqlite(_: &SourceSettings) -> Result<Pool, DataError> {
    Err(DataError::Unsupported)
}
