//! The auto-configurations that ship with boot, and the list an application passes to
//! [`OxbloomApplicationBuilder::auto_configurations`](crate::OxbloomApplicationBuilder::auto_configurations).
//!
//! Each one backs off when the application registers its own bean of the type it provides,
//! and its contributor beans ([`SecurityCustomizer`] with `security`; `dyn Controller` and
//! `dyn Filter` with `web`) are collected from the container.

#[cfg(feature = "security")]
mod jwt;
#[cfg(feature = "web")]
mod request_log;
#[cfg(feature = "security")]
mod security;
#[cfg(feature = "web")]
mod server;

#[cfg(feature = "security")]
pub use jwt::JwtDecoderAutoConfiguration;
#[cfg(feature = "web")]
pub use request_log::RequestLogAutoConfiguration;
#[cfg(feature = "security")]
pub use security::{SecurityAutoConfiguration, SecurityCustomizer};
#[cfg(feature = "web")]
pub use server::WebServerAutoConfiguration;

use crate::autoconfig::AutoConfigurations;

/// The auto-configurations of the enabled features, in order: with `security`,
/// `JwtDecoderAutoConfiguration` and then `SecurityAutoConfiguration`; with `web`,
/// `RequestLogAutoConfiguration` and then `WebServerAutoConfiguration` (after the two when
/// both features are on). Remove one with
/// [`exclude`](crate::OxbloomApplicationBuilder::exclude).
///
/// # Examples
///
/// ```
/// use oxbloom_boot::{Banner, OxbloomApplication, OxbloomApplicationBuilder, auto_configurations};
/// use oxbloom_config::Config;
///
/// // `security` adds two auto-configurations, `web` adds two.
/// let expected = 2 * usize::from(cfg!(feature = "security")) + 2 * usize::from(cfg!(feature = "web"));
/// assert_eq!(auto_configurations().len(), expected);
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// // No `security.jwt` section and no controller: the JWT decoder and the web server are
/// // skipped, the default chain (every request must be authenticated) is registered.
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .auto_configurations(auto_configurations())
///     .run()
///     .await?;
///
/// // The metadata lists the keys of each auto-configuration unless it is excluded.
/// let keys = |b: OxbloomApplicationBuilder, prefix: &str| {
///     b.property_metadata().iter().filter(|p| p.key.starts_with(prefix)).count()
/// };
/// let all = || OxbloomApplication::builder().auto_configurations(auto_configurations());
/// #[cfg(feature = "security")]
/// {
///     use oxbloom_boot::JwtDecoderAutoConfiguration;
///     assert_eq!(keys(all(), "security.jwt."), 7);
///     assert_eq!(keys(all().exclude::<JwtDecoderAutoConfiguration>(), "security.jwt."), 0);
/// }
/// #[cfg(feature = "web")]
/// {
///     use oxbloom_boot::{RequestLogAutoConfiguration, WebServerAutoConfiguration};
///     assert_eq!(keys(all(), "logging.request."), 3);
///     assert_eq!(keys(all().exclude::<RequestLogAutoConfiguration>(), "logging.request."), 0);
///     assert_eq!(keys(all(), "server."), 25);
///     assert_eq!(keys(all().exclude::<WebServerAutoConfiguration>(), "server."), 0);
/// }
/// # let _ = (keys, all);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn auto_configurations() -> AutoConfigurations {
    let list = AutoConfigurations::new();
    #[cfg(feature = "security")]
    let list = list
        .add::<JwtDecoderAutoConfiguration>()
        .add::<SecurityAutoConfiguration>();
    #[cfg(feature = "web")]
    let list = list
        .add::<RequestLogAutoConfiguration>()
        .add::<WebServerAutoConfiguration>();
    list
}
