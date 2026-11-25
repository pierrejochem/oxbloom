//! The health indicator of a data source.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use oxbloom_data::{DataError, DataSources};

use crate::{Health, HealthIndicator, Status};

/// How long the check waits for the `SELECT 1`.
const TIMEOUT: Duration = Duration::from_secs(3);
/// The text of a failed query, an unknown source or a closed pool.
const CONNECTION_FAILED: &str = "connection failed";
/// The text of a check that ran into its timeout.
const TIMED_OUT: &str = "timed out";

/// Reports one source of a [`DataSources`] registry as the component `db` (the default source)
/// or `db.<name>` (any other source).
///
/// The check runs `SELECT 1` within 3 seconds (see [`with_timeout`](Self::with_timeout)). When
/// it succeeds the status is `Up` and the details are `driver` (`postgres`, `mysql` or
/// `sqlite`), `size` (the open connections) and `idle`. When it fails the status is `Down` and
/// the details are `error` and `driver`. The `error` is always one of two fixed texts:
/// `timed out` when the check used its whole timeout, and `connection failed` otherwise (a
/// failed query, a closed pool or an unknown source). The registry cannot tell a timeout from a
/// closed pool by its error, so the indicator measures the time the check took. The report
/// never contains the URL, a file path, a host, a user or the text of a driver error.
///
/// Available with the cargo feature `data`, which needs no driver of its own.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::Arc;
/// use oxbloom_actuator::{DataHealthIndicator, HealthIndicator, Status};
/// use oxbloom_config::Config;
/// use oxbloom_data::{DataError, DataSettings, DataSources};
///
/// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
/// let settings = DataSettings::from_config(&config)?;
/// let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls))?);
/// let indicator = DataHealthIndicator::new(sources, "default");
/// assert_eq!(indicator.name(), "db");
/// let health = indicator.health().await;
/// assert_eq!(health.status, Status::Up);
/// assert_eq!(health.details["driver"], "sqlite");
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
pub struct DataHealthIndicator {
    sources: Arc<DataSources>,
    source: String,
    name: String,
    timeout: Duration,
}

impl DataHealthIndicator {
    /// Creates the indicator of the source `name` of `sources`.
    ///
    /// The indicator is called `db` when `name` is the default source and `db.<name>`
    /// otherwise. An unknown `name` does not panic: the indicator reports `Down` with
    /// `connection failed`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use oxbloom_actuator::{DataHealthIndicator, HealthIndicator};
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    /// let _guard = runtime.enter();
    /// let config = Config::from_str(
    ///     "[data]\nurl = \"sqlite::memory:\"\n[data.sources.replica]\nurl = \"sqlite::memory:\"",
    /// )?;
    /// let settings = DataSettings::from_config(&config)?;
    /// let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls))?);
    /// assert_eq!(DataHealthIndicator::new(sources.clone(), "default").name(), "db");
    /// assert_eq!(DataHealthIndicator::new(sources, "replica").name(), "db.replica");
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn new(sources: Arc<DataSources>, name: &str) -> Self {
        let component = if sources.default_name() == Some(name) {
            "db".to_owned()
        } else {
            format!("db.{name}")
        };
        Self {
            sources,
            source: name.to_owned(),
            name: component,
            timeout: TIMEOUT,
        }
    }

    /// Replaces the timeout of the check (3 seconds by default).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_actuator::{DataHealthIndicator, HealthIndicator, Status};
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls))?);
    /// let indicator = DataHealthIndicator::new(sources, "default").with_timeout(Duration::ZERO);
    /// // A pool that is already connected answers within any timeout, a zero one included.
    /// let health = indicator.health().await;
    /// assert!(matches!(health.status, Status::Up | Status::Down));
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// The fixed text of a failed check: `timed out` for a pool error after the whole timeout,
/// `connection failed` for every other failure.
fn failure_text(error: &DataError, elapsed: Duration, timeout: Duration) -> &'static str {
    if matches!(error, DataError::Pool) && elapsed >= timeout {
        TIMED_OUT
    } else {
        CONNECTION_FAILED
    }
}

#[async_trait]
impl HealthIndicator for DataHealthIndicator {
    fn name(&self) -> &str {
        &self.name
    }

    async fn health(&self) -> Health {
        let started = Instant::now();
        let result = self.sources.ping(&self.source, self.timeout).await;
        let driver = self.sources.driver_of(&self.source).map(|d| d.to_string());
        let mut health = match result {
            Ok(()) => {
                let mut health = Health::new(Status::Up);
                if let Some((size, idle)) = self.sources.pool_counts(&self.source) {
                    health = health.detail("size", size).detail("idle", idle);
                }
                health
            }
            Err(error) => {
                let text = failure_text(&error, started.elapsed(), self.timeout);
                Health::new(Status::Down).detail("error", text)
            }
        };
        if let Some(driver) = driver {
            health = health.detail("driver", driver);
        }
        health
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pool_error_after_the_whole_timeout_is_timed_out() {
        let t = Duration::from_secs(3);
        assert_eq!(failure_text(&DataError::Pool, t, t), "timed out");
        assert_eq!(failure_text(&DataError::Pool, t * 2, t), "timed out");
    }

    #[test]
    fn every_other_failure_is_connection_failed() {
        let t = Duration::from_secs(3);
        let quick = Duration::from_millis(5);
        assert_eq!(
            failure_text(&DataError::Pool, quick, t),
            "connection failed"
        );
        assert_eq!(
            failure_text(&DataError::Connect, t * 2, t),
            "connection failed"
        );
        assert_eq!(
            failure_text(&DataError::Config, quick, t),
            "connection failed"
        );
    }
}
