//! The explicit transaction closure API: [`transaction`], its [`TransactionOptions`] and
//! [`TransactionError`], and the [`RetryableError`] classification of serialization failures
//! and deadlocks.
//!
//! The per-driver parts (the begin statement that applies the isolation level and the
//! read-only flag, and which driver errors are worth a retry) live in the sealed trait
//! [`TransactionDatabase`], implemented for the enabled drivers only.

use std::fmt;

/// The isolation level of a transaction, set with [`TransactionOptions::isolation`].
///
/// Postgres and MySQL accept every level. SQLite transactions are always serializable, so a
/// SQLite pool accepts only [`Isolation::Serializable`] and refuses the others with
/// [`TransactionError::Unsupported`].
///
/// # Examples
///
/// ```
/// use oxbloom_data::{Isolation, TransactionOptions};
///
/// let options = TransactionOptions::default().isolation(Isolation::RepeatableRead);
/// assert_eq!(options.get_isolation(), Some(Isolation::RepeatableRead));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Isolation {
    /// `READ UNCOMMITTED`: a transaction can see uncommitted changes of others.
    ReadUncommitted,
    /// `READ COMMITTED`: each statement sees the changes committed before it started.
    ReadCommitted,
    /// `REPEATABLE READ`: repeated reads in one transaction see the same rows.
    RepeatableRead,
    /// `SERIALIZABLE`: the transactions behave as if they ran one after another.
    Serializable,
}

#[cfg(test)]
impl Isolation {
    /// The SQL name of the level.
    fn sql(self) -> &'static str {
        match self {
            Self::ReadUncommitted => "READ UNCOMMITTED",
            Self::ReadCommitted => "READ COMMITTED",
            Self::RepeatableRead => "REPEATABLE READ",
            Self::Serializable => "SERIALIZABLE",
        }
    }
}

/// The options of one [`transaction`] (or `transaction_with_retry`) call.
///
/// The default is the driver's own isolation level and a read-write transaction. The struct
/// is the same in every configuration of the cargo features; the retry policy is an argument
/// of `transaction_with_retry`.
/// The options are applied through each driver's begin statement: Postgres runs
/// `BEGIN ISOLATION LEVEL .. READ ONLY`, MySQL runs `SET TRANSACTION ISOLATION LEVEL ..`
/// followed by `START TRANSACTION READ ONLY`. An option a driver cannot honor is refused with
/// [`TransactionError::Unsupported`] before a connection is acquired, never ignored: SQLite
/// refuses `read_only(true)` and every isolation level except
/// [`Serializable`](Isolation::Serializable).
///
/// # Examples
///
/// ```
/// use oxbloom_data::{Isolation, TransactionOptions};
///
/// let options = TransactionOptions::default()
///     .isolation(Isolation::Serializable)
///     .read_only(true);
/// assert_eq!(options.get_isolation(), Some(Isolation::Serializable));
/// assert!(options.is_read_only());
/// assert!(!TransactionOptions::default().is_read_only());
/// ```
#[derive(Debug, Clone, Default)]
pub struct TransactionOptions {
    isolation: Option<Isolation>,
    read_only: bool,
}

impl TransactionOptions {
    /// Sets the isolation level of the transaction.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_data::sqlx::{self, sqlite::SqlitePoolOptions};
    /// use oxbloom_data::{Isolation, TransactionError, TransactionOptions, transaction};
    ///
    /// let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
    /// let serializable = TransactionOptions::default().isolation(Isolation::Serializable);
    /// let one: i64 = transaction(&pool, &serializable, async |tx| {
    ///     sqlx::query_scalar("SELECT 1").fetch_one(&mut **tx).await
    /// })
    /// .await?;
    /// assert_eq!(one, 1);
    ///
    /// // SQLite has no weaker level, so it refuses the option instead of ignoring it.
    /// let weaker = TransactionOptions::default().isolation(Isolation::ReadCommitted);
    /// let err = transaction(&pool, &weaker, async |_tx| Ok::<_, sqlx::Error>(())).await;
    /// assert!(matches!(err, Err(TransactionError::Unsupported)));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    #[must_use]
    pub fn isolation(mut self, isolation: Isolation) -> Self {
        self.isolation = Some(isolation);
        self
    }

