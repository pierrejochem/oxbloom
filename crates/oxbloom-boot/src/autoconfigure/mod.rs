//! The auto-configurations that ship with boot, and the list an application passes to
//! [`OxbloomApplicationBuilder::auto_configurations`](crate::OxbloomApplicationBuilder::auto_configurations).
//!
//! Each one backs off when the application registers its own bean of the type it provides,
//! and its contributor beans ([`SecurityCustomizer`] with `security`; `dyn Controller` and
//! `dyn Filter` with `web`) are collected from the container. With `ssl`, the web server
//! receives the `SslBundles` bean for `server.ssl.bundle`.

#[cfg(feature = "actuator")]
mod actuator;
#[cfg(feature = "security")]
mod jwt;
#[cfg(feature = "security")]
mod password;
#[cfg(feature = "web")]
mod request_log;
#[cfg(feature = "security")]
mod security;
#[cfg(feature = "web")]
mod server;
#[cfg(feature = "security")]
mod session;
#[cfg(feature = "ssl")]
mod ssl;
#[cfg(feature = "tasks")]
mod tasks;

#[cfg(feature = "actuator")]
pub use actuator::ActuatorAutoConfiguration;
#[cfg(feature = "security")]
pub use jwt::JwtDecoderAutoConfiguration;
#[cfg(feature = "security")]
pub use password::PasswordEncoderAutoConfiguration;
#[cfg(feature = "web")]
pub use request_log::RequestLogAutoConfiguration;
#[cfg(feature = "security")]
pub use security::{SecurityAutoConfiguration, SecurityCustomizer};
#[cfg(feature = "web")]
pub use server::WebServerAutoConfiguration;
#[cfg(feature = "security")]
pub use session::SessionAutoConfiguration;
#[cfg(feature = "ssl")]
pub use ssl::SslAutoConfiguration;
#[cfg(feature = "tasks")]
pub use tasks::{RetryAutoConfiguration, SchedulerService, TasksAutoConfiguration};

use crate::autoconfig::AutoConfigurations;

/// The auto-configurations of the enabled features, in order: with `security`,
/// `JwtDecoderAutoConfiguration`, `PasswordEncoderAutoConfiguration`,
/// `SessionAutoConfiguration` and then `SecurityAutoConfiguration`; with `ssl`,
/// `SslAutoConfiguration`; with `actuator`, `ActuatorAutoConfiguration`; with `web`,
/// `RequestLogAutoConfiguration` and then `WebServerAutoConfiguration` (after the others when
/// several features are on); with `tasks`, `TasksAutoConfiguration` and then
/// `RetryAutoConfiguration` last. Remove one with
/// [`exclude`](crate::OxbloomApplicationBuilder::exclude).
///
/// # Examples
///
/// ```
/// use oxbloom_boot::{Banner, OxbloomApplication, OxbloomApplicationBuilder, auto_configurations};
/// use oxbloom_config::Config;
///
/// // `security` adds four auto-configurations, `ssl` one, `actuator` one, `web` two and
/// // `tasks` two.
/// let expected = 4 * usize::from(cfg!(feature = "security"))
///     + usize::from(cfg!(feature = "ssl"))
///     + usize::from(cfg!(feature = "actuator"))
///     + 2 * usize::from(cfg!(feature = "web"))
///     + 2 * usize::from(cfg!(feature = "tasks"));
/// assert_eq!(auto_configurations().len(), expected);
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// // No `security.jwt` or `security.session` section and no controller: the JWT decoder, the
/// // session filter and the web server are skipped, the default chain (every request must be authenticated) is registered. The
/// // actuator joins only an application with a controller of its own, so it is skipped too.
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
///     use oxbloom_boot::{
///         JwtDecoderAutoConfiguration, PasswordEncoderAutoConfiguration, SecurityAutoConfiguration,
///         SessionAutoConfiguration,
///     };
///     assert_eq!(keys(all(), "security.jwt."), 8);
///     assert_eq!(keys(all().exclude::<JwtDecoderAutoConfiguration>(), "security.jwt."), 0);
///     // 8 `security.jwt` keys, 4 `security.password` keys, 2 `security.http-basic` keys,
///     // 4 `security.users` keys, 10 `security.session` keys and 10 `security.form-login` keys
///     // (listed by `SecurityAutoConfiguration`, so excluding it hides them).
///     assert_eq!(keys(all(), "security."), 38);
///     assert_eq!(keys(all().exclude::<PasswordEncoderAutoConfiguration>(), "security."), 28);
///     assert_eq!(keys(all(), "security.session."), 10);
///     assert_eq!(keys(all().exclude::<SessionAutoConfiguration>(), "security."), 28);
///     assert_eq!(keys(all(), "security.form-login."), 10);
///     assert_eq!(keys(all().exclude::<SecurityAutoConfiguration>(), "security.form-login."), 0);
/// }
/// #[cfg(feature = "web")]
/// {
///     use oxbloom_boot::{RequestLogAutoConfiguration, WebServerAutoConfiguration};
///     assert_eq!(keys(all(), "logging.request."), 3);
///     assert_eq!(keys(all().exclude::<RequestLogAutoConfiguration>(), "logging.request."), 0);
///     // 25 keys of the web server, and `server.ssl.bundle` from `boot_properties()`.
///     assert_eq!(keys(all(), "server."), 26);
///     assert_eq!(keys(all().exclude::<WebServerAutoConfiguration>(), "server."), 1);
/// }
/// #[cfg(feature = "ssl")]
/// {
///     use oxbloom_boot::SslAutoConfiguration;
///     assert_eq!(keys(all(), "ssl."), oxbloom_ssl::PROPERTIES.len());
///     assert_eq!(keys(all().exclude::<SslAutoConfiguration>(), "ssl."), 0);
/// }
/// #[cfg(feature = "actuator")]
/// {
///     use oxbloom_boot::ActuatorAutoConfiguration;
///     assert_eq!(keys(all(), "management."), 9);
///     assert_eq!(keys(all().exclude::<ActuatorAutoConfiguration>(), "management."), 0);
/// }
/// #[cfg(feature = "tasks")]
/// {
///     use oxbloom_boot::{RetryAutoConfiguration, TasksAutoConfiguration};
///     // 5 `tasks.scheduling` keys and 6 `tasks.retry` keys.
///     assert_eq!(keys(all(), "tasks."), 11);
///     assert_eq!(keys(all().exclude::<TasksAutoConfiguration>(), "tasks."), 6);
///     assert_eq!(keys(all().exclude::<RetryAutoConfiguration>(), "tasks."), 5);
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
        .add::<PasswordEncoderAutoConfiguration>()
        .add::<SessionAutoConfiguration>()
        .add::<SecurityAutoConfiguration>();
    #[cfg(feature = "ssl")]
    let list = list.add::<SslAutoConfiguration>();
    #[cfg(feature = "actuator")]
    let list = list.add::<ActuatorAutoConfiguration>();
    #[cfg(feature = "web")]
    let list = list
        .add::<RequestLogAutoConfiguration>()
        .add::<WebServerAutoConfiguration>();
    #[cfg(feature = "tasks")]
    let list = list
        .add::<TasksAutoConfiguration>()
        .add::<RetryAutoConfiguration>();
    list
}
