//! `WebServerAutoConfiguration`: a `WebServer` serving the routes of every `dyn Controller`.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
#[cfg(feature = "security")]
use oxbloom_security::SecurityFilterChain;
use oxbloom_web::{App, Controller, ErrorRenderer, Filter, ProblemRenderer};

use crate::autoconfig::{AutoConfiguration, ConfigRef};
use crate::metadata::PropertyMetadata;
use crate::service::Service;
use crate::web::WebServer;

/// Registers a [`WebServer`] as a `dyn Service` when the application has at least one
/// `dyn Controller` bean, unless it registered its own `WebServer`.
///
/// The group runs only with a `dyn Controller` present: a server without routes would only
/// answer 404. The `App` gets the `SecurityFilterChain` (with the feature `security`, when
/// there is one), every `dyn Filter` bean and the routes of every controller. Each filter
/// declares its own slot through `Filter::order()`, so the order is decided by the filters, not
/// by registration: the request log `-400`, the security chain `-100`, plain filters `0`. The
/// group also registers a `ProblemRenderer` (RFC 9457 `application/problem+json`) as the
/// `dyn ErrorRenderer` bean unless the application provides one, and the server uses whichever
/// bean exists. The `server.*` keys (limits, timeouts, forwarded headers, error detail) are
/// read by [`WebServer::new`]; an unusable value fails the build with the `ConfigError` naming
/// the key (never the value). With `security` it runs after
/// [`SecurityAutoConfiguration`](crate::SecurityAutoConfiguration). A duplicate route fails the
/// build as `DiError::Creation` for `WebServer` with the `RouteError` as source; a bad
/// `server.address` or `server.port` has the `ConfigError` naming the key as source.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_boot::{
///     AutoConfiguration, Banner, OxbloomApplication, StartupPhase, WebServerAutoConfiguration,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_web::{Controller, HttpMethod, Route, RouteError};
///
/// struct Health;
/// impl Controller for Health {
///     fn routes(self: Arc<Self>) -> Vec<Route> {
///         vec![
///             Route::new(HttpMethod::Get, "/health", |_| async { "ok" }),
///             Route::new(HttpMethod::Get, "/health", |_| async { "again" }),
///         ]
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// // No controller: no server, so `run()` returns after the runners.
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .auto_configuration::<WebServerAutoConfiguration>()
///     .run()
///     .await?;
///
/// // A controller with a duplicate route: the server cannot be built.
/// let err = OxbloomApplication::builder()
///     .with_config(Config::from_str("[server]\nport = 0")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .auto_configuration::<WebServerAutoConfiguration>()
///     .configure(|b| {
///         b.instance(Health)
///             .provides(|c: Arc<Health>| -> Arc<dyn Controller> { c })
///     })
///     .run()
///     .await
///     .unwrap_err();
/// assert_eq!(err.phase(), StartupPhase::Container);
/// assert!(err.find_cause::<RouteError>().is_some());
///
/// let keys: Vec<_> = WebServerAutoConfiguration::properties().iter().map(|p| p.key).collect();
/// assert_eq!(keys.len(), 25);
/// assert!(keys.contains(&"server.max-body-bytes"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct WebServerAutoConfiguration;

impl Configuration for WebServerAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(problem_renderer)
            .provides(|r: Arc<ProblemRenderer>| -> Arc<dyn ErrorRenderer> { r })
            .on_missing_bean::<dyn ErrorRenderer>()
            .try_bean(web_server)
            .provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })
            .on_missing_bean::<WebServer>()
    }
}

impl AutoConfiguration for WebServerAutoConfiguration {
    fn conditions() -> Conditions {
        Conditions::new().on_bean::<dyn Controller>()
    }

    fn after() -> Vec<ConfigRef> {
        #[cfg(feature = "security")]
        let after = vec![ConfigRef::of::<super::SecurityAutoConfiguration>()];
        #[cfg(not(feature = "security"))]
        let after = Vec::new();
        after
    }

    fn properties() -> &'static [PropertyMetadata] {
        &SERVER
    }
}

/// The default error renderer, with `server.error.include-message`; the `ConfigError` stays
/// the typed source.
fn problem_renderer(config: Arc<Config>) -> Result<ProblemRenderer, DiError> {
    crate::web::include_message(&config)
        .map(ProblemRenderer::new)
        .map_err(DiError::creation_from::<ProblemRenderer>)
}

