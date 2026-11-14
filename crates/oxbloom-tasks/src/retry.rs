//! Retry: a policy that runs an async operation again when it fails, with fixed or
//! exponential backoff and jitter, and the process-wide defaults behind `#[retryable]`.

use std::fmt;
use std::future::Future;
use std::str::FromStr;
use std::sync::{PoisonError, RwLock};
use std::time::Duration;

/// The tracing target of retry events, shared with the scheduler.
const TARGET: &str = "oxbloom::tasks";

/// The built-in values: three attempts, exponential backoff from 100 ms doubling up to 30 s,
/// and full jitter.
const BUILT_IN: RetryDefaults = RetryDefaults {
    max_attempts: 3,
    backoff: Backoff::Exponential,
    initial: Duration::from_millis(100),
    multiplier: 2.0,
    max_delay: Duration::from_secs(30),
    jitter: Jitter::Full,
};

/// The process-wide defaults read by [`defaults`].
static DEFAULTS: RwLock<RetryDefaults> = RwLock::new(BUILT_IN);

/// An error parsing a [`Backoff`] or a [`Jitter`] name.
///
/// The message lists the accepted names but never repeats the offending value, so a value
/// that came from a secret or a mistyped property does not leak into logs.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::Backoff;
///
/// let err = "linear".parse::<Backoff>().unwrap_err();
/// assert_eq!(err.to_string(), "unknown backoff, expected fixed or exponential");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryError {
    message: &'static str,
}

impl RetryError {
    const fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl fmt::Display for RetryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for RetryError {}

/// How the delay between attempts grows.
///
/// The names are `fixed` and `exponential`, as in the `backoff` argument of `#[retryable]`
/// and the `tasks.retry.backoff` key.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::Backoff;
///
/// assert_eq!("fixed".parse::<Backoff>()?, Backoff::Fixed);
/// assert_eq!(Backoff::Exponential.to_string(), "exponential");
/// # Ok::<(), oxbloom_tasks::RetryError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backoff {
    /// Every retry waits `initial`.
    Fixed,
    /// Retry *n* (from 1) waits `initial * multiplier^(n-1)`, the built-in default.
    Exponential,
}

impl fmt::Display for Backoff {
    /// Writes the lowercase name, `fixed` or `exponential`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(oxbloom_tasks::Backoff::Fixed.to_string(), "fixed");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Backoff::Fixed => "fixed",
            Backoff::Exponential => "exponential",
        })
    }
}

impl FromStr for Backoff {
    type Err = RetryError;

    /// Parses `fixed` or `exponential`, lowercase, ignoring surrounding whitespace.
    ///
    /// # Errors
    ///
    /// Returns a [`RetryError`] for any other name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::Backoff;
    ///
    /// assert_eq!("exponential".parse::<Backoff>()?, Backoff::Exponential);
    /// assert!("Exponential".parse::<Backoff>().is_err());
    /// # Ok::<(), oxbloom_tasks::RetryError>(())
    /// ```
    fn from_str(name: &str) -> Result<Backoff, RetryError> {
        match name.trim() {
            "fixed" => Ok(Backoff::Fixed),
            "exponential" => Ok(Backoff::Exponential),
            _ => Err(RetryError::new(
                "unknown backoff, expected fixed or exponential",
            )),
        }
    }
}

/// How randomness spreads the delay between attempts, so that many callers that failed
/// together do not retry together.
///
/// The names are `none`, `full` and `equal`, as in the `jitter` argument of `#[retryable]`
/// and the `tasks.retry.jitter` key.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::Jitter;
///
/// assert_eq!("equal".parse::<Jitter>()?, Jitter::Equal);
/// assert_eq!(Jitter::Full.to_string(), "full");
/// # Ok::<(), oxbloom_tasks::RetryError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jitter {
    /// The delay as computed by the backoff.
    None,
    /// A random value between zero and the delay, the built-in default.
    Full,
    /// Half the delay plus a random value up to the other half.
    Equal,
}

impl fmt::Display for Jitter {
    /// Writes the lowercase name, `none`, `full` or `equal`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(oxbloom_tasks::Jitter::None.to_string(), "none");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Jitter::None => "none",
            Jitter::Full => "full",
            Jitter::Equal => "equal",
        })
    }
}

impl FromStr for Jitter {
    type Err = RetryError;

