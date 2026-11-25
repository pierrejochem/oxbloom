//! `DataHealthIndicator` over SQLite sources: the details, the fixed failure texts and the
//! names. No test waits for the real 3 second timeout.

use std::sync::Arc;

use oxbloom_actuator::{DataHealthIndicator, HealthIndicator, Status};
use oxbloom_config::Config;
use oxbloom_data::{DataError, DataSettings, DataSources};
use serde_json::json;

fn sources(toml: &str) -> Arc<DataSources> {
    let settings = DataSettings::from_config(&Config::from_str(toml).unwrap()).unwrap();
    Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap())
}

const MISSING: &str = "sqlite:/no-such-oxbloom-health-dir/secret-file-name.db";

#[tokio::test]
async fn up_with_driver_and_pool_counts() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    let health = DataHealthIndicator::new(s, "default").health().await;
    assert_eq!(health.status, Status::Up);
    assert_eq!(health.details["driver"], json!("sqlite"));
    assert_eq!(health.details["size"], json!(1));
    assert!(health.details["idle"].as_u64().unwrap() <= 1);
    assert!(!health.details.contains_key("error"));
}

#[tokio::test]
async fn down_with_fixed_text_for_an_unreachable_source() {
    let s = sources(&format!("[data]\nurl = \"{MISSING}\""));
    let health = DataHealthIndicator::new(s, "default").health().await;
    assert_eq!(health.status, Status::Down);
    assert_eq!(health.details["error"], json!("connection failed"));
    assert_eq!(health.details["driver"], json!("sqlite"));
}

#[tokio::test]
async fn down_when_the_pool_is_closed() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    s.close_all().await;
    let health = DataHealthIndicator::new(s, "default").health().await;
    assert_eq!(health.status, Status::Down);
    // A closed pool answers at once, so it is not a timeout.
    assert_eq!(health.details["error"], json!("connection failed"));
    assert!(!health.details.contains_key("size"));
}

#[tokio::test]
async fn an_unknown_name_is_down_without_panicking() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    let i = DataHealthIndicator::new(s, "reports");
    assert_eq!(i.name(), "db.reports");
    let health = i.health().await;
    assert_eq!(health.status, Status::Down);
    assert_eq!(health.details["error"], json!("connection failed"));
}

#[tokio::test]
async fn names_are_db_and_db_dot_name() {
    let s = sources(
        "[data]\nurl = \"sqlite::memory:\"\n[data.sources.replica]\nurl = \"sqlite::memory:\"",
    );
    assert_eq!(DataHealthIndicator::new(s.clone(), "default").name(), "db");
    assert_eq!(DataHealthIndicator::new(s, "replica").name(), "db.replica");
}

#[tokio::test]
async fn the_report_never_contains_the_url_or_path() {
    let s = sources(&format!(
        "[data]\nurl = \"{MISSING}\"\n[data.sources.other]\nurl = \"sqlite:/no-such-oxbloom-health-dir/pw-hunter2.db?mode=rwc\""
    ));
    for name in ["default", "other"] {
        let health = DataHealthIndicator::new(s.clone(), name).health().await;
        let text = json!({"status": health.status.as_str(), "details": health.details}).to_string();
        for secret in [
            "no-such-oxbloom",
            "secret-file-name",
            "hunter2",
            "sqlite:/",
            "unable",
            "os error",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
}

async fn history(s: &DataSources, table: &str, rows: &str) {
    let pool = s.sqlite("default").unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TABLE {table} (version BIGINT PRIMARY KEY, description TEXT NOT NULL, \
         success BOOLEAN NOT NULL)"
    )))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} VALUES {rows}"
    )))
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_latest_applied_version_is_a_detail() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    history(
        &s,
        "hist",
        "(1, 'a', true), (20240101, 'b', true), (7, 'c', true)",
    )
    .await;
    let health = DataHealthIndicator::new(s, "default")
        .with_migrations_table("hist")
        .health()
        .await;
    assert_eq!(health.status, Status::Up);
    assert_eq!(health.details["migrations"], json!(20240101));
    assert_eq!(health.details["driver"], json!("sqlite"));
}

#[tokio::test]
async fn without_migrations_there_is_no_detail() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    history(&s, "hist", "(1, 'a', true)").await;
    let health = DataHealthIndicator::new(s, "default").health().await;
    assert_eq!(health.status, Status::Up);
    assert!(!health.details.contains_key("migrations"));
}

#[tokio::test]
async fn a_missing_history_table_leaves_the_status_up_and_the_detail_absent() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    let health = DataHealthIndicator::new(s, "default")
        .with_migrations_table("nothing_here")
        .health()
        .await;
    assert_eq!(health.status, Status::Up);
    assert!(!health.details.contains_key("migrations"));
    assert_eq!(health.details["driver"], json!("sqlite"));
}

#[tokio::test]
async fn an_empty_history_and_a_bad_table_name_leave_the_detail_absent() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    history(&s, "hist", "(1, 'a', false)").await;
    for table in ["hist", "hist; DROP TABLE hist", "", "1hist", "h\"ist"] {
        let health = DataHealthIndicator::new(s.clone(), "default")
            .with_migrations_table(table)
            .health()
            .await;
        assert_eq!(health.status, Status::Up, "{table}");
        assert!(!health.details.contains_key("migrations"), "{table}");
    }
    // The table of the bad names was not touched.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hist")
        .fetch_one(s.sqlite("default").unwrap())
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn a_failed_row_is_not_counted() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    history(
        &s,
        "hist",
        "(1, 'a', true), (2, 'b', true), (3, 'c', false)",
    )
    .await;
    let health = DataHealthIndicator::new(s, "default")
        .with_migrations_table("hist")
        .health()
        .await;
    assert_eq!(health.details["migrations"], json!(2));
}

#[tokio::test]
async fn the_detail_has_no_description_or_sql() {
    let s = sources("[data]\nurl = \"sqlite::memory:\"");
    history(&s, "hist", "(5, 'create secret users table', true)").await;
    let health = DataHealthIndicator::new(s, "default")
        .with_migrations_table("hist")
        .health()
        .await;
    let text = serde_json::to_string(&health.details).unwrap();
    assert!(text.contains("\"migrations\":5"), "{text}");
    for word in ["secret", "create", "SELECT", "hist"] {
        assert!(!text.contains(word), "{word} in {text}");
    }
}

#[tokio::test]
async fn a_down_source_has_no_migrations_detail() {
    let s = sources(&format!("[data]\nurl = \"{MISSING}\""));
    let health = DataHealthIndicator::new(s, "default")
        .with_migrations_table("hist")
        .health()
        .await;
    assert_eq!(health.status, Status::Down);
    assert!(!health.details.contains_key("migrations"));
}
