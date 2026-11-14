use std::sync::Arc;
use std::time::Duration;

use oxbloom_actuator::{
    ActuatorController, ActuatorSettings, Availability, BuildInfo, HealthRunner, Info, Platform,
};
use oxbloom_config::Config;
use oxbloom_logging::__private::scoped;
use oxbloom_logging::{Level, LoggingSystem};
use oxbloom_web::{App, Controller, HttpMethod, Request};

fn app_with(settings: ActuatorSettings) -> (App, LoggingSystem, tracing::Dispatch) {
    let (dispatch, logging) = scoped(
        &Config::from_str("[logging.level]\nroot = \"warn\"").unwrap(),
        std::io::sink,
    )
    .unwrap();
    let controller = ActuatorController::new(
        settings,
        Arc::new(Availability::new()),
        Arc::new(HealthRunner::new(vec![], Duration::from_secs(1))),
        Arc::new(Info {
            build: BuildInfo::default(),
            platform: Platform::None,
            contributors: vec![],
        }),
    )
    .with_loggers(Arc::new(logging.clone()));
    let app = App::new().routes(Arc::new(controller).routes()).unwrap();
    (app, logging, dispatch)
}

fn app() -> (App, LoggingSystem, tracing::Dispatch) {
    app_with(ActuatorSettings::default())
}

async fn call(app: &App, method: HttpMethod, path: &str, body: Option<&str>) -> (u16, String) {
    let mut req = Request::new(method, path);
    if let Some(body) = body {
        req.headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        req.body = body.to_string().into();
    }
    let res = app.handle(req).await;
    (
        res.status().as_u16(),
        String::from_utf8(res.body().to_vec()).unwrap(),
    )
}

#[tokio::test]
async fn post_changes_the_level_and_get_reports_it() {
    let (app, logging, _d) = app();
    let (code, body) = call(
        &app,
        HttpMethod::Post,
        "/actuator/loggers/oxbloom_web::router",
        Some(r#"{"configuredLevel":"DEBUG"}"#),
    )
    .await;
    assert_eq!((code, body.as_str()), (204, ""));
    assert_eq!(logging.level("oxbloom_web::router"), Level::Debug);

    let (code, body) = call(
        &app,
        HttpMethod::Get,
        "/actuator/loggers/oxbloom_web::router",
        None,
    )
    .await;
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["configuredLevel"], "DEBUG");
    assert_eq!(v["effectiveLevel"], "DEBUG");

    let (code, body) = call(&app, HttpMethod::Get, "/actuator/loggers", None).await;
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["levels"]["oxbloom_web::router"], "DEBUG");
    assert_eq!(v["levels"]["root"], "WARN");
}

#[tokio::test]
async fn post_with_bad_json_is_400_and_changes_nothing() {
    let (app, logging, _d) = app();
    let before = logging.levels();
    for body in [
        "{not json",
        "",
        r#"{"configuredLevel":5}"#,
        r#"{"other":"DEBUG"}"#,
        r#"{"configuredLevel":null}"#,
    ] {
        let (code, text) = call(&app, HttpMethod::Post, "/actuator/loggers/app", Some(body)).await;
        assert_eq!(code, 400, "{body}");
        assert!(text.contains("\"status\":400"), "{text}");
    }
    assert_eq!(logging.levels(), before);
}

#[tokio::test]
async fn post_without_json_content_type_is_415_and_changes_nothing() {
    let (app, logging, _d) = app();
    let before = logging.levels();
    for content_type in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
    ] {
        let mut req = Request::new(HttpMethod::Post, "/actuator/loggers/app");
        if let Some(content_type) = content_type {
            req.headers.insert(
                http::header::CONTENT_TYPE,
                http::HeaderValue::from_static(content_type),
            );
        }
        req.body = r#"{"configuredLevel":"DEBUG"}"#.to_string().into();
        let res = app.handle(req).await;
        assert_eq!(res.status().as_u16(), 415, "{content_type:?}");
        let text = String::from_utf8(res.body().to_vec()).unwrap();
        assert!(text.contains("\"status\":415"), "{text}");
        assert!(text.contains("application/json"), "{text}");
    }
    assert_eq!(logging.levels(), before);
}

#[tokio::test]
async fn post_with_oversized_body_is_400() {
    let (app, logging, _d) = app();
    let before = logging.levels();
    let big = format!(
        r#"{{"configuredLevel":"DEBUG","pad":"{}"}}"#,
        "x".repeat(10_000)
    );
    let (code, _) = call(&app, HttpMethod::Post, "/actuator/loggers/app", Some(&big)).await;
    assert_eq!(code, 400);
    assert_eq!(logging.levels(), before);
}

#[tokio::test]
async fn post_with_unknown_level_is_400() {
    let (app, logging, _d) = app();
    let before = logging.levels();
    let (code, text) = call(
        &app,
        HttpMethod::Post,
        "/actuator/loggers/app",
        Some(r#"{"configuredLevel":"LOUD"}"#),
    )
    .await;
    assert_eq!(code, 400);
    assert!(!text.contains("LOUD"), "{text}");
    assert_eq!(logging.levels(), before);
}

#[tokio::test]
async fn post_with_invalid_target_is_400() {
    let (app, logging, _d) = app();
    let before = logging.levels();
    let (code, _) = call(
        &app,
        HttpMethod::Post,
        "/actuator/loggers/not,a,target",
        Some(r#"{"configuredLevel":"DEBUG"}"#),
    )
    .await;
    assert_eq!(code, 400);
    assert_eq!(logging.levels(), before);
}

#[tokio::test]
async fn get_with_dotted_target_is_400() {
    let (app, _logging, _d) = app();
    for path in ["/actuator/loggers/a.b", "/actuator/loggers/not,a,target"] {
        let (code, text) = call(&app, HttpMethod::Get, path, None).await;
        assert_eq!(code, 400, "{path}");
        assert!(text.contains("the logger name is not valid"), "{text}");
    }
}

#[tokio::test]
async fn empty_target_is_400() {
    let (app, logging, _d) = app();
    let before = logging.levels();
    let (code, _) = call(
        &app,
        HttpMethod::Post,
        "/actuator/loggers/",
        Some(r#"{"configuredLevel":"DEBUG"}"#),
    )
    .await;
    assert_eq!(code, 400);
    let (code, _) = call(&app, HttpMethod::Get, "/actuator/loggers/", None).await;
    assert_eq!(code, 400);
    assert_eq!(logging.levels(), before);
}

#[tokio::test]
async fn get_unknown_target_reports_the_inherited_level_with_no_configured_level() {
    let (app, _logging, _d) = app();
    let (code, body) = call(&app, HttpMethod::Get, "/actuator/loggers/never_seen", None).await;
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["configuredLevel"], serde_json::Value::Null);
    assert_eq!(v["effectiveLevel"], "WARN");
}

#[tokio::test]
async fn disabled_loggers_register_no_routes() {
    let settings = ActuatorSettings::from_config(
        &Config::from_str("[management.endpoint.loggers]\nenabled = false").unwrap(),
    )
    .unwrap();
    let (app, _logging, _d) = app_with(settings);
    let (code, _) = call(&app, HttpMethod::Get, "/actuator/loggers", None).await;
    assert_eq!(code, 404);
}