/// The server; the security chain is one more filter, placed by its own `order()`.
#[cfg(feature = "security")]
fn web_server(
    config: Arc<Config>,
    controllers: Vec<Arc<dyn Controller>>,
    filters: Vec<Arc<dyn Filter>>,
    chain: Option<Arc<SecurityFilterChain>>,
    renderer: Option<Arc<dyn ErrorRenderer>>,
) -> Result<WebServer, DiError> {
    let chain = chain.map(|c| -> Arc<dyn Filter> { c });
    build(&config, chain, filters, controllers, renderer)
}

/// The server; without the feature `security` there is no chain.
#[cfg(not(feature = "security"))]
fn web_server(
    config: Arc<Config>,
    controllers: Vec<Arc<dyn Controller>>,
    filters: Vec<Arc<dyn Filter>>,
    renderer: Option<Arc<dyn ErrorRenderer>>,
) -> Result<WebServer, DiError> {
    build(&config, None, filters, controllers, renderer)
}

/// The `App` of `chain` and `filters` (each placed by its own `Filter::order()`), the optional
/// renderer and every controller's routes, served by a `WebServer` configured from `config`.
fn build(
    config: &Config,
    chain: Option<Arc<dyn Filter>>,
    filters: Vec<Arc<dyn Filter>>,
    controllers: Vec<Arc<dyn Controller>>,
    renderer: Option<Arc<dyn ErrorRenderer>>,
) -> Result<WebServer, DiError> {
    let mut app = App::new();
    if let Some(renderer) = renderer {
        app = app.error_renderer(renderer);
    }
    for filter in chain.into_iter().chain(filters) {
        app = app.filter(filter);
    }
    for controller in controllers {
        app = app
            .routes(controller.routes())
            .map_err(DiError::creation_from::<WebServer>)?;
    }
    // The `ConfigError` naming the key stays the typed source.
    WebServer::new(app, config).map_err(DiError::creation_from::<WebServer>)
}