    /// Makes the transaction read-only (`true`) or read-write (`false`, the default).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use oxbloom_data::sqlx::{self, sqlite::SqlitePoolOptions};
    /// use oxbloom_data::{TransactionError, TransactionOptions, transaction};
    ///
    /// let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
    /// // Postgres and MySQL honor a read-only transaction; SQLite cannot and refuses it.
    /// let read_only = TransactionOptions::default().read_only(true);
    /// let err = transaction(&pool, &read_only, async |_tx| Ok::<_, sqlx::Error>(())).await;
    /// assert!(matches!(err, Err(TransactionError::Unsupported)));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    #[must_use]
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// The isolation level, or `None` for the driver's default.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::{Isolation, TransactionOptions};
    ///
    /// assert_eq!(TransactionOptions::default().get_isolation(), None);
    /// let options = TransactionOptions::default().isolation(Isolation::ReadCommitted);
    /// assert_eq!(options.get_isolation(), Some(Isolation::ReadCommitted));
    /// ```
    pub fn get_isolation(&self) -> Option<Isolation> {
        self.isolation
    }

    /// Whether the transaction is read-only.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::TransactionOptions;
    ///
    /// assert!(TransactionOptions::default().read_only(true).is_read_only());
    /// ```
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }
}

/// The failure of a [`transaction`].
///
/// It separates a failed begin, a failed commit, options the driver cannot honor, and the
/// error of the work itself. The `Display` text is fixed and the `Debug` output names only
/// the variant (`Inner(..)` for the work's error), so neither contains SQL, row data or the
/// text of a driver error. The error of the work stays available through
/// [`into_inner`](TransactionError::into_inner) and, when it is a standard error, through
/// [`source`](std::error::Error::source).
///
/// # Examples
///
/// ```
/// use oxbloom_data::TransactionError;
///
/// let err: TransactionError<std::io::Error> = TransactionError::Commit;
/// assert_eq!(err.to_string(), "could not commit the database transaction");
/// assert_eq!(format!("{err:?}"), "Commit");
///
/// let err = TransactionError::Inner(std::io::Error::other("disk full"));
/// assert_eq!(format!("{err:?}"), "Inner(..)");
/// assert_eq!(err.into_inner().map(|e| e.to_string()), Some("disk full".to_string()));
/// ```
#[non_exhaustive]
pub enum TransactionError<E> {
    /// The transaction could not be started: no connection, or the begin statement failed.
    Begin,
    /// The work succeeded, but the commit failed. When the database refused the commit, the
    /// transaction is rolled back; when the connection was lost during the `COMMIT`, the
    /// outcome is unknown: the database may have committed the transaction or not.
    Commit,
    /// The driver cannot honor the [`TransactionOptions`]; nothing was run.
    Unsupported,
    /// The work returned this error; the transaction was rolled back.
    Inner(E),
}

impl<E> TransactionError<E> {
    /// The error of the work, or `None` for the other variants.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::TransactionError;
    ///
    /// assert_eq!(TransactionError::Inner("out of stock").into_inner(), Some("out of stock"));
    /// assert_eq!(TransactionError::<&str>::Begin.into_inner(), None);
    /// ```
    pub fn into_inner(self) -> Option<E> {
        match self {
            Self::Inner(error) => Some(error),
            Self::Begin | Self::Commit | Self::Unsupported => None,
        }
    }
}

impl<E> fmt::Display for TransactionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Begin => "could not begin the database transaction",
            Self::Commit => "could not commit the database transaction",
            Self::Unsupported => "the database driver does not support these transaction options",
            Self::Inner(_) => "the work of the database transaction failed and it was rolled back",
        })
    }
}

/// Written by hand so that no variant shows a driver error or the work's error, and so that
/// it needs no `E: Debug`.
impl<E> fmt::Debug for TransactionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Begin => "Begin",
            Self::Commit => "Commit",
            Self::Unsupported => "Unsupported",
            Self::Inner(_) => "Inner(..)",
        })
    }
}

impl<E: std::error::Error + 'static> std::error::Error for TransactionError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inner(error) => Some(error),
            Self::Begin | Self::Commit | Self::Unsupported => None,
        }
    }
}

