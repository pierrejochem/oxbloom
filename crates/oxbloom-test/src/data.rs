//! `TestDatabase`: a throwaway database for a test.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oxbloom_data::sqlx::{self, Connection, SqliteConnection, SqlitePool};

#[cfg(feature = "data-mysql")]
use oxbloom_data::sqlx::MySqlPool;
#[cfg(feature = "data-postgres")]
use oxbloom_data::sqlx::PgPool;

/// The environment variable with the URL of the Postgres server for `TestDatabase::postgres`.
#[cfg(feature = "data-postgres")]
const POSTGRES_ENV: &str = "OXBLOOM_TEST_POSTGRES_URL";
/// The environment variable with the URL of the MySQL server for `TestDatabase::mysql`.
#[cfg(feature = "data-mysql")]
const MYSQL_ENV: &str = "OXBLOOM_TEST_MYSQL_URL";

/// A name part that is unique in this process and, with the clock and the process id, across
/// processes: the process id, the nanoseconds of the clock and a counter, in hex, separated by `_`.
fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}_{:x}_{:x}", std::process::id(), nanos, count)
}

/// The text as a TOML basic string, with the quotes: `\` and `"` escaped.
fn toml_basic_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
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

/// The line printed when a test database could not be dropped; the reason is a fixed text, so
/// neither a URL nor a driver message ends up in it.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
fn drop_failure_message(name: &str, reason: &str) -> String {
    format!("oxbloom-test: could not drop test database {name}: {reason}")
}

/// A database for one test, created for it and removed again by [`close`](TestDatabase::close).
///
/// [`TestDatabase::sqlite`] needs nothing but the feature `data`. With the features
/// `data-postgres` and `data-mysql`, `TestDatabase::postgres` and `TestDatabase::mysql` create a
/// uniquely named database on the server whose URL is in `OXBLOOM_TEST_POSTGRES_URL` or
/// `OXBLOOM_TEST_MYSQL_URL`, and `make test-db` starts such servers. The URL of a database holds
/// credentials, so neither `Debug` nor any error of this type shows it.
///
/// Call `close` at the end of a test. The cleanup of `Drop` is best effort: it needs a running
/// runtime and usually does not get to run on a current-thread `#[tokio::test]`, which ends
/// right after the test, and a test that panics before `close` leaves its `oxbloom_test_*`
/// database behind on the server. `make test-db` runs the servers on tmpfs and removes them
/// with `down -v`, so nothing stays; on a persistent server the `oxbloom_test_` prefix lets you
/// drop the leftovers by hand.
///
/// # Examples
///
/// ```
/// use oxbloom_data::sqlx;
/// use oxbloom_test::TestDatabase;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let db = TestDatabase::sqlite();
/// sqlx::query("CREATE TABLE t (id INTEGER)")
///     .execute(db.sqlite_pool())
///     .await
///     .unwrap();
/// assert!(db.config_snippet().starts_with("[data]\n"));
/// db.close().await;
/// # }
/// ```
pub struct TestDatabase {
    url: String,
    inner: Inner,
}

enum Inner {
    Sqlite {
        pool: SqlitePool,
        /// Keeps the shared in-memory database alive while the pool has no connection.
        keeper: Option<SqliteConnection>,
    },
    #[cfg(feature = "data-postgres")]
    Postgres {
        pool: PgPool,
        server: ServerDatabase,
    },
    #[cfg(feature = "data-mysql")]
    Mysql {
        pool: MySqlPool,
        server: ServerDatabase,
    },
}

/// The database a test created on a server, and how to drop it again.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
struct ServerDatabase {
    admin_url: String,
    name: String,
    drop_sql: String,
    postgres: bool,
    /// Whether `close` or the best-effort cleanup of `Drop` already took care of the database.
    done: bool,
}

