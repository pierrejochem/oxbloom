use std::collections::BTreeMap;

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
