use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The `management.*` settings of the actuator endpoints.
///
/// Read with [`from_config`](ActuatorSettings::from_config). The keys are
/// `management.endpoints.enabled`, `management.endpoints.base-path` (default `/actuator`),
/// `management.endpoints.health.public`, `management.endpoint.<id>.enabled` for the ids
/// `health`, `info` and `loggers`, `management.health.show-details` (`never`, the default, or
/// `always`), `management.health.timeout` (default 5 seconds) and
/// `management.health.readiness.include`. A key that is not listed, directly under
/// `management`, `management.endpoints`, `management.endpoint` or `management.health`, is a
/// [`ConfigError`] that names the key and lists the valid ones, as under `ssl`.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::ActuatorSettings;
/// use oxbloom_config::Config;
///
/// let config = Config::from_str("[management.endpoints]\nbase-path = \"ops/\"")?;
/// let settings = ActuatorSettings::from_config(&config)?;
/// assert_eq!(settings.base_path, "/ops");
/// assert!(!settings.show_details);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActuatorSettings {
    /// `management.endpoints.enabled`: `false` turns the whole actuator off. Default `true`.
    pub enabled: bool,
    /// `management.endpoints.base-path`, normalized with a leading `/` and no trailing `/`.
    pub base_path: String,
    /// `management.endpoints.health.public`: `true` opens the health endpoints to everyone.
    /// Default `false`.
    pub health_public: bool,
    /// `management.health.show-details` is `always`.
    pub show_details: bool,
    /// `management.health.timeout`, the limit for each health indicator.
    pub timeout: Duration,
    /// `management.health.readiness.include`: the indicators that count for readiness.
    pub readiness_include: Vec<String>,
    /// `management.endpoint.health.enabled`.
    pub health_enabled: bool,
    /// `management.endpoint.info.enabled`.
    pub info_enabled: bool,
    /// `management.endpoint.loggers.enabled`.
    pub loggers_enabled: bool,
}

impl Default for ActuatorSettings {
    /// The defaults: everything enabled, base path `/actuator`, details hidden, 5 s timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::ActuatorSettings;
    ///
    /// let settings = ActuatorSettings::default();
    /// assert_eq!(settings.base_path, "/actuator");
    /// assert!(settings.health_enabled);
    /// ```
    fn default() -> Self {
        Self {
            enabled: true,
            base_path: "/actuator".to_string(),
            health_public: false,
            show_details: false,
            timeout: DEFAULT_TIMEOUT,
            readiness_include: Vec::new(),
            health_enabled: true,
            info_enabled: true,
            loggers_enabled: true,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawManagement {
    endpoints: RawEndpoints,
    endpoint: RawEndpoint,
    health: RawHealth,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawEndpoints {
    enabled: Option<bool>,
    base_path: Option<String>,
    health: RawEndpointsHealth,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawEndpointsHealth {
    public: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawEndpoint {
    health: RawEnabled,
    info: RawEnabled,
    loggers: RawEnabled,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawEnabled {
    enabled: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawHealth {
    show_details: Option<String>,
    timeout: Option<Duration>,
    readiness: RawReadiness,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawReadiness {
    include: Vec<String>,
}

impl ActuatorSettings {
    /// Reads the settings from the `management` keys of `config`.
    ///
    /// A missing `management` section gives the defaults. Fails with
    /// [`ConfigError::Invalid`] for an unknown key, for a `base-path` of `/` or an empty one,
    /// and for a `show-details` that is neither `never` nor `always`. The error never shows a
    /// configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::ActuatorSettings;
    /// use oxbloom_config::Config;
    ///
    /// let config = Config::from_str("[management.endpoints]\nbase-path = \"/\"")?;
    /// assert!(ActuatorSettings::from_config(&config).is_err());
    ///
    /// let config = Config::from_str("[management.health]\nshow-details = \"always\"")?;
    /// assert!(ActuatorSettings::from_config(&config)?.show_details);
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let raw = match config.get::<RawManagement>("management") {
            Ok(raw) => raw,
            Err(ConfigError::Missing { .. }) => RawManagement::default(),
            Err(e) => return Err(e),
        };
        let defaults = Self::default();
        let enabled = raw.endpoints.enabled.unwrap_or(true);
        let base_path = match raw.endpoints.base_path {
            Some(path) => normalize(&path)?,
            None => defaults.base_path,
        };
        let show_details = match raw.health.show_details.as_deref() {
            None | Some("never") => false,
            Some("always") => true,
            Some(_) => {
                return Err(ConfigError::invalid(
                    "management.health.show-details",
                    "must be `never` or `always`",
                ));
            }
        };
        Ok(Self {
            enabled,
            base_path,
            health_public: raw.endpoints.health.public.unwrap_or(false),
            show_details,
            timeout: raw.health.timeout.unwrap_or(defaults.timeout),
            readiness_include: raw.health.readiness.include,
            health_enabled: raw.endpoint.health.enabled.unwrap_or(true),
            info_enabled: raw.endpoint.info.enabled.unwrap_or(true),
            loggers_enabled: raw.endpoint.loggers.enabled.unwrap_or(true),
        })
    }
}

fn normalize(path: &str) -> Result<String, ConfigError> {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err(ConfigError::invalid(
            "management.endpoints.base-path",
            "must name a path below the root, such as `/actuator`",
        ));
    }
    Ok(format!("/{trimmed}"))
}