/// Whether an error is a serialization failure or a deadlock, after which a transaction may
/// succeed when it runs again.
///
/// `transaction_with_retry` (cargo feature `retry`) requires it of the work's error type and
/// runs the work again when it returns `true`. It is implemented for `sqlx::Error`
/// (Postgres SQLSTATE `40001` and `40P01`, MySQL errors `1213` and `1205`, SQLite
/// `SQLITE_BUSY` and `SQLITE_LOCKED`, for each enabled driver), for a boxed error that holds a
/// `sqlx::Error` anywhere in its [`source`](std::error::Error::source) chain, and for a
/// [`TransactionError`] whose inner error is retryable. Implement it for an application error
/// that wraps a `sqlx::Error` by delegating to it.
///
/// SQLite reports `SQLITE_LOCKED` also for a conflict within one connection, such as dropping
/// a table that an open statement still reads; such an error fails the same way on every run,
/// so it is retried until the policy's `max_attempts` runs out and then returned.
///
/// # Examples
///
/// ```
/// use oxbloom_data::RetryableError;
/// use oxbloom_data::sqlx;
///
/// enum OrderError {
///     Database(sqlx::Error),
///     OutOfStock,
/// }
///
/// impl RetryableError for OrderError {
///     fn is_retryable(&self) -> bool {
///         match self {
///             OrderError::Database(e) => e.is_retryable(),
///             OrderError::OutOfStock => false,
///         }
///     }
/// }
///
/// assert!(!OrderError::OutOfStock.is_retryable());
/// assert!(!OrderError::Database(sqlx::Error::RowNotFound).is_retryable());
/// ```
pub trait RetryableError {
    /// `true` when running the transaction again may succeed.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::RetryableError;
    ///
    /// // A pool timeout is not a serialization failure.
    /// assert!(!oxbloom_data::sqlx::Error::PoolTimedOut.is_retryable());
    /// ```
    fn is_retryable(&self) -> bool;
}

impl RetryableError for sqlx::Error {
    fn is_retryable(&self) -> bool {
        #[cfg(feature = "postgres")]
        if <sqlx::Postgres as private::Sealed>::is_retryable(self) {
            return true;
        }
        #[cfg(feature = "mysql")]
        if <sqlx::MySql as private::Sealed>::is_retryable(self) {
            return true;
        }
        #[cfg(feature = "sqlite")]
        if <sqlx::Sqlite as private::Sealed>::is_retryable(self) {
            return true;
        }
        false
    }
}

/// Whether `error` or any error in its source chain is a retryable `sqlx::Error`.
fn chain_is_retryable(error: &(dyn std::error::Error + 'static)) -> bool {
    std::iter::successors(Some(error), |e| e.source())
        .filter_map(|e| e.downcast_ref::<sqlx::Error>())
        .any(RetryableError::is_retryable)
}

impl RetryableError for Box<dyn std::error::Error + Send + Sync> {
    fn is_retryable(&self) -> bool {
        chain_is_retryable(self.as_ref())
    }
}

impl RetryableError for Box<dyn std::error::Error> {
    fn is_retryable(&self) -> bool {
        chain_is_retryable(self.as_ref())
    }
}

impl<E: RetryableError> RetryableError for TransactionError<E> {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Inner(error) => error.is_retryable(),
            Self::Begin | Self::Commit | Self::Unsupported => false,
        }
    }
}

/// A sqlx database type that [`transaction`] supports: `Postgres`, `MySql` or `Sqlite`, for
/// each enabled driver feature.
///
/// The trait is sealed. It carries the driver's begin statement for the
/// [`TransactionOptions`] and the test for retryable errors.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_data::sqlx::{self, Pool, sqlite::SqlitePoolOptions};
/// use oxbloom_data::{TransactionDatabase, TransactionError, TransactionOptions, transaction};
///
/// // Generic over the driver, for code that works with any enabled database.
/// async fn touch<DB: TransactionDatabase>(
///     pool: &Pool<DB>,
/// ) -> Result<(), TransactionError<sqlx::Error>> {
///     transaction(pool, &TransactionOptions::default(), async |_tx| Ok(())).await
/// }
///
/// let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
/// touch(&pool).await?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
pub trait TransactionDatabase: private::Sealed {}

impl<DB: private::Sealed> TransactionDatabase for DB {}

mod private {
    use super::Isolation;

    /// The per-driver parts of a transaction; not nameable outside the crate.
    pub trait Sealed: sqlx::Database {
        /// The statement that begins a transaction with these options, `Ok(None)` for the
        /// driver's plain begin, or `Err(())` when the driver cannot honor the options.
        fn begin_statement(
            isolation: Option<Isolation>,
            read_only: bool,
        ) -> Result<Option<&'static str>, ()>;

