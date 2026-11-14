//! `RequestLogAutoConfiguration`: a `RequestLogFilter` from the `logging.request` section.

use std::str::FromStr;
use std::sync::Arc;

use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_web::{Filter, RequestLogFilter};
use tracing::Level;

use crate::autoconfig::{AutoConfiguration, ConfigRef};
use crate::metadata::PropertyMetadata;

/// Registers a [`RequestLogFilter`] as a `dyn Filter` when `logging.request.enabled` is
/// `true` and the application registers no `RequestLogFilter` of its own.
///
/// The group is skipped with the key missing or any other value; the conditions report names
/// the reason. The filter declares the slot `order::REQUEST_LOG` (outermost), so it runs before
/// the error rendering, the body limit and the security chain and records their 401, 403 and 413
/// answers too. `logging.request.level` (default `info`) takes
/// error, warn, info, debug or trace; `off` and any other value fail the build with
/// `DiError::Creation` whose source is the `ConfigError` naming the key, never the value.
/// `logging.request.headers` is the allowlist of header names to print: entries are trimmed
/// and empty ones dropped. The allowlist is the operator's responsibility; the names
/// authorization, proxy-authorization, cookie, set-cookie, x-api-key and x-auth-token (in any
/// `-` or `_` spelling) always print as `<redacted>`.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     AutoConfiguration, Banner, OxbloomApplication, RequestLogAutoConfiguration, RunListener,
///     StartupPhase,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_web::Filter;
///
/// #[derive(Clone, Default)]
/// struct Filters(Arc<Mutex<usize>>);
/// impl RunListener for Filters {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get_all::<dyn Filter>().unwrap().len();
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let run = |toml: &str, seen: Filters| {
///     OxbloomApplication::builder()
///         .with_config(Config::from_str(toml).unwrap())
///         .banner(Banner::off())
///         .output(std::io::sink())
///         .listener(seen)
///         .auto_configuration::<RequestLogAutoConfiguration>()
///         .run()
/// };
///
/// let seen = Filters::default();
/// run("[logging.request]\nenabled = true\nheaders = [\"user-agent\"]", seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), 1);
///
/// // Without the switch there is no filter.
/// let seen = Filters::default();
/// run("", seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), 0);
///
/// // `off` is not a request level; the error names the key.
/// let err = run("[logging.request]\nenabled = true\nlevel = \"off\"", Filters::default())
///     .await
///     .unwrap_err();
/// assert_eq!(err.phase(), StartupPhase::Container);
/// assert!(err.find_cause::<oxbloom_config::ConfigError>().unwrap().to_string().contains("logging.request.level"));
///
/// let keys: Vec<_> = RequestLogAutoConfiguration::properties().iter().map(|p| p.key).collect();
/// assert_eq!(keys, ["logging.request.enabled", "logging.request.headers", "logging.request.level"]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct RequestLogAutoConfiguration;

impl Configuration for RequestLogAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(request_log_filter)
            .order(i32::MIN)
            .provides(|f: Arc<RequestLogFilter>| -> Arc<dyn Filter> { f })
            .on_missing_bean::<RequestLogFilter>()
    }
}

impl AutoConfiguration for RequestLogAutoConfiguration {
    fn conditions() -> Conditions {
        Conditions::new().on_property_value("logging.request.enabled", "true")
    }

    /// The server collects the filters when it is built, so this runs first.
    fn before() -> Vec<ConfigRef> {
        vec![ConfigRef::of::<super::WebServerAutoConfiguration>()]
    }

    fn properties() -> &'static [PropertyMetadata] {
        &REQUEST
    }
}

/// The filter of the `logging.request` section; the `ConfigError` stays the typed source.
fn request_log_filter(config: Arc<Config>) -> Result<RequestLogFilter, DiError> {
    let level = match optional::<String>(&config, "logging.request.level")? {
        None => Level::INFO,
        // `Level::from_str` also accepts the numbers 1 to 5 and `off` is no level: one fixed
        // message, without the value.
        Some(text) => Level::from_str(text.trim()).map_err(|_| {
            DiError::creation_from::<RequestLogFilter>(ConfigError::invalid(
                "logging.request.level",
                "expected error, warn, info, debug or trace",
            ))
        })?,
    };
    let headers = optional::<Vec<String>>(&config, "logging.request.headers")?
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .collect();
    Ok(RequestLogFilter::new(level, headers))
}

/// The value at `key`, or `None` when the key is not set.
fn optional<T: serde::de::DeserializeOwned>(
    config: &Config,
    key: &str,
) -> Result<Option<T>, DiError> {
    match config.get::<T>(key) {
        Ok(value) => Ok(Some(value)),
        Err(ConfigError::Missing { .. }) => Ok(None),
        Err(e) => Err(DiError::creation_from::<RequestLogFilter>(e)),
    }
}

/// Exactly the keys `request_log_filter` reads, plus the switch (pinned by a test).
static REQUEST: [PropertyMetadata; 3] = [
    PropertyMetadata::new(
        "logging.request.enabled",
        "bool",
        Some("false"),
        "Log one event per HTTP request at target `oxbloom::web::request`, with method, path \
         (no query), status and latency. The security chain runs first, so requests it \
         rejects are not logged.",
    ),
    PropertyMetadata::new(
        "logging.request.headers",
        "array of String",
        None,
        "The request headers to print (an allowlist; case-insensitive). Choosing them is the \
         operator's responsibility. The names authorization, proxy-authorization, cookie, \
         set-cookie, x-api-key and x-auth-token, in any `-` or `_` spelling, always print as \
         `<redacted>`.",
    ),
    PropertyMetadata::new(
        "logging.request.level",
        "Level",
        Some("info"),
        "The level of the request event: error, warn, info, debug or trace.",
    ),
];
