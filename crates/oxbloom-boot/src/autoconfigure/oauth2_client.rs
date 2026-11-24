//! `OAuth2ClientAutoConfiguration`: the `OAuth2Client` bean of the
//! `security.oauth2.client.*` keys over the `dyn HttpClient` bean.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_http_client::HttpClient;
use oxbloom_security::OAuth2Client;

use super::oauth2::{OAuth2LoginError, OAuth2Properties, REGISTRATIONS, oauth2_properties};
use crate::autoconfig::{AutoConfiguration, ConfigRef};

/// Registers one `OAuth2Client` bean (fetched as `Arc<OAuth2Client>`) for the registrations
/// and providers of `security.oauth2.client`, unless the application registers an
/// `OAuth2Client` of its own.
///
/// Available with the cargo feature `oauth2`. It applies when the configuration has the table
/// `security.oauth2.client.registration`. The client:
///
/// - reads the same parsed settings as the OAuth2 login of
///   [`SecurityAutoConfiguration`](crate::SecurityAutoConfiguration): the section is parsed
///   once per application, and parsed here only when `SecurityAutoConfiguration` is excluded;
/// - sends its requests through the `dyn HttpClient` bean, from
///   [`HttpClientAutoConfiguration`](crate::HttpClientAutoConfiguration) or the application, so
///   the `http.client.*` keys apply (each provider request still has its own 10 second
///   timeout). Without that bean the startup fails with the same error as the login, which
///   says that an `HttpClient` bean is needed;
/// - takes its time from the `dyn Clock` bean when there is one, and keeps the default skew
///   of 30 seconds.
///
/// It reads no keys of its own, so it adds no property metadata: the `security.oauth2.*` keys
/// are listed by `SecurityAutoConfiguration`. A bad registration fails the startup with a
/// `ConfigError` that names the full key and never holds the value or the client secret. It
/// runs after `HttpClientAutoConfiguration` and `SecurityAutoConfiguration`.
///
/// The client itself needs neither form login nor `security.oauth2.base-url`. But with
/// `SecurityAutoConfiguration` in the list (as in `auto_configurations()`), every registration
/// also turns on OAuth2 login, which needs both, unless `security.oauth2.login.enabled` is
/// `false`: an application that only uses the client sets that key, registers its own
/// `SecurityFilterChain` bean or excludes `SecurityAutoConfiguration`. The client does not
/// depend on the key.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     Banner, HttpClientAutoConfiguration, OAuth2ClientAutoConfiguration, OxbloomApplication,
///     RunListener,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_security::OAuth2Client;
///
/// #[derive(Clone, Default)]
/// struct Seen(Arc<Mutex<Option<Arc<OAuth2Client>>>>);
/// impl RunListener for Seen {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get_opt::<OAuth2Client>().unwrap();
///     }
/// }
///
/// let toml = r#"
/// [security.oauth2.client.registration.billing]
/// client-id = "orders-service"
/// client-secret = "orders-secret"
/// provider = "idp"
///
/// [security.oauth2.client.provider.idp]
/// issuer-uri = "https://idp.example.test"
/// "#;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let seen = Seen::default();
/// OxbloomApplication::builder()
///     .with_config(Config::from_str(toml)?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(seen.clone())
///     .auto_configuration::<HttpClientAutoConfiguration>()
///     .auto_configuration::<OAuth2ClientAutoConfiguration>()
///     .run()
///     .await?;
/// let client = seen.0.lock().unwrap().clone().expect("the OAuth2Client bean");
/// // In a component: `client.client_credentials("billing").await` gets the application's
/// // token; `Debug` never shows the secret.
/// assert!(format!("{client:?}").contains("registrations: 1"));
/// assert!(!format!("{client:?}").contains("orders-secret"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct OAuth2ClientAutoConfiguration;

impl Configuration for OAuth2ClientAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(oauth2_client)
            .on_missing_bean::<OAuth2Client>()
    }
}

impl AutoConfiguration for OAuth2ClientAutoConfiguration {
    /// The table `security.oauth2.client.registration` is present.
    fn conditions() -> Conditions {
        Conditions::new().on_property(REGISTRATIONS)
    }

    /// `HttpClientAutoConfiguration` and `SecurityAutoConfiguration`, whose beans (the client
    /// and the shared parsed section) it reads. The beans are resolved when the client is
    /// created, so the order states the dependency rather than enabling it; an excluded one
    /// is ignored.
    fn after() -> Vec<ConfigRef> {
        vec![
            ConfigRef::of::<super::HttpClientAutoConfiguration>(),
            ConfigRef::of::<super::SecurityAutoConfiguration>(),
        ]
    }
}

/// The client of the shared settings (or, without `SecurityAutoConfiguration`, of its own
/// parse of the same section), the `dyn HttpClient` bean and the clock bean.
fn oauth2_client(
    config: Arc<Config>,
    properties: Option<Arc<OAuth2Properties>>,
    client: Option<Arc<dyn HttpClient>>,
    clock: Option<Arc<dyn Clock>>,
) -> Result<OAuth2Client, DiError> {
    let properties = match properties {
        Some(properties) => properties,
        None => Arc::new(oauth2_properties(config)?),
    };
    let client = client.ok_or_else(|| {
        DiError::creation_from::<OAuth2Client>(OAuth2LoginError::WithoutHttpClient)
    })?;
    let oauth2 = OAuth2Client::new(properties.settings().clone(), client);
    Ok(match clock {
        Some(clock) => oauth2.clock(clock),
        None => oauth2,
    })
}
