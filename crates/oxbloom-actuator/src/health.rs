use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

/// The status of a component, ordered from best to worst: `Up`, `Unknown`, `OutOfService`,
/// `Down`.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::Status;
///
/// assert_eq!(Status::Down.as_str(), "DOWN");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// The component works.
    Up,
    /// The component failed.
    Down,
    /// The component is deliberately out of service.
    OutOfService,
    /// The state of the component is not known.
    Unknown,
}

impl Status {
    fn severity(self) -> u8 {
        match self {
            Status::Up => 0,
            Status::Unknown => 1,
            Status::OutOfService => 2,
            Status::Down => 3,
        }
    }

    /// Returns the worst status of `statuses`, or `Up` when there are none.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::Status;
    ///
    /// assert_eq!(Status::worst([Status::Up, Status::OutOfService]), Status::OutOfService);
    /// assert_eq!(Status::worst([]), Status::Up);
    /// ```
    pub fn worst(statuses: impl IntoIterator<Item = Status>) -> Status {
        statuses
            .into_iter()
            .max_by_key(|s| s.severity())
            .unwrap_or(Status::Up)
    }

    /// Returns the wire name: `UP`, `DOWN`, `OUT_OF_SERVICE` or `UNKNOWN`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::Status;
    ///
    /// assert_eq!(Status::OutOfService.as_str(), "OUT_OF_SERVICE");
    /// ```
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Up => "UP",
            Status::Down => "DOWN",
            Status::OutOfService => "OUT_OF_SERVICE",
            Status::Unknown => "UNKNOWN",
        }
    }

    /// Returns whether the status is `Up`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::Status;
    ///
    /// assert!(Status::Up.is_up());
    /// assert!(!Status::Unknown.is_up());
    /// ```
    pub fn is_up(&self) -> bool {
        matches!(self, Status::Up)
    }
}

/// A status with named details.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::{Health, Status};
///
/// let health = Health::down().detail("error", "connection refused");
/// assert_eq!(health.status, Status::Down);
/// assert_eq!(health.details["error"], "connection refused");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    /// The status.
    pub status: Status,
    /// Extra facts about the status, ordered by key.
    pub details: BTreeMap<String, serde_json::Value>,
}

impl Health {
    /// Creates a health with `status` and no details.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Health, Status};
    ///
    /// assert_eq!(Health::new(Status::Unknown).status, Status::Unknown);
    /// ```
    pub fn new(status: Status) -> Self {
        Self {
            status,
            details: BTreeMap::new(),
        }
    }

    /// Creates an `Up` health.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Health, Status};
    ///
    /// assert_eq!(Health::up().status, Status::Up);
    /// ```
    pub fn up() -> Self {
        Self::new(Status::Up)
    }

    /// Creates a `Down` health.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Health, Status};
    ///
    /// assert_eq!(Health::down().status, Status::Down);
    /// ```
    pub fn down() -> Self {
        Self::new(Status::Down)
    }

    /// Adds a detail, replacing an earlier one with the same key.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::Health;
    ///
    /// let health = Health::up().detail("connections", 3);
    /// assert_eq!(health.details["connections"], 3);
    /// ```
    pub fn detail(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }
}

/// Reports the health of one dependency.
///
/// # Examples
///
/// ```
/// use async_trait::async_trait;
/// use oxbloom_actuator::{Health, HealthIndicator};
///
/// struct Database;
///
/// #[async_trait]
/// impl HealthIndicator for Database {
///     fn name(&self) -> &str {
///         "db"
///     }
///
///     async fn health(&self) -> Health {
///         Health::up().detail("database", "postgres")
///     }
/// }
///
/// assert_eq!(Database.name(), "db");
/// ```
#[async_trait]
pub trait HealthIndicator: Send + Sync {
    /// The name of the component in the health report.
    fn name(&self) -> &str;

    /// Checks the component.
    async fn health(&self) -> Health;
}