        /// Whether `error` is a serialization failure or a deadlock of this driver.
        fn is_retryable(error: &sqlx::Error) -> bool;
    }
}

/// The driver error of `error`, when it is a database error of the driver type `T`.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn database_error<T: sqlx::error::DatabaseError>(error: &sqlx::Error) -> Option<&T> {
    match error {
        sqlx::Error::Database(db) => db.try_downcast_ref::<T>(),
        _ => None,
    }
}

/// Postgres: `40001` is `serialization_failure`, `40P01` is `deadlock_detected`.
#[cfg(any(feature = "postgres", test))]
fn postgres_code_is_retryable(sqlstate: &str) -> bool {
    matches!(sqlstate, "40001" | "40P01")
}

/// MySQL: `1213` is `ER_LOCK_DEADLOCK`, `1205` is `ER_LOCK_WAIT_TIMEOUT`.
#[cfg(any(feature = "mysql", test))]
fn mysql_number_is_retryable(number: u16) -> bool {
    matches!(number, 1213 | 1205)
}

/// SQLite: the primary result code (the low byte of the extended code) `SQLITE_BUSY` (5) or
/// `SQLITE_LOCKED` (6), so `SQLITE_BUSY_SNAPSHOT` (517) counts as busy too.
#[cfg(any(feature = "sqlite", test))]
fn sqlite_code_is_retryable(extended_code: &str) -> bool {
    extended_code
        .parse::<i32>()
        .is_ok_and(|code| matches!(code & 0xff, 5 | 6))
}

/// The Postgres begin statement: `BEGIN ISOLATION LEVEL <level> [READ ONLY]`.
#[cfg(any(feature = "postgres", test))]
fn postgres_begin(isolation: Option<Isolation>, read_only: bool) -> Option<&'static str> {
    use Isolation::{ReadCommitted, ReadUncommitted, RepeatableRead, Serializable};
    Some(match (isolation, read_only) {
        (None, false) => return None,
        (None, true) => "BEGIN READ ONLY",
        (Some(ReadUncommitted), false) => "BEGIN ISOLATION LEVEL READ UNCOMMITTED",
        (Some(ReadUncommitted), true) => "BEGIN ISOLATION LEVEL READ UNCOMMITTED READ ONLY",
        (Some(ReadCommitted), false) => "BEGIN ISOLATION LEVEL READ COMMITTED",
        (Some(ReadCommitted), true) => "BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY",
        (Some(RepeatableRead), false) => "BEGIN ISOLATION LEVEL REPEATABLE READ",
        (Some(RepeatableRead), true) => "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY",
        (Some(Serializable), false) => "BEGIN ISOLATION LEVEL SERIALIZABLE",
        (Some(Serializable), true) => "BEGIN ISOLATION LEVEL SERIALIZABLE READ ONLY",
    })
}

/// The MySQL begin statement. `SET TRANSACTION` without `SESSION` applies to the next
/// transaction only; sqlx runs both statements in one text-protocol query (the connection
/// allows multiple statements) and then checks that a transaction is open.
#[cfg(any(feature = "mysql", test))]
fn mysql_begin(isolation: Option<Isolation>, read_only: bool) -> Option<&'static str> {
    use Isolation::{ReadCommitted, ReadUncommitted, RepeatableRead, Serializable};
    Some(match (isolation, read_only) {
        (None, false) => return None,
        (None, true) => "START TRANSACTION READ ONLY",
        (Some(ReadUncommitted), false) => {
            "SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED; START TRANSACTION"
        }
        (Some(ReadUncommitted), true) => {
            "SET TRANSACTION ISOLATION LEVEL READ UNCOMMITTED; START TRANSACTION READ ONLY"
        }
        (Some(ReadCommitted), false) => {
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED; START TRANSACTION"
        }
        (Some(ReadCommitted), true) => {
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED; START TRANSACTION READ ONLY"
        }
        (Some(RepeatableRead), false) => {
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ; START TRANSACTION"
        }
        (Some(RepeatableRead), true) => {
            "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ; START TRANSACTION READ ONLY"
        }
        (Some(Serializable), false) => {
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE; START TRANSACTION"
        }
        (Some(Serializable), true) => {
            "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE; START TRANSACTION READ ONLY"
        }
    })
}

