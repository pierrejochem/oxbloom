//! A source of the current time that tests can replace.

use std::sync::Arc;
use std::time::SystemTime;

/// A source of the current wall-clock time.
///
/// Components that need the time take a `Clock` so that tests can supply a fixed or
/// manually advanced time instead of the system time.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, SystemTime};
/// use oxbloom_core::Clock;
///
/// struct AtEpochPlusTen;
///
/// impl Clock for AtEpochPlusTen {
///     fn now(&self) -> SystemTime {
///         SystemTime::UNIX_EPOCH + Duration::from_secs(10)
///     }
/// }
///
/// assert_eq!(
///     AtEpochPlusTen.now().duration_since(SystemTime::UNIX_EPOCH).unwrap(),
///     Duration::from_secs(10)
/// );
/// ```
pub trait Clock: Send + Sync + 'static {
    /// Returns the current time according to this clock.
    fn now(&self) -> SystemTime;
}

/// The clock that reads the operating system time.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
/// use oxbloom_core::{Clock, SystemClock};
///
/// let before = SystemTime::now();
/// let read = SystemClock.now();
/// assert!(read >= before);
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now(&self) -> SystemTime {
        (**self).now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_reads_the_system_time() {
        let before = SystemTime::now();
        let read = SystemClock.now();
        let after = SystemTime::now();
        assert!(before <= read && read <= after);
    }

    #[test]
    fn an_arc_of_a_clock_is_a_clock() {
        fn read(clock: impl Clock) -> SystemTime {
            clock.now()
        }
        let before = SystemTime::now();
        let dynamic: Arc<dyn Clock> = Arc::new(SystemClock);
        let concrete = Arc::new(SystemClock);
        assert!(read(dynamic) >= before);
        assert!(read(concrete) >= before);
    }
}
