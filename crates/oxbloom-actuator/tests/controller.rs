use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oxbloom_actuator::{
    ActuatorController, ActuatorSettings, Availability, BuildInfo, Health, HealthIndicator,
    HealthRunner, Info, Platform, Readiness,
};
use oxbloom_config::Config;
use oxbloom_web::{App, Controller, HttpMethod, Request};

struct Fixed(&'static str, Health);

#[async_trait]
impl HealthIndicator for Fixed {
    fn name(&self) -> &str {
        self.0
    }
    async fn health(&self) -> Health {
        self.1.clone()
    }
}

struct Panics;

#[async_trait]
impl HealthIndicator for Panics {
    fn name(&self) -> &str {
        "boom"
    }
    async fn health(&self) -> Health {
        panic!("panicked on purpose");
    }
}

fn settings(toml: &str) -> ActuatorSettings {
    ActuatorSettings::from_config(&Config::from_str(toml).unwrap()).unwrap()
}

fn app(
    settings: ActuatorSettings,
    availability: Arc<Availability>,
    indicators: Vec<Arc<dyn HealthIndicator>>,
) -> App {
    let runner = Arc::new(HealthRunner::new(indicators, settings.timeout));
    let info = Arc::new(Info {
        build: BuildInfo::new(Some("demo"), Some("1.0.0"), None, None, None),
        platform: Platform::Kubernetes,
        contributors: vec![],
    });
    let controller = Arc::new(ActuatorController::new(
        settings,
        availability,
        runner,
        info,
    ));
    App::new().routes(controller.routes()).unwrap()
}

async fn get(app: &App, path: &str) -> (u16, String) {
    let res = app.handle(Request::new(HttpMethod::Get, path)).await;
    (
        res.status().as_u16(),
        String::from_utf8(res.body().to_vec()).unwrap(),
    )
}

#[tokio::test]
async fn health_is_up_without_indicators_and_hides_details_by_default() {
    let app = app(
        ActuatorSettings::default(),
        Arc::new(Availability::new()),
        vec![],
    );
    let (code, body) = get(&app, "/actuator/health").await;
    assert_eq!((code, body.as_str()), (200, r#"{"status":"UP"}"#));
}

#[tokio::test]
async fn health_shows_components_when_show_details() {
    let db: Arc<dyn HealthIndicator> = Arc::new(Fixed("db", Health::up().detail("pool", 3)));
    let app = app(
        settings("[management.health]\nshow-details = \"always\""),
        Arc::new(Availability::new()),
        vec![db],
    );
    let (code, body) = get(&app, "/actuator/health").await;
    assert_eq!(code, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["status"], "UP");
    assert_eq!(json["components"]["db"]["status"], "UP");
    assert_eq!(json["components"]["db"]["details"]["pool"], 3);
}

#[tokio::test]
async fn health_hides_error_text_when_details_never() {
    let boom: Arc<dyn HealthIndicator> = Arc::new(Panics);
    let db: Arc<dyn HealthIndicator> = Arc::new(Fixed("db", Health::up().detail("pool", 3)));
    let app = app(
        ActuatorSettings::default(),
        Arc::new(Availability::new()),
        vec![boom, db],
    );
    let (code, body) = get(&app, "/actuator/health").await;
    assert_eq!(code, 503);
    assert_eq!(body, r#"{"status":"DOWN"}"#);
    assert!(!body.contains("panicked"));
}

#[tokio::test]
async fn liveness_503_after_break_liveness() {
    let availability = Arc::new(Availability::new());
    let boom: Arc<dyn HealthIndicator> = Arc::new(Panics);
    let app = app(
        ActuatorSettings::default(),
        availability.clone(),
        vec![boom],
    );
    let (code, body) = get(&app, "/actuator/health/liveness").await;
    assert_eq!((code, body.as_str()), (200, r#"{"status":"UP"}"#));
    availability.break_liveness();
    let (code, body) = get(&app, "/actuator/health/liveness").await;
    assert_eq!((code, body.as_str()), (503, r#"{"status":"DOWN"}"#));
}

#[tokio::test]
async fn readiness_503_until_accepting() {
    let availability = Arc::new(Availability::new());
    let app = app(ActuatorSettings::default(), availability.clone(), vec![]);
    let (code, body) = get(&app, "/actuator/health/readiness").await;
    assert_eq!((code, body.as_str()), (503, r#"{"status":"DOWN"}"#));
    availability.set_readiness(Readiness::AcceptingTraffic);
    let (code, body) = get(&app, "/actuator/health/readiness").await;
    assert_eq!((code, body.as_str()), (200, r#"{"status":"UP"}"#));
}

#[tokio::test]
async fn readiness_includes_only_listed_indicators() {
    let availability = Arc::new(Availability::new());
    availability.set_readiness(Readiness::AcceptingTraffic);
    let bad: Arc<dyn HealthIndicator> = Arc::new(Fixed("cache", Health::down()));
    let good: Arc<dyn HealthIndicator> = Arc::new(Fixed("db", Health::up()));
    let indicators = vec![bad, good];
    let unlisted = app(
        ActuatorSettings::default(),
        availability.clone(),
        indicators.clone(),
    );
    assert_eq!(get(&unlisted, "/actuator/health/readiness").await.0, 200);
    let listed = app(
        settings("[management.health.readiness]\ninclude = [\"cache\"]"),
        availability.clone(),
        indicators.clone(),
    );
    assert_eq!(get(&listed, "/actuator/health/readiness").await.0, 503);
    let other = app(
        settings("[management.health.readiness]\ninclude = [\"db\"]"),
        availability,
        indicators,
    );
    assert_eq!(get(&other, "/actuator/health/readiness").await.0, 200);
}

#[tokio::test]
async fn info_has_build_and_platform() {
    let app = app(
        ActuatorSettings::default(),
        Arc::new(Availability::new()),
        vec![],
    );
    let (code, body) = get(&app, "/actuator/info").await;
    assert_eq!(code, 200);
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["build"]["name"], "demo");
    assert_eq!(json["platform"], "kubernetes");
}

#[tokio::test]
async fn disabled_info_is_404() {
    let app = app(
        settings("[management.endpoint.info]\nenabled = false"),
        Arc::new(Availability::new()),
        vec![],
    );
    assert_eq!(get(&app, "/actuator/info").await.0, 404);
    assert_eq!(get(&app, "/actuator/health").await.0, 200);
}

#[test]
fn base_path_is_normalized() {
    let s = settings("[management.endpoints]\nbase-path = \"management/\"");
    assert_eq!(s.base_path, "/management");
}

#[test]
fn root_base_path_is_rejected() {
    let config = Config::from_str("[management.endpoints]\nbase-path = \"/\"").unwrap();
    let err = ActuatorSettings::from_config(&config).unwrap_err();
    assert!(
        err.to_string().contains("management.endpoints.base-path"),
        "{err}"
    );
}

#[test]
fn unknown_management_key_is_rejected_naming_the_key() {
    for (toml, key) in [
        (
            "[management]\nshow-detials = \"MARKER-VALUE-7\"",
            "show-detials",
        ),
        (
            "[management.health]\nshow-detials = \"MARKER-VALUE-7\"",
            "show-detials",
        ),
        (
            "[management.endpoints]\nbase-pat = \"MARKER-VALUE-7\"",
            "base-pat",
        ),
        ("[management.endpoint.info]\nenabeld = true", "enabeld"),
    ] {
        let config = Config::from_str(toml).unwrap();
        let text = ActuatorSettings::from_config(&config)
            .unwrap_err()
            .to_string();
        assert!(text.contains(key), "{text}");
        assert!(!text.contains("MARKER-VALUE-7"), "value leaked: {text}");
    }
}

#[test]
fn timeout_reads_a_duration() {
    let s = settings("[management.health]\ntimeout = \"2s\"");
    assert_eq!(s.timeout, Duration::from_secs(2));
}
