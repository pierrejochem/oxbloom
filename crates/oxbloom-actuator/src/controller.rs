use std::sync::Arc;

use oxbloom_web::{Controller, HttpMethod, Json, Route};
use serde_json::{Value, json};

use crate::{ActuatorSettings, Availability, HealthRunner, Info, Liveness, Readiness, Status};

/// The web controller of the actuator: `GET {base}/health`, `{base}/health/liveness`,
/// `{base}/health/readiness` and `GET {base}/info`.
///
/// Available with the cargo feature `web`. An endpoint that the settings disable registers no
/// routes, so the router answers 404. `/health*` answers 200 only for `UP` and 503 otherwise.
/// `/health` shows the components only with `show-details=always`; it never shows error text
/// or details otherwise. Liveness never runs an indicator. Readiness runs only the indicators
/// named in `readiness.include`, and answers `{"status":"UP"}` or `{"status":"DOWN"}`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use oxbloom_actuator::*;
/// use oxbloom_web::{App, Controller, HttpMethod, Request};
///
/// # tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
/// let controller = Arc::new(ActuatorController::new(
///     ActuatorSettings::default(),
///     Arc::new(Availability::new()),
///     Arc::new(HealthRunner::new(vec![], Duration::from_secs(1))),
///     Arc::new(Info { build: BuildInfo::default(), platform: Platform::None, contributors: vec![] }),
/// ));
/// let app = App::new().routes(controller.routes()).unwrap();
/// let res = app.handle(Request::new(HttpMethod::Get, "/actuator/health")).await;
/// assert_eq!(res.status(), 200);
/// assert_eq!(res.body(), r#"{"status":"UP"}"#);
/// # });
/// ```
pub struct ActuatorController {
    settings: ActuatorSettings,
    availability: Arc<Availability>,
    runner: Arc<HealthRunner>,
    info: Arc<Info>,
    #[cfg(feature = "logging")]
    loggers: Option<Arc<oxbloom_logging::LoggingSystem>>,
}

impl ActuatorController {
    /// Creates the controller.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_actuator::*;
    ///
    /// let _controller = ActuatorController::new(
    ///     ActuatorSettings::default(),
    ///     Arc::new(Availability::new()),
    ///     Arc::new(HealthRunner::new(vec![], Duration::from_secs(1))),
    ///     Arc::new(Info { build: BuildInfo::default(), platform: Platform::None, contributors: vec![] }),
    /// );
    /// ```
    pub fn new(
        settings: ActuatorSettings,
        availability: Arc<Availability>,
        runner: Arc<HealthRunner>,
        info: Arc<Info>,
    ) -> Self {
        Self {
            settings,
            availability,
            runner,
            info,
            #[cfg(feature = "logging")]
            loggers: None,
        }
    }

    /// Serves the `loggers` endpoint over `logging`: `GET {base}/loggers` answers
    /// `{"levels":{target:level}}` (levels in upper case, `root` included),
    /// `GET {base}/loggers/{target}` answers `{"configuredLevel":..,"effectiveLevel":..}` and
    /// `POST {base}/loggers/{target}` with `{"configuredLevel":"DEBUG"}` sets the level and
    /// answers 204.
    ///
    /// A target without a level of its own is not an error: `configuredLevel` is `null` and
    /// `effectiveLevel` is the level it inherits, as `LoggingSystem::level` reports. A body
    /// that is not that JSON object, is over 1 KiB, names an unknown level, or an empty or
    /// invalid target is a 400 problem answer and changes nothing. The endpoint follows
    /// `management.endpoint.loggers.enabled`.
    ///
    /// Available with the cargo feature `logging`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_actuator::*;
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::{Level, LoggingSystem};
    /// use oxbloom_web::{App, Controller, HttpMethod, Request};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
    /// let logging = Arc::new(LoggingSystem::init(&Config::from_str("").unwrap()).unwrap());
    /// let controller = Arc::new(
    ///     ActuatorController::new(
    ///         ActuatorSettings::default(),
    ///         Arc::new(Availability::new()),
    ///         Arc::new(HealthRunner::new(vec![], Duration::from_secs(1))),
    ///         Arc::new(Info { build: BuildInfo::default(), platform: Platform::None, contributors: vec![] }),
    ///     )
    ///     .with_loggers(Arc::clone(&logging)),
    /// );
    /// let app = App::new().routes(controller.routes()).unwrap();
    /// let mut req = Request::new(HttpMethod::Post, "/actuator/loggers/app");
    /// req.headers.insert("content-type", "application/json".parse().unwrap());
    /// req.body = r#"{"configuredLevel":"DEBUG"}"#.into();
    /// assert_eq!(app.handle(req).await.status(), 204);
    /// assert_eq!(logging.level("app"), Level::Debug);
    /// # });
    /// ```
    #[cfg(feature = "logging")]
    #[must_use]
    pub fn with_loggers(mut self, logging: Arc<oxbloom_logging::LoggingSystem>) -> Self {
        self.loggers = Some(logging);
        self
    }
}

fn answer(status: Status, body: Value) -> (http::StatusCode, Json<Value>) {
    let code = if status.is_up() {
        http::StatusCode::OK
    } else {
        http::StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body))
}

fn state(up: bool) -> Status {
    if up { Status::Up } else { Status::Down }
}

impl Controller for ActuatorController {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        let mut routes = Vec::new();
        let base = self.settings.base_path.clone();
        if self.settings.health_enabled {
            let this = Arc::clone(&self);
            routes.push(Route::new(
                HttpMethod::Get,
                format!("{base}/health"),
                move |_| {
                    let this = Arc::clone(&this);
                    async move {
                        let (status, parts) = this.runner.run(None).await;
                        let mut body = json!({"status": status.as_str()});
                        if this.settings.show_details {
                            let components: serde_json::Map<String, Value> = parts
                                .iter()
                                .map(|(name, health)| {
                                    let mut c = json!({"status": health.status.as_str()});
                                    if !health.details.is_empty() {
                                        c["details"] = json!(health.details);
                                    }
                                    (name.clone(), c)
                                })
                                .collect();
                            body["components"] = Value::Object(components);
                        }
                        answer(status, body)
                    }
                },
            ));
            let this = Arc::clone(&self);
            routes.push(Route::new(
                HttpMethod::Get,
                format!("{base}/health/liveness"),
                move |_| {
                    let status = state(this.availability.liveness() == Liveness::Correct);
                    async move { answer(status, json!({"status": status.as_str()})) }
                },
            ));
            let this = Arc::clone(&self);
            routes.push(Route::new(
                HttpMethod::Get,
                format!("{base}/health/readiness"),
                move |_| {
                    let this = Arc::clone(&this);
                    async move {
                        let accepting =
                            this.availability.readiness() == Readiness::AcceptingTraffic;
                        let (indicators, _) = this
                            .runner
                            .run(Some(&this.settings.readiness_include))
                            .await;
                        let status = state(accepting && indicators.is_up());
                        answer(status, json!({"status": status.as_str()}))
                    }
                },
            ));
        }
        if self.settings.info_enabled {
            let this = Arc::clone(&self);
            routes.push(Route::new(
                HttpMethod::Get,
                format!("{base}/info"),
                move |_| {
                    let body = this.info.render();
                    async move { Json(body) }
                },
            ));
        }
        #[cfg(feature = "logging")]
        if self.settings.loggers_enabled
            && let Some(logging) = &self.loggers
        {
            routes.extend(crate::loggers::routes(&base, logging));
        }
        routes
    }
}