/// Runs health indicators concurrently, each under a timeout, and combines the results.
///
/// An indicator that exceeds the timeout or panics is reported `Down` with an `error`
/// detail of `timed out` or `panicked`. Each indicator runs in its own task, so a panic
/// never reaches the caller. Indicators with the same name get the keys `name`, `name#2`,
/// `name#3` and so on, and every one counts towards the overall status.
///
/// # Examples
///
/// ```
/// # tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
/// use std::sync::Arc;
/// use std::time::Duration;
/// use async_trait::async_trait;
/// use oxbloom_actuator::{Health, HealthIndicator, HealthRunner, Status};
///
/// struct Database;
///
/// #[async_trait]
/// impl HealthIndicator for Database {
///     fn name(&self) -> &str { "db" }
///     async fn health(&self) -> Health { Health::up() }
/// }
///
/// let runner = HealthRunner::new(vec![Arc::new(Database)], Duration::from_secs(1));
/// let (status, parts) = runner.run(None).await;
/// assert_eq!(status, Status::Up);
/// assert_eq!(parts["db"].status, Status::Up);
/// # });
/// ```
pub struct HealthRunner {
    indicators: Vec<Arc<dyn HealthIndicator>>,
    timeout: Duration,
}

impl HealthRunner {
    /// Creates a runner for `indicators` with a per-indicator `timeout`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_actuator::HealthRunner;
    ///
    /// let _runner = HealthRunner::new(vec![], Duration::from_secs(5));
    /// ```
    pub fn new(indicators: Vec<Arc<dyn HealthIndicator>>, timeout: Duration) -> Self {
        Self {
            indicators,
            timeout,
        }
    }

    /// Runs the indicators, or only those named in `only`, and returns the worst status and
    /// each indicator's health by name.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
    /// use std::time::Duration;
    /// use oxbloom_actuator::{HealthRunner, Status};
    ///
    /// let runner = HealthRunner::new(vec![], Duration::from_secs(1));
    /// let (status, parts) = runner.run(None).await;
    /// assert_eq!(status, Status::Up);
    /// assert!(parts.is_empty());
    /// # });
    /// ```
    pub async fn run(&self, only: Option<&[String]>) -> (Status, BTreeMap<String, Health>) {
        let handles: Vec<_> =
            self.indicators
                .iter()
                .filter_map(|indicator| {
                    let name = indicator.name().to_string();
                    if only.is_some_and(|names| !names.contains(&name)) {
                        return None;
                    }
                    let indicator = Arc::clone(indicator);
                    let timeout = self.timeout;
                    let handle = tokio::spawn(async move {
                        tokio::time::timeout(timeout, indicator.health()).await
                    });
                    Some((name, handle))
                })
                .collect();
        let mut parts = BTreeMap::new();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        let mut status = Status::Up;
        for (name, handle) in handles {
            let health = match handle.await {
                Ok(Ok(health)) => health,
                Ok(Err(_elapsed)) => Health::down().detail("error", "timed out"),
                Err(e) if e.is_panic() => Health::down().detail("error", "panicked"),
                Err(_) => Health::down().detail("error", "cancelled"),
            };
            status = Status::worst([status, health.status]);
            let count = counts.entry(name.clone()).or_insert(0);
            *count += 1;
            let key = if *count == 1 {
                name
            } else {
                format!("{name}#{count}")
            };
            parts.insert(key, health);
        }
        (status, parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worst_orders_down_over_out_of_service_over_unknown_over_up() {
        let order = [
            Status::Up,
            Status::Unknown,
            Status::OutOfService,
            Status::Down,
        ];
        for (i, a) in order.iter().enumerate() {
            for (j, b) in order.iter().enumerate() {
                let expected = order[i.max(j)];
                assert_eq!(Status::worst([*a, *b]), expected, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn empty_is_up() {
        assert_eq!(Status::worst([]), Status::Up);
    }

    #[test]
    fn as_str_values() {
        assert_eq!(Status::Up.as_str(), "UP");
        assert_eq!(Status::Down.as_str(), "DOWN");
        assert_eq!(Status::OutOfService.as_str(), "OUT_OF_SERVICE");
        assert_eq!(Status::Unknown.as_str(), "UNKNOWN");
    }
}