/// SQLite: transactions are always serializable and cannot be made read-only, so only the
/// default options and [`Isolation::Serializable`] are accepted, with the plain `BEGIN`.
#[cfg(any(feature = "sqlite", test))]
fn sqlite_begin(isolation: Option<Isolation>, read_only: bool) -> Result<Option<&'static str>, ()> {
    match (isolation, read_only) {
        (None | Some(Isolation::Serializable), false) => Ok(None),
        _ => Err(()),
    }
}

#[cfg(feature = "postgres")]
impl private::Sealed for sqlx::Postgres {
    fn begin_statement(
        isolation: Option<Isolation>,
        read_only: bool,
    ) -> Result<Option<&'static str>, ()> {
        Ok(postgres_begin(isolation, read_only))
    }

    fn is_retryable(error: &sqlx::Error) -> bool {
        database_error::<sqlx::postgres::PgDatabaseError>(error)
            .is_some_and(|e| postgres_code_is_retryable(e.code()))
    }
}

#[cfg(feature = "mysql")]
impl private::Sealed for sqlx::MySql {
    fn begin_statement(
        isolation: Option<Isolation>,
        read_only: bool,
    ) -> Result<Option<&'static str>, ()> {
        Ok(mysql_begin(isolation, read_only))
    }

    fn is_retryable(error: &sqlx::Error) -> bool {
        database_error::<sqlx::mysql::MySqlDatabaseError>(error)
            .is_some_and(|e| mysql_number_is_retryable(e.number()))
    }
}

#[cfg(feature = "sqlite")]
impl private::Sealed for sqlx::Sqlite {
    fn begin_statement(
        isolation: Option<Isolation>,
        read_only: bool,
    ) -> Result<Option<&'static str>, ()> {
        sqlite_begin(isolation, read_only)
    }

    fn is_retryable(error: &sqlx::Error) -> bool {
        use sqlx::error::DatabaseError;
        database_error::<sqlx::sqlite::SqliteError>(error)
            .and_then(DatabaseError::code)
            .is_some_and(|code| sqlite_code_is_retryable(&code))
    }
}

