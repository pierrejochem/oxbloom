//! The reader of the session management keys `security.session.max-sessions-per-user` and
//! `security.session.revalidate-interval`.

use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;

const SECTION: &str = "security.session";
const MAX_SESSIONS_PER_USER: &str = "security.session.max-sessions-per-user";
const REVALIDATE_INTERVAL: &str = "security.session.revalidate-interval";

/// The revalidation interval when `security.session.revalidate-interval` is not set.
const DEFAULT_REVALIDATE_INTERVAL: Duration = Duration::from_secs(300);

/// Only the two keys of this reader; the other `security.session` keys belong to
/// [`SessionConfig`](crate::SessionConfig), which rejects unknown keys.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RawManagement {
    #[serde(rename = "max-sessions-per-user")]
    max_sessions_per_user: Option<usize>,
    #[serde(rename = "revalidate-interval")]
    revalidate_interval: Option<Duration>,
}

/// The session management settings: the per-user session limit and the revalidation interval.
///
/// The default has no limit and an interval of five minutes that was not set explicitly.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_security::SessionManagement;
///
/// let defaults = SessionManagement::default();
/// assert_eq!(defaults.max_sessions_per_user, None);
/// assert_eq!(defaults.revalidate_interval, Some(Duration::from_secs(300)));
/// assert!(!defaults.revalidate_explicit);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionManagement {
    /// `security.session.max-sessions-per-user`: the most sessions one user may hold, at least
    /// 1; `None` for no limit.
    pub max_sessions_per_user: Option<usize>,
    /// `security.session.revalidate-interval`: how often the user of a session login is loaded
    /// again, at least one second; `None` when revalidation is off (`0s`).
    pub revalidate_interval: Option<Duration>,
    /// Whether `security.session.revalidate-interval` was set in the configuration, rather
    /// than taken from the default.
    pub revalidate_explicit: bool,
}

impl Default for SessionManagement {
    fn default() -> Self {
        Self {
            max_sessions_per_user: None,
            revalidate_interval: Some(DEFAULT_REVALIDATE_INTERVAL),
            revalidate_explicit: false,
        }
    }
}

/// The reader of the session management settings under `security.session`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::SessionManagementProperties;
///
/// let config = Config::from_str("[security.session]\nmax-sessions-per-user = 2")?;
/// let management = SessionManagementProperties::from_config(&config)?;
/// assert_eq!(management.max_sessions_per_user, Some(2));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct SessionManagementProperties;

