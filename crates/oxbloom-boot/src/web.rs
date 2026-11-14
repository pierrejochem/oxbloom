use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use oxbloom_config::{Config, ConfigError, DataSize};
#[cfg(feature = "ssl")]
use oxbloom_ssl::SslBundles;
use oxbloom_web::{
    App, CompressionConfig, CompressionLevel, Controller, CorsConfig, DiskResources,
    ForwardStrategy, RouteError, StaticResources, TrustedProxies,
};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
#[cfg(feature = "ssl")]
use tokio_rustls::TlsAcceptor;

use crate::error::ServiceError;
use crate::service::Service;

/// The `server.address` and `server.port` properties; the other `server.*` keys are read one by
/// one by [`Limits::from_config`], so that no deserializer message can echo a value.
#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ServerProperties {
    #[serde(default = "default_address")]
    address: IpAddr,
    #[serde(default = "default_port")]
    port: u16,
}

fn default_address() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

fn default_port() -> u16 {
    8080
}

/// The defaults of `server.address` and `server.port`, for the metadata pin test.
#[cfg(test)]
pub(crate) fn defaults() -> (IpAddr, u16) {
    (default_address(), default_port())
}

/// Reads `key`: `None` when absent, a fixed-text `ConfigError::Invalid` naming `key` (never the
/// value) when it has the wrong type or an unusable value.
fn read<T: DeserializeOwned>(
    config: &Config,
    key: &str,
    rule: &'static str,
) -> Result<Option<T>, ConfigError> {
    match config.get::<T>(key) {
        Ok(value) => Ok(Some(value)),
        Err(ConfigError::Missing { .. }) => Ok(None),
        Err(ConfigError::Invalid { .. }) => Err(ConfigError::invalid(key, rule)),
        Err(other) => Err(other),
    }
}

const SHUTDOWN_TIMEOUT: &str = "server.shutdown-timeout";
const MAX_BODY_BYTES: &str = "server.max-body-bytes";
const HEADER_READ_TIMEOUT: &str = "server.header-read-timeout";
const FORWARD_STRATEGY: &str = "server.forward-headers.strategy";
const TRUSTED_PROXIES: &str = "server.forward-headers.trusted-proxies";
const INCLUDE_MESSAGE: &str = "server.error.include-message";
const SSL_BUNDLE: &str = "server.ssl.bundle";

/// The limits, timeouts and forwarded-header settings of the `server.*` keys that are present;
/// `None` for an absent key, so the value the `App` already has stays.
struct Limits {
    shutdown_timeout: Option<Duration>,
    max_body_bytes: Option<usize>,
    header_read_timeout: Option<Duration>,
    strategy: Option<ForwardStrategy>,
    trusted: Option<TrustedProxies>,
}

impl Limits {
    fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let duration_rule = "must be a duration such as `30s` or `500ms`";
        let shutdown_timeout = read(config, SHUTDOWN_TIMEOUT, duration_rule)?;
        let header_read_timeout: Option<Duration> =
            read(config, HEADER_READ_TIMEOUT, duration_rule)?;
        if let Some(t) = header_read_timeout
            && (t.is_zero() || t > Duration::from_secs(86_400))
        {
            return Err(ConfigError::invalid(
                HEADER_READ_TIMEOUT,
                "must be between 1 millisecond and 1 day",
            ));
        }
        let size: Option<DataSize> = read(
            config,
            MAX_BODY_BYTES,
            "must be a size such as `1MiB` or `65536`",
        )?;
        let max_body_bytes = size
            .map(|s| usize::try_from(s.bytes()))
            .transpose()
            .map_err(|_| ConfigError::invalid(MAX_BODY_BYTES, "is too large for this platform"))?;
        let strategy: Option<String> = read(config, FORWARD_STRATEGY, STRATEGY_RULE)?;
        let strategy = match strategy.as_deref() {
            None => None,
            Some("none") => Some(ForwardStrategy::None),
            Some("framework") => Some(ForwardStrategy::Framework),
            Some(_) => return Err(ConfigError::invalid(FORWARD_STRATEGY, STRATEGY_RULE)),
        };
        let entries: Option<Vec<String>> = read(
            config,
            TRUSTED_PROXIES,
            "must be a list of IP addresses or CIDR networks",
        )?;
        let trusted = entries
            .map(|entries| TrustedProxies::parse(&entries))
            .transpose()
            .map_err(|_| {
                ConfigError::invalid(
                    TRUSTED_PROXIES,
                    "every entry must be an IP address or a CIDR network",
                )
            })?;
        Ok(Self {
            shutdown_timeout,
            max_body_bytes,
            header_read_timeout,
            strategy,
            trusted,
        })
    }

    /// Sets on `app` every setting whose key is present; the others keep the `App`'s value.
    fn apply(self, mut app: App) -> App {
        if let Some(n) = self.max_body_bytes {
            app = app.max_body_bytes(n);
        }
        if let Some(d) = self.header_read_timeout {
            app = app.header_read_timeout(d);
        }
        if let Some(d) = self.shutdown_timeout {
            app = app.shutdown_timeout(d);
        }
        if self.strategy.is_some() || self.trusted.is_some() {
            let mut forwarded = app.forwarded_config().clone();
            if let Some(strategy) = self.strategy {
                forwarded.strategy = strategy;
            }
            if let Some(trusted) = self.trusted {
                forwarded.trusted = trusted;
            }
            app = app.forwarded(forwarded);
        }
        app
    }
}