impl fmt::Debug for TestDatabase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.inner {
            Inner::Sqlite { .. } => "sqlite",
            #[cfg(feature = "data-postgres")]
            Inner::Postgres { .. } => "postgres",
            #[cfg(feature = "data-mysql")]
            Inner::Mysql { .. } => "mysql",
        };
        f.debug_struct("TestDatabase")
            .field("kind", &kind)
            .field("url", &"<redacted>")
            .finish()
    }
}

impl TestDatabase {
    /// Creates a uniquely named in-memory SQLite database with the shared cache, so every
    /// connection of the pool sees the same data. One connection stays open for the life of the
    /// value and the pool never closes idle or old connections, so the data lives until
    /// [`close`](TestDatabase::close).
    ///
    /// # Panics
    ///
    /// Panics when SQLite cannot open the database, which is a broken test environment. The
    /// call is not async: it opens the kept connection on a short-lived helper thread, and the
    /// pool connects lazily on the runtime of the test.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestDatabase;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let a = TestDatabase::sqlite();
    /// let b = TestDatabase::sqlite();
    /// assert_ne!(a.url(), b.url());
    /// assert!(a.url().starts_with("sqlite:file:oxbloom-test-"));
    /// a.close().await;
    /// b.close().await;
    /// # }
    /// ```
    pub fn sqlite() -> TestDatabase {
        use sqlx::sqlite::SqlitePoolOptions;
        let url = format!(
            "sqlite:file:oxbloom-test-{}?mode=memory&cache=shared",
            unique_suffix()
        );
        let connect_url = url.clone();
        // The kept connection is opened first so that the database exists before any pool
        // connection; it is opened on a helper thread because this function is not async.
        let (keeper, pool) = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime for the SQLite test database");
            runtime.block_on(async move {
                let keeper = SqliteConnection::connect(&connect_url)
                    .await
                    .expect("the in-memory SQLite test database opens");
                (keeper, connect_url)
            })
        })
        .join()
        .expect("the SQLite test database thread");
        // The pool connects lazily: it opens its connections on the runtime of the test.
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_lazy(&pool)
            .expect("the SQLite test database URL is valid");
        TestDatabase {
            url,
            inner: Inner::Sqlite {
                pool,
                keeper: Some(keeper),
            },
        }
    }

    /// The URL of the database, with credentials for a server database. Do not print it.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A `[data]` section for [`ContextRunner::config_str`](crate::ContextRunner::config_str)
    /// that makes the data auto-configuration connect to this database.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestDatabase;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let db = TestDatabase::sqlite();
    /// let snippet = db.config_snippet();
    /// assert!(snippet.starts_with("[data]\nurl = \""));
    /// assert!(snippet.contains("max-connections"));
    /// db.close().await;
    /// # }
    /// ```
    pub fn config_snippet(&self) -> String {
        let mut snippet = format!("[data]\nurl = {}\n", toml_basic_string(&self.url));
        if matches!(self.inner, Inner::Sqlite { .. }) {
            // The shared cache allows several connections to the in-memory database.
            snippet.push_str("max-connections = 4\n");
        }
        snippet
    }

    /// The pool of a SQLite test database.
    ///
    /// # Panics
    ///
    /// Panics when this is not a SQLite database.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_data::sqlx;
    /// use oxbloom_test::TestDatabase;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let db = TestDatabase::sqlite();
    /// let (one,): (i64,) = sqlx::query_as("SELECT 1").fetch_one(db.sqlite_pool()).await.unwrap();
    /// assert_eq!(one, 1);
    /// db.close().await;
    /// # }
    /// ```
    pub fn sqlite_pool(&self) -> &SqlitePool {
        match &self.inner {
            Inner::Sqlite { pool, .. } => pool,
            #[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
            _ => panic!("this TestDatabase is not a SQLite database"),
        }
    }

    /// Closes the pool and removes the database. The pool is unusable afterwards, also for
    /// clones of it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestDatabase;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let db = TestDatabase::sqlite();
    /// let pool = db.sqlite_pool().clone();
    /// db.close().await;
    /// assert!(pool.is_closed());
    /// # }
    /// ```
    #[allow(unused_mut)]
    pub async fn close(mut self) {
        match &mut self.inner {
            Inner::Sqlite { pool, keeper } => {
                pool.close().await;
                if let Some(keeper) = keeper.take() {
                    let _ = keeper.close().await;
                }
            }
            #[cfg(feature = "data-postgres")]
            Inner::Postgres { pool, server } => {
                pool.close().await;
                server.drop_database().await;
            }
            #[cfg(feature = "data-mysql")]
            Inner::Mysql { pool, server } => {
                pool.close().await;
                server.drop_database().await;
            }
        }
    }
}