impl SessionManagementProperties {
    /// Reads `security.session.max-sessions-per-user` and
    /// `security.session.revalidate-interval` of `config`.
    ///
    /// `max-sessions-per-user` is a positive integer; unset means no limit.
    /// `revalidate-interval` is a duration in the configuration's duration form, such as `90s`
    /// or `5m`, of at least one second; `0s` (or `0`) turns revalidation off, and unset means
    /// five minutes. [`SessionManagement::revalidate_explicit`] tells whether the interval was
    /// set, so a caller can treat an explicit interval more strictly than the default.
    ///
    /// This reader ignores the other `security.session` keys. Unknown keys in the section are
    /// rejected by [`SessionConfig::from_config`](crate::SessionConfig::from_config), which
    /// accepts these two keys and leaves them to this reader.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for a value of the wrong type,
    /// `max-sessions-per-user = 0` and an interval between zero and one second. The error never
    /// shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_security::SessionManagementProperties;
    ///
    /// let config = Config::from_str("[security.session]\nrevalidate-interval = \"1m\"")?;
    /// let management = SessionManagementProperties::from_config(&config)?;
    /// assert_eq!(management.revalidate_interval, Some(Duration::from_secs(60)));
    /// assert!(management.revalidate_explicit);
    ///
    /// // `0s` turns revalidation off.
    /// let config = Config::from_str("[security.session]\nrevalidate-interval = \"0s\"")?;
    /// assert_eq!(SessionManagementProperties::from_config(&config)?.revalidate_interval, None);
    ///
    /// // Below one second is an error that names the key and not the value.
    /// let config = Config::from_str("[security.session]\nrevalidate-interval = \"500ms\"")?;
    /// let err = SessionManagementProperties::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.session.revalidate-interval"));
    /// assert!(!err.to_string().contains("500"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<SessionManagement, ConfigError> {
        let raw = match config.get::<RawManagement>(SECTION) {
            Ok(raw) => raw,
            Err(ConfigError::Missing { key }) if key == SECTION => RawManagement::default(),
            Err(e) => return Err(e),
        };
        let mut management = SessionManagement::default();
        if let Some(max) = raw.max_sessions_per_user {
            if max == 0 {
                return Err(ConfigError::invalid(
                    MAX_SESSIONS_PER_USER,
                    "must be at least 1",
                ));
            }
            management.max_sessions_per_user = Some(max);
        }
        if let Some(interval) = raw.revalidate_interval {
            management.revalidate_explicit = true;
            management.revalidate_interval = if interval.is_zero() {
                None
            } else if interval < Duration::from_secs(1) {
                return Err(ConfigError::invalid(
                    REVALIDATE_INTERVAL,
                    "must be `0s` (off) or at least one second",
                ));
            } else {
                Some(interval)
            };
        }
        Ok(management)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(body: &str) -> Result<SessionManagement, ConfigError> {
        let toml = format!("[security.session]\n{body}\n");
        SessionManagementProperties::from_config(&Config::from_str(&toml).unwrap())
    }

    fn rejects(body: &str, key: &str, value: &str) {
        let err = read(body).unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(text.contains(key), "{text}");
        assert!(!text.contains(value), "{text} echoes {value}");
    }

    #[test]
    fn defaults_without_the_keys() {
        let empty = Config::from_str("").unwrap();
        assert_eq!(
            SessionManagementProperties::from_config(&empty).unwrap(),
            SessionManagement::default()
        );
        // The other session keys are not this reader's business.
        assert_eq!(
            read("store = \"memory\"\nidle-timeout = \"10m\"").unwrap(),
            SessionManagement::default()
        );
    }

    #[test]
    fn both_keys_are_read() {
        let management = read("max-sessions-per-user = 1\nrevalidate-interval = \"1s\"").unwrap();
        assert_eq!(
            management,
            SessionManagement {
                max_sessions_per_user: Some(1),
                revalidate_interval: Some(Duration::from_secs(1)),
                revalidate_explicit: true,
            }
        );
        // An explicit default is still explicit.
        let management = read("revalidate-interval = \"5m\"").unwrap();
        assert!(management.revalidate_explicit);
        assert_eq!(
            management.revalidate_interval,
            Some(DEFAULT_REVALIDATE_INTERVAL)
        );
    }

    #[test]
    fn zero_turns_revalidation_off() {
        for zero in ["\"0s\"", "0", "\"0ms\""] {
            let management = read(&format!("revalidate-interval = {zero}")).unwrap();
            assert_eq!(management.revalidate_interval, None, "{zero}");
            assert!(management.revalidate_explicit, "{zero}");
        }
    }

    #[test]
    fn bad_values_name_the_key_not_the_value() {
        rejects("max-sessions-per-user = 0", MAX_SESSIONS_PER_USER, "= 0");
        rejects(
            "max-sessions-per-user = \"lots\"",
            MAX_SESSIONS_PER_USER,
            "lots",
        );
        rejects("max-sessions-per-user = -3", MAX_SESSIONS_PER_USER, "-3");
        rejects(
            "revalidate-interval = \"999ms\"",
            REVALIDATE_INTERVAL,
            "999",
        );
        rejects("revalidate-interval = \"1ns\"", REVALIDATE_INTERVAL, "1ns");
        rejects(
            "revalidate-interval = \"weekly\"",
            REVALIDATE_INTERVAL,
            "weekly",
        );
    }
}
