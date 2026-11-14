//! `RetryDefaults::from_config`: the `tasks.retry.*` keys.

use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::retry::{Backoff, Jitter, RetryDefaults};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawTasks {
    // Read by `TasksSettings::from_config`; only its key names are checked here, so a typo
    // in either section is rejected by both readers.
    #[allow(dead_code)]
    scheduling: SchedulingKeys,
    retry: RawRetry,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[allow(dead_code)]
struct SchedulingKeys {
    enabled: IgnoredAny,
    max_concurrent: IgnoredAny,
    shutdown_timeout: IgnoredAny,
    zone: IgnoredAny,
    initial_delay: IgnoredAny,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRetry {
    max_attempts: Option<u32>,
    backoff: Option<String>,
    initial: Option<Duration>,
    multiplier: Option<f64>,
    max_delay: Option<Duration>,
    jitter: Option<String>,
}

const MAX_ATTEMPTS: &str = "tasks.retry.max-attempts";
const BACKOFF: &str = "tasks.retry.backoff";
const INITIAL: &str = "tasks.retry.initial";
const MULTIPLIER: &str = "tasks.retry.multiplier";
const MAX_DELAY: &str = "tasks.retry.max-delay";
const JITTER: &str = "tasks.retry.jitter";

impl RetryDefaults {
    /// Reads the retry defaults from the `tasks.retry` keys of `config`.
    ///
    /// The keys are `tasks.retry.max-attempts` (default 3, at least 1), `tasks.retry.backoff`
    /// (`fixed` or `exponential`, default `exponential`), `tasks.retry.initial` (default
    /// 100 ms), `tasks.retry.multiplier` (default 2.0, finite and at least 1.0),
    /// `tasks.retry.max-delay` (default 30 seconds) and `tasks.retry.jitter` (`none`, `full`
    /// or `equal`, default `full`). Durations take the configuration's duration form, such as
    /// `250ms` or `2s`. A missing section gives [`RetryDefaults::default`]. The
    /// `tasks.scheduling` section is read by [`TasksSettings::from_config`]; here only its key
    /// names are validated and its values are not read.
    ///
    /// [`TasksSettings::from_config`]: crate::TasksSettings::from_config
    ///
    /// # Errors
    ///
    /// Fails with a [`ConfigError`] naming the key for an unknown key under `tasks`,
    /// `tasks.retry` or `tasks.scheduling`, a value of the wrong type or a bad duration, a `max-attempts` of zero, a
    /// `multiplier` below 1.0 or not finite, an `initial` greater than `max-delay`, and an
    /// unknown `backoff` or `jitter` name. The error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_tasks::{Backoff, RetryDefaults};
    ///
    /// let config = Config::from_str("[tasks.retry]\nmax-attempts = 5\nbackoff = \"fixed\"\ninitial = \"250ms\"")?;
    /// let defaults = RetryDefaults::from_config(&config)?;
    /// assert_eq!(defaults.max_attempts, 5);
    /// assert_eq!(defaults.backoff, Backoff::Fixed);
    /// assert_eq!(defaults.initial, Duration::from_millis(250));
    ///
    /// let config = Config::from_str("[tasks.retry]\nmax-attempts = 0")?;
    /// let err = RetryDefaults::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("tasks.retry.max-attempts"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<RetryDefaults, ConfigError> {
        let raw = match config.get::<RawTasks>("tasks") {
            Ok(raw) => raw.retry,
            Err(ConfigError::Missing { .. }) => RawRetry::default(),
            Err(e) => return Err(e),
        };
        let defaults = RetryDefaults::default();
        let max_attempts = match raw.max_attempts {
            Some(0) => return Err(ConfigError::invalid(MAX_ATTEMPTS, "must be at least 1")),
            Some(n) => n,
            None => defaults.max_attempts,
        };
        let backoff = match raw.backoff {
            Some(name) => name
                .parse::<Backoff>()
                .map_err(|_| ConfigError::invalid(BACKOFF, "must be `fixed` or `exponential`"))?,
            None => defaults.backoff,
        };
        let jitter = match raw.jitter {
            Some(name) => name
                .parse::<Jitter>()
                .map_err(|_| ConfigError::invalid(JITTER, "must be `none`, `full` or `equal`"))?,
            None => defaults.jitter,
        };
        let multiplier = match raw.multiplier {
            Some(m) if !m.is_finite() || m < 1.0 => {
                return Err(ConfigError::invalid(
                    MULTIPLIER,
                    "must be a finite number of at least 1.0",
                ));
            }
            Some(m) => m,
            None => defaults.multiplier,
        };
        let initial = raw.initial.unwrap_or(defaults.initial);
        let max_delay = raw.max_delay.unwrap_or(defaults.max_delay);
        if initial > max_delay {
            // Name the key the user actually set: `max-delay` when `initial` is the default.
            let key = if raw.initial.is_some() {
                INITIAL
            } else {
                MAX_DELAY
            };
            return Err(ConfigError::invalid(
                key,
                "initial must not be greater than max-delay",
            ));
        }
        Ok(RetryDefaults {
            max_attempts,
            backoff,
            initial,
            multiplier,
            max_delay,
            jitter,
        })
    }
}
