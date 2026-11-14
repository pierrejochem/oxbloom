//! Tasks: a name, a trigger and the work to run.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::trigger::{Trigger, TriggerError};

/// A boxed, sendable future, the return type of a task's work.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::BoxFuture;
///
/// let work: BoxFuture<'static, Result<(), String>> = Box::pin(async { Ok(()) });
/// # drop(work);
/// ```
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The work of a task: called once per run, it returns the future to await. The error is the
/// [`Display`](fmt::Display) string of the task's own error.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_tasks::RunFn;
///
/// let run: RunFn = Arc::new(|| Box::pin(async { Ok(()) }));
/// # drop(run);
/// ```
pub type RunFn = Arc<dyn Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// How a run of a task ended.
///
/// # Examples
///
/// ```
/// use oxbloom_tasks::Outcome;
///
/// let outcome = Outcome::Skipped;
/// assert_ne!(outcome, Outcome::Ok);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The run returned `Ok`.
    Ok,
    /// The run returned an error.
    Failed,
    /// The run panicked.
    Panicked,
    /// The trigger fired while the previous run was still active, so no run started.
    Skipped,
}

/// A scheduled task: a name, a [`Trigger`], an optional initial delay and the work to run.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_tasks::{Task, Trigger};
///
/// let task = Task::new("Reports::refresh", Trigger::fixed_rate(Duration::from_secs(30))?, || {
///     Box::pin(async { Ok(()) })
/// })
/// .initial_delay(Duration::from_secs(5));
/// assert_eq!(task.name(), "Reports::refresh");
/// # Ok::<(), oxbloom_tasks::TriggerError>(())
/// ```
#[derive(Clone)]
pub struct Task {
    name: String,
    /// The trigger, or the key whose value was invalid and why.
    trigger: Result<Trigger, (&'static str, TriggerError)>,
    initial_delay: Option<Duration>,
    run: RunFn,
}

impl Task {
    /// A task named `name` that runs `run` whenever `trigger` fires.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::{Task, Trigger};
    ///
    /// let task = Task::new("cleanup", Trigger::fixed_delay(Duration::from_secs(60))?, || {
    ///     Box::pin(async { Err("disk full".to_owned()) })
    /// });
    /// assert_eq!(task.trigger().map(Trigger::describe).as_deref(), Some("fixed delay 60s"));
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn new<F>(name: impl Into<String>, trigger: Trigger, run: F) -> Task
    where
        F: Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync + 'static,
    {
        Task {
            name: name.into(),
            trigger: Ok(trigger),
            initial_delay: None,
            run: Arc::new(run),
        }
    }

    /// A task whose trigger could not be built because the value of `key` (such as `cron`,
    /// `zone`, `fixed_rate`, `fixed_delay` or `initial_delay`) was invalid. It never runs:
    /// [`Scheduler::try_new`](crate::Scheduler::try_new) fails with a [`TaskError`] naming the
    /// task and the key.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_tasks::{Task, Trigger, Zone};
    ///
    /// let error = Trigger::cron("every day", Zone::Utc).unwrap_err();
    /// let task = Task::invalid("Reports::nightly", "cron", error);
    /// assert!(task.trigger().is_none());
    /// ```
    pub fn invalid(name: impl Into<String>, key: &'static str, error: TriggerError) -> Task {
        Task {
            name: name.into(),
            trigger: Err((key, error)),
            initial_delay: None,
            run: Arc::new(|| Box::pin(async { Ok(()) })),
        }
    }

    /// Postpones the first run by `delay` after the scheduler starts. Without it the
    /// scheduler's [`SchedulerConfig::initial_delay`](crate::SchedulerConfig) applies. For a cron
    /// trigger the first run is the first matching time after the start plus the delay.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::{Task, Trigger};
    ///
    /// let task = Task::new("warm", Trigger::fixed_rate(Duration::from_secs(1))?, || {
    ///     Box::pin(async { Ok(()) })
    /// })
    /// .initial_delay(Duration::from_secs(10));
    /// assert_eq!(task.delay(), Some(Duration::from_secs(10)));
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    #[must_use]
    pub fn initial_delay(mut self, delay: Duration) -> Task {
        self.initial_delay = Some(delay);
        self
    }