    /// Parses `none`, `full` or `equal`, lowercase, ignoring surrounding whitespace.
    ///
    /// # Errors
    ///
    /// Returns a [`RetryError`] for any other name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::Jitter;
    ///
    /// assert_eq!("none".parse::<Jitter>()?, Jitter::None);
    /// assert!("random".parse::<Jitter>().is_err());
    /// # Ok::<(), oxbloom_tasks::RetryError>(())
    /// ```
    fn from_str(name: &str) -> Result<Jitter, RetryError> {
        match name.trim() {
            "none" => Ok(Jitter::None),
            "full" => Ok(Jitter::Full),
            "equal" => Ok(Jitter::Equal),
            _ => Err(RetryError::new(
                "unknown jitter, expected none, full or equal",
            )),
        }
    }
}

/// When and how often an async operation is run again after it fails.
///
/// [`run`](RetryPolicy::run) calls the operation up to `max_attempts` times. Before retry
/// *n* (from 1) it sleeps [`delay(n, ..)`](RetryPolicy::delay) with `tokio::time::sleep`, so
/// tests with paused tokio time work. When the attempts run out, the last error is
/// returned. A panic is never retried and is not caught.
///
/// [`RetryPolicy::default`] holds the built-in values, not the process-wide [`defaults`];
/// use `defaults().policy()` for those.
///
/// # Examples
///
/// ```
/// use std::cell::Cell;
/// use std::time::Duration;
/// use oxbloom_tasks::{Jitter, RetryPolicy};
///
/// let policy = RetryPolicy { max_attempts: 4, jitter: Jitter::None, ..RetryPolicy::default() };
/// assert_eq!(policy.delay(3, 0.0), Duration::from_millis(400));
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build()?.block_on(async {
/// let calls = Cell::new(0);
/// let result = policy
///     .run(
///         "fetch",
///         || {
///             calls.set(calls.get() + 1);
///             let n = calls.get();
///             async move { if n < 3 { Err("unavailable") } else { Ok(n) } }
///         },
///         |_| true,
///     )
///     .await;
/// assert_eq!(result, Ok(3));
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RetryPolicy {
    /// How many times the operation runs at most, counting the first call. Zero behaves as
    /// one. Built-in default 3.
    pub max_attempts: u32,
    /// How the delay grows. Built-in default [`Backoff::Exponential`].
    pub backoff: Backoff,
    /// The delay before the first retry, before jitter. Built-in default 100 ms.
    pub initial: Duration,
    /// The factor of exponential backoff, ignored by fixed backoff. Built-in default 2.0.
    pub multiplier: f64,
    /// The cap on every delay. Built-in default 30 seconds.
    pub max_delay: Duration,
    /// How randomness spreads the delay. Built-in default [`Jitter::Full`].
    pub jitter: Jitter,
}

impl Default for RetryPolicy {
    /// The built-in values: three attempts, exponential backoff from 100 ms with multiplier
    /// 2.0 up to 30 seconds, and full jitter.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::{Jitter, RetryPolicy};
    ///
    /// let policy = RetryPolicy::default();
    /// assert_eq!(policy.max_attempts, 3);
    /// assert_eq!(policy.jitter, Jitter::Full);
    /// ```
    fn default() -> Self {
        BUILT_IN.policy()
    }
}

impl RetryPolicy {
    /// The delay before retry `retry_number` (from 1; zero counts as 1), given a random
    /// value `unit_random`, normally in `[0, 1)`.
    ///
    /// The backoff delay is computed in seconds as a float and capped at `max_delay` before
    /// it becomes a [`Duration`], so no input panics or overflows. Full jitter multiplies
    /// it by `unit_random`; equal jitter takes half of it plus `unit_random` times the other
    /// half. The `unit_random` is clamped to `[0, 1]`, and NaN counts as zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::{Backoff, Jitter, RetryPolicy};
    ///
    /// let policy = RetryPolicy { jitter: Jitter::None, ..RetryPolicy::default() };
    /// assert_eq!(policy.delay(1, 0.7), Duration::from_millis(100));
    /// assert_eq!(policy.delay(4, 0.7), Duration::from_millis(800));
    /// assert_eq!(policy.delay(30, 0.7), Duration::from_secs(30));
    ///
    /// let full = RetryPolicy { jitter: Jitter::Full, ..policy.clone() };
    /// assert_eq!(full.delay(2, 0.25), Duration::from_millis(50));
    ///
    /// let fixed = RetryPolicy { backoff: Backoff::Fixed, ..policy };
    /// assert_eq!(fixed.delay(9, 0.0), Duration::from_millis(100));
    /// ```
    #[must_use]
    pub fn delay(&self, retry_number: u32, unit_random: f64) -> Duration {
        let cap = self.max_delay.as_secs_f64();
        let initial = self.initial.as_secs_f64();
        let mut secs = match self.backoff {
            Backoff::Fixed => initial,
            Backoff::Exponential if initial == 0.0 => 0.0,
            Backoff::Exponential => {
                let exponent = f64::from(retry_number.max(1) - 1);
                initial * self.multiplier.powf(exponent)
            }
        };
        if secs.is_nan() {
            secs = cap;
        }
        secs = secs.clamp(0.0, cap);
        let random = if unit_random.is_nan() {
            0.0
        } else {
            unit_random.clamp(0.0, 1.0)
        };
        secs = match self.jitter {
            Jitter::None => secs,
            Jitter::Full => secs * random,
            Jitter::Equal => secs / 2.0 + secs / 2.0 * random,
        };
        Duration::try_from_secs_f64(secs)
            .unwrap_or(self.max_delay)
            .min(self.max_delay)
    }

    /// Runs `op` until it succeeds, `retry_if` rejects its error, or `max_attempts` calls
    /// have failed, and returns the last result.
    ///
    /// `label` names the operation in the events: each retry logs a `warn` event on target
    /// `oxbloom::tasks` with the method, the failed attempt, the delay in milliseconds and
    /// the error text, and running out of attempts logs one `error` event. A success on the
    /// first attempt and an error that `retry_if` rejects log nothing. Jitter draws from
    /// `fastrand`. Dropping the returned future stops the retries.
    ///
    /// # Errors
    ///
    /// Returns the error of the last attempt, or the first error that `retry_if` rejects.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::RetryPolicy;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build()?.block_on(async {
    /// let result: Result<(), &str> = RetryPolicy::default()
    ///     .run("charge", || async { Err("card declined") }, |error| *error != "card declined")
    ///     .await;
    /// assert_eq!(result, Err("card declined"));
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub async fn run<T, E, Fut, Op, If>(&self, label: &str, op: Op, retry_if: If) -> Result<T, E>
    where
        Op: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: fmt::Display,
        If: Fn(&E) -> bool,
    {
        self.run_with_random(label, op, retry_if, fastrand::f64)
            .await
    }

    /// [`run`](RetryPolicy::run) with the random source of the jitter replaced; for tests.
    ///
    /// # Errors
    ///
    /// As [`run`](RetryPolicy::run).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::{Jitter, RetryPolicy};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build()?.block_on(async {
    /// let policy = RetryPolicy { jitter: Jitter::Full, ..RetryPolicy::default() };
    /// let start = tokio::time::Instant::now();
    /// let result: Result<(), &str> = policy
    ///     .run_with_random("charge", || async { Err("boom") }, |_| true, || 0.5)
    ///     .await;
    /// assert_eq!(result, Err("boom"));
    /// assert_eq!(start.elapsed(), std::time::Duration::from_millis(50 + 100));
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[doc(hidden)]
    pub async fn run_with_random<T, E, Fut, Op, If, R>(
        &self,
        label: &str,
        mut op: Op,
        retry_if: If,
        mut random: R,
    ) -> Result<T, E>
    where
        Op: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: fmt::Display,
        If: Fn(&E) -> bool,
        R: FnMut() -> f64,
    {
        let mut attempt: u32 = 1;
        loop {
            let error = match op().await {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            if !retry_if(&error) {
                return Err(error);
            }
            if attempt >= self.max_attempts {
                tracing::error!(
                    target: TARGET,
                    method = label,
                    attempts = attempt,
                    error = %error,
                    "retry attempts exhausted"
                );
                return Err(error);
            }
            let delay = self.delay(attempt, random());
            tracing::warn!(
                target: TARGET,
                method = label,
                attempt,
                delay_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                error = %error,
                "attempt failed, retrying"
            );
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }
}

/// The process-wide retry defaults: the values `#[retryable]` uses for every field its
/// attribute omits, read at call time.
///
/// Boot's retry auto-configuration stores them from the `tasks.retry.*` keys with
/// [`set_defaults`]. They are global state: two application contexts in one process share
/// the values that were stored last, and [`reset_defaults`] restores the built-in values.
/// [`RetryDefaults::default`] holds the built-in values.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_tasks::{Backoff, RetryDefaults};
///
/// let defaults = RetryDefaults { backoff: Backoff::Fixed, ..RetryDefaults::default() };
/// let policy = defaults.policy();
/// assert_eq!(policy.backoff, Backoff::Fixed);
/// assert_eq!(policy.initial, Duration::from_millis(100));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryDefaults {
    /// See [`RetryPolicy::max_attempts`]. Built-in default 3.
    pub max_attempts: u32,
    /// See [`RetryPolicy::backoff`]. Built-in default [`Backoff::Exponential`].
    pub backoff: Backoff,
    /// See [`RetryPolicy::initial`]. Built-in default 100 ms.
    pub initial: Duration,
    /// See [`RetryPolicy::multiplier`]. Built-in default 2.0.
    pub multiplier: f64,
    /// See [`RetryPolicy::max_delay`]. Built-in default 30 seconds.
    pub max_delay: Duration,
    /// See [`RetryPolicy::jitter`]. Built-in default [`Jitter::Full`].
    pub jitter: Jitter,
}

impl Default for RetryDefaults {
    /// The built-in values, the same as [`RetryPolicy::default`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::RetryDefaults;
    ///
    /// assert_eq!(RetryDefaults::default().max_delay, Duration::from_secs(30));
    /// ```
    fn default() -> Self {
        BUILT_IN
    }
}

impl RetryDefaults {
    /// A [`RetryPolicy`] with these values.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::{RetryDefaults, RetryPolicy};
    ///
    /// assert_eq!(RetryDefaults::default().policy(), RetryPolicy::default());
    /// ```
    #[must_use]
    pub const fn policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_attempts: self.max_attempts,
            backoff: self.backoff,
            initial: self.initial,
            multiplier: self.multiplier,
            max_delay: self.max_delay,
            jitter: self.jitter,
        }
    }
}

/// Stores the process-wide retry defaults that [`defaults`] returns.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::{RetryDefaults, defaults, reset_defaults, set_defaults};
///
/// set_defaults(RetryDefaults { max_attempts: 5, ..RetryDefaults::default() });
/// assert_eq!(defaults().max_attempts, 5);
/// reset_defaults();
/// assert_eq!(defaults(), RetryDefaults::default());
/// ```
pub fn set_defaults(defaults: RetryDefaults) {
    *DEFAULTS.write().unwrap_or_else(PoisonError::into_inner) = defaults;
}

/// The process-wide retry defaults: the last value given to [`set_defaults`], or the
/// built-in values when nothing was stored or after [`reset_defaults`].
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::{RetryDefaults, defaults};
///
/// assert_eq!(defaults(), RetryDefaults::default());
/// ```
#[must_use]
pub fn defaults() -> RetryDefaults {
    *DEFAULTS.read().unwrap_or_else(PoisonError::into_inner)
}

/// Restores the built-in retry defaults.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::{RetryDefaults, defaults, reset_defaults, set_defaults};
///
/// set_defaults(RetryDefaults { max_attempts: 9, ..RetryDefaults::default() });
/// reset_defaults();
/// assert_eq!(defaults().max_attempts, 3);
/// ```
pub fn reset_defaults() {
    set_defaults(BUILT_IN);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exponential() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 5,
            backoff: Backoff::Exponential,
            initial: Duration::from_millis(100),
            multiplier: 2.0,
            max_delay: Duration::from_secs(30),
            jitter: Jitter::None,
        }
    }

    #[test]
    fn delay_exponential_doubles_from_initial() {
        let policy = exponential();
        assert_eq!(policy.delay(1, 0.3), Duration::from_millis(100));
        assert_eq!(policy.delay(2, 0.3), Duration::from_millis(200));
        assert_eq!(policy.delay(3, 0.3), Duration::from_millis(400));
    }

    #[test]
    fn delay_fixed_ignores_multiplier() {
        let policy = RetryPolicy {
            backoff: Backoff::Fixed,
            multiplier: 10.0,
            ..exponential()
        };
        for n in 1..=5 {
            assert_eq!(policy.delay(n, 0.3), Duration::from_millis(100));
        }
    }

    #[test]
    fn delay_is_capped_at_max_delay() {
        let policy = RetryPolicy {
            max_delay: Duration::from_millis(300),
            ..exponential()
        };
        assert_eq!(policy.delay(2, 0.0), Duration::from_millis(200));
        assert_eq!(policy.delay(3, 0.0), Duration::from_millis(300));
        assert_eq!(policy.delay(20, 0.0), Duration::from_millis(300));
    }

    #[test]
    fn delay_jitter_full_scales_by_random() {
        let policy = RetryPolicy {
            jitter: Jitter::Full,
            ..exponential()
        };
        assert_eq!(policy.delay(1, 0.5), Duration::from_millis(50));
        assert_eq!(policy.delay(2, 0.5), Duration::from_millis(100));
        assert_eq!(policy.delay(1, 0.0), Duration::ZERO);
    }

    #[test]
    fn delay_jitter_equal_is_half_plus_half_random() {
        let policy = RetryPolicy {
            jitter: Jitter::Equal,
            ..exponential()
        };
        assert_eq!(policy.delay(1, 0.0), Duration::from_millis(50));
        let almost = policy.delay(1, 1.0 - f64::EPSILON);
        assert!(almost > Duration::from_micros(99_900), "{almost:?}");
        assert!(almost <= Duration::from_millis(100), "{almost:?}");
    }

    #[test]
    fn delay_never_overflows() {
        let capped = RetryPolicy {
            multiplier: 1e300,
            max_delay: Duration::from_millis(1),
            ..exponential()
        };
        assert_eq!(capped.delay(u32::MAX, 0.5), Duration::from_millis(1));

        let unbounded = RetryPolicy {
            multiplier: 1e300,
            max_delay: Duration::MAX,
            ..exponential()
        };
        assert!(unbounded.delay(u32::MAX, 0.0) <= Duration::MAX);
        assert!(unbounded.delay(u32::MAX, 0.999) <= Duration::MAX);
        let full = RetryPolicy {
            jitter: Jitter::Full,
            ..unbounded.clone()
        };
        assert!(full.delay(u32::MAX, 0.999) <= Duration::MAX);
    }

    #[test]
    fn delay_handles_odd_inputs_without_panicking() {
        let zero = RetryPolicy {
            initial: Duration::ZERO,
            multiplier: 1e300,
            ..exponential()
        };
        assert_eq!(zero.delay(u32::MAX, 0.5), Duration::ZERO);
        let nan = RetryPolicy {
            multiplier: f64::NAN,
            ..exponential()
        };
        assert!(nan.delay(3, 0.5) <= Duration::from_secs(30));
        let negative = RetryPolicy {
            multiplier: -2.0,
            ..exponential()
        };
        assert!(negative.delay(2, 0.5) <= Duration::from_secs(30));
        let jittered = RetryPolicy {
            jitter: Jitter::Full,
            ..exponential()
        };
        assert!(jittered.delay(1, f64::NAN) <= Duration::from_millis(100));
        assert!(jittered.delay(1, 7.0) <= Duration::from_millis(100));
        assert!(jittered.delay(1, -1.0) <= Duration::from_millis(100));
        assert_eq!(exponential().delay(0, 0.5), Duration::from_millis(100));
    }

    #[test]
    fn from_str_accepts_the_names_and_rejects_others() {
        assert_eq!("fixed".parse::<Backoff>(), Ok(Backoff::Fixed));
        assert_eq!("exponential".parse::<Backoff>(), Ok(Backoff::Exponential));
        assert_eq!("none".parse::<Jitter>(), Ok(Jitter::None));
        assert_eq!("full".parse::<Jitter>(), Ok(Jitter::Full));
        assert_eq!("equal".parse::<Jitter>(), Ok(Jitter::Equal));
        for name in ["fixed", "exponential"] {
            assert_eq!(name.parse::<Backoff>().unwrap().to_string(), name);
        }
        for name in ["none", "full", "equal"] {
            assert_eq!(name.parse::<Jitter>().unwrap().to_string(), name);
        }

        let err = "linear-secret".parse::<Backoff>().unwrap_err();
        assert!(!err.to_string().contains("linear-secret"), "{err}");
        assert!(err.to_string().contains("backoff"), "{err}");
        let err = "random-secret".parse::<Jitter>().unwrap_err();
        assert!(!err.to_string().contains("random-secret"), "{err}");
        assert!(err.to_string().contains("jitter"), "{err}");
        assert!("".parse::<Backoff>().is_err());
        assert!("Full".parse::<Jitter>().is_err());
    }

    #[test]
    fn default_policy_has_the_built_in_values() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.backoff, Backoff::Exponential);
        assert_eq!(policy.initial, Duration::from_millis(100));
        assert_eq!(policy.multiplier, 2.0);
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert_eq!(policy.jitter, Jitter::Full);
        assert_eq!(RetryDefaults::default().policy(), policy);
    }
}