/// Runs `work` in a database transaction on a connection of `pool`, commits when it
/// returns `Ok` and rolls back when it returns `Err`.
///
/// `pool` is a sqlx pool of any enabled driver. `work` is an async closure that gets
/// the transaction as `&mut sqlx::Transaction`; run statements on it with
/// `.execute(&mut **tx)`. Its error type `E` is the application's own; it comes back
/// as [`TransactionError::Inner`] after the rollback. A panic in `work` or dropping
/// the returned future also rolls the transaction back: sqlx's `Transaction` queues a
/// rollback when it is dropped, which runs on the connection before its next use, so
/// the next user of the connection sees neither rows nor locks of it.
///
/// The [`TransactionOptions`] are applied through the driver's begin statement; an
/// option the driver cannot honor fails with [`TransactionError::Unsupported`] before
/// a connection is acquired. `work` runs once and nothing is retried; to run it again after
/// a serialization failure or a deadlock, use `transaction_with_retry` (cargo feature
/// `retry`), which takes the same closure. `E` needs no trait here.
///
/// A nested transaction inside `work` goes through sqlx's `tx.begin()` (from the
/// re-exported `sqlx::Acquire`), which creates a savepoint: rolling it back undoes only
/// what ran after it.
///
/// Where the returned future must be `Send`, as in a spawned task or a web handler,
/// write `work` as an `async move` closure that owns what it uses (clone it in
/// first). A closure that borrows from the surrounding async code makes the future
/// not `Send`, because of a current limitation of the compiler with async closures
/// (rust-lang/rust#110338); outside such contexts borrowing is fine.
///
/// # Errors
///
/// [`TransactionError::Unsupported`] for options the driver cannot honor,
/// [`TransactionError::Begin`] when no connection is available or the begin statement
/// fails, [`TransactionError::Inner`] with the error of `work`, and
/// [`TransactionError::Commit`] when the commit fails. The driver errors of begin and
/// commit are not kept, so that no SQL or server text reaches a log or a response.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_data::sqlx::{self, sqlite::SqlitePoolOptions};
/// use oxbloom_data::{RetryableError, TransactionError, TransactionOptions, transaction};
///
/// // The application's own error. `RetryableError` is needed only by
/// // `transaction_with_retry`, which recognizes a serialization failure inside it.
/// #[derive(Debug)]
/// enum TransferError {
///     Database(sqlx::Error),
///     InsufficientFunds,
/// }
///
/// impl From<sqlx::Error> for TransferError {
///     fn from(error: sqlx::Error) -> Self {
///         TransferError::Database(error)
///     }
/// }
///
/// impl RetryableError for TransferError {
///     fn is_retryable(&self) -> bool {
///         matches!(self, TransferError::Database(e) if e.is_retryable())
///     }
/// }
///
/// let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
/// sqlx::query("CREATE TABLE accounts (name TEXT PRIMARY KEY, balance INTEGER)")
///     .execute(&pool)
///     .await?;
///
/// // Both inserts commit together.
/// let options = TransactionOptions::default();
/// transaction(&pool, &options, async |tx| {
///     sqlx::query("INSERT INTO accounts VALUES ('alice', 100)").execute(&mut **tx).await?;
///     sqlx::query("INSERT INTO accounts VALUES ('bob', 0)").execute(&mut **tx).await?;
///     Ok::<_, sqlx::Error>(())
/// })
/// .await?;
///
/// // An error rolls the transfer back and comes back as `Inner`.
/// let result = transaction(&pool, &options, async |tx| {
///     sqlx::query("UPDATE accounts SET balance = balance - 500 WHERE name = 'alice'")
///         .execute(&mut **tx)
///         .await?;
///     let balance: i64 =
///         sqlx::query_scalar("SELECT balance FROM accounts WHERE name = 'alice'")
///             .fetch_one(&mut **tx)
///             .await?;
///     if balance < 0 {
///         return Err(TransferError::InsufficientFunds);
///     }
///     Ok(())
/// })
/// .await;
/// assert!(matches!(result, Err(TransactionError::Inner(TransferError::InsufficientFunds))));
/// let balance: i64 = sqlx::query_scalar("SELECT balance FROM accounts WHERE name = 'alice'")
///     .fetch_one(&pool)
///     .await?;
/// assert_eq!(balance, 100);
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
///
/// In a spawned task, the closure owns its data:
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_data::sqlx::{self, sqlite::SqlitePoolOptions};
/// use oxbloom_data::{TransactionOptions, transaction};
///
/// let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
/// sqlx::query("CREATE TABLE users (name TEXT)").execute(&pool).await?;
/// let task_pool = pool.clone();
/// let name = String::from("alice");
/// tokio::spawn(async move {
///     transaction(&task_pool, &TransactionOptions::default(), async move |tx| {
///         sqlx::query("INSERT INTO users VALUES (?)").bind(&name).execute(&mut **tx).await?;
///         Ok::<_, sqlx::Error>(())
///     })
///     .await
/// })
/// .await??;
/// let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&pool).await?;
/// assert_eq!(users, 1);
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
///
/// A savepoint inside the transaction:
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_data::sqlx::{self, Acquire, sqlite::SqlitePoolOptions};
/// use oxbloom_data::{TransactionOptions, transaction};
///
/// let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
/// sqlx::query("CREATE TABLE log (line TEXT)").execute(&pool).await?;
/// transaction(&pool, &TransactionOptions::default(), async |tx| {
///     sqlx::query("INSERT INTO log VALUES ('kept')").execute(&mut **tx).await?;
///     let mut savepoint = tx.begin().await?;
///     sqlx::query("INSERT INTO log VALUES ('undone')").execute(&mut *savepoint).await?;
///     savepoint.rollback().await?;
///     Ok::<_, sqlx::Error>(())
/// })
/// .await?;
/// let lines: Vec<String> = sqlx::query_scalar("SELECT line FROM log").fetch_all(&pool).await?;
/// assert_eq!(lines, ["kept"]);
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
pub async fn transaction<DB, T, E, F>(
    pool: &sqlx::Pool<DB>,
    options: &TransactionOptions,
    mut work: F,
) -> Result<T, TransactionError<E>>
where
    DB: TransactionDatabase,
    F: for<'t, 'c> AsyncFnMut(&'t mut sqlx::Transaction<'c, DB>) -> Result<T, E>,
{
    let begin = begin_statement::<DB, E>(options)?;
    attempt_once(pool, begin, &mut work, |_| false)
        .await
        .map_err(|failure| failure.error)
}

