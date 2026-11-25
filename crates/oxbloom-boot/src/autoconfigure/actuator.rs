//! `ActuatorAutoConfiguration`: the actuator endpoints of `oxbloom-actuator`, its availability
//! listener, health indicators and info contributors.

use std::sync::Arc;

use oxbloom_actuator::{
    ActuatorController, ActuatorSettings, Availability, BuildInfo, HealthIndicator, HealthRunner,
    Info, InfoContributor, Platform, Readiness,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{Conditions, Configuration, Container, ContainerBuilder, DiError, component};
use oxbloom_web::Controller;

use crate::autoconfig::{AutoConfiguration, ConfigRef};
use crate::listener::RunListener;
use crate::metadata::PropertyMetadata;

/// The key whose entries must name registered health indicators.
const READINESS_INCLUDE: &str = "management.health.readiness.include";

/// Registers the actuator endpoints of `oxbloom-actuator` as a `dyn Controller`, with the beans
/// they read, in a web application, unless `management.endpoints.enabled` is `false`.
///
/// Available with the cargo feature `actuator`. The group runs when the application registers
/// a `dyn Controller` of its own (the condition `on_bean`, the one `WebServerAutoConfiguration`
/// has), so the actuator joins a web application and never turns another run into a server;
/// and when `management.endpoints.enabled` is missing or `true` (the condition
/// `on_property_value_or_missing`). The conditions report names the reason for a skip. It
/// registers:
///
/// - the [`ActuatorSettings`] of the `management.*` keys; an unknown key or a bad value fails
///   the build with `DiError::Creation` whose source is the `ConfigError` naming the key;
/// - an [`Availability`] (unless the application registered one) and a `RunListener` bean that
///   sets readiness to `AcceptingTraffic` at `ready`, after the services (the web server) have
///   started, and back to `RefusingTraffic` at `stopping`. It has the lowest `order`, so it runs
///   before the other `RunListener` beans (builder listeners still run first);
/// - a [`HealthRunner`] over every `dyn HealthIndicator` bean, with `management.health.timeout`
///   per indicator. An entry of `management.health.readiness.include` that names no registered
///   indicator fails the build with a `ConfigError` on that key listing the unknown names, so
///   readiness never ignores a misspelt indicator;
/// - an [`Info`] with the application's [`BuildInfo`] bean (all fields absent without one), the
///   detected [`Platform`] and every `dyn InfoContributor` bean;
/// - the [`ActuatorController`], which also serves `/loggers` with the feature `logging` when
///   there is a `LoggingSystem` bean. Without the feature `security` nothing authenticates the
///   actuator endpoints, so boot logs a `warn` event at the target `oxbloom::boot` when it
///   registers the loggers routes, which can change log levels;
/// - with the feature `ssl` and an `SslBundles` bean, an [`SslHealthIndicator`](oxbloom_actuator::SslHealthIndicator)
///   as `dyn HealthIndicator`, reading the time from the `dyn Clock` bean when there is one;
/// - with a `data-*` feature and a `DataSources` bean, one `DataHealthIndicator` per data
///   source: `db` for the default source and `db.<name>` for the others. The container has no
///   way to register a bean per source name, so one `DataIndicators` bean builds them when it
///   is created and the runner reads them next to the `dyn HealthIndicator` beans (their names
///   count for `management.health.readiness.include`);
/// - with the feature `security` and `management.endpoints.health.public = true`, a
///   [`SecurityCustomizer`](crate::SecurityCustomizer) that permits `GET <base-path>/health/**`
///   and requires authentication for every other request. It owns the authorization rules, so
///   an application with a customizer of its own that calls `authorize` should open the health
///   path there instead (two `authorize` sections fail the build). Without it, the default
///   chain requires authentication for every actuator endpoint. Without the feature
///   `security` every actuator endpoint is open, the loggers `POST` included.
///
/// It runs after `SslAutoConfiguration` and before `WebServerAutoConfiguration`, which collects
/// its controller.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_actuator::{ActuatorController, Availability, Readiness};
/// use oxbloom_boot::{ActuatorAutoConfiguration, AutoConfiguration, Banner, OxbloomApplication, RunListener};
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_web::{App, Controller, HttpMethod, Request, Route};
///
/// /// The application's own controller: without one the actuator is skipped.
/// struct Hello;
/// impl Controller for Hello {
///     fn routes(self: Arc<Self>) -> Vec<Route> {
///         vec![Route::new(HttpMethod::Get, "/hello", |_| async { "hello" })]
///     }
/// }
///
/// #[derive(Clone, Default)]
/// struct Seen(Arc<Mutex<Option<(Arc<ActuatorController>, Arc<Availability>)>>>);
/// impl RunListener for Seen {
///     fn context_ready(&self, c: &Container) {
///         let beans = (c.get::<ActuatorController>().unwrap(), c.get::<Availability>().unwrap());
///         *self.0.lock().unwrap() = Some(beans);
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let seen = Seen::default();
/// // No `WebServerAutoConfiguration` here, so no server: `run()` returns after `ready`.
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("[management.endpoints]\nbase-path = \"/ops\"")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(seen.clone())
///     .auto_configuration::<ActuatorAutoConfiguration>()
///     .configure(|b| b.instance(Hello).provides(|c: Arc<Hello>| -> Arc<dyn Controller> { c }))
///     .run()
///     .await?;
/// let (controller, availability) = seen.0.lock().unwrap().take().unwrap();
/// // The run has stopped, so the listener has set readiness back to refusing.
/// assert_eq!(availability.readiness(), Readiness::RefusingTraffic);
/// let app = App::new().routes(controller.routes())?;
/// let res = app.handle(Request::new(HttpMethod::Get, "/ops/health/readiness")).await;
/// assert_eq!(res.status(), 503);
/// let res = app.handle(Request::new(HttpMethod::Get, "/ops/health/liveness")).await;
/// assert_eq!(res.status(), 200);
///
/// assert!(ActuatorAutoConfiguration::properties().iter().any(|p| p.key == "management.health.timeout"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct ActuatorAutoConfiguration;

impl Configuration for ActuatorAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        let builder = builder
            .try_bean(actuator_settings)
            .bean(Availability::new)
            .on_missing_bean::<Availability>()
            .bean(|availability: Arc<Availability>| AvailabilityListener(availability))
            .order(i32::MIN)
            .provides(|l: Arc<AvailabilityListener>| -> Arc<dyn RunListener> { l })
            .try_bean(health_runner)
            .bean(info)
            .bean(actuator_controller)
            .provides(|c: Arc<ActuatorController>| -> Arc<dyn Controller> { c })
            .on_missing_bean::<ActuatorController>();
        #[cfg(feature = "ssl")]
        let builder = builder
            .bean(ssl_indicator)
            .provides(
                |i: Arc<oxbloom_actuator::SslHealthIndicator>| -> Arc<dyn HealthIndicator> { i },
            )
            .on_bean::<oxbloom_ssl::SslBundles>();
        #[cfg(any(
            feature = "data-postgres",
            feature = "data-mysql",
            feature = "data-sqlite"
        ))]
        let builder = builder
            .bean(data_indicators)
            .on_bean::<oxbloom_data::DataSources>();
        #[cfg(feature = "security")]
        let builder = builder
            .bean(|settings: Arc<ActuatorSettings>| PublicHealth {
                pattern: format!("{}/health/**", settings.base_path),
            })
            .provides(|c: Arc<PublicHealth>| -> Arc<dyn crate::SecurityCustomizer> { c })
            .on_property_value("management.endpoints.health.public", "true");
        builder
    }
}

