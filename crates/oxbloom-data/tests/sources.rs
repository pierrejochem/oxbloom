//! `DataSources`: lazy sqlx pools built from `DataSettings`, the registry accessors, ping and
//! close, the TLS material and the secrecy of the URL.
//!
//! The SQLite tests need no server. The Postgres and MySQL tests only build connect options
//! and lazy pools, so they need no server either; the tests against a live server belong to
//! the conformance suite.

#![cfg(feature = "sqlite")]

use std::time::Duration;

use oxbloom_config::Config;
use oxbloom_data::{DataError, DataSettings, DataSources, Driver, TlsMaterial};

fn settings(toml: &str) -> DataSettings {
    DataSettings::from_config(&Config::from_str(toml).unwrap()).unwrap()
}

fn no_tls(_: &str) -> Result<TlsMaterial, DataError> {
    Err(DataError::Tls)
}

fn build(toml: &str) -> Result<DataSources, DataError> {
    DataSources::build(&settings(toml), &no_tls)
}

/// A SQLite file URL that is never opened by the tests that use it (the pools are lazy).
fn unopened_file_url() -> String {
    let path = std::env::temp_dir().join("oxbloom-data-never-opened.db");
    format!("sqlite:{}", path.display())
}

#[tokio::test]
async fn a_sqlite_source_builds_a_working_lazy_pool() {
    let sources = build("[data]\nurl = \"sqlite::memory:\"").unwrap();
    let pool = sources.sqlite("default").unwrap();
    assert_eq!(pool.size(), 0, "a lazy pool opens no connection when built");
    let one: i64 = oxbloom_data::sqlx::query_scalar("SELECT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(one, 1);
}

#[tokio::test]
async fn settings_are_applied_to_the_pool() {
    let toml = format!(
        "[data]\nurl = \"{}\"\nmax-connections = 7\nmin-connections = 0\n\
         acquire-timeout = \"5s\"\nidle-timeout = \"1m\"\nmax-lifetime = \"2m\"\n\
         test-before-acquire = false",
        unopened_file_url()
    );
    let sources = build(&toml).unwrap();
    let options = sources.sqlite("default").unwrap().options();
    assert_eq!(options.get_max_connections(), 7);
    assert_eq!(options.get_min_connections(), 0);
    assert_eq!(options.get_acquire_timeout(), Duration::from_secs(5));
    assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(60)));
    assert_eq!(options.get_max_lifetime(), Some(Duration::from_secs(120)));
    assert!(!options.get_test_before_acquire());

    // Unset timeouts keep the sqlx defaults for a database that outlives its connections.
    let sources = build(&format!("[data]\nurl = \"{}\"", unopened_file_url())).unwrap();
    let options = sources.sqlite("default").unwrap().options();
    assert_eq!(options.get_max_connections(), 10);
    assert_eq!(options.get_acquire_timeout(), Duration::from_secs(30));
    assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(600)));
    assert_eq!(options.get_max_lifetime(), Some(Duration::from_secs(1800)));
    assert!(options.get_test_before_acquire());
}

#[tokio::test]
async fn an_in_memory_pool_never_reaps_its_connections_unless_told_to() {
    let sources = build("[data]\nurl = \"sqlite::memory:\"").unwrap();
    let options = sources.sqlite("default").unwrap().options();
    assert_eq!(options.get_max_connections(), 1);
    assert_eq!(options.get_idle_timeout(), None);
    assert_eq!(options.get_max_lifetime(), None);

    let sources = build(
        "[data]\nurl = \"sqlite:file:reaped?mode=memory&cache=shared\"\n\
         idle-timeout = \"5m\"\nmax-lifetime = \"1h\"",
    )
    .unwrap();
    let options = sources.sqlite("default").unwrap().options();
    assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(300)));
    assert_eq!(options.get_max_lifetime(), Some(Duration::from_secs(3600)));
}

#[tokio::test]
async fn the_registry_holds_default_and_named_sources() {
    let toml = format!(
        "[data]\nurl = \"sqlite::memory:\"\n[data.sources.reports]\nurl = \"{}\"",
        unopened_file_url()
    );
    let sources = build(&toml).unwrap();
    assert_eq!(sources.len(), 2);
    assert!(!sources.is_empty());
    assert_eq!(sources.names(), ["default", "reports"]);
    assert_eq!(sources.default_name(), Some("default"));
    assert_eq!(sources.driver_of("reports"), Some(Driver::Sqlite));
    assert_eq!(sources.driver_of("missing"), None);
    assert!(sources.sqlite("reports").is_some());

    // Only named sources: there is no default.
    let toml = format!("[data.sources.reports]\nurl = \"{}\"", unopened_file_url());
    let sources = build(&toml).unwrap();
    assert_eq!(sources.default_name(), None);
    assert_eq!(sources.names(), ["reports"]);

    // No `data` section: an empty registry.
    let sources = build("").unwrap();
    assert!(sources.is_empty());
    assert_eq!(sources.len(), 0);
    assert_eq!(sources.default_name(), None);
}