/// As [`transaction`], but runs `work` again, in a new transaction, after a serialization
/// failure or a deadlock, following `policy`: at most `policy.max_attempts` runs (zero acts as
/// one), sleeping the policy's delay with `tokio::time::sleep` before each new run. Needs the
/// cargo feature `retry`.
///
/// An error counts as such a failure when its [`RetryableError::is_retryable`] says so:
/// Postgres SQLSTATE `40001` and `40P01`, MySQL errors `1213` and `1205`, and SQLite
/// `SQLITE_BUSY` and `SQLITE_LOCKED`, whether the work returns it or the commit fails with
/// it. A failed begin and every other error, such as a constraint violation or a syntax
/// error, are returned at once. When the attempts run out, the last error is returned. The
/// work must be safe to run again: everything it did in the database is rolled back, but
/// effects outside the database, such as a sent message, are not. The retries log nothing,
/// so no driver error text reaches the logs.
///
/// # Errors
///
/// As [`transaction`], with the error of the last run.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::time::Duration;
/// use oxbloom_data::sqlx::{self, sqlite::SqlitePoolOptions};
/// use oxbloom_data::{TransactionError, TransactionOptions, transaction_with_retry};
/// use oxbloom_tasks::{Backoff, RetryPolicy};
///
/// let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
/// sqlx::query("CREATE TABLE counters (n INTEGER UNIQUE)").execute(&pool).await?;
/// let policy = RetryPolicy {
///     max_attempts: 5,
///     backoff: Backoff::Fixed,
///     initial: Duration::from_millis(10),
///     ..RetryPolicy::default()
/// };
/// let options = TransactionOptions::default();
/// let mut runs = 0;
/// transaction_with_retry(&pool, &options, &policy, async |tx| {
///     runs += 1;
///     sqlx::query("INSERT INTO counters (n) VALUES (1)").execute(&mut **tx).await?;
///     Ok::<_, sqlx::Error>(())
/// })
/// .await?;
/// assert_eq!(runs, 1);
///
/// // A constraint violation is not a serialization failure: it runs once.
/// let result = transaction_with_retry(&pool, &options, &policy, async |tx| {
///     sqlx::query("INSERT INTO counters (n) VALUES (1)").execute(&mut **tx).await?;
///     Ok::<_, sqlx::Error>(())
/// })
/// .await;
/// assert!(matches!(result, Err(TransactionError::Inner(_))));
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
#[cfg(feature = "retry")]
pub async fn transaction_with_retry<DB, T, E, F>(
    pool: &sqlx::Pool<DB>,
    options: &TransactionOptions,
    policy: &oxbloom_tasks::RetryPolicy,
    mut work: F,
) -> Result<T, TransactionError<E>>
where
    DB: TransactionDatabase,
    E: RetryableError,
    F: for<'t, 'c> AsyncFnMut(&'t mut sqlx::Transaction<'c, DB>) -> Result<T, E>,
{
    let begin = begin_statement::<DB, E>(options)?;
    let mut attempt: u32 = 1;
    loop {
        let failure = match attempt_once(pool, begin, &mut work, E::is_retryable).await {
            Ok(value) => return Ok(value),
            Err(failure) => failure,
        };
        if !failure.retryable || attempt >= policy.max_attempts {
            return Err(failure.error);
        }
        tokio::time::sleep(policy.delay(attempt, fastrand::f64())).await;
        attempt += 1;
    }
}

/// The begin statement of `DB` for `options`, refused before any connection is acquired when
/// the driver cannot honor them.
fn begin_statement<DB: TransactionDatabase, E>(
    options: &TransactionOptions,
) -> Result<Option<&'static str>, TransactionError<E>> {
    <DB as private::Sealed>::begin_statement(options.isolation, options.read_only)
        .map_err(|()| TransactionError::Unsupported)
}

/// One failed run: the error to return and whether a new run may succeed.
struct Failure<E> {
    error: TransactionError<E>,
    /// Read only by `transaction_with_retry`.
    #[cfg_attr(not(feature = "retry"), allow(dead_code))]
    retryable: bool,
}

