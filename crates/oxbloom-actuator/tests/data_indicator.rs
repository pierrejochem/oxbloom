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