impl AutoConfiguration for ActuatorAutoConfiguration {
    /// Runs in a web application (an application `dyn Controller`), and backs off when
    /// `management.endpoints.enabled` is set and is not `true`.
    fn conditions() -> Conditions {
        Conditions::new()
            .on_property_value_or_missing("management.endpoints.enabled", "true")
            .on_bean::<dyn Controller>()
    }

    /// The `SslBundles` and `DataSources` beans decide whether the `ssl` and `db` indicators
    /// are registered.
    fn after() -> Vec<ConfigRef> {
        [
            #[cfg(feature = "ssl")]
            ConfigRef::of::<super::SslAutoConfiguration>(),
            #[cfg(any(
                feature = "data-postgres",
                feature = "data-mysql",
                feature = "data-sqlite"
            ))]
            ConfigRef::of::<super::DataAutoConfiguration>(),
        ]
        .into()
    }

    /// The server collects the controllers when it is built, so this runs first.
    fn before() -> Vec<ConfigRef> {
        vec![ConfigRef::of::<super::WebServerAutoConfiguration>()]
    }

    fn properties() -> &'static [PropertyMetadata] {
        &MANAGEMENT
    }
}

/// Sets readiness to accepting at `ready` and to refusing at `stopping`.
struct AvailabilityListener(Arc<Availability>);

