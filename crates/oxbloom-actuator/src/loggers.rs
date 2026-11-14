//! The `loggers` endpoint: list, read and change log levels at runtime.

use std::sync::Arc;

use http::StatusCode;
use oxbloom_logging::{Level, LoggingError, LoggingSystem};
use oxbloom_web::{HttpError, HttpMethod, IntoResponse, Json, Request, Route};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// The largest request body that `POST {base}/loggers/{target}` reads; a larger one is a 400.
const MAX_BODY: usize = 1024;

#[derive(Deserialize)]
struct Change {
    #[serde(rename = "configuredLevel")]
    configured_level: String,
}

fn bad_request(detail: &str) -> HttpError {
    HttpError::new(StatusCode::BAD_REQUEST).detail(detail)
}

fn name(level: Level) -> String {
    level.to_string().to_uppercase()
}

/// The target of the request, or the 400 that answers an empty or invalid one.
///
/// A valid target holds only letters, digits, `_`, `-` and `:`, as
/// [`LoggingSystem::validate_target`] checks; reads and writes share the rule.
#[allow(clippy::result_large_err)] // `HttpError` is the error type of handlers.
fn target(req: &Request) -> Result<String, HttpError> {
    match req.param("target") {
        Some(t) if !t.is_empty() => {
            LoggingSystem::validate_target(t)
                .map_err(|_| bad_request("the logger name is not valid"))?;
            Ok(t.to_string())
        }
        _ => Err(bad_request("the logger name is empty")),
    }
}

#[allow(clippy::result_large_err)] // `HttpError` is the error type of handlers.
fn change(logging: &LoggingSystem, req: &Request) -> Result<(), HttpError> {
    let target = target(req)?;
    if req.body_too_large() || req.body.len() > MAX_BODY {
        return Err(bad_request("the request body is too large"));
    }
    // `Request::json` answers a missing or non-JSON `Content-Type` with a 415, so a
    // cross-origin form or `text/plain` POST cannot change a level without a CORS preflight.
    let change: Change = req.json().map_err(|error| {
        if error.clone().into_response().status() == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            error
        } else {
            bad_request("the body must be {\"configuredLevel\": \"<level>\"}")
        }
    })?;
    let level: Level = change.configured_level.parse().map_err(|_| {
        bad_request("configuredLevel must be one of OFF, ERROR, WARN, INFO, DEBUG, TRACE")
    })?;
    logging.set_level(&target, level).map_err(|e| match e {
        LoggingError::Config(_) => bad_request("the logger name is not valid"),
        _ => HttpError::new(StatusCode::INTERNAL_SERVER_ERROR),
    })
}

/// The routes of the loggers endpoint under `base`.
pub(crate) fn routes(base: &str, logging: &Arc<LoggingSystem>) -> Vec<Route> {
    let path = format!("{base}/loggers");
    let list = {
        let logging = Arc::clone(logging);
        Route::new(HttpMethod::Get, path.clone(), move |_| {
            let levels: Map<String, Value> = logging
                .levels()
                .into_iter()
                .map(|(target, level)| (target, json!(name(level))))
                .collect();
            async move { Json(json!({ "levels": levels })) }
        })
    };
    let get = {
        let logging = Arc::clone(logging);
        Route::new(
            HttpMethod::Get,
            format!("{path}/{{target}}"),
            move |req: Request| {
                let response = match target(&req) {
                    Err(error) => error.into_response(),
                    Ok(target) => Json(json!({
                        "configuredLevel": logging.levels().get(&target).map(|l| name(*l)),
                        "effectiveLevel": name(logging.level(&target)),
                    }))
                    .into_response(),
                };
                async move { response }
            },
        )
    };
    let post = {
        let logging = Arc::clone(logging);
        Route::new(
            HttpMethod::Post,
            format!("{path}/{{target}}"),
            move |req: Request| {
                let response = match change(&logging, &req) {
                    Ok(()) => StatusCode::NO_CONTENT.into_response(),
                    Err(error) => error.into_response(),
                };
                async move { response }
            },
        )
    };
    // `{target}` never matches an empty segment, so the empty name gets routes of its own.
    let empty = [HttpMethod::Get, HttpMethod::Post].map(|method| {
        Route::new(method, format!("{path}/"), |_| async {
            bad_request("the logger name is empty").into_response()
        })
    });
    let [empty_get, empty_post] = empty;
    vec![list, get, post, empty_get, empty_post]
}