#[tokio::test]
async fn typed_accessors_return_none_for_other_drivers_and_unknown_names() {
    let sources = build("[data]\nurl = \"sqlite::memory:\"").unwrap();
    assert!(sources.sqlite("default").is_some());
    assert!(sources.sqlite("unknown").is_none());
    assert_eq!(sources.pool_counts("unknown"), None);
    #[cfg(feature = "postgres")]
    assert!(sources.postgres("default").is_none());
    #[cfg(feature = "mysql")]
    assert!(sources.mysql("default").is_none());
}

#[cfg(feature = "postgres")]
#[tokio::test]
async fn the_sqlite_accessor_returns_none_for_a_postgres_source() {
    let sources = build("[data]\nurl = \"postgres://app@db.internal/app\"").unwrap();
    assert!(sources.postgres("default").is_some());
    assert!(sources.sqlite("default").is_none());
    assert_eq!(sources.driver_of("default"), Some(Driver::Postgres));
}

#[tokio::test]
async fn in_memory_sqlite_shares_one_database() {
    use oxbloom_data::sqlx::Acquire;

    // The default one-connection pool keeps the same database across acquires.
    let sources = build("[data]\nurl = \"sqlite::memory:\"").unwrap();
    let pool = sources.sqlite("default").unwrap();
    {
        let mut conn = pool.acquire().await.unwrap();
        oxbloom_data::sqlx::query("CREATE TABLE t (v INTEGER)")
            .execute(conn.acquire().await.unwrap())
            .await
            .unwrap();
        oxbloom_data::sqlx::query("INSERT INTO t VALUES (42)")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let v: i64 = oxbloom_data::sqlx::query_scalar("SELECT v FROM t")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(v, 42);

    // A shared-cache memory database is seen by two connections held at the same time.
    let sources = build(
        "[data]\nurl = \"sqlite:file:oxbloom-shared?mode=memory&cache=shared\"\nmax-connections = 3",
    )
    .unwrap();
    let pool = sources.sqlite("default").unwrap();
    assert_eq!(pool.options().get_max_connections(), 3);
    let mut first = pool.acquire().await.unwrap();
    let mut second = pool.acquire().await.unwrap();
    oxbloom_data::sqlx::query("CREATE TABLE s (v INTEGER)")
        .execute(&mut *first)
        .await
        .unwrap();
    oxbloom_data::sqlx::query("INSERT INTO s VALUES (7)")
        .execute(&mut *first)
        .await
        .unwrap();
    let v: i64 = oxbloom_data::sqlx::query_scalar("SELECT v FROM s")
        .fetch_one(&mut *second)
        .await
        .unwrap();
    assert_eq!(v, 7);
}

#[tokio::test]
async fn ping_all_reports_each_source() {
    let missing_dir = std::env::temp_dir()
        .join("oxbloom-data-no-such-dir")
        .join("app.db");
    let toml = format!(
        "[data]\nurl = \"sqlite::memory:\"\n[data.sources.broken]\nurl = \"sqlite:{}\"",
        missing_dir.display()
    );
    let sources = build(&toml).unwrap();
    let results = sources.ping_all(Duration::from_secs(10)).await;
    assert_eq!(
        results,
        vec![
            ("default".to_string(), Ok(())),
            ("broken".to_string(), Err(DataError::Connect)),
        ]
    );
    assert_eq!(
        sources.ping("default", Duration::from_secs(10)).await,
        Ok(())
    );
    assert_eq!(
        sources.ping("unknown", Duration::from_secs(10)).await,
        Err(DataError::Config)
    );
    // The good pool now holds its one connection; sqlx returns it to the idle queue
    // asynchronously after the query, so wait for that.
    assert_eq!(sources.pool_counts("default").map(|c| c.0), Some(1));
    for _ in 0..200 {
        if sources.pool_counts("default") == Some((1, 1)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(sources.pool_counts("default"), Some((1, 1)));
}

#[tokio::test]
async fn ping_times_out_when_the_pool_is_exhausted() {
    let sources = build("[data]\nurl = \"sqlite::memory:\"").unwrap();
    let held = sources.sqlite("default").unwrap().acquire().await.unwrap();
    assert_eq!(sources.pool_counts("default"), Some((1, 0)));
    assert_eq!(
        sources.ping("default", Duration::from_millis(100)).await,
        Err(DataError::Pool)
    );
    drop(held);
}

#[tokio::test]
async fn close_all_closes_pools() {
    let toml = format!(
        "[data]\nurl = \"sqlite::memory:\"\n[data.sources.reports]\nurl = \"{}\"",
        unopened_file_url()
    );
    let sources = build(&toml).unwrap();
    sources
        .ping("default", Duration::from_secs(10))
        .await
        .unwrap();
    sources.close_all().await;
    assert!(sources.sqlite("default").unwrap().is_closed());
    assert!(sources.sqlite("reports").unwrap().is_closed());
    assert_eq!(
        sources.ping("default", Duration::from_secs(10)).await,
        Err(DataError::Pool)
    );
}

#[cfg(not(feature = "postgres"))]
#[tokio::test]
async fn a_driver_without_its_feature_is_unsupported() {
    assert_eq!(
        build("[data]\nurl = \"postgres://app@db/app\"").unwrap_err(),
        DataError::Unsupported
    );
}

#[cfg(not(feature = "mysql"))]
#[tokio::test]
async fn mysql_without_its_feature_is_unsupported() {
    let toml = "[data]\nurl = \"sqlite::memory:\"\n[data.sources.rep]\nurl = \"mysql://db/rep\"";
    assert_eq!(build(toml).unwrap_err(), DataError::Unsupported);
}

#[tokio::test]
async fn debug_of_the_registry_shows_no_url() {
    let toml = "[data]\nurl = \"sqlite:/var/lib/s3cret-dir/app.db\"\n\
                [data.sources.mem]\nurl = \"sqlite:file:s3cret?mode=memory&cache=shared\"";
    let sources = build(toml).unwrap();
    let debug = format!("{sources:?}");
    assert!(debug.contains("default"), "{debug}");
    assert!(debug.contains("mem"), "{debug}");
    assert!(debug.contains("Sqlite"), "{debug}");
    assert!(!debug.contains("s3cret"), "{debug}");
    assert!(!debug.contains("var/lib"), "{debug}");

    let material = TlsMaterial {
        trust_pem: Some(b"s3cret-trust".to_vec()),
        identity: Some((b"s3cret-chain".to_vec(), b"s3cret-key".to_vec())),
    };
    let debug = format!("{material:?}");
    assert!(!debug.contains("s3cret"), "{debug}");
    assert!(debug.contains("TlsMaterial"), "{debug}");
}

#[tokio::test]
async fn a_bad_url_option_is_a_config_error_without_its_value() {
    // The settings accept the URL; sqlx refuses the option value, and its message (which
    // echoes the value) is never shown.
    let err = build("[data]\nurl = \"sqlite:app.db?mode=s3cret\"").unwrap_err();
    assert_eq!(err, DataError::Config);
    assert!(!err.to_string().contains("s3cret"));
    assert!(!format!("{err:?}").contains("s3cret"));
}

#[test]
fn building_outside_a_tokio_runtime_is_an_error_not_a_panic() {
    assert_eq!(
        build("[data]\nurl = \"sqlite::memory:\"").unwrap_err(),
        DataError::Pool
    );
    // Without sources there is nothing to spawn, so it works anywhere.
    assert!(build("").unwrap().is_empty());
}

#[tokio::test]
async fn the_tls_function_is_only_called_for_sources_with_a_bundle() {
    let called = std::cell::Cell::new(false);
    let tls = |_: &str| {
        called.set(true);
        Err(DataError::Tls)
    };
    let sources = DataSources::build(&settings("[data]\nurl = \"sqlite::memory:\""), &tls).unwrap();
    assert_eq!(sources.len(), 1);
    assert!(!called.get());
}

#[test]
fn the_registry_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DataSources>();
    assert_send_sync::<TlsMaterial>();
}

#[cfg(feature = "postgres")]
mod postgres {
    use super::*;
    use oxbloom_data::sqlx::postgres::PgSslMode;

    #[tokio::test]
    async fn an_ssl_mode_verify_full_source_builds_connect_options_with_that_mode() {
        let sources =
            build("[data]\nurl = \"postgres://app@db.internal/app\"\nssl-mode = \"verify-full\"")
                .unwrap();
        let options = sources.postgres("default").unwrap().connect_options();
        assert!(matches!(options.get_ssl_mode(), PgSslMode::VerifyFull));
        assert_eq!(sources.postgres("default").unwrap().size(), 0);
    }

    #[tokio::test]
    async fn every_ssl_mode_maps_onto_the_postgres_mode() {
        for (value, check) in [
            (
                "disable",
                (|m| matches!(m, PgSslMode::Disable)) as fn(PgSslMode) -> bool,
            ),
            ("prefer", |m| matches!(m, PgSslMode::Prefer)),
            ("require", |m| matches!(m, PgSslMode::Require)),
            ("verify-ca", |m| matches!(m, PgSslMode::VerifyCa)),
            ("verify-full", |m| matches!(m, PgSslMode::VerifyFull)),
        ] {
            let toml = format!("[data]\nurl = \"postgres://db/app\"\nssl-mode = \"{value}\"");
            let sources = build(&toml).unwrap();
            let mode = sources
                .postgres("default")
                .unwrap()
                .connect_options()
                .get_ssl_mode();
            assert!(check(mode), "{value} gave {mode:?}");
        }
        // Without `ssl-mode` the URL decides.
        let sources = build("[data]\nurl = \"postgres://db/app?sslmode=require\"").unwrap();
        let mode = sources
            .postgres("default")
            .unwrap()
            .connect_options()
            .get_ssl_mode();
        assert!(matches!(mode, PgSslMode::Require));
    }

    #[tokio::test]
    async fn the_bundle_name_reaches_the_tls_function_and_its_material_is_accepted() {
        let seen = std::cell::RefCell::new(Vec::new());
        let tls = |bundle: &str| {
            seen.borrow_mut().push(bundle.to_string());
            Ok(TlsMaterial {
                trust_pem: Some(b"-----BEGIN CERTIFICATE-----\n".to_vec()),
                identity: Some((b"chain".to_vec(), b"key".to_vec())),
            })
        };
        let toml = "[data]\nurl = \"postgres://db/app\"\nssl-mode = \"verify-full\"\n\
                    ssl-bundle = \"db-trust\"";
        let sources = DataSources::build(&settings(toml), &tls).unwrap();
        assert!(sources.postgres("default").is_some());
        assert_eq!(*seen.borrow(), ["db-trust"]);
    }

    #[tokio::test]
    async fn a_failing_tls_function_fails_the_build() {
        let toml = "[data]\nurl = \"postgres://db/app\"\nssl-bundle = \"missing\"";
        assert_eq!(build(toml).unwrap_err(), DataError::Tls);
    }

    #[tokio::test]
    async fn a_bad_postgres_url_is_a_config_error_without_its_value() {
        let err = build("[data]\nurl = \"postgres://u:pw@db/app?sslmode=s3cret\"").unwrap_err();
        assert_eq!(err, DataError::Config);
    }

    #[tokio::test]
    async fn debug_of_a_postgres_registry_shows_no_url() {
        let sources = build("[data]\nurl = \"postgres://app:s3cret@db.internal/app\"").unwrap();
        let debug = format!("{sources:?}");
        assert!(debug.contains("Postgres"), "{debug}");
        assert!(!debug.contains("s3cret"), "{debug}");
        assert!(!debug.contains("db.internal"), "{debug}");
    }
}

#[cfg(feature = "mysql")]
mod mysql {
    use super::*;
    use oxbloom_data::sqlx::mysql::MySqlSslMode;

    #[tokio::test]
    async fn an_ssl_mode_verify_full_source_builds_connect_options_with_that_mode() {
        let sources =
            build("[data]\nurl = \"mysql://app@db.internal/app\"\nssl-mode = \"verify-full\"")
                .unwrap();
        let options = sources.mysql("default").unwrap().connect_options();
        assert!(matches!(
            options.get_ssl_mode(),
            MySqlSslMode::VerifyIdentity
        ));
    }

    #[tokio::test]
    async fn every_ssl_mode_maps_onto_the_mysql_mode() {
        for (value, check) in [
            (
                "disable",
                (|m| matches!(m, MySqlSslMode::Disabled)) as fn(MySqlSslMode) -> bool,
            ),
            ("prefer", |m| matches!(m, MySqlSslMode::Preferred)),
            ("require", |m| matches!(m, MySqlSslMode::Required)),
            ("verify-ca", |m| matches!(m, MySqlSslMode::VerifyCa)),
            ("verify-full", |m| matches!(m, MySqlSslMode::VerifyIdentity)),
        ] {
            let toml = format!("[data]\nurl = \"mariadb://db/app\"\nssl-mode = \"{value}\"");
            let sources = build(&toml).unwrap();
            let mode = sources
                .mysql("default")
                .unwrap()
                .connect_options()
                .get_ssl_mode();
            assert!(check(mode), "{value} gave {mode:?}");
        }
    }

    #[tokio::test]
    async fn the_bundle_material_is_accepted() {
        let tls = |bundle: &str| {
            assert_eq!(bundle, "db-trust");
            Ok(TlsMaterial {
                trust_pem: Some(b"-----BEGIN CERTIFICATE-----\n".to_vec()),
                identity: None,
            })
        };
        let toml = "[data]\nurl = \"mysql://db/app\"\nssl-bundle = \"db-trust\"";
        let sources = DataSources::build(&settings(toml), &tls).unwrap();
        assert_eq!(sources.driver_of("default"), Some(Driver::Mysql));
    }

    #[tokio::test]
    async fn a_bad_mysql_url_is_a_config_error() {
        let err = build("[data]\nurl = \"mysql://db/app?ssl-mode=s3cret\"").unwrap_err();
        assert_eq!(err, DataError::Config);
    }
}
