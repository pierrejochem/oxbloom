//! `TasksSettings`: the `tasks.scheduling.*` keys.

use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;

use crate::scheduler::SchedulerConfig;
use crate::trigger::Zone;

/// The `tasks.scheduling.*` settings of the task scheduler.
///
/// Read with [`from_config`](TasksSettings::from_config). The keys are
/// `tasks.scheduling.enabled` (default `true`), `tasks.scheduling.max-concurrent` (default 8,
/// at least 1), `tasks.scheduling.shutdown-timeout` (default 10 seconds),
/// `tasks.scheduling.zone` (default `UTC`, or an IANA zone name) and
/// `tasks.scheduling.initial-delay` (default 0). Durations take the configuration's duration
/// form, such as `500ms`, `1.5s` or `2m`. A key that is not listed, directly under `tasks` or
/// under `tasks.scheduling`, is a [`ConfigError`] that names the key, as under `management`.
/// No error shows a configured value.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_config::Config;
/// use oxbloom_tasks::TasksSettings;
///
/// let config = Config::from_str("[tasks.scheduling]\nmax-concurrent = 2\ninitial-delay = \"5s\"")?;
/// let settings = TasksSettings::from_config(&config)?;
/// assert_eq!(settings.max_concurrent, 2);
/// assert_eq!(settings.initial_delay, Duration::from_secs(5));
/// assert!(settings.enabled);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TasksSettings {
    /// `tasks.scheduling.enabled`. Default `true`. This type only records the key; boot's
    /// tasks auto-configuration reads it and registers nothing when it is not `true`.
    pub enabled: bool,
    /// `tasks.scheduling.max-concurrent`: how many runs execute at the same time across all
    /// tasks. Default 8; zero is rejected.
    pub max_concurrent: usize,
    /// `tasks.scheduling.shutdown-timeout`: how long the scheduler waits for runs in flight
    /// when it stops. Default 10 seconds.
    pub shutdown_timeout: Duration,
    /// `tasks.scheduling.zone`: the zone of cron triggers without a `zone` of their own.
    /// Default UTC.
    pub zone: Zone,
    /// `tasks.scheduling.initial-delay`: the initial delay of tasks without one of their own.
    /// Default zero.
    pub initial_delay: Duration,
}

impl Default for TasksSettings {
    /// The defaults: enabled, eight concurrent runs, a ten second shutdown timeout, UTC and
    /// no initial delay.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::{TasksSettings, Zone};
    ///
    /// let settings = TasksSettings::default();
    /// assert!(settings.enabled);
    /// assert_eq!(settings.zone, Zone::Utc);
    /// ```
    fn default() -> Self {
        let config = SchedulerConfig::default();
        TasksSettings {
            enabled: true,
            max_concurrent: config.max_concurrent,
            shutdown_timeout: config.shutdown_timeout,
            zone: config.zone,
            initial_delay: config.initial_delay,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawTasks {
    scheduling: RawScheduling,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawScheduling {
    enabled: Option<bool>,
    max_concurrent: Option<usize>,
    shutdown_timeout: Option<Duration>,
    zone: Option<String>,
    initial_delay: Option<Duration>,
}

const MAX_CONCURRENT: &str = "tasks.scheduling.max-concurrent";
const ZONE: &str = "tasks.scheduling.zone";

impl TasksSettings {
    /// Reads the settings from the `tasks` keys of `config`.
    ///
    /// A missing `tasks` section gives the defaults.
    ///
    /// # Errors
    ///
    /// Fails with a [`ConfigError`] naming the key for an unknown key under `tasks` or
    /// `tasks.scheduling`, a value of the wrong type or a bad duration, a `max-concurrent` of
    /// zero, and a `zone` that is neither `UTC` nor an IANA zone name. The error never shows
    /// the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_tasks::TasksSettings;
    ///
    /// let config = Config::from_str("[tasks.scheduling]\nzone = \"Atlantis/Capital\"")?;
    /// let err = TasksSettings::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("tasks.scheduling.zone"));
    /// assert!(!err.to_string().contains("Atlantis"));
    ///
    /// let config = Config::from_str("[tasks]\nschedule = true")?;
    /// assert!(TasksSettings::from_config(&config).unwrap_err().to_string().contains("schedule"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<TasksSettings, ConfigError> {
        let raw = match config.get::<RawTasks>("tasks") {
            Ok(raw) => raw.scheduling,
            Err(ConfigError::Missing { .. }) => RawScheduling::default(),
            Err(e) => return Err(e),
        };
        let defaults = TasksSettings::default();
        let max_concurrent = match raw.max_concurrent {
            Some(0) => {
                return Err(ConfigError::invalid(MAX_CONCURRENT, "must be at least 1"));
            }
            Some(n) => n,
            None => defaults.max_concurrent,
        };
        let zone = match raw.zone {
            Some(name) => {
                Zone::parse(&name).map_err(|error| ConfigError::invalid(ZONE, error.to_string()))?
            }
            None => defaults.zone,
        };
        Ok(TasksSettings {
            enabled: raw.enabled.unwrap_or(defaults.enabled),
            max_concurrent,
            shutdown_timeout: raw.shutdown_timeout.unwrap_or(defaults.shutdown_timeout),
            zone,
            initial_delay: raw.initial_delay.unwrap_or(defaults.initial_delay),
        })
    }

    /// The [`SchedulerConfig`] of these settings.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::TasksSettings;
    ///
    /// let settings = TasksSettings { shutdown_timeout: Duration::from_secs(3), ..TasksSettings::default() };
    /// assert_eq!(settings.scheduler_config().shutdown_timeout, Duration::from_secs(3));
    /// ```
    pub fn scheduler_config(&self) -> SchedulerConfig {
        SchedulerConfig {
            max_concurrent: self.max_concurrent,
            shutdown_timeout: self.shutdown_timeout,
            zone: self.zone,
            initial_delay: self.initial_delay,
        }
    }
}