const CORS_ORIGINS: &str = "server.cors.allowed-origins";
const CORS_METHODS: &str = "server.cors.allowed-methods";
const CORS_HEADERS: &str = "server.cors.allowed-headers";
const CORS_EXPOSED: &str = "server.cors.exposed-headers";
const CORS_CREDENTIALS: &str = "server.cors.allow-credentials";
const CORS_MAX_AGE: &str = "server.cors.max-age";
const COMPRESSION_ENABLED: &str = "server.compression.enabled";
const COMPRESSION_MIN_SIZE: &str = "server.compression.min-size";
const COMPRESSION_MIME_TYPES: &str = "server.compression.mime-types";
const COMPRESSION_LEVEL: &str = "server.compression.level";
const COMPRESSION_MAX_BODY: &str = "server.compression.max-body-bytes";
const STATIC_ENABLED: &str = "server.static.enabled";
const STATIC_DIRECTORY: &str = "server.static.directory";
const STATIC_PATH: &str = "server.static.path";
const STATIC_INDEX: &str = "server.static.index";
const STATIC_CACHE_MAX_AGE: &str = "server.static.cache-max-age";
const STATIC_MAX_FILE: &str = "server.static.max-file-bytes";

const LIST_RULE: &str = "must be a list of strings";
const SIZE_RULE: &str = "must be a size such as `1MiB` or `65536`";
const DURATION_RULE: &str = "must be a duration such as `30s` or `500ms`";
const BOOL_RULE: &str = "must be `true` or `false`";
const STRING_RULE: &str = "must be a string";

/// Reads the size at `key` as a byte count that fits this platform's `usize`.
fn read_bytes(config: &Config, key: &str) -> Result<Option<usize>, ConfigError> {
    read::<DataSize>(config, key, SIZE_RULE)?
        .map(|s| usize::try_from(s.bytes()))
        .transpose()
        .map_err(|_| ConfigError::invalid(key, "is too large for this platform"))
}

/// The key of the `server.cors.*` setting that a `CorsError` names.
fn cors_key(setting: &str) -> &'static str {
    match setting {
        "allowed-methods" => CORS_METHODS,
        "allowed-headers" => CORS_HEADERS,
        "exposed-headers" => CORS_EXPOSED,
        "allow-credentials" => CORS_CREDENTIALS,
        _ => CORS_ORIGINS,
    }
}

/// The `server.cors.*` settings; `None` while `allowed-origins` is absent, which leaves a filter
/// set in code alone. The other keys are still read and checked.
fn cors_from(config: &Config) -> Result<Option<CorsConfig>, ConfigError> {
    let origins: Option<Vec<String>> = read(config, CORS_ORIGINS, LIST_RULE)?;
    let methods: Option<Vec<String>> = read(config, CORS_METHODS, LIST_RULE)?;
    let headers: Option<Vec<String>> = read(config, CORS_HEADERS, LIST_RULE)?;
    let exposed: Option<Vec<String>> = read(config, CORS_EXPOSED, LIST_RULE)?;
    let credentials: Option<bool> = read(config, CORS_CREDENTIALS, BOOL_RULE)?;
    let max_age: Option<Duration> = read(config, CORS_MAX_AGE, DURATION_RULE)?;
    let Some(origins) = origins else {
        return Ok(None);
    };
    let mut cors = CorsConfig::new().allowed_origins(origins);
    if let Some(v) = methods {
        cors = cors.allowed_methods(v);
    }
    if let Some(v) = headers {
        cors = cors.allowed_headers(v);
    }
    if let Some(v) = exposed {
        cors = cors.exposed_headers(v);
    }
    if let Some(v) = credentials {
        cors = cors.allow_credentials(v);
    }
    if let Some(v) = max_age {
        cors = cors.max_age(v);
    }
    Ok(Some(cors))
}

