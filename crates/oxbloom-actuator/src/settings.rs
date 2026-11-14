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
    /// `management.endpoints.enabled`. Default `true`. This type only records the key; boot's
    /// actuator auto-configuration reads it and registers nothing when it is not `true`.
    /// [`ActuatorController`](crate::ActuatorController) does not look at it.
    pub enabled: bool,
    /// `management.endpoints.base-path`, normalized with a leading `/` and no trailing `/`.
    ///
    /// Surrounding whitespace and outer slashes are trimmed. Each segment between slashes
    /// must be non-empty and hold only ASCII letters, digits, `-`, `_`, `.` and `~`; so `/`,
    /// an empty path, `/a//b`, `/{x}`, `*`, `?` and inner whitespace are rejected.
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
    /// [`ConfigError::Invalid`] for an unknown key, for a `base-path` of `/`, an empty one or
    /// one that breaks the segment rule of [`base_path`](Self::base_path), and for a
    /// `show-details` that is neither `never` nor `always`. The error never shows a
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

const BASE_PATH: &str = "management.endpoints.base-path";

/// Trims the base path and checks its segments. The error names the key, never the value.
fn normalize(path: &str) -> Result<String, ConfigError> {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        return Err(ConfigError::invalid(
            BASE_PATH,
            "must name a path below the root, such as `/actuator`",
        ));
    }
    let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~');
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || !segment.chars().all(safe))
    {
        return Err(ConfigError::invalid(
            BASE_PATH,
            "each segment must be non-empty and hold only ASCII letters, digits, `-`, `_`, `.` \
             and `~`",
        ));
    }
    Ok(format!("/{trimmed}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejected(path: &str) {
        let error = normalize(path).expect_err(path);
        let text = error.to_string();
        assert!(text.contains(BASE_PATH), "{text}");
        let inner = path.trim().trim_matches('/');
        assert!(
            inner.is_empty() || !text.contains(inner),
            "{text} shows the value"
        );
    }

    #[test]
    fn normalize_trims_and_accepts_safe_segments() {
        assert_eq!(normalize("ops/").unwrap(), "/ops");
        assert_eq!(normalize(" //a/b-c_d.e~f/ ").unwrap(), "/a/b-c_d.e~f");
        assert_eq!(normalize("/v1/manage").unwrap(), "/v1/manage");
    }

    #[test]
    fn normalize_rejects_the_root_and_an_empty_path() {
        for path in ["/", "", "  ", "//"] {
            rejected(path);
        }
    }

    #[test]
    fn normalize_rejects_route_pattern_characters() {
        for path in ["/{x}", "/a{b", "/a}b", "/*", "/a*", "/a?b"] {
            rejected(path);
        }
    }

    #[test]
    fn normalize_rejects_inner_whitespace() {
        for path in ["/a b", "/a\tb", "/a/ b"] {
            rejected(path);
        }
    }

    #[test]
    fn normalize_rejects_empty_inner_segments() {
        for path in ["/a//b", "a///b/"] {
            rejected(path);
        }
    }
}