#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
impl ServerDatabase {
    /// Drops the database through a fresh admin connection. A failure is reported on stderr
    /// with a fixed reason and the generated name, never with the URL.
    async fn drop_database(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if let Err(reason) = try_admin(&self.admin_url, self.postgres, &self.drop_sql).await {
            eprintln!("{}", drop_failure_message(&self.name, reason));
        }
    }
}

/// Runs one statement on a fresh connection to the server. The error text is replaced by a
/// fixed phrase, since an sqlx connect error can quote parts of the URL.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
async fn try_admin(admin_url: &str, postgres: bool, sql: &str) -> Result<(), &'static str> {
    if postgres {
        #[cfg(feature = "data-postgres")]
        {
            let mut conn = sqlx::PgConnection::connect(admin_url)
                .await
                .map_err(|_| "could not connect to the Postgres test server")?;
            // The statement is built from a generated name that holds only `[a-z0-9_]`.
            let result = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
                .execute(&mut conn)
                .await;
            let _ = conn.close().await;
            return result
                .map(|_| ())
                .map_err(|_| "the Postgres test server rejected the statement");
        }
    } else {
        #[cfg(feature = "data-mysql")]
        {
            let mut conn = sqlx::MySqlConnection::connect(admin_url)
                .await
                .map_err(|_| "could not connect to the MySQL test server")?;
            // The statement is built from a generated name that holds only `[a-z0-9_]`.
            let result = sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
                .execute(&mut conn)
                .await;
            let _ = conn.close().await;
            return result
                .map(|_| ())
                .map_err(|_| "the MySQL test server rejected the statement");
        }
    }
    #[allow(unreachable_code)]
    Err("the driver of the test server is not enabled")
}

/// The URL of the server with the database path replaced.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
fn with_database(server_url: &str, database: &str, var: &str) -> String {
    let mut url =
        url::Url::parse(server_url).unwrap_or_else(|_| panic!("{var} is not a valid URL"));
    url.set_path(database);
    // sqlx lets a `dbname` query pair override the path, so it must not survive.
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "dbname")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
    url.into()
}

/// Reads the server URL variable; the panic names the variable and never the value.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
fn server_url(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| {
        panic!("the environment variable {var} is not set; run `make test-db` or set it to the URL of a server whose user may create databases")
    })
}

/// A generated database name: only `[a-z0-9_]`, so it is safe as an identifier.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
fn database_name() -> String {
    let name = format!("oxbloom_test_{}", unique_suffix());
    assert!(
        name.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    );
    name
}

