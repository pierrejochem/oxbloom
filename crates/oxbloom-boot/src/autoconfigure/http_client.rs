//! `HttpClientAutoConfiguration`: the `dyn HttpClient` bean of the `http.client.*` keys.

use std::sync::Arc;

use async_trait::async_trait;
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_http_client::{HttpClient, HttpClientError, HttpClientSettings, Request, Response};
#[cfg(feature = "ssl")]
use oxbloom_ssl::SslBundles;

use crate::autoconfig::AutoConfiguration;
use crate::metadata::PropertyMetadata;

/// Registers one `Arc<dyn HttpClient>` bean built from the `http.client.*` keys, unless the
/// application registers a `dyn HttpClient` bean of its own.
///
/// Available with the cargo feature `http-client`. It has no conditions: with the feature on
/// the bean exists, with the defaults of [`HttpClientSettings`] when the section is missing,
/// so other auto-configurations can depend on it. The bean is a `ReqwestClient`, wrapped in a
/// `RetryingClient` when `http.client.retry.max-attempts` is above 1. With the feature `ssl`,
/// `http.client.ssl-bundle` names a bundle of the `SslBundles` bean whose TLS client
/// configuration the client uses for its server certificates; an unknown name fails the build
/// with a `ConfigError::Invalid` on that key that lists the available bundle names. Without
/// the feature `ssl` the key is an error. A bad value or an unknown key fails the build with
/// `DiError::Creation` for the client whose source is the `ConfigError` naming the key, never
/// the value.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     AutoConfiguration, Banner, HttpClientAutoConfiguration, OxbloomApplication, RunListener,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_http_client::HttpClient;
///
/// #[derive(Clone, Default)]
/// struct Seen(Arc<Mutex<bool>>);
/// impl RunListener for Seen {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get::<dyn HttpClient>().is_ok();
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let seen = Seen::default();
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("[http.client]\ntimeout = \"3s\"")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(seen.clone())
///     .auto_configuration::<HttpClientAutoConfiguration>()
///     .run()
///     .await?;
/// assert!(*seen.0.lock().unwrap());
///
/// let keys: Vec<_> = HttpClientAutoConfiguration::properties().iter().map(|p| p.key).collect();
/// assert!(keys.contains(&"http.client.retry.max-attempts"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct HttpClientAutoConfiguration;

impl Configuration for HttpClientAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(http_client)
            .provides(|c: Arc<ConfiguredClient>| -> Arc<dyn HttpClient> { c })
            .on_missing_bean::<dyn HttpClient>()
    }
}

impl AutoConfiguration for HttpClientAutoConfiguration {
    /// None: the bean is registered whenever the feature is on.
    fn conditions() -> Conditions {
        Conditions::new()
    }

    fn properties() -> &'static [PropertyMetadata] {
        &HTTP_CLIENT
    }
}

/// The client built from the settings; it only hands every call to the built client, so the
/// container can provide it as `dyn HttpClient`. Its `Debug` shows nothing.
pub(crate) struct ConfiguredClient(Arc<dyn HttpClient>);

impl std::fmt::Debug for ConfiguredClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfiguredClient")
    }
}

#[async_trait]
impl HttpClient for ConfiguredClient {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        self.0.execute(request).await
    }
}

const SSL_BUNDLE: &str = "http.client.ssl-bundle";