/// The `server.compression.*` settings; `None` unless `enabled = true`. `codecs` is the list of
/// compiled-in codecs: with none, `enabled = true` is an error, because nothing could be
/// compressed.
fn compression_from(
    config: &Config,
    codecs: &[&str],
) -> Result<Option<CompressionConfig>, ConfigError> {
    let enabled: Option<bool> = read(config, COMPRESSION_ENABLED, BOOL_RULE)?;
    let min_size = read_bytes(config, COMPRESSION_MIN_SIZE)?;
    let mime_types: Option<Vec<String>> = read(config, COMPRESSION_MIME_TYPES, LIST_RULE)?;
    let level: Option<String> = read(config, COMPRESSION_LEVEL, LEVEL_RULE)?;
    let level = match level.as_deref() {
        None => None,
        Some("fast") => Some(CompressionLevel::Fast),
        Some("default") => Some(CompressionLevel::Default),
        Some("best") => Some(CompressionLevel::Best),
        Some(_) => return Err(ConfigError::invalid(COMPRESSION_LEVEL, LEVEL_RULE)),
    };
    let max_body = read_bytes(config, COMPRESSION_MAX_BODY)?;
    if enabled != Some(true) {
        return Ok(None);
    }
    if codecs.is_empty() {
        return Err(ConfigError::invalid(
            COMPRESSION_ENABLED,
            "needs a compression codec; build `oxbloom-web` with `gzip`, `brotli` or `zstd`",
        ));
    }
    let mut compression = CompressionConfig::new();
    if let Some(n) = min_size {
        compression = compression.min_size(n);
    }
    if let Some(v) = mime_types {
        compression = compression.mime_types(v);
    }
    if let Some(l) = level {
        compression = compression.level(l);
    }
    if let Some(n) = max_body {
        compression = compression.max_body_bytes(n);
    }
    Ok(Some(compression))
}

const INDEX_RULE: &str = "must be a plain file name: 1 to 255 bytes, not starting with `.`, \
                          without `/`, `\\`, `%` or NUL";
const LEVEL_RULE: &str = "must be `fast`, `default` or `best`";

/// The `server.static.*` settings; `None` unless `enabled = true`.
fn static_from(config: &Config) -> Result<Option<StaticResources>, ConfigError> {
    let enabled: Option<bool> = read(config, STATIC_ENABLED, BOOL_RULE)?;
    let directory: Option<String> = read(config, STATIC_DIRECTORY, STRING_RULE)?;
    let path: Option<String> = read(config, STATIC_PATH, STRING_RULE)?;
    let index: Option<String> = read(config, STATIC_INDEX, STRING_RULE)?;
    let cache: Option<Duration> = read(config, STATIC_CACHE_MAX_AGE, DURATION_RULE)?;
    let max_file = read_bytes(config, STATIC_MAX_FILE)?;
    if let Some(index) = &index
        && (index.is_empty()
            || index.len() > 255
            || index.starts_with('.')
            || index.contains(['/', '\\', '%', '\0']))
    {
        return Err(ConfigError::invalid(STATIC_INDEX, INDEX_RULE));
    }
    if enabled != Some(true) {
        return Ok(None);
    }
    let directory = directory.unwrap_or_else(|| "static".to_owned());
    if !std::path::Path::new(&directory).is_dir() {
        return Err(ConfigError::invalid(
            STATIC_DIRECTORY,
            "must be an existing directory",
        ));
    }
    let mut disk = DiskResources::new(directory);
    if let Some(n) = max_file {
        disk = disk.max_file_bytes(n);
    }
    let mut resources = StaticResources::new(path.unwrap_or_else(|| "/".to_owned()), disk);
    if let Some(index) = index {
        resources = resources.index(index);
    }
    if let Some(d) = cache {
        resources = resources.cache_max_age(d);
    }
    Ok(Some(resources))
}

/// The CORS, compression and static-resource settings of the `server.*` keys, read once in
/// `WebServer::new` and applied to the `App`. The large parts are boxed so that the value stays
/// small.
struct Extras {
    cors: Option<Box<CorsConfig>>,
    compression: Option<Box<CompressionConfig>>,
    statics: Option<Box<StaticResources>>,
}

impl Extras {
    fn from_config(config: &Config) -> Result<Self, ConfigError> {
        Self::from_config_with(config, oxbloom_web::codecs())
    }

    fn from_config_with(config: &Config, codecs: &[&str]) -> Result<Self, ConfigError> {
        Ok(Self {
            cors: cors_from(config)?.map(Box::new),
            compression: compression_from(config, codecs)?.map(Box::new),
            statics: static_from(config)?.map(Box::new),
        })
    }

    /// Adds the filters and routes of the settings that are present; the rest of `app` stays.
    fn apply(self, mut app: App) -> Result<App, ConfigError> {
        if let Some(cors) = self.cors {
            app = app.cors(*cors).map_err(|e| {
                ConfigError::invalid(
                    cors_key(e.setting()),
                    format!("is not usable as a CORS setting: {}", e.reason()),
                )
            })?;
        }
        if let Some(compression) = self.compression {
            app = app.compression(*compression);
        }
        if let Some(statics) = self.statics {
            app = app
                .routes(Controller::routes(Arc::new(*statics)))
                .map_err(|e| {
                    let rule = match e {
                        RouteError::Duplicate(..) => "conflicts with a route of the application",
                        RouteError::Path(..) => {
                            "is not a usable route path or conflicts with a route of the application"
                        }
                    };
                    ConfigError::invalid(STATIC_PATH, rule)
                })?;
        }
        Ok(app)
    }
}

const STRATEGY_RULE: &str = "must be `none` or `framework`";