#[cfg(feature = "data-postgres")]
impl TestDatabase {
    /// Creates a uniquely named database on the Postgres server in `OXBLOOM_TEST_POSTGRES_URL`
    /// and a pool for it.
    ///
    /// # Panics
    ///
    /// Panics naming the variable when it is not set, and when the server cannot be reached or
    /// refuses to create the database.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Needs a server: run `make test-db` or set OXBLOOM_TEST_POSTGRES_URL.
    /// # async fn run() {
    /// let db = oxbloom_test::TestDatabase::postgres().await;
    /// let _pool = db.postgres_pool();
    /// db.close().await;
    /// # }
    /// ```
    pub async fn postgres() -> TestDatabase {
        let admin_url = server_url(POSTGRES_ENV);
        let name = database_name();
        let url = with_database(&admin_url, &name, POSTGRES_ENV);
        let create = format!("CREATE DATABASE {name}");
        try_admin(&admin_url, true, &create)
            .await
            .unwrap_or_else(|reason| panic!("{reason} named by {POSTGRES_ENV}"));
        let pool = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
        {
            Ok(pool) => pool,
            Err(_) => {
                // The database exists already: drop it before failing the test.
                let sql = format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)");
                if let Err(reason) = try_admin(&admin_url, true, &sql).await {
                    eprintln!("{}", drop_failure_message(&name, reason));
                }
                panic!("could not connect to the database created on {POSTGRES_ENV}");
            }
        };
        TestDatabase {
            url,
            inner: Inner::Postgres {
                pool,
                server: ServerDatabase {
                    admin_url,
                    drop_sql: format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
                    name,
                    postgres: true,
                    done: false,
                },
            },
        }
    }

    /// The pool of a Postgres test database.
    ///
    /// # Panics
    ///
    /// Panics when this is not a Postgres database.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Needs a server: run `make test-db` or set OXBLOOM_TEST_POSTGRES_URL.
    /// # async fn run() {
    /// let db = oxbloom_test::TestDatabase::postgres().await;
    /// let _pool: &oxbloom_data::sqlx::PgPool = db.postgres_pool();
    /// db.close().await;
    /// # }
    /// ```
    pub fn postgres_pool(&self) -> &PgPool {
        match &self.inner {
            Inner::Postgres { pool, .. } => pool,
            _ => panic!("this TestDatabase is not a Postgres database"),
        }
    }
}

#[cfg(feature = "data-mysql")]
impl TestDatabase {
    /// Creates a uniquely named database on the MySQL server in `OXBLOOM_TEST_MYSQL_URL` and a
    /// pool for it. The user of the URL must be allowed to create databases.
    ///
    /// # Panics
    ///
    /// Panics naming the variable when it is not set, and when the server cannot be reached or
    /// refuses to create the database.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Needs a server: run `make test-db` or set OXBLOOM_TEST_MYSQL_URL.
    /// # async fn run() {
    /// let db = oxbloom_test::TestDatabase::mysql().await;
    /// let _pool = db.mysql_pool();
    /// db.close().await;
    /// # }
    /// ```
    pub async fn mysql() -> TestDatabase {
        let admin_url = server_url(MYSQL_ENV);
        let name = database_name();
        let url = with_database(&admin_url, &name, MYSQL_ENV);
        let create = format!("CREATE DATABASE {name}");
        try_admin(&admin_url, false, &create)
            .await
            .unwrap_or_else(|reason| panic!("{reason} named by {MYSQL_ENV}"));
        let pool = match sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
        {
            Ok(pool) => pool,
            Err(_) => {
                // The database exists already: drop it before failing the test.
                let sql = format!("DROP DATABASE IF EXISTS {name}");
                if let Err(reason) = try_admin(&admin_url, false, &sql).await {
                    eprintln!("{}", drop_failure_message(&name, reason));
                }
                panic!("could not connect to the database created on {MYSQL_ENV}");
            }
        };
        TestDatabase {
            url,
            inner: Inner::Mysql {
                pool,
                server: ServerDatabase {
                    admin_url,
                    drop_sql: format!("DROP DATABASE IF EXISTS {name}"),
                    name,
                    postgres: false,
                    done: false,
                },
            },
        }
    }