/// Exactly the keys `WebServer::new` reads, with its defaults (pinned by tests).
static SERVER: [PropertyMetadata; 25] = [
    PropertyMetadata::new(
        "server.address",
        "IpAddr",
        Some("127.0.0.1"),
        "The IP address the web server binds, such as `0.0.0.0` for every IPv4 interface. A \
         host name is not accepted.",
    ),
    PropertyMetadata::new(
        "server.port",
        "u16",
        Some("8080"),
        "The TCP port the web server binds; `0` picks a free port.",
    ),
    PropertyMetadata::new(
        "server.shutdown-timeout",
        "Duration",
        Some("30s"),
        "How long the server waits for open connections to drain after a stop; then it stops \
         anyway and logs a warning. `0` drops open connections at once.",
    ),
    PropertyMetadata::new(
        "server.max-body-bytes",
        "DataSize",
        Some("1048576"),
        "The largest request body the server accepts, such as `1MiB`; a larger one gets 413. `0` rejects every non-empty body.",
    ),
    PropertyMetadata::new(
        "server.header-read-timeout",
        "Duration",
        Some("10s"),
        "How long a client may take to send the request headers; above zero, at most `1d`.",
    ),
    PropertyMetadata::new(
        "server.forward-headers.strategy",
        "String",
        Some("none"),
        "`none` ignores `Forwarded` and `X-Forwarded-*`; `framework` trusts them from \
         `server.forward-headers.trusted-proxies`. Use `framework` only behind a proxy that \
         sets or strips `Forwarded` and overwrites or strips `X-Forwarded-Proto` and \
         `X-Forwarded-Host` (and appends to `X-Forwarded-For`); otherwise clients control the \
         client address, scheme and host. An empty `trusted-proxies` list trusts nobody, so \
         `framework` then has no effect.",
    ),
    PropertyMetadata::new(
        "server.forward-headers.trusted-proxies",
        "List<String>",
        Some("loopback"),
        "IP addresses or CIDR networks of the proxies whose forwarded headers are believed, \
         used when the strategy is `framework`; the default is loopback only. A trusted proxy \
         must set or strip `Forwarded` and overwrite or strip `X-Forwarded-Proto` and \
         `X-Forwarded-Host` (and append to `X-Forwarded-For`), otherwise clients control the \
         client address, scheme and host. The default is loopback only, so a reverse proxy on \
         another host is not trusted until its IP or CIDR is listed; an empty list trusts \
         nobody.",
    ),
    PropertyMetadata::new(
        "server.error.include-message",
        "String",
        Some("never"),
        "`never` keeps internal error messages out of error responses; `always` puts them in \
         the problem `detail`. Applies to the default error renderer.",
    ),
    PropertyMetadata::new(
        "server.cors.allowed-origins",
        "List<String>",
        None,
        "The origins that may call the server from a browser: `*` or entries such as \
         `https://app.example`. CORS is off until this key is set; the other `server.cors.*` \
         keys then refine it. `*` cannot be combined with `allow-credentials`.",
    ),
    PropertyMetadata::new(
        "server.cors.allowed-methods",
        "List<String>",
        Some("GET,HEAD,POST"),
        "The methods a preflight allows; `*` allows every method.",
    ),
    PropertyMetadata::new(
        "server.cors.allowed-headers",
        "List<String>",
        Some(""),
        "The request headers a preflight allows; empty allows none, `*` allows every header.",
    ),
    PropertyMetadata::new(
        "server.cors.exposed-headers",
        "List<String>",
        Some(""),
        "The response headers that scripts may read.",
    ),
    PropertyMetadata::new(
        "server.cors.allow-credentials",
        "bool",
        Some("false"),
        "Whether browsers may send credentials such as cookies. Not allowed with `*` origins.",
    ),
    PropertyMetadata::new(
        "server.cors.max-age",
        "Duration",
        Some("30m"),
        "How long a browser may cache a preflight answer.",
    ),
    PropertyMetadata::new(
        "server.compression.enabled",
        "bool",
        Some("false"),
        "Compresses responses with gzip, brotli or zstd, as the client accepts. Fails startup \
         when `oxbloom-web` was built without any codec feature.",
    ),
    PropertyMetadata::new(
        "server.compression.min-size",
        "DataSize",
        Some("1KiB"),
        "Bodies smaller than this are not compressed.",
    ),
    PropertyMetadata::new(
        "server.compression.mime-types",
        "List<String>",
        Some(
            "text/*,application/json,application/problem+json,application/javascript,\
             image/svg+xml,application/xml",
        ),
        "The content types that are compressed; an entry ending in `/*` matches a whole family.",
    ),
    PropertyMetadata::new(
        "server.compression.level",
        "String",
        Some("default"),
        "`fast`, `default` or `best`: speed against ratio.",
    ),
    PropertyMetadata::new(
        "server.compression.max-body-bytes",
        "DataSize",
        Some("8MiB"),
        "Bodies larger than this are not compressed.",
    ),
    PropertyMetadata::new(
        "server.static.enabled",
        "bool",
        Some("false"),
        "Serves the files below `server.static.directory` at `server.static.path`.",
    ),
    PropertyMetadata::new(
        "server.static.directory",
        "String",
        Some("static"),
        "The root directory of the static files; it must exist when static resources are enabled.",
    ),
    PropertyMetadata::new(
        "server.static.path",
        "String",
        Some("/"),
        "The URL prefix the static files are served under.",
    ),
    PropertyMetadata::new(
        "server.static.index",
        "String",
        Some("index.html"),
        "The file served for a directory.",
    ),
    PropertyMetadata::new(
        "server.static.cache-max-age",
        "Duration",
        Some("0s"),
        "The `max-age` of `Cache-Control` on static files; `0s` sends `no-cache`.",
    ),
    PropertyMetadata::new(
        "server.static.max-file-bytes",
        "DataSize",
        Some("10MiB"),
        "Files larger than this answer 404.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_defaults_are_the_server_defaults() {
        let default = |key: &str| {
            SERVER
                .iter()
                .find(|p| p.key == key)
                .and_then(|p| p.default)
                .unwrap()
        };
        let (address, port) = crate::web::defaults();
        assert_eq!(default("server.address"), address.to_string());
        assert_eq!(default("server.port"), port.to_string());
        assert_eq!(default("server.shutdown-timeout"), "30s");
        assert_eq!(default("server.max-body-bytes"), "1048576");
        assert_eq!(default("server.header-read-timeout"), "10s");
        assert_eq!(default("server.forward-headers.strategy"), "none");
        assert_eq!(
            default("server.forward-headers.trusted-proxies"),
            "loopback"
        );
        assert_eq!(default("server.error.include-message"), "never");
        assert_eq!(default("server.cors.allowed-methods"), "GET,HEAD,POST");
        assert_eq!(default("server.cors.max-age"), "30m");
        assert_eq!(default("server.compression.min-size"), "1KiB");
        assert_eq!(default("server.compression.max-body-bytes"), "8MiB");
        assert_eq!(default("server.static.directory"), "static");
        assert_eq!(default("server.static.cache-max-age"), "0s");
        assert_eq!(default("server.static.max-file-bytes"), "10MiB");
        let described = |key: &str| SERVER.iter().find(|p| p.key == key).unwrap().description;
        for key in [
            "server.forward-headers.strategy",
            "server.forward-headers.trusted-proxies",
        ] {
            assert!(described(key).contains("X-Forwarded-Proto"), "{key}");
            assert!(described(key).contains("X-Forwarded-For"), "{key}");
        }
    }
}