/// Reads `server.error.include-message` (`never` or `always`, default `never`).
///
/// Used by the renderer bean of the web auto-configuration; `WebServer::new` validates it too.
pub(crate) fn include_message(config: &Config) -> Result<bool, ConfigError> {
    let rule = "must be `never` or `always`";
    match read::<String>(config, INCLUDE_MESSAGE, rule)?.as_deref() {
        None | Some("never") => Ok(false),
        Some("always") => Ok(true),
        Some(_) => Err(ConfigError::invalid(INCLUDE_MESSAGE, rule)),
    }
}

/// Reads `server.ssl.bundle`: `None` when absent, the bundle name otherwise.
fn ssl_bundle_name(config: &Config) -> Result<Option<String>, ConfigError> {
    read(config, SSL_BUNDLE, "must be the name of an SSL bundle")
}

/// The names of `bundles` for an error text, or `none`.
#[cfg(feature = "ssl")]
fn available(bundles: Option<&SslBundles>) -> String {
    let names = bundles.map(SslBundles::names).unwrap_or_default();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// The TLS acceptor of `server.ssl.bundle`: `None` when the key is absent, an error naming the
/// key and listing the available bundle names when the bundle is missing or has no identity.
/// The texts carry bundle names only, never a file path or file content.
#[cfg(feature = "ssl")]
fn tls_acceptor(
    config: &Config,
    bundles: Option<&SslBundles>,
) -> Result<Option<TlsAcceptor>, ConfigError> {
    let Some(name) = ssl_bundle_name(config)? else {
        return Ok(None);
    };
    let unknown = || {
        ConfigError::invalid(
            SSL_BUNDLE,
            format!(
                "names an SSL bundle that does not exist; available bundles: {}",
                available(bundles)
            ),
        )
    };
    let bundle = bundles
        .ok_or_else(unknown)?
        .get(&name)
        .map_err(|_| unknown())?;
    let server = bundle.server_config().map_err(|_| {
        ConfigError::invalid(
            SSL_BUNDLE,
            format!(
                "names SSL bundle `{}`, which has no certificate and private key, but a server \
                 needs both; available bundles: {}",
                bundle.name(),
                available(bundles)
            ),
        )
    })?;
    Ok(Some(TlsAcceptor::from(server)))
}

/// Without the feature `ssl` a set `server.ssl.bundle` is an error, so the server never falls
/// back to plain HTTP when HTTPS was asked for.
#[cfg(not(feature = "ssl"))]
fn refuse_ssl_bundle(config: &Config) -> Result<(), ConfigError> {
    match ssl_bundle_name(config)? {
        None => Ok(()),
        Some(_) => Err(ConfigError::invalid(
            SSL_BUNDLE,
            "needs the `ssl` feature of `oxbloom-boot`",
        )),
    }
}

impl ServerProperties {
    fn from_config(config: &Config) -> Result<Self, ConfigError> {
        match config.get::<ServerProperties>("server") {
            Err(ConfigError::Missing { key }) if key == "server" => Ok(Self {
                address: default_address(),
                port: default_port(),
            }),
            other => other,
        }
    }
}

/// A [`Service`] that serves an [`oxbloom_web::App`] over HTTP, or over HTTPS when built with
/// `WebServer::with_ssl` (feature `ssl`) and `server.ssl.bundle`.
///
/// Available with the cargo feature `web`. It reads `server.address` (default
/// `127.0.0.1`) and `server.port` (default `8080`, `0` picks a free port) from
/// the configuration. [`phase`](Service::phase) is `i32::MAX`, so the server
/// starts after every other service and stops before them. `start` binds the
/// listener and serves in a spawned task; `stop` stops accepting connections
/// and waits until the open ones have drained. A server starts once: a second
/// `start` is a [`ServiceError`]. Clones share one server.
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_boot::{OxbloomApplication, Service, WebServer};
/// use oxbloom_config::Config;
/// use oxbloom_web::{App, HttpMethod, Route};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let config = Config::from_str("[server]\nport = 0")?;
/// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
/// let server = WebServer::new(app, &config)?;
///
/// let builder = OxbloomApplication::builder()
///     .with_config(config)
///     .output(std::io::sink())
///     .configure(move |b| {
///         b.instance(server)
///             .provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })
///     });
/// // Serves until shutdown. Triggered up front here, so `run()` starts the
/// // server and stops it again right away.
/// builder.shutdown_handle().trigger();
/// assert_eq!(builder.run().await?.code(), 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct WebServer {
    inner: Arc<Inner>,
}

struct Inner {
    address: IpAddr,
    port: u16,
    app: Mutex<Option<App>>,
    local_addr: Mutex<Option<SocketAddr>>,
    running: Mutex<Option<Running>>,
    /// The acceptor of `server.ssl.bundle`; with one the port speaks only HTTPS.
    #[cfg(feature = "ssl")]
    tls: Option<TlsAcceptor>,
}

/// The serving task and the sender that ends it.
struct Running {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<io::Result<()>>,
}