/// One run of `work` in its own transaction.
async fn attempt_once<DB, T, E, F>(
    pool: &sqlx::Pool<DB>,
    begin: Option<&'static str>,
    work: &mut F,
    is_retryable: fn(&E) -> bool,
) -> Result<T, Failure<E>>
where
    DB: TransactionDatabase,
    F: for<'t, 'c> AsyncFnMut(&'t mut sqlx::Transaction<'c, DB>) -> Result<T, E>,
{
    let begun = match begin {
        Some(statement) => pool.begin_with(statement).await,
        None => pool.begin().await,
    };
    let mut tx = begun.map_err(|_| Failure {
        error: TransactionError::Begin,
        retryable: false,
    })?;
    match work(&mut tx).await {
        Ok(value) => match tx.commit().await {
            Ok(()) => Ok(value),
            // A failed commit leaves the transaction to sqlx's rollback on drop.
            Err(error) => Err(Failure {
                error: TransactionError::Commit,
                retryable: <DB as private::Sealed>::is_retryable(&error),
            }),
        },
        Err(error) => {
            // The work's error is what the caller needs; a failed rollback still ends the
            // transaction, because sqlx closes or rolls back the connection on drop.
            let _ = tx.rollback().await;
            let retryable = is_retryable(&error);
            Err(Failure {
                error: TransactionError::Inner(error),
                retryable,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Isolation::{ReadCommitted, ReadUncommitted, RepeatableRead, Serializable};

    #[test]
    fn postgres_begin_statements() {
        assert_eq!(postgres_begin(None, false), None);
        assert_eq!(postgres_begin(None, true), Some("BEGIN READ ONLY"));
        for level in [ReadUncommitted, ReadCommitted, RepeatableRead, Serializable] {
            let plain = format!("BEGIN ISOLATION LEVEL {}", level.sql());
            assert_eq!(postgres_begin(Some(level), false), Some(plain.as_str()));
            let read_only = format!("{plain} READ ONLY");
            assert_eq!(postgres_begin(Some(level), true), Some(read_only.as_str()));
        }
    }

    #[test]
    fn mysql_begin_statements() {
        assert_eq!(mysql_begin(None, false), None);
        assert_eq!(mysql_begin(None, true), Some("START TRANSACTION READ ONLY"));
        for level in [ReadUncommitted, ReadCommitted, RepeatableRead, Serializable] {
            let set = format!("SET TRANSACTION ISOLATION LEVEL {}", level.sql());
            let plain = format!("{set}; START TRANSACTION");
            assert_eq!(mysql_begin(Some(level), false), Some(plain.as_str()));
            let read_only = format!("{set}; START TRANSACTION READ ONLY");
            assert_eq!(mysql_begin(Some(level), true), Some(read_only.as_str()));
        }
    }

    #[test]
    fn sqlite_begin_statements() {
        assert_eq!(sqlite_begin(None, false), Ok(None));
        assert_eq!(sqlite_begin(Some(Serializable), false), Ok(None));
        assert_eq!(sqlite_begin(None, true), Err(()));
        assert_eq!(sqlite_begin(Some(Serializable), true), Err(()));
        for level in [ReadUncommitted, ReadCommitted, RepeatableRead] {
            assert_eq!(sqlite_begin(Some(level), false), Err(()));
        }
    }

    #[test]
    fn retryable_driver_codes() {
        assert!(postgres_code_is_retryable("40001"));
        assert!(postgres_code_is_retryable("40P01"));
        assert!(!postgres_code_is_retryable("23505"), "unique violation");
        assert!(!postgres_code_is_retryable("42601"), "syntax error");

        assert!(mysql_number_is_retryable(1213));
        assert!(mysql_number_is_retryable(1205));
        assert!(!mysql_number_is_retryable(1062), "duplicate entry");
        assert!(!mysql_number_is_retryable(1064), "syntax error");

        assert!(sqlite_code_is_retryable("5"), "SQLITE_BUSY");
        assert!(sqlite_code_is_retryable("6"), "SQLITE_LOCKED");
        assert!(sqlite_code_is_retryable("517"), "SQLITE_BUSY_SNAPSHOT");
        assert!(sqlite_code_is_retryable("262"), "SQLITE_LOCKED_SHAREDCACHE");
        assert!(
            !sqlite_code_is_retryable("2067"),
            "SQLITE_CONSTRAINT_UNIQUE"
        );
        assert!(!sqlite_code_is_retryable("1"), "SQLITE_ERROR");
        assert!(!sqlite_code_is_retryable("busy"));
    }

    #[test]
    fn transaction_error_is_retryable_only_through_its_inner_error() {
        struct Always;
        impl RetryableError for Always {
            fn is_retryable(&self) -> bool {
                true
            }
        }
        assert!(TransactionError::Inner(Always).is_retryable());
        assert!(!TransactionError::<Always>::Commit.is_retryable());
        assert!(!TransactionError::<Always>::Begin.is_retryable());
        assert!(!TransactionError::<Always>::Unsupported.is_retryable());
    }
}
