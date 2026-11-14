use oxbloom_di::{ContainerBuilder, DiError, Environment};

use crate::node::NodeValue;
use crate::{Config, ConfigError};

/// Registers a [`Config`] in a `oxbloom-di` container. Requires the `di` feature.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_config::{Config, ContainerBuilderExt};
/// use oxbloom_di::{ContainerBuilder, component};
///
/// #[component]
/// struct Mailer { config: Arc<Config> }
///
/// let config = Config::from_str("[mail]\nfrom = \"noreply@example.com\"")?;
/// let container = ContainerBuilder::new().config(config).component::<Mailer>().build()?;
/// let mailer = container.get::<Mailer>()?;
/// assert_eq!(mailer.config.get::<String>("mail.from")?, "noreply@example.com");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait ContainerBuilderExt {
    /// Registers `config` as a singleton, resolvable as `Arc<Config>`.
    ///
    /// It does not pass the configuration's active profiles to the container. For
    /// profile-conditional beans, call
    /// `.active_profiles(config.active_profiles().iter().cloned())` on the builder as well.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, ContainerBuilderExt};
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// let container = ContainerBuilder::new()
    ///     .config(Config::from_str("n = 1")?)
    ///     .build()?;
    /// assert_eq!(container.get::<Config>()?.get::<u8>("n")?, 1);
    ///
    /// // Forward the active profiles yourself.
    /// struct DevTools;
    /// # let dir = tempfile::tempdir()?;
    /// # std::fs::write(dir.path().join("application-dev.toml"), "")?;
    /// let config = Config::builder().dir(dir.path()).profile("dev").env_from([]).build()?;
    /// let container = ContainerBuilder::new()
    ///     .active_profiles(config.active_profiles().iter().cloned())
    ///     .config(config)
    ///     .instance(DevTools).profile("dev")
    ///     .build()?;
    /// assert!(container.get::<DevTools>().is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    fn config(self, config: Config) -> Self;
}

impl ContainerBuilderExt for ContainerBuilder {
    fn config(self, config: Config) -> Self {
        self.instance(config)
    }
}

/// Lets `?` turn a [`ConfigError`] into a [`DiError`] inside a `try_bean` factory. Requires the
/// `di` feature.
///
/// The result is a `DiError::Creation` whose message is the error's `Display` (which never
/// contains configuration values) and whose source is the original [`ConfigError`]. `try_bean`
/// replaces the type name with the bean being built and keeps the source.
///
/// Only `try_bean` and `#[bean(fallible)]` rewrite the type name. A `factory` or
/// `Component::create` that uses `?` on a `ConfigError` keeps the label of `ConfigError` itself;
/// use `try_bean` there, or `map_err(DiError::creation_from::<T>)` inside a `factory`.
///
/// # Examples
///
/// ```
/// use std::error::Error;
/// use oxbloom_config::{Config, ConfigError};
/// use oxbloom_di::{ContainerBuilder, DiError};
///
/// struct Pool { size: u16 }
///
/// let err = ContainerBuilder::new()
///     .try_bean(|| -> Result<Pool, DiError> {
///         let config = Config::from_str("")?;
///         Ok(Pool { size: config.get("pool.size")? })
///     })
///     .build()
///     .err()
///     .unwrap();
/// assert!(matches!(&err, DiError::Creation { type_name, .. } if type_name.ends_with("Pool")));
/// let source = err.source().and_then(|s| s.downcast_ref::<ConfigError>());
/// assert!(matches!(source, Some(ConfigError::Missing { key }) if key == "pool.size"));
/// ```
impl From<ConfigError> for DiError {
    fn from(err: ConfigError) -> Self {
        DiError::creation_from::<ConfigError>(err)
    }
}

/// Lets `oxbloom-di` conditions (`on_property`, `on_property_value`) read a [`Config`]. Requires
/// the `di` feature.
///
/// `property(key)` finds `key` with the same rules as [`Config::get`]: a dotted path whose
/// segments match case-insensitively with `-` and `_` ignored. Indexed list elements such as
/// `hosts[0]` are not paths. It returns `Some(text)` only for scalar leaves and `None` for absent
/// keys, tables, lists and the empty key (the root is a table). The text is:
///
/// - strings as they are;
/// - integers in decimal (`8080`, `-3`), floats in TOML form (`1.5`, `inf`, `nan`), booleans as
///   `true` or `false`;
/// - dates and times in their TOML text form (`2026-10-25`).
///
/// `contains(key)` (what `on_property` asks) is `true` for any key that exists under the same rules,
/// tables and lists included, so `on_property("security.jwt")` matches a `[security.jwt]` section. It
/// is `false` for absent keys, indexed elements and the empty key.
///
/// A [`Secret`](crate::Secret) field is an ordinary leaf here, so its plain text is returned
/// to the condition. The condition report never contains values. The expected value of
/// `on_property_value(key, value)` sits in the source code and is compared as plain text, so
/// never use a condition to compare against a secret.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_di::{ContainerBuilder, Environment};
///
/// struct Metrics;
///
/// let config = Config::from_str("[metrics]\nenabled = true\nport = 9100\nhosts = [\"a\"]")?;
/// assert_eq!(config.property("Metrics.ENABLED").as_deref(), Some("true"));
/// assert_eq!(config.property("metrics.port").as_deref(), Some("9100"));
/// assert_eq!(config.property("metrics"), None); // a table
/// assert_eq!(config.property("metrics.hosts"), None); // a list
/// assert_eq!(config.property("metrics.nope"), None);
/// // `contains` (what `on_property` asks) also sees tables and lists.
/// assert!(config.contains("metrics") && config.contains("metrics.hosts"));
/// assert!(!config.contains("metrics.nope") && !config.contains(""));
///
/// let container = ContainerBuilder::new()
///     .environment(config)
///     .instance(Metrics)
///     .on_property_value("metrics.enabled", "true")
///     .build()?;
/// assert!(container.get_opt::<Metrics>()?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl Environment for Config {
    fn property(&self, key: &str) -> Option<String> {
        if key.is_empty() {
            return None;
        }
        match &self.node_at(key)?.value {
            NodeValue::String(s) => Some(s.clone()),
            NodeValue::Integer(i) => Some(i.to_string()),
            NodeValue::Float(f) => Some(toml::Value::Float(*f).to_string()),
            NodeValue::Boolean(b) => Some(b.to_string()),
            NodeValue::Datetime(d) => Some(d.to_string()),
            NodeValue::Table(_) | NodeValue::Array(_) => None,
        }
    }

    /// Whether `key` is present: a scalar, a list or a table, found with the same rules as
    /// `property`. So `on_property("security.jwt")` matches a `[security.jwt]` section.
    fn contains(&self, key: &str) -> bool {
        !key.is_empty() && self.node_at(key).is_some()
    }
}
