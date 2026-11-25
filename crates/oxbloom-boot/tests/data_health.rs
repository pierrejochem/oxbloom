//! The data health indicators of `ActuatorAutoConfiguration`: one per data source, listed by
//! `GET /actuator/health`, run in process through `TestApp` with SQLite sources.

#![cfg(all(feature = "actuator", feature = "data-sqlite"))]

use std::sync::Arc;

use oxbloom_boot::{ActuatorAutoConfiguration, DataAutoConfiguration};
use oxbloom_test::TestApp;
use oxbloom_web::{Controller, HttpMethod, Route};
use serde_json::Value;

struct Ping;
impl Controller for Ping {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    }
}

fn app(config: &str) -> TestApp {
    TestApp::web()
        .config_str(format!(
            "[management.health]\nshow-details = \"always\"\n{config}"
        ))
        .auto_configuration::<DataAutoConfiguration>()
        .auto_configuration::<ActuatorAutoConfiguration>()
        .configure(|b| {
            b.instance(Ping)
                .provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c })
        })
}

const TWO_SOURCES: &str = "[data]\nurl = \"sqlite::memory:\"\n\
    [data.sources.replica]\nurl = \"sqlite::memory:\"";

#[tokio::test]
async fn the_health_endpoint_lists_db_for_the_default_and_named_sources() {
    let app = app(TWO_SOURCES).start().await;
    let res = app.client().get("/actuator/health").send().await;
    assert_eq!(res.status(), 200);
    let body: Value = serde_json::from_str(&res.text()).unwrap();
    assert_eq!(body["status"], "UP");
    for name in ["db", "db.replica"] {
        assert_eq!(body["components"][name]["status"], "UP", "{body}");
        assert_eq!(body["components"][name]["details"]["driver"], "sqlite");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_down_source_turns_the_health_down_without_its_path() {
    let app = app(
        "[data]\nurl = \"sqlite::memory:\"\nconnect-on-startup = false\n\
         [data.sources.replica]\nurl = \"sqlite:/no-such-oxbloom-data-dir/secret-file-name.db\"",
    )
    .start()
    .await;
    let res = app.client().get("/actuator/health").send().await;
    assert_eq!(res.status(), 503);
    let text = res.text();
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["components"]["db"]["status"], "UP", "{body}");
    assert_eq!(body["components"]["db.replica"]["status"], "DOWN", "{body}");
    assert!(
        !text.contains("secret-file-name") && !text.contains("no-such"),
        "{text}"
    );
    app.stop().await;
}

#[tokio::test]
async fn readiness_can_include_a_data_indicator() {
    let app = app(&format!(
        "{TWO_SOURCES}\n[management.health.readiness]\ninclude = [\"db\", \"db.replica\"]"
    ))
    .start()
    .await;
    let res = app.client().get("/actuator/health/readiness").send().await;
    assert_eq!(res.status(), 200, "{}", res.text());
    app.stop().await;
}
