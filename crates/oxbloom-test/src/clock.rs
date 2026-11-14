use oxbloom_core::Clock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// A clock that always reports the same time.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use oxbloom_core::Clock;
/// use oxbloom_test::FixedClock;
///
/// let at = SystemTime::UNIX_EPOCH + Duration::from_secs(60);
/// assert_eq!(FixedClock::at(at).now(), at);
/// ```
#[derive(Clone, Debug)]
pub struct FixedClock(SystemTime);

impl FixedClock {
    /// Creates a clock fixed at `time`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_core::Clock;
    /// let clock = oxbloom_test::FixedClock::at(std::time::SystemTime::UNIX_EPOCH);
    /// assert_eq!(clock.now(), std::time::SystemTime::UNIX_EPOCH);
    /// ```
    pub fn at(time: SystemTime) -> FixedClock {
        FixedClock(time)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// A clock that moves only when told to.
///
/// Clones share one time, so advancing through one clone is visible through every other.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use oxbloom_core::Clock;
/// use oxbloom_test::ManualClock;
///
/// let clock = ManualClock::at(SystemTime::UNIX_EPOCH);
/// let other = clock.clone();
/// clock.advance(Duration::from_secs(30));
/// assert_eq!(other.now(), SystemTime::UNIX_EPOCH + Duration::from_secs(30));
/// ```
#[derive(Clone, Debug)]
pub struct ManualClock(Arc<Mutex<SystemTime>>);

impl ManualClock {
    /// Creates a clock that starts at `time`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_core::Clock;
    /// let clock = oxbloom_test::ManualClock::at(std::time::SystemTime::UNIX_EPOCH);
    /// assert_eq!(clock.now(), std::time::SystemTime::UNIX_EPOCH);
    /// ```
    pub fn at(time: SystemTime) -> ManualClock {
        ManualClock(Arc::new(Mutex::new(time)))
    }

    /// Moves the clock forward by `by`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use oxbloom_core::Clock;
    /// let clock = oxbloom_test::ManualClock::at(SystemTime::UNIX_EPOCH);
    /// clock.advance(Duration::from_secs(1));
    /// assert_eq!(clock.now(), SystemTime::UNIX_EPOCH + Duration::from_secs(1));
    /// ```
    pub fn advance(&self, by: Duration) {
        let mut time = self.0.lock().unwrap_or_else(|p| p.into_inner());
        *time += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxbloom_core::Clock;
    use std::time::{Duration, SystemTime};

    #[test]
    fn a_fixed_clock_never_moves() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        let c = FixedClock::at(t);
        assert_eq!(c.now(), t);
        assert_eq!(c.clone().now(), t);
    }

    #[test]
    fn a_manual_clock_advances_and_clones_share_it() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(5);
        let a = ManualClock::at(t);
        let b = a.clone();
        a.advance(Duration::from_secs(7));
        assert_eq!(b.now(), t + Duration::from_secs(7));
    }
}
