//! Helpers shared by the conformance suites `data_conformance.rs` and
//! `data_conformance_boot.rs`: configuration from TOML, unique names, a closed port, the retry
//! policy, and the user with a password that needs percent-encoding on Postgres and MySQL.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxbloom_config::Config;
use oxbloom_data::{DataError, DataSettings, DataSources};
use oxbloom_tasks::{Backoff, Jitter, RetryPolicy};

/// The password of the percent-encoding test: every character after `p` needs encoding in the
/// user-info part of a URL.
pub const SPECIAL_PASSWORD: &str = "p@ss:w/rd#?";
/// [`SPECIAL_PASSWORD`] percent-encoded.
pub const SPECIAL_PASSWORD_ENCODED: &str = "p%40ss%3Aw%2Frd%23%3F";

/// A name part that is unique in this process and across processes.
pub fn unique() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{:x}_{:x}_{}",
        std::process::id(),
        nanos & 0xffff_ffff,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// A local port that was free a moment ago, so a connection to it is refused at once.
pub fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
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

/// The `data` settings of a TOML document.
pub fn settings(toml: &str) -> DataSettings {
    DataSettings::from_config(&Config::from_str(toml).unwrap()).unwrap()
}

/// A registry built from a TOML document; no source names an SSL bundle.
pub fn sources(toml: &str) -> DataSources {
    DataSources::build(&settings(toml), &|_| Err(DataError::Tls)).unwrap()
}

/// A `[data]` section with `url` and the extra `lines`.
pub fn data_section(url: &str, lines: &str) -> String {
    format!("[data]\nurl = {}\n{lines}", toml_str(url))
}

/// A fixed-delay policy without jitter.
pub fn policy(max_attempts: u32, delay_ms: u64) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        backoff: Backoff::Fixed,
        initial: Duration::from_millis(delay_ms),
        jitter: Jitter::None,
        ..RetryPolicy::default()
    }
}

/// The URL of the test database with the user and the password replaced; the password is put
/// in as given, so pass it already encoded.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
pub fn url_with_user(database_url: &str, user: &str, encoded_password: &str) -> String {
    let url = url::Url::parse(database_url).unwrap();
    format!(
        "{}://{user}:{encoded_password}@{}:{}{}",
        url.scheme(),
        url.host_str().unwrap(),
        url.port().unwrap(),
        url.path(),
    )
}

/// The host and port of a URL as `host:port`.
#[cfg(any(feature = "data-postgres", feature = "data-mysql"))]
pub fn host_port(database_url: &str) -> String {
    let url = url::Url::parse(database_url).unwrap();
    format!("{}:{}", url.host_str().unwrap(), url.port().unwrap())
}

/// A Postgres role with [`SPECIAL_PASSWORD`], created through the pool of the test database.
#[cfg(feature = "data-postgres")]
pub async fn create_postgres_role(pool: &oxbloom_data::sqlx::PgPool) -> String {
    use oxbloom_data::sqlx;
    let role = format!("oxbloom_pe_{}", unique());
    // The role name holds only `[a-z0-9_]` and the password is a constant without quotes.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE ROLE {role} LOGIN PASSWORD '{SPECIAL_PASSWORD}'"
    )))
    .execute(pool)
    .await
    .unwrap();
    role
}

/// Drops the role of [`create_postgres_role`]; a failure is printed, not raised, so it never
/// hides the result of the test.
#[cfg(feature = "data-postgres")]
pub async fn drop_postgres_role(pool: &oxbloom_data::sqlx::PgPool, role: &str) {
    use oxbloom_data::sqlx;
    if sqlx::query(sqlx::AssertSqlSafe(format!("DROP ROLE IF EXISTS {role}")))
        .execute(pool)
        .await
        .is_err()
    {
        eprintln!("could not drop the test role {role}");
    }
}

/// A MySQL user with [`SPECIAL_PASSWORD`] and every privilege on the test database, created
/// through the pool of the test database.
#[cfg(feature = "data-mysql")]
pub async fn create_mysql_user(pool: &oxbloom_data::sqlx::MySqlPool) -> String {
    use oxbloom_data::sqlx;
    let user = format!("oxbloom_pe_{}", unique());
    let database: String = sqlx::query_scalar("SELECT DATABASE()")
        .fetch_one(pool)
        .await
        .unwrap();
    // The names hold only `[a-z0-9_]` and the password is a constant without quotes.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE USER '{user}'@'%' IDENTIFIED BY '{SPECIAL_PASSWORD}'"
    )))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "GRANT ALL PRIVILEGES ON `{database}`.* TO '{user}'@'%'"
    )))
    .execute(pool)
    .await
    .unwrap();
    user
}

/// Drops the user of [`create_mysql_user`]; a failure is printed, not raised.
#[cfg(feature = "data-mysql")]
pub async fn drop_mysql_user(pool: &oxbloom_data::sqlx::MySqlPool, user: &str) {
    use oxbloom_data::sqlx;
    if sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP USER IF EXISTS '{user}'@'%'"
    )))
    .execute(pool)
    .await
    .is_err()
    {
        eprintln!("could not drop the test user {user}");
    }
}