/// The client of the `http.client` section; the `ConfigError` stays the typed source of a
/// `DiError::Creation` that names the client.
fn http_client(
    config: Arc<Config>,
    #[cfg(feature = "ssl")] bundles: Option<Arc<SslBundles>>,
) -> Result<ConfiguredClient, DiError> {
    let creation = DiError::creation_from::<ConfiguredClient>;
    let settings = HttpClientSettings::from_config(&config).map_err(creation)?;
    let build_failed = || {
        // A build failure has a fixed text and no key of its own: the user agent is the one
        // setting that can make it fail, so name that key.
        creation(ConfigError::invalid(
            "http.client.user-agent",
            "the HTTP client cannot be built with these settings",
        ))
    };
    let client = match settings.ssl_bundle.as_deref() {
        None => settings.build(None).map_err(|_| build_failed())?,
        #[cfg(feature = "ssl")]
        Some(name) => {
            // The error lists bundle names only, never a file path or file content.
            let (bundle, available) = bundle_named(name, bundles.as_deref()).map_err(creation)?;
            let tls = bundle.client_config().map_err(|_| {
                creation(ConfigError::invalid(
                    SSL_BUNDLE,
                    format!(
                        "names SSL bundle `{}`, whose TLS client cannot be built; available \
                         bundles: {available}",
                        bundle.name()
                    ),
                ))
            })?;
            settings.build(Some(tls)).map_err(|_| build_failed())?
        }
        #[cfg(not(feature = "ssl"))]
        Some(_) => {
            return Err(creation(ConfigError::invalid(
                SSL_BUNDLE,
                "needs the `ssl` feature of `oxbloom-boot`; available bundles: none",
            )));
        }
    };
    Ok(ConfiguredClient(client))
}

/// The bundle `name` and the list of available bundle names for an error text: an error on
/// `http.client.ssl-bundle` that lists the names when the bundle does not exist.
#[cfg(feature = "ssl")]
fn bundle_named(
    name: &str,
    bundles: Option<&SslBundles>,
) -> Result<(oxbloom_ssl::SslBundle, String), ConfigError> {
    use crate::web::available;

    let available = available(bundles);
    let unknown = || {
        ConfigError::invalid(
            SSL_BUNDLE,
            format!("names an SSL bundle that does not exist; available bundles: {available}"),
        )
    };
    let bundle = bundles
        .ok_or_else(unknown)?
        .get(name)
        .map_err(|_| unknown())?;
    Ok((bundle, available))
}

/// Exactly the keys `HttpClientSettings::from_config` reads (pinned by a test).
static HTTP_CLIENT: [PropertyMetadata; 12] = [
    PropertyMetadata::new(
        "http.client.timeout",
        "Duration",
        Some("10s"),
        "The total timeout of a request: connecting, sending and reading the whole response.",
    ),
    PropertyMetadata::new(
        "http.client.connect-timeout",
        "Duration",
        None,
        "The timeout of connecting alone. Unset, only the total timeout applies.",
    ),
    PropertyMetadata::new(
        "http.client.max-body-bytes",
        "usize",
        Some("1048576"),
        "The size limit of a response body in bytes; a larger body is an error. At least 1.",
    ),
    PropertyMetadata::new(
        "http.client.follow-redirects",
        "usize",
        Some("0"),
        "How many redirects are followed. 0 follows none and returns the 3xx response.",
    ),
    PropertyMetadata::new(
        "http.client.user-agent",
        "String",
        None,
        "The `User-Agent` header of every request. Unset, none is sent.",
    ),
    PropertyMetadata::new(
        "http.client.ssl-bundle",
        "String",
        None,
        "The name of the SSL bundle (`ssl.bundle.pem.<name>` or `ssl.bundle.pkcs12.<name>`) whose \
         trust store and client certificate the client uses; needs the `ssl` feature. Unset, the \
         platform roots are used.",
    ),
    PropertyMetadata::new(
        "http.client.retry.max-attempts",
        "u32",
        None,
        "How many times an idempotent request is sent at most, the first call included. Retrying \
         happens only when this is set and above 1; the other `retry` keys apply with it.",
    ),
    PropertyMetadata::new(
        "http.client.retry.backoff",
        "String",
        Some("exponential"),
        "How the delay between attempts grows: `fixed` or `exponential`.",
    ),
    PropertyMetadata::new(
        "http.client.retry.initial",
        "Duration",
        Some("100ms"),
        "The delay before the first retry; at most `http.client.retry.max-delay`.",
    ),
    PropertyMetadata::new(
        "http.client.retry.multiplier",
        "f64",
        Some("2.0"),
        "The factor of each next delay with `exponential` backoff; finite and at least 1.0.",
    ),
    PropertyMetadata::new(
        "http.client.retry.max-delay",
        "Duration",
        Some("30s"),
        "The cap on every delay between attempts.",
    ),
    PropertyMetadata::new(
        "http.client.retry.jitter",
        "String",
        Some("full"),
        "The randomness added to each delay: `none`, `full` or `equal`.",
    ),
];