impl RunListener for AvailabilityListener {
    fn ready(&self, _container: &Container) {
        self.0.set_readiness(Readiness::AcceptingTraffic);
    }

    fn stopping(&self) {
        self.0.set_readiness(Readiness::RefusingTraffic);
    }
}

/// The settings of the `management` keys; the `ConfigError` stays the typed source.
fn actuator_settings(config: Arc<Config>) -> Result<ActuatorSettings, DiError> {
    ActuatorSettings::from_config(&config).map_err(DiError::creation_from::<ActuatorSettings>)
}

/// The runner over every indicator bean. Every `readiness.include` entry must name one of
/// them: an unknown name would otherwise drop out of readiness without a trace.
fn health_runner(
    settings: Arc<ActuatorSettings>,
    indicators: Vec<Arc<dyn HealthIndicator>>,
    #[cfg(any(
        feature = "data-postgres",
        feature = "data-mysql",
        feature = "data-sqlite"
    ))]
    data: Option<Arc<DataIndicators>>,
) -> Result<HealthRunner, DiError> {
    #[cfg(any(
        feature = "data-postgres",
        feature = "data-mysql",
        feature = "data-sqlite"
    ))]
    let indicators: Vec<Arc<dyn HealthIndicator>> = indicators
        .into_iter()
        .chain(data.iter().flat_map(|d| d.0.iter().cloned()))
        .collect();
    let unknown: Vec<&str> = settings
        .readiness_include
        .iter()
        .map(String::as_str)
        .filter(|name| !indicators.iter().any(|i| i.name() == *name))
        .collect();
    if !unknown.is_empty() {
        let known: Vec<&str> = indicators.iter().map(|i| i.name()).collect();
        let known = if known.is_empty() {
            "(none)".to_owned()
        } else {
            known.join(", ")
        };
        let message = format!(
            "names no registered health indicator: {}; registered: {known}",
            unknown.join(", ")
        );
        let error = ConfigError::invalid(READINESS_INCLUDE, message);
        return Err(DiError::creation_from::<HealthRunner>(error));
    }
    Ok(HealthRunner::new(indicators, settings.timeout))
}

/// The `/info` sources: the application's build info, the platform and the contributors.
fn info(build: Option<Arc<BuildInfo>>, contributors: Vec<Arc<dyn InfoContributor>>) -> Info {
    Info {
        build: build.map(|b| (*b).clone()).unwrap_or_default(),
        platform: Platform::detect(),
        contributors,
    }
}

/// The controller, serving `/loggers` when logging is on and its system is a bean.
fn actuator_controller(
    settings: Arc<ActuatorSettings>,
    availability: Arc<Availability>,
    runner: Arc<HealthRunner>,
    info: Arc<Info>,
    #[cfg(feature = "logging")] logging: Option<Arc<oxbloom_logging::LoggingSystem>>,
) -> ActuatorController {
    let controller = ActuatorController::new((*settings).clone(), availability, runner, info);
    #[cfg(feature = "logging")]
    let controller = match logging {
        Some(logging) => {
            // Without `security` nothing authenticates the actuator routes, and the loggers
            // endpoint is the one that changes state.
            #[cfg(not(feature = "security"))]
            if settings.loggers_enabled {
                tracing::warn!(
                    target: "oxbloom::boot",
                    "the actuator loggers endpoint can change log levels and is unauthenticated \
                     without the security feature"
                );
            }
            controller.with_loggers(logging)
        }
        None => controller,
    };
    controller
}