impl fmt::Debug for WebServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No address or port: those are configuration values.
        f.debug_struct("WebServer")
            .field("started", &self.inner.local_addr().is_some())
            .finish_non_exhaustive()
    }
}

/// Locks `m`, ignoring poisoning: the guarded values stay consistent.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    fn local_addr(&self) -> Option<SocketAddr> {
        *lock(&self.local_addr)
    }
}

/// Binding the web server's listener failed.
///
/// `Display` names neither the address nor the port: they are configuration
/// values. The address is available through [`address`](BindError::address)
/// for the port-in-use failure report, the one place that shows it. `Debug`
/// omits it as well.
///
/// A `BindError` is the cause of a [`StartupError`](crate::StartupError) in
/// the `Service` phase.
///
/// ```
/// use oxbloom_boot::{BindError, OxbloomApplication, Service, WebServer};
/// use oxbloom_config::Config;
/// use oxbloom_web::App;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// // Hold a port so that the server cannot bind it.
/// let taken = std::net::TcpListener::bind("127.0.0.1:0")?;
/// let port = taken.local_addr()?.port();
/// let config = Config::from_str(&format!("[server]\nport = {port}"))?;
/// let server = WebServer::new(App::new(), &config)?;
///
/// let error = OxbloomApplication::builder()
///     .with_config(config)
///     .output(std::io::sink())
///     .configure(move |b| {
///         b.instance(server)
///             .provides(|a: std::sync::Arc<WebServer>| -> std::sync::Arc<dyn Service> { a })
///     })
///     .run()
///     .await
///     .unwrap_err();
/// let bind = error.find_cause::<BindError>().expect("a bind error");
/// assert_eq!(bind.address().port(), port);
/// assert_eq!(bind.kind(), std::io::ErrorKind::AddrInUse);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct BindError {
    address: SocketAddr,
    source: io::Error,
}

impl BindError {
    pub(crate) fn new(address: SocketAddr, source: io::Error) -> Self {
        BindError { address, source }
    }

    /// The address the server tried to bind: the configured address and port.
    ///
    /// ```
    /// # use oxbloom_boot::BindError;
    /// # fn show(e: &BindError) {
    /// let _address: std::net::SocketAddr = e.address();
    /// # }
    /// ```
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// The kind of the operating system error.
    ///
    /// ```
    /// # use oxbloom_boot::BindError;
    /// # fn show(e: &BindError) {
    /// let in_use = e.kind() == std::io::ErrorKind::AddrInUse;
    /// # let _ = in_use;
    /// # }
    /// ```
    pub fn kind(&self) -> io::ErrorKind {
        self.source.kind()
    }
}

impl fmt::Debug for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No address or port: those are configuration values.
        f.debug_struct("BindError")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the web server cannot bind its listener: {}",
            self.source
        )
    }
}

