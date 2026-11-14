use std::sync::atomic::{AtomicU8, Ordering};

/// Whether the application is internally correct.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::{Availability, Liveness};
///
/// assert_eq!(Availability::new().liveness(), Liveness::Correct);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    /// The application works.
    Correct,
    /// The application is broken and only a restart can fix it.
    Broken,
}

/// Whether the application accepts traffic.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::{Availability, Readiness};
///
/// assert_eq!(Availability::new().readiness(), Readiness::RefusingTraffic);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// The application takes requests.
    AcceptingTraffic,
    /// The application does not take requests, for example while starting or stopping.
    RefusingTraffic,
}

const CORRECT: u8 = 0;
const BROKEN: u8 = 1;
const ACCEPTING: u8 = 0;
const REFUSING: u8 = 1;

/// The liveness and readiness states of the application, shared without locks.
///
/// A new value is `Correct` and `RefusingTraffic`. Liveness only goes from `Correct` to
/// `Broken`.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::{Availability, Liveness, Readiness};
///
/// let availability = Availability::new();
/// availability.set_readiness(Readiness::AcceptingTraffic);
/// assert_eq!(availability.readiness(), Readiness::AcceptingTraffic);
/// availability.break_liveness();
/// assert_eq!(availability.liveness(), Liveness::Broken);
/// ```
#[derive(Debug)]
pub struct Availability {
    liveness: AtomicU8,
    readiness: AtomicU8,
}

impl Availability {
    /// Creates the states `Correct` and `RefusingTraffic`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Availability, Liveness, Readiness};
    ///
    /// let availability = Availability::new();
    /// assert_eq!(availability.liveness(), Liveness::Correct);
    /// assert_eq!(availability.readiness(), Readiness::RefusingTraffic);
    /// ```
    pub fn new() -> Self {
        Self {
            liveness: AtomicU8::new(CORRECT),
            readiness: AtomicU8::new(REFUSING),
        }
    }

    /// Returns the liveness state.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Availability, Liveness};
    ///
    /// assert_eq!(Availability::new().liveness(), Liveness::Correct);
    /// ```
    pub fn liveness(&self) -> Liveness {
        match self.liveness.load(Ordering::Acquire) {
            CORRECT => Liveness::Correct,
            _ => Liveness::Broken,
        }
    }

    /// Returns the readiness state.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Availability, Readiness};
    ///
    /// assert_eq!(Availability::new().readiness(), Readiness::RefusingTraffic);
    /// ```
    pub fn readiness(&self) -> Readiness {
        match self.readiness.load(Ordering::Acquire) {
            ACCEPTING => Readiness::AcceptingTraffic,
            _ => Readiness::RefusingTraffic,
        }
    }

    /// Marks the application as broken. There is no way back.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Availability, Liveness};
    ///
    /// let availability = Availability::new();
    /// availability.break_liveness();
    /// assert_eq!(availability.liveness(), Liveness::Broken);
    /// ```
    pub fn break_liveness(&self) {
        self.liveness.store(BROKEN, Ordering::Release);
    }

    /// Sets the readiness state.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{Availability, Readiness};
    ///
    /// let availability = Availability::new();
    /// availability.set_readiness(Readiness::AcceptingTraffic);
    /// assert_eq!(availability.readiness(), Readiness::AcceptingTraffic);
    /// ```
    pub fn set_readiness(&self, readiness: Readiness) {
        let value = match readiness {
            Readiness::AcceptingTraffic => ACCEPTING,
            Readiness::RefusingTraffic => REFUSING,
        };
        self.readiness.store(value, Ordering::Release);
    }
}

impl Default for Availability {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_correct_and_refusing() {
        let a = Availability::new();
        assert_eq!(a.liveness(), Liveness::Correct);
        assert_eq!(a.readiness(), Readiness::RefusingTraffic);
    }

    #[test]
    fn readiness_flips() {
        let a = Availability::new();
        a.set_readiness(Readiness::AcceptingTraffic);
        assert_eq!(a.readiness(), Readiness::AcceptingTraffic);
        a.set_readiness(Readiness::RefusingTraffic);
        assert_eq!(a.readiness(), Readiness::RefusingTraffic);
    }

    #[test]
    fn break_liveness_sticks() {
        let a = Availability::new();
        a.break_liveness();
        assert_eq!(a.liveness(), Liveness::Broken);
        a.set_readiness(Readiness::AcceptingTraffic);
        assert_eq!(a.liveness(), Liveness::Broken);
    }
}