/// The indicators of the data sources, one per source, built from the `DataSources` bean.
#[cfg(any(
    feature = "data-postgres",
    feature = "data-mysql",
    feature = "data-sqlite"
))]
struct DataIndicators(Vec<Arc<dyn HealthIndicator>>);

/// One `DataHealthIndicator` per source of the registry.
#[cfg(any(
    feature = "data-postgres",
    feature = "data-mysql",
    feature = "data-sqlite"
))]
fn data_indicators(sources: Arc<oxbloom_data::DataSources>) -> DataIndicators {
    let indicators = sources
        .names()
        .into_iter()
        .map(|name| {
            Arc::new(oxbloom_actuator::DataHealthIndicator::new(
                Arc::clone(&sources),
                name,
            )) as Arc<dyn HealthIndicator>
        })
        .collect();
    DataIndicators(indicators)
}

/// The `ssl` indicator over the bundles, on the `dyn Clock` bean or the system time.
#[cfg(feature = "ssl")]
fn ssl_indicator(
    bundles: Arc<oxbloom_ssl::SslBundles>,
    clock: Option<Arc<dyn oxbloom_core::Clock>>,
) -> oxbloom_actuator::SslHealthIndicator {
    let clock = clock.unwrap_or_else(|| Arc::new(oxbloom_core::SystemClock));
    oxbloom_actuator::SslHealthIndicator::new(bundles, clock)
}

/// Permits `GET <base-path>/health/**` to everyone; everything else needs authentication.
#[cfg(feature = "security")]
struct PublicHealth {
    pattern: String,
}

#[cfg(feature = "security")]
impl crate::SecurityCustomizer for PublicHealth {
    fn customize(&self, http: oxbloom_security::HttpSecurity) -> oxbloom_security::HttpSecurity {
        http.authorize(|a| {
            a.get(&self.pattern)
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}

/// Exactly the keys `ActuatorSettings::from_config` reads (pinned by a test): nine, because
/// `management.endpoint.<id>.enabled` is one key per endpoint id.
static MANAGEMENT: [PropertyMetadata; 9] = [
    PropertyMetadata::new(
        "management.endpoints.enabled",
        "bool",
        Some("true"),
        "`false` turns the whole actuator off: no endpoint, no availability listener, no \
         indicator.",
    ),
    PropertyMetadata::new(
        "management.endpoints.base-path",
        "String",
        Some("/actuator"),
        "The path the endpoints are served under; `/`, empty segments and characters other than \
         ASCII letters, digits, `-`, `_`, `.` and `~` are refused.",
    ),
    PropertyMetadata::new(
        "management.endpoints.health.public",
        "bool",
        Some("false"),
        "With security, `true` permits `GET <base-path>/health/**` without authentication; \
         every other endpoint stays protected.",
    ),
    PropertyMetadata::new(
        "management.endpoint.health.enabled",
        "bool",
        Some("true"),
        "`false` removes `/health`, `/health/liveness` and `/health/readiness` (404).",
    ),
    PropertyMetadata::new(
        "management.endpoint.info.enabled",
        "bool",
        Some("true"),
        "`false` removes `/info` (404).",
    ),
    PropertyMetadata::new(
        "management.endpoint.loggers.enabled",
        "bool",
        Some("true"),
        "`false` removes `/loggers` (404). The endpoint needs the `logging` feature and a \
         `LoggingSystem` bean.",
    ),
    PropertyMetadata::new(
        "management.health.show-details",
        "String",
        Some("never"),
        "`always` adds each indicator's status and details to `/health`; `never` shows the \
         overall status only.",
    ),
    PropertyMetadata::new(
        "management.health.timeout",
        "Duration",
        Some("5s"),
        "The time limit of each health indicator; one that exceeds it is reported `DOWN`.",
    ),
    PropertyMetadata::new(
        "management.health.readiness.include",
        "array of String",
        None,
        "The health indicators that also decide `/health/readiness`. Each must name a \
         registered indicator, or startup fails.",
    ),
];