impl std::error::Error for BindError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl WebServer {
    /// Creates a server for `app`, reading the `server` properties from `config`.
    ///
    /// A missing `server` table gives `127.0.0.1:8080`. An unusable value, such
    /// as an address that is not an IP address, is a `ConfigError` that names
    /// the key, not the value.
    ///
    /// The other keys are `server.shutdown-timeout` (30s), `server.max-body-bytes` (1 MiB),
    /// `server.header-read-timeout` (10s), `server.forward-headers.strategy` (`none` or
    /// `framework`), `server.forward-headers.trusted-proxies` (IP addresses or CIDR networks;
    /// loopback by default) and `server.error.include-message` (`never` or `always`).
    ///
    /// Only the keys that are present are applied to `app`, before it is stored. A setting made
    /// in code, such as `App::new().max_body_bytes(16)`, wins when its key is absent; the
    /// defaults above are the `App`'s own. `strategy` and `trusted-proxies` each replace only
    /// their own part of the [`App::forwarded`] setting, so `trusted-proxies` alone keeps the
    /// strategy of `app`. `server.error.include-message` is validated here but sets no
    /// renderer: it only configures the `ProblemRenderer` that the auto-configuration
    /// registers. Without the auto-configuration, choose the renderer with
    /// [`App::error_renderer`].
    ///
    /// The CORS keys are `server.cors.allowed-origins` (a list; `*` or `scheme://host[:port]`;
    /// CORS stays off while it is absent), `allowed-methods` (`GET`, `HEAD` and `POST`),
    /// `allowed-headers` (none), `exposed-headers` (none), `allow-credentials` (`false`) and
    /// `max-age` (`30m`), all below `server.cors.`. The compression keys are
    /// `server.compression.enabled` (`false`), `min-size` (`1KiB`), `mime-types`, `level`
    /// (`fast`, `default` or `best`) and `max-body-bytes` (`8MiB`); `enabled = true` needs a
    /// codec, which the default features of `oxbloom-web` provide. The static keys are
    /// `server.static.enabled` (`false`), `directory` (`static`, which must exist), `path`
    /// (`/`), `index` (`index.html`), `cache-max-age` (`0s`) and `max-file-bytes` (`10MiB`).
    /// A present `allowed-origins` adds a CORS filter to `app`, `enabled = true` adds the
    /// compression filter and the static routes; an absent key leaves what the code set. A bad
    /// value names its key and never the value; a CORS list problem names the matching
    /// `server.cors.*` key.
    ///
    /// A present `server.cors.allowed-origins` adds a CORS filter next to one set in code, so
    /// both run; use one or the other.
    ///
    /// The sub-keys (`server.compression.min-size`, `mime-types`, `level` and
    /// `max-body-bytes`; the `server.cors.*` keys other than `allowed-origins`; the
    /// `server.static.*` keys other than `enabled`) are always validated, but have no effect
    /// without their switch key (`server.compression.enabled = true`,
    /// `server.cors.allowed-origins`, `server.static.enabled = true`). They never tune a filter
    /// or route set in code. An explicitly empty `server.cors.allowed-origins` list fails, and
    /// the error of an unusable CORS list gives the reason without any configured value.
    ///
    /// `server.ssl.bundle` needs `WebServer::with_ssl` (feature `ssl`), which receives the SSL
    /// bundles. `new` has none, so a set `server.ssl.bundle` is a `ConfigError` on that key
    /// instead of a silent fallback to plain HTTP.
    ///
    /// `server.header-read-timeout` must be above zero and at most one day. An empty
    /// `trusted-proxies` list with strategy `framework` trusts nobody, so the strategy has no
    /// effect; the default list is loopback only, so a reverse proxy on another host is not
    /// trusted until its IP or CIDR is listed.
    ///
    /// ```
    /// use oxbloom_boot::WebServer;
    /// use oxbloom_config::{Config, ConfigError};
    /// use oxbloom_web::App;
    ///
    /// let server = WebServer::new(App::new(), &Config::from_str("[server]\nport = 0")?)?;
    /// assert_eq!(server.local_addr(), None); // not started yet
    ///
    /// let bad = Config::from_str("[server]\naddress = \"localhost\"")?;
    /// let err = WebServer::new(App::new(), &bad).unwrap_err();
    /// assert!(matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.address"));
    ///
    /// let bad = Config::from_str("[server.cors]\nallowed-origins = [\"*\"]\nallow-credentials = true")?;
    /// let err = WebServer::new(App::new(), &bad).unwrap_err();
    /// assert!(matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.cors.allow-credentials"));
    ///
    /// let bad = Config::from_str("[server]\nmax-body-bytes = \"hunter2\"")?;
    /// let err = WebServer::new(App::new(), &bad).unwrap_err();
    /// assert!(matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.max-body-bytes"));
    /// assert!(!err.to_string().contains("hunter2"));
    ///
    /// // No `server.max-body-bytes` key: the 16 bytes set in code stay.
    /// let server = WebServer::new(App::new().max_body_bytes(16), &Config::from_str("")?)?;
    /// # let _ = server;
    /// # Ok::<(), ConfigError>(())
    /// ```
    pub fn new(app: App, config: &Config) -> Result<WebServer, ConfigError> {
        #[cfg(feature = "ssl")]
        return Self::with_ssl(app, config, None);
        #[cfg(not(feature = "ssl"))]
        {
            refuse_ssl_bundle(config)?;
            Self::build(app, config)
        }
    }

    /// Creates a server for `app` like [`WebServer::new`], serving HTTPS with the identity of
    /// the bundle that `server.ssl.bundle` names in `bundles`.
    ///
    /// Available with the cargo feature `ssl`. Without `server.ssl.bundle` this is exactly
    /// [`WebServer::new`]: plain HTTP. With it, the port speaks only HTTPS: there is no HTTP
    /// listener and no redirect. The TLS configuration is the bundle's
    /// [`server_config`](oxbloom_ssl::SslBundle::server_config) (ALPN `http/1.1` only), which
    /// resolves the certificate at every handshake, so a reloaded bundle reaches the next
    /// connection without a restart.
    ///
    /// # Errors
    ///
    /// Every error of [`WebServer::new`], and a `ConfigError::Invalid` on `server.ssl.bundle`
    /// when `bundles` is `None`, has no bundle of that name, or the bundle has no certificate
    /// and private key. Its message lists the available bundle names and never a file path or
    /// file content.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::WebServer;
    /// use oxbloom_config::{Config, ConfigError};
    /// use oxbloom_ssl::SslBundles;
    /// use oxbloom_web::App;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let (cert, key) = (generated.cert.pem(), generated.signing_key.serialize_pem());
    /// // `cert` and `key` hold PEM text; a file path works the same way.
    /// let config = Config::from_str(&format!(
    ///     "[server]\nport = 0\n[server.ssl]\nbundle = \"web\"\n\
    ///      [ssl.bundle.pem.web]\ncertificate = '''\n{cert}'''\nprivate-key = '''\n{key}'''"
    /// ))?;
    /// let bundles = SslBundles::from_config(&config)?;
    /// let server = WebServer::with_ssl(App::new(), &config, Some(&bundles))?;
    /// # let _ = server;
    ///
    /// // An unknown name is an error on the key that lists the bundles.
    /// let typo = Config::from_str("[server.ssl]\nbundle = \"wbe\"")?;
    /// let err = WebServer::with_ssl(App::new(), &typo, Some(&bundles)).unwrap_err();
    /// assert!(matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.ssl.bundle"));
    /// assert!(err.to_string().contains("available bundles: web"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg(feature = "ssl")]
    pub fn with_ssl(
        app: App,
        config: &Config,
        bundles: Option<&SslBundles>,
    ) -> Result<WebServer, ConfigError> {
        let tls = tls_acceptor(config, bundles)?;
        Self::build(app, config, tls)
    }