    /// The task's name, used in logs and in [`Scheduler::tasks`](crate::Scheduler::tasks).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::{Task, Trigger};
    ///
    /// let task = Task::new("ping", Trigger::fixed_rate(Duration::from_secs(1))?, || {
    ///     Box::pin(async { Ok(()) })
    /// });
    /// assert_eq!(task.name(), "ping");
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The trigger, or `None` for a task made with [`Task::invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::{Task, Trigger};
    ///
    /// let trigger = Trigger::fixed_rate(Duration::from_secs(1))?;
    /// let task = Task::new("ping", trigger.clone(), || Box::pin(async { Ok(()) }));
    /// assert_eq!(task.trigger(), Some(&trigger));
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn trigger(&self) -> Option<&Trigger> {
        self.trigger.as_ref().ok()
    }

    /// The initial delay set with [`Task::initial_delay`], if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::{Task, Trigger};
    ///
    /// let task = Task::new("ping", Trigger::fixed_rate(Duration::from_secs(1))?, || {
    ///     Box::pin(async { Ok(()) })
    /// });
    /// assert_eq!(task.delay(), None);
    /// # Ok::<(), oxbloom_tasks::TriggerError>(())
    /// ```
    pub fn delay(&self) -> Option<Duration> {
        self.initial_delay
    }

    /// Splits a valid task into its parts, or names it in a [`TaskError`].
    pub(crate) fn into_parts(
        self,
    ) -> Result<(String, Trigger, Option<Duration>, RunFn), TaskError> {
        match self.trigger {
            Ok(trigger) => Ok((self.name, trigger, self.initial_delay, self.run)),
            Err((key, error)) => Err(TaskError {
                task: self.name,
                key,
                error,
            }),
        }
    }
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task")
            .field("name", &self.name)
            .field("trigger", &self.trigger)
            .field("initial_delay", &self.initial_delay)
            .finish_non_exhaustive()
    }
}

/// A source of tasks, such as a bean with `#[scheduled]` methods.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use oxbloom_tasks::{Scheduling, Task, Trigger};
///
/// struct Reports;
///
/// impl Scheduling for Reports {
///     fn scheduled(self: Arc<Self>) -> Vec<Task> {
///         let trigger = Trigger::fixed_rate(Duration::from_secs(30)).expect("non-zero period");
///         vec![Task::new("Reports::refresh", trigger, move || {
///             let _reports = self.clone();
///             Box::pin(async { Ok(()) })
///         })]
///     }
/// }
///
/// let source: Arc<dyn Scheduling> = Arc::new(Reports);
/// assert_eq!(source.scheduled()[0].name(), "Reports::refresh");
/// ```
pub trait Scheduling: Send + Sync + 'static {
    /// The tasks this source contributes.
    fn scheduled(self: Arc<Self>) -> Vec<Task>;
}

/// A task that cannot be scheduled because its trigger is invalid.
///
/// The message names the task, the key whose value was invalid (such as `cron` or
/// `initial_delay`, or `name` for a name two tasks share) and what was wrong with it, never
/// the offending value.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_core::{Clock, SystemClock};
/// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger, Zone};
///
/// let error = Trigger::cron("secret-ish value", Zone::Utc).unwrap_err();
/// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
/// let task = Task::invalid("Jobs::sync", "cron", error);
/// let err = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock).unwrap_err();
/// assert_eq!(err.task(), "Jobs::sync");
/// assert_eq!(err.key(), "cron");
/// assert!(err.to_string().contains("cron expression"));
/// assert!(!err.to_string().contains("secret-ish"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskError {
    task: String,
    key: &'static str,
    error: TriggerError,
}

impl TaskError {
    /// A task whose name another task already has.
    pub(crate) fn duplicate(task: String) -> TaskError {
        TaskError {
            task,
            key: "name",
            error: TriggerError::new("another scheduled task has the same name"),
        }
    }

    /// The name of the task that could not be scheduled.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger};
    ///
    /// let error = Trigger::fixed_rate(Duration::ZERO).unwrap_err();
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let task = Task::invalid("a", "fixed_rate", error);
    /// let err = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock).unwrap_err();
    /// assert_eq!(err.task(), "a");
    /// ```
    pub fn task(&self) -> &str {
        &self.task
    }

    /// The key whose value was invalid, such as `cron`, `zone`, `fixed_rate`, `fixed_delay` or
    /// `initial_delay`, or `name` when another task has the same name.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Zone};
    ///
    /// let error = Zone::parse("Atlantis/Capital").unwrap_err();
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let task = Task::invalid("Jobs::nightly", "zone", error);
    /// let err = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock).unwrap_err();
    /// assert_eq!(err.key(), "zone");
    /// ```
    pub fn key(&self) -> &'static str {
        self.key
    }
}

impl fmt::Display for TaskError {
    /// Writes the task name and the key in backticks, then what was wrong.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger};
    ///
    /// let error = Trigger::fixed_delay(Duration::ZERO).unwrap_err();
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let task = Task::invalid("Jobs::poll", "fixed_delay", error);
    /// let err = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock).unwrap_err();
    /// assert_eq!(
    ///     err.to_string(),
    ///     "scheduled task `Jobs::poll`, key `fixed_delay`: fixed delay must be greater than zero"
    /// );
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "scheduled task `{}`, key `{}`: {}",
            self.task, self.key, self.error
        )
    }
}

impl std::error::Error for TaskError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
