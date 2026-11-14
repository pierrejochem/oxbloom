//! `SecurityAutoConfiguration` and its contributor trait, `SecurityCustomizer`.

use std::sync::Arc;

use oxbloom_di::{Configuration, ContainerBuilder, DiError, component};
use oxbloom_security::{HttpSecurity, JwtDecoder, SecurityFilterChain};

use super::jwt::JwtDecoderAutoConfiguration;
use crate::autoconfig::{AutoConfiguration, ConfigRef};

/// Contributes rules to the `SecurityFilterChain` that [`SecurityAutoConfiguration`] builds.
///
/// Register an implementation as `dyn SecurityCustomizer`. The customizers are applied in the
/// order the container gives them (ascending `order`, then registration order), each on the
/// result of the one before. `HttpSecurity::authorize` may be called only once in total, so
/// exactly one customizer should own the rules. With no customizer at all, every request must
/// be authenticated.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     Banner, OxbloomApplication, RunListener, SecurityCustomizer, auto_configurations,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_security::{HttpSecurity, SecurityFilterChain};
///
/// /// Opens `/health`; everything else needs authentication.
/// struct PublicHealth;
/// impl SecurityCustomizer for PublicHealth {
///     fn customize(&self, http: HttpSecurity) -> HttpSecurity {
///         http.authorize(|a| a.get("/health").permit_all().any_request().authenticated())
///     }
/// }
///
/// #[derive(Clone, Default)]
/// struct Chains(Arc<Mutex<usize>>);
/// impl RunListener for Chains {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get_all::<SecurityFilterChain>().unwrap().len();
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let chains = Chains::default();
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(chains.clone())
///     .auto_configurations(auto_configurations())
///     .configure(|b| {
///         b.instance(PublicHealth)
///             .provides(|c: Arc<PublicHealth>| -> Arc<dyn SecurityCustomizer> { c })
///     })
///     .run()
///     .await?;
/// assert_eq!(*chains.0.lock().unwrap(), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait SecurityCustomizer: Send + Sync {
    /// Returns `http` with this customizer's settings added.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::SecurityCustomizer;
    /// use oxbloom_security::HttpSecurity;
    ///
    /// /// Opens `/health`; everything else needs authentication.
    /// struct PublicHealth;
    /// impl SecurityCustomizer for PublicHealth {
    ///     fn customize(&self, http: HttpSecurity) -> HttpSecurity {
    ///         http.authorize(|a| a.get("/health").permit_all().any_request().authenticated())
    ///     }
    /// }
    ///
    /// let chain = PublicHealth.customize(HttpSecurity::new()).build();
    /// assert!(chain.is_ok());
    /// ```
    fn customize(&self, http: HttpSecurity) -> HttpSecurity;
}

/// Registers the `SecurityFilterChain`, unless the application registered its own.
///
/// The chain starts from `HttpSecurity::new()`. The [`SecurityCustomizer`] beans are applied
/// in order; when there is none, the only rule is `any_request().authenticated()`, so the
/// application stays closed until it opens routes. A `dyn JwtDecoder` bean (from
/// [`JwtDecoderAutoConfiguration`], which runs first, or the application's) is added with
/// `oauth2_resource_server`. A `SecurityBuildError` fails the build as `DiError::Creation`
/// with that error as source.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     AutoConfiguration, Banner, ConfigRef, JwtDecoderAutoConfiguration, OxbloomApplication,
///     RunListener, SecurityAutoConfiguration, auto_configurations,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::{Container, DiError};
/// use oxbloom_security::{HttpSecurity, SecurityFilterChain};
///
/// #[derive(Clone, Default)]
/// struct Skipped(Arc<Mutex<bool>>);
/// impl RunListener for Skipped {
///     fn context_ready(&self, c: &Container) {
///         assert_eq!(c.get_all::<SecurityFilterChain>().unwrap().len(), 1);
///         *self.0.lock().unwrap() = c.condition_report().records().iter().any(|r| {
///             !r.matched() && r.reason().contains("SecurityFilterChain")
///         });
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// // The application's own chain: the default one steps aside.
/// let skipped = Skipped::default();
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(skipped.clone())
///     .auto_configurations(auto_configurations())
///     .configure(|b| {
///         b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
///             Ok(HttpSecurity::new().authorize(|a| a.any_request().permit_all()).build()?)
///         })
///     })
///     .run()
///     .await?;
/// assert!(*skipped.0.lock().unwrap());
/// assert_eq!(
///     SecurityAutoConfiguration::after(),
///     [ConfigRef::of::<JwtDecoderAutoConfiguration>()]
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct SecurityAutoConfiguration;

impl Configuration for SecurityAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(security_filter_chain)
            .on_missing_bean::<SecurityFilterChain>()
    }
}

impl AutoConfiguration for SecurityAutoConfiguration {
    fn after() -> Vec<ConfigRef> {
        vec![ConfigRef::of::<JwtDecoderAutoConfiguration>()]
    }
}

/// The chain from the customizers, or the closed default, plus the decoder when there is one.
fn security_filter_chain(
    customizers: Vec<Arc<dyn SecurityCustomizer>>,
    decoder: Option<Arc<dyn JwtDecoder>>,
) -> Result<SecurityFilterChain, DiError> {
    let mut http = HttpSecurity::new();
    if customizers.is_empty() {
        // Closed by default: nothing is reachable without authentication.
        http = http.authorize(|a| a.any_request().authenticated());
    }
    for customizer in &customizers {
        http = customizer.customize(http);
    }
    if let Some(decoder) = decoder {
        http = http.oauth2_resource_server(|r| r.jwt(decoder));
    }
    http.build()
        .map_err(DiError::creation_from::<SecurityFilterChain>)
}