    /// The server of the `server.*` keys; HTTPS with `tls`.
    fn build(
        app: App,
        config: &Config,
        #[cfg(feature = "ssl")] tls: Option<TlsAcceptor>,
    ) -> Result<WebServer, ConfigError> {
        let properties = ServerProperties::from_config(config)?;
        let limits = Limits::from_config(config)?;
        include_message(config)?;
        let extras = Extras::from_config(config)?;
        let app = extras.apply(limits.apply(app))?;
        Ok(WebServer {
            inner: Arc::new(Inner {
                address: properties.address,
                port: properties.port,
                app: Mutex::new(Some(app)),
                local_addr: Mutex::new(None),
                running: Mutex::new(None),
                #[cfg(feature = "ssl")]
                tls,
            }),
        })
    }

    /// The address the server listens on, `None` before a successful `start`.
    ///
    /// With `server.port = 0` this is how to learn the port that was picked.
    ///
    /// ```
    /// use oxbloom_boot::{Service, WebServer};
    /// use oxbloom_config::Config;
    /// use oxbloom_web::App;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let server = WebServer::new(App::new(), &Config::from_str("[server]\nport = 0")?)?;
    /// server.start().await?;
    /// let addr = server.local_addr().expect("started");
    /// assert!(addr.ip().is_loopback());
    /// assert_ne!(addr.port(), 0);
    /// server.stop().await;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.inner.local_addr()
    }

    #[cfg(test)]
    fn bind_addr(&self) -> SocketAddr {
        SocketAddr::new(self.inner.address, self.inner.port)
    }
}

#[async_trait]
impl Service for WebServer {
    async fn start(&self) -> Result<(), ServiceError> {
        let Some(app) = lock(&self.inner.app).take() else {
            return Err(ServiceError::new(
                "the web server was already started; it starts only once",
            ));
        };
        let wanted = SocketAddr::new(self.inner.address, self.inner.port);
        let listener = TcpListener::bind(wanted)
            .await
            .map_err(|e| ServiceError::new(BindError::new(wanted, e)))?;
        let addr = listener
            .local_addr()
            .map_err(|e| ServiceError::new(BindError::new(wanted, e)))?;
        let (shutdown, stopped) = oneshot::channel::<()>();
        let stopped = async {
            let _ = stopped.await;
        };
        #[cfg(feature = "ssl")]
        let task = match &self.inner.tls {
            Some(acceptor) => tokio::spawn(app.serve_tls(listener, acceptor.clone(), stopped)),
            None => tokio::spawn(app.serve(listener, stopped)),
        };
        #[cfg(not(feature = "ssl"))]
        let task = tokio::spawn(app.serve(listener, stopped));
        *lock(&self.inner.local_addr) = Some(addr);
        *lock(&self.inner.running) = Some(Running { shutdown, task });
        #[cfg(feature = "logging")]
        crate::events::listening(addr);
        Ok(())
    }

    async fn stop(&self) {
        let Some(running) = lock(&self.inner.running).take() else {
            return;
        };
        // The receiver is gone only if the serving task already ended.
        let _ = running.shutdown.send(());
        // `serve` returns once every open connection has drained.
        let _ = running.task.await;
    }

    fn phase(&self) -> i32 {
        i32::MAX
    }

    fn name(&self) -> &'static str {
        "WebServer"
    }
}

#[cfg(test)]
mod tests {
    use oxbloom_web::ForwardedConfig;

    use super::*;

    #[test]
    fn limit_defaults_are_the_documented_ones() {
        // Absent keys set nothing, so the `App` defaults apply: 30 s, 1 MiB, 10 s, strategy
        // `none` with loopback trusted (pinned by `App`'s own `new_has_the_documented_defaults`
        // and, over HTTP, by `tests/server_properties.rs`).
        let limits = Limits::from_config(&Config::from_str("").unwrap()).unwrap();
        assert_eq!(limits.shutdown_timeout, None);
        assert_eq!(limits.max_body_bytes, None);
        assert_eq!(limits.header_read_timeout, None);
        assert_eq!(limits.strategy, None);
        assert_eq!(limits.trusted, None);
        let app = limits.apply(App::new());
        assert_eq!(app.forwarded_config(), &ForwardedConfig::default());
        assert_eq!(app.forwarded_config().strategy, ForwardStrategy::None);
        assert_eq!(
            app.forwarded_config().trusted,
            TrustedProxies::loopback(),
            "loopback by default"
        );
        assert!(!include_message(&Config::from_str("").unwrap()).unwrap());
    }

