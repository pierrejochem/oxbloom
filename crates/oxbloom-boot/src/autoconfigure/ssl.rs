//! `SslAutoConfiguration`: the `SslBundles` of the `ssl.bundle.*` keys.

use std::sync::{Arc, OnceLock};

use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_ssl::SslBundles;

use crate::autoconfig::AutoConfiguration;
use crate::metadata::PropertyMetadata;

/// Registers the [`SslBundles`] of the `ssl.*` keys when the configuration has any
/// `ssl.bundle.*` key, unless the application registered its own `SslBundles`.
///
/// Available with the cargo feature `ssl`. The group runs only with `ssl.bundle` present (the
/// condition `on_property("ssl.bundle")`, which a `[ssl.bundle.pem.<name>]` or
/// `[ssl.bundle.pkcs12.<name>]` section satisfies), so the conditions report says why it ran
/// or not; it never shows a value. An optional `dyn Clock` bean (`oxbloom_core::Clock`)
/// replaces the system time in the certificate validity checks; it is set before the container
/// starts the reload task through the `Lifecycle` hook of `SslBundles`, and the task is stopped
/// at shutdown. A bundle that cannot be loaded fails the build with `DiError::Creation` whose
/// source is the `SslError` naming the bundle and the key to fix; the built-in failure report
/// explains it and never prints a private key, a password or a file path.
///
/// `WebServerAutoConfiguration` runs after this one and serves HTTPS with the bundle that
/// `server.ssl.bundle` names. The property metadata lists every `ssl.*` key of
/// `oxbloom_ssl::PROPERTIES`.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     AutoConfiguration, Banner, OxbloomApplication, RunListener, SslAutoConfiguration,
///     StartupPhase,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_ssl::{SslBundles, SslError};
///
/// /// The bundle names the container held when it was ready.
/// #[derive(Clone, Default)]
/// struct Names(Arc<Mutex<Option<Vec<String>>>>);
/// impl RunListener for Names {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get_opt::<SslBundles>().unwrap().map(|b| b.names());
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let run = |toml: &str, seen: Names| {
///     OxbloomApplication::builder()
///         .with_config(Config::from_str(toml).unwrap())
///         .banner(Banner::off())
///         .output(std::io::sink())
///         .listener(seen)
///         .auto_configuration::<SslAutoConfiguration>()
///         .run()
/// };
///
/// # let ca = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?.cert.pem();
/// // `ca` holds a CA certificate as PEM text; a file path works the same way.
/// let seen = Names::default();
/// run(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca}'''"), seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), Some(vec!["client".to_string()]));
///
/// // Without any `ssl.bundle.*` key there is no `SslBundles` bean.
/// let seen = Names::default();
/// run("", seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), None);
///
/// // A broken bundle fails the container with the `SslError` naming the key.
/// let err = run("[ssl.bundle.pem.web]\ntrust = \"no-such-file.pem\"", Names::default())
///     .await
///     .unwrap_err();
/// assert_eq!(err.phase(), StartupPhase::Container);
/// assert!(matches!(err.find_cause::<SslError>(), Some(SslError::Io { .. })));
/// assert!(err.analysis().unwrap().to_string().contains("ssl.bundle.pem.web.trust"));
///
/// assert!(SslAutoConfiguration::properties().iter().any(|p| p.key == "ssl.reload-interval"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct SslAutoConfiguration;

impl Configuration for SslAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(ssl_bundles)
            .lifecycle::<SslBundles>()
            .on_missing_bean::<SslBundles>()
    }
}

impl AutoConfiguration for SslAutoConfiguration {
    fn conditions() -> Conditions {
        Conditions::new().on_property("ssl.bundle")
    }

    fn properties() -> &'static [PropertyMetadata] {
        static SSL: OnceLock<Vec<PropertyMetadata>> = OnceLock::new();
        SSL.get_or_init(|| {
            oxbloom_ssl::PROPERTIES
                .iter()
                .map(|&(key, ty, default, text)| PropertyMetadata::new(key, ty, default, text))
                .collect()
        })
    }
}

/// The bundles of the `ssl.*` keys, checked against the `dyn Clock` bean when there is one.
/// The clock is set here, before the container runs the `Lifecycle` hook that starts the
/// reload task. The `SslError` stays the typed source of a `DiError::Creation`.
fn ssl_bundles(config: Arc<Config>, clock: Option<Arc<dyn Clock>>) -> Result<SslBundles, DiError> {
    let bundles = SslBundles::from_config(&config).map_err(DiError::creation_from::<SslBundles>)?;
    Ok(match clock {
        Some(clock) => bundles.with_clock(clock),
        None => bundles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_are_exactly_the_ssl_crate_keys() {
        let keys: Vec<_> = SslAutoConfiguration::properties()
            .iter()
            .map(|p| p.key)
            .collect();
        let expected: Vec<_> = oxbloom_ssl::PROPERTIES.iter().map(|p| p.0).collect();
        assert_eq!(keys, expected);
    }
}