    /// The pool of a MySQL test database.
    ///
    /// # Panics
    ///
    /// Panics when this is not a MySQL database.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// // Needs a server: run `make test-db` or set OXBLOOM_TEST_MYSQL_URL.
    /// # async fn run() {
    /// let db = oxbloom_test::TestDatabase::mysql().await;
    /// let _pool: &oxbloom_data::sqlx::MySqlPool = db.mysql_pool();
    /// db.close().await;
    /// # }
    /// ```
    pub fn mysql_pool(&self) -> &MySqlPool {
        match &self.inner {
            Inner::Mysql { pool, .. } => pool,
            _ => panic!("this TestDatabase is not a MySQL database"),
        }
    }
}

#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
impl Drop for TestDatabase {
    /// Best effort when `close` was not called: when a runtime is running, a task closes the
    /// pool and drops the database. It never panics and prints nothing.
    fn drop(&mut self) {
        let (pool_close, server) = match &mut self.inner {
            Inner::Sqlite { .. } => return,
            #[cfg(feature = "data-postgres")]
            Inner::Postgres { pool, server } => (Closer::Pg(pool.clone()), server),
            #[cfg(feature = "data-mysql")]
            Inner::Mysql { pool, server } => (Closer::My(pool.clone()), server),
        };
        if server.done {
            return;
        }
        server.done = true;
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (admin_url, postgres, sql, name) = (
            server.admin_url.clone(),
            server.postgres,
            server.drop_sql.clone(),
            server.name.clone(),
        );
        handle.spawn(async move {
            match pool_close {
                #[cfg(feature = "data-postgres")]
                Closer::Pg(pool) => pool.close().await,
                #[cfg(feature = "data-mysql")]
                Closer::My(pool) => pool.close().await,
            }
            if let Err(reason) = try_admin(&admin_url, postgres, &sql).await {
                eprintln!("{}", drop_failure_message(&name, reason));
            }
        });
    }
}

#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
enum Closer {
    #[cfg(feature = "data-postgres")]
    Pg(PgPool),
    #[cfg(feature = "data-mysql")]
    My(MySqlPool),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_basic_string_escapes_backslash_and_quote() {
        let url = "sqlite:file:a\\b\"c?mode=memory";
        let quoted = toml_basic_string(url);
        assert_eq!(quoted, "\"sqlite:file:a\\\\b\\\"c?mode=memory\"");
        let config = oxbloom_config::Config::from_str(&format!("url = {quoted}")).unwrap();
        assert_eq!(config.get::<String>("url").unwrap(), url);
    }

    #[test]
    fn the_unique_suffix_separates_its_parts_and_never_repeats() {
        let a = unique_suffix();
        let b = unique_suffix();
        assert_ne!(a, b);
        assert_eq!(a.matches('_').count(), 2);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit() || c == b'_'));
    }

    #[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
    #[test]
    fn the_drop_failure_message_has_the_name_and_the_fixed_reason_only() {
        let text =
            drop_failure_message("oxbloom_test_1_2_3", "could not connect to the test server");
        assert_eq!(
            text,
            "oxbloom-test: could not drop test database oxbloom_test_1_2_3: could not connect to the test server"
        );
    }

    #[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
    #[test]
    fn a_missing_env_var_panics_with_the_variable_name_and_no_url() {
        let var = format!("OXBLOOM_TEST_UNSET_{}", unique_suffix());
        let payload = std::panic::catch_unwind(|| server_url(&var)).expect_err("a panic");
        let text = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .expect("a text payload");
        assert!(text.contains(&var), "{text}");
        assert!(!text.contains("://"), "{text}");
    }

    #[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
    #[test]
    fn with_database_replaces_the_path_and_drops_a_dbname_pair() {
        let url = with_database(
            "postgres://u:p@127.0.0.1:55432/postgres?dbname=other&sslmode=disable",
            "oxbloom_test_x",
            "VAR",
        );
        assert_eq!(
            url,
            "postgres://u:p@127.0.0.1:55432/oxbloom_test_x?sslmode=disable"
        );
        let only = with_database("mysql://root:p@h:1/mysql?dbname=other", "d", "VAR");
        assert_eq!(only, "mysql://root:p@h:1/d");
    }
}