    #[test]
    fn present_keys_are_read_and_absent_ones_stay_none() {
        let config = Config::from_str(
            "[server]\nshutdown-timeout = \"5s\"\nmax-body-bytes = \"2KiB\"\n\
             header-read-timeout = \"3s\"\n[server.forward-headers]\nstrategy = \"framework\"",
        )
        .unwrap();
        let limits = Limits::from_config(&config).unwrap();
        assert_eq!(limits.shutdown_timeout, Some(Duration::from_secs(5)));
        assert_eq!(limits.max_body_bytes, Some(2048));
        assert_eq!(limits.header_read_timeout, Some(Duration::from_secs(3)));
        assert_eq!(limits.strategy, Some(ForwardStrategy::Framework));
        assert_eq!(limits.trusted, None);
    }

    #[test]
    fn forwarded_keys_replace_only_their_own_part() {
        let code = ForwardedConfig {
            strategy: ForwardStrategy::Framework,
            trusted: TrustedProxies::parse(&["10.0.0.0/8".into()]).unwrap(),
        };
        let applied = |toml: &str| {
            let limits = Limits::from_config(&Config::from_str(toml).unwrap()).unwrap();
            limits
                .apply(App::new().forwarded(code.clone()))
                .forwarded_config()
                .clone()
        };
        assert_eq!(applied(""), code);
        let only_list = applied("[server.forward-headers]\ntrusted-proxies = [\"192.0.2.1\"]");
        assert_eq!(only_list.strategy, ForwardStrategy::Framework);
        assert_eq!(
            only_list.trusted,
            TrustedProxies::parse(&["192.0.2.1".into()]).unwrap()
        );
        let only_strategy = applied("[server.forward-headers]\nstrategy = \"none\"");
        assert_eq!(only_strategy.strategy, ForwardStrategy::None);
        assert_eq!(only_strategy.trusted, code.trusted);
    }

    #[test]
    fn header_read_timeout_bounds() {
        let limits = |v: &str| {
            Limits::from_config(
                &Config::from_str(&format!("[server]\nheader-read-timeout = \"{v}\"")).unwrap(),
            )
        };
        assert!(limits("1s").is_ok());
        assert!(limits("1d").is_ok());
        for bad in ["0s", "0", "2d", "100000000000000d"] {
            let err = limits(bad).err().expect(bad);
            assert!(
                matches!(&err, ConfigError::Invalid { key, .. } if key == HEADER_READ_TIMEOUT),
                "{err:?}"
            );
            assert!(!err.to_string().contains(bad) || bad == "0", "{err}");
        }
    }

    #[test]
    fn zero_shutdown_timeout_and_body_limit_are_valid() {
        let config =
            Config::from_str("[server]\nshutdown-timeout = \"0s\"\nmax-body-bytes = 0").unwrap();
        let limits = Limits::from_config(&config).unwrap();
        assert_eq!(limits.shutdown_timeout, Some(Duration::ZERO));
        assert_eq!(limits.max_body_bytes, Some(0));
    }

    #[test]
    fn defaults_when_no_server_table() {
        let server = WebServer::new(App::new(), &Config::from_str("").unwrap()).unwrap();
        assert_eq!(server.bind_addr(), "127.0.0.1:8080".parse().unwrap());
    }

    #[test]
    fn defaults_for_absent_keys_in_a_server_table() {
        let config = Config::from_str("[server]\nport = 9000").unwrap();
        let server = WebServer::new(App::new(), &config).unwrap();
        assert_eq!(server.bind_addr(), "127.0.0.1:9000".parse().unwrap());

        let config = Config::from_str("[server]\naddress = \"::1\"").unwrap();
        let server = WebServer::new(App::new(), &config).unwrap();
        assert_eq!(server.bind_addr(), "[::1]:8080".parse().unwrap());
    }

    #[test]
    fn debug_shows_no_address() {
        let config = Config::from_str("[server]\nport = 4321").unwrap();
        let server = WebServer::new(App::new(), &config).unwrap();
        let debug = format!("{server:?}");
        assert!(!debug.contains("4321"), "{debug}");
        assert!(!debug.contains("127.0.0.1"), "{debug}");
    }

    #[test]
    fn compression_enabled_without_any_codec_names_the_enabled_key() {
        let on = Config::from_str("[server.compression]\nenabled = true").unwrap();
        let err = compression_from(&on, &[]).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.compression.enabled"),
            "{err}"
        );
        assert!(compression_from(&on, &["gzip"]).unwrap().is_some());
        let off = Config::from_str("[server.compression]\nenabled = false").unwrap();
        assert!(compression_from(&off, &[]).unwrap().is_none());
        assert!(
            compression_from(&Config::from_str("").unwrap(), &[])
                .unwrap()
                .is_none()
        );
    }
}
