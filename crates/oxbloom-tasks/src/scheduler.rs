//! The scheduler: one tokio loop per task, a concurrency bound and a bounded shutdown.

use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use oxbloom_core::Clock;
use tokio::sync::{Semaphore, oneshot};
use tokio::task::{AbortHandle, JoinHandle};

use crate::task::{Outcome, RunFn, Task, TaskError};
use crate::trigger::{Trigger, Zone};

/// The tracing target of every scheduler event.
const TARGET: &str = "oxbloom::tasks";

/// The longest single sleep; the clock is read again after each one, so a clock that jumps
/// is noticed within this time.
const MAX_CHUNK: Duration = Duration::from_secs(1);

/// Settings for a [`Scheduler`], bound from `tasks.scheduling.*` by the boot integration.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_tasks::SchedulerConfig;
///
/// let config = SchedulerConfig { max_concurrent: 2, ..SchedulerConfig::default() };
/// assert_eq!(config.shutdown_timeout, Duration::from_secs(10));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// How many runs may execute at the same time across all tasks. Zero is treated as one.
    pub max_concurrent: usize,
    /// How long [`Scheduler::stop`] waits for runs in flight before it aborts them.
    pub shutdown_timeout: Duration,
    /// The default zone for cron triggers that name none. The scheduler itself evaluates each
    /// trigger in the zone the trigger was built with; this value is for the code that builds
    /// the triggers (the `#[scheduled]` integration).
    pub zone: Zone,
    /// The initial delay of a task that sets none with [`Task::initial_delay`].
    pub initial_delay: Duration,
}

impl Default for SchedulerConfig {
    /// Eight concurrent runs, a ten second shutdown timeout, UTC and no initial delay.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_tasks::{SchedulerConfig, Zone};
    ///
    /// let config = SchedulerConfig::default();
    /// assert_eq!(config.max_concurrent, 8);
    /// assert_eq!(config.zone, Zone::Utc);
    /// assert_eq!(config.initial_delay, Duration::ZERO);
    /// ```
    fn default() -> Self {
        SchedulerConfig {
            max_concurrent: 8,
            shutdown_timeout: Duration::from_secs(10),
            zone: Zone::Utc,
            initial_delay: Duration::ZERO,
        }
    }
}

/// A read-only snapshot of one task, from [`Scheduler::tasks`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use oxbloom_core::{Clock, SystemClock};
/// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger};
///
/// let task = Task::new("ping", Trigger::fixed_rate(Duration::from_secs(5))?, || {
///     Box::pin(async { Ok(()) })
/// });
/// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
/// let scheduler = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock)?;
/// let status = &scheduler.tasks()[0];
/// assert_eq!(status.name, "ping");
/// assert_eq!(status.trigger, "fixed rate 5s");
/// assert_eq!(status.last_outcome, None);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    /// The task's name.
    pub name: String,
    /// The trigger, described as by [`Trigger::describe`].
    pub trigger: String,
    /// When the last run started (after it got a concurrency permit).
    pub last_start: Option<SystemTime>,
    /// When the last run ended.
    pub last_end: Option<SystemTime>,
    /// How the last run ended, or [`Outcome::Skipped`] when the last trigger was skipped.
    pub last_outcome: Option<Outcome>,
    /// How many triggers were skipped because the previous run was still active.
    pub skipped: u64,
}

/// What the scheduler tracks per task.
struct Slot {
    name: String,
    trigger: Trigger,
    initial_delay: Duration,
    run: RunFn,
    /// True from the moment a trigger fires (including the wait for a permit) until the run
    /// ends.
    active: AtomicBool,
    skipped: AtomicU64,
    history: Mutex<History>,
    /// The run in flight. Spawning checks `stopping` under this lock, so no run starts after
    /// [`Scheduler::stop`] took it.
    current: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default, Clone, Copy)]
struct History {
    last_start: Option<SystemTime>,
    last_end: Option<SystemTime>,
    last_outcome: Option<Outcome>,
}

/// State shared by the scheduler and its loops.
struct Shared {
    clock: Arc<dyn Clock>,
    permits: Arc<Semaphore>,
    stopping: AtomicBool,
}

#[derive(Default)]
struct Lifecycle {
    started: bool,
    stopped: bool,
    loops: Vec<JoinHandle<()>>,
}

/// Runs [`Task`]s on tokio when their triggers fire.
///
/// Each task gets its own loop. A loop reads the injected [`Clock`], sleeps until the next
/// fire time in chunks of at most one second (reading the clock again after each chunk, so a
/// clock that jumps is noticed), and then starts a run in its own tokio task.
///
/// - A task never runs concurrently with itself: a trigger that finds the previous run still
///   active (running, or waiting for a permit) is skipped, counted and logged at `warn`.
/// - `max_concurrent` bounds the runs executing at once across all tasks.
/// - An `Err` or a panic is logged at `error` with the task name, the error in the `error`
///   field, and the schedule continues.
/// - After a run the next fire time is computed from the current time, so missed fire times
///   are not caught up: a long clock jump gives one run, not a burst.
/// - When the clock jumps backwards, a fixed-rate or fixed-delay task does not wait for
///   `last_start + period` to come round again: a wait is never longer than the time planned
///   when it began, and never longer than one period.
///
/// Events use the target `oxbloom::tasks`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::{AtomicUsize, Ordering};
/// use std::time::Duration;
/// use oxbloom_core::{Clock, SystemClock};
/// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger};
///
/// let runs = Arc::new(AtomicUsize::new(0));
/// let counter = runs.clone();
/// let task = Task::new("count", Trigger::fixed_rate(Duration::from_secs(60))?, move || {
///     let counter = counter.clone();
///     Box::pin(async move {
///         counter.fetch_add(1, Ordering::SeqCst);
///         Ok(())
///     })
/// });
/// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
/// let scheduler = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock)?;
///
/// let rt = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
/// rt.block_on(async {
///     scheduler.start().await;
///     tokio::time::sleep(Duration::from_millis(50)).await;
///     scheduler.stop().await;
/// });
/// assert_eq!(runs.load(Ordering::SeqCst), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Scheduler {
    slots: Vec<Arc<Slot>>,
    shared: Arc<Shared>,
    shutdown_timeout: Duration,
    lifecycle: Mutex<Lifecycle>,
}

impl Scheduler {
    /// Builds a scheduler for `tasks`. Nothing runs until [`Scheduler::start`].
    ///
    /// # Errors
    ///
    /// Returns a [`TaskError`] naming the first task made with [`Task::invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger, Zone};
    ///
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let ok = Task::new("ok", Trigger::fixed_delay(Duration::from_secs(1))?, || {
    ///     Box::pin(async { Ok(()) })
    /// });
    /// assert!(Scheduler::try_new(vec![ok], SchedulerConfig::default(), clock.clone()).is_ok());
    ///
    /// let bad = Task::invalid("bad", Trigger::cron("* * *", Zone::Utc).unwrap_err());
    /// assert!(Scheduler::try_new(vec![bad], SchedulerConfig::default(), clock).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn try_new(
        tasks: Vec<Task>,
        config: SchedulerConfig,
        clock: Arc<dyn Clock>,
    ) -> Result<Scheduler, TaskError> {
        let slots = tasks
            .into_iter()
            .map(|task| {
                let (name, trigger, initial_delay, run) = task.into_parts()?;
                Ok(Arc::new(Slot {
                    name,
                    trigger,
                    initial_delay: initial_delay.unwrap_or(config.initial_delay),
                    run,
                    active: AtomicBool::new(false),
                    skipped: AtomicU64::new(0),
                    history: Mutex::new(History::default()),
                    current: Mutex::new(None),
                }))
            })
            .collect::<Result<Vec<_>, TaskError>>()?;
        let permits = config.max_concurrent.clamp(1, Semaphore::MAX_PERMITS);
        Ok(Scheduler {
            slots,
            shared: Arc::new(Shared {
                clock,
                permits: Arc::new(Semaphore::new(permits)),
                stopping: AtomicBool::new(false),
            }),
            shutdown_timeout: config.shutdown_timeout,
            lifecycle: Mutex::new(Lifecycle::default()),
        })
    }

    /// Spawns one loop per task on the current tokio runtime. Calling it again, or after
    /// [`Scheduler::stop`], does nothing.
    ///
    /// # Panics
    ///
    /// Panics when called outside a tokio runtime, like [`tokio::spawn`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig};
    ///
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let scheduler = Scheduler::try_new(Vec::new(), SchedulerConfig::default(), clock)?;
    /// let rt = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
    /// rt.block_on(async {
    ///     scheduler.start().await;
    ///     scheduler.start().await; // idempotent
    ///     scheduler.stop().await;
    /// });
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn start(&self) {
        let mut lifecycle = lock(&self.lifecycle);
        if lifecycle.started || lifecycle.stopped {
            return;
        }
        lifecycle.started = true;
        lifecycle.loops = self
            .slots
            .iter()
            .map(|slot| tokio::spawn(task_loop(slot.clone(), self.shared.clone())))
            .collect();
        tracing::info!(
            target: TARGET,
            tasks = self.slots.len(),
            "task scheduler started"
        );
    }

    /// Stops the scheduler: no trigger fires any more, runs in flight get up to
    /// `shutdown_timeout` to finish, and runs still going at the deadline are aborted with a
    /// `warn` event naming each task.
    ///
    /// `stop` never hangs: it returns at the deadline at the latest. A run whose future
    /// blocks its thread (for example inside [`std::thread::sleep`]) cannot be aborted; on a
    /// multi-thread runtime that thread is left to finish on its own (it is leaked until
    /// then) and `stop` does not wait for it. Calling `stop` again does nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger};
    ///
    /// let task = Task::new("stuck", Trigger::fixed_rate(Duration::from_secs(1))?, || {
    ///     Box::pin(std::future::pending())
    /// });
    /// let config = SchedulerConfig {
    ///     shutdown_timeout: Duration::from_millis(20),
    ///     ..SchedulerConfig::default()
    /// };
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let scheduler = Scheduler::try_new(vec![task], config, clock)?;
    /// let rt = tokio::runtime::Builder::new_current_thread().enable_time().build()?;
    /// rt.block_on(async {
    ///     scheduler.start().await;
    ///     tokio::task::yield_now().await;
    ///     scheduler.stop().await; // returns after 20 ms, aborting the stuck run
    /// });
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn stop(&self) {
        let loops = {
            let mut lifecycle = lock(&self.lifecycle);
            if lifecycle.stopped {
                return;
            }
            lifecycle.stopped = true;
            std::mem::take(&mut lifecycle.loops)
        };
        self.shared.stopping.store(true, Ordering::SeqCst);
        // Runs still waiting for a permit give up instead of starting.
        self.shared.permits.close();
        for handle in &loops {
            handle.abort();
        }
        let in_flight: Vec<(String, JoinHandle<()>)> = self
            .slots
            .iter()
            .filter_map(|slot| {
                let handle = lock(&slot.current).take()?;
                Some((slot.name.clone(), handle))
            })
            .collect();
        let deadline = tokio::time::Instant::now() + self.shutdown_timeout;
        for (name, mut handle) in in_flight {
            if tokio::time::timeout_at(deadline, &mut handle)
                .await
                .is_err()
            {
                // Not awaited again: a run blocking its thread would hold `stop` forever.
                handle.abort();
                tracing::warn!(
                    target: TARGET,
                    task = %name,
                    "scheduled task run abandoned at shutdown"
                );
            }
        }
        tracing::info!(target: TARGET, "task scheduler stopped");
    }

    /// A snapshot of every task, in the order the tasks were given.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig, Task, Trigger, Zone};
    ///
    /// let task = Task::new("nightly", Trigger::cron("0 0 3 * * *", Zone::Utc)?, || {
    ///     Box::pin(async { Ok(()) })
    /// });
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let scheduler = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock)?;
    /// let tasks = scheduler.tasks();
    /// assert_eq!(tasks[0].trigger, "cron 0 0 3 * * * (UTC)");
    /// assert_eq!(tasks[0].skipped, 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn tasks(&self) -> Vec<TaskStatus> {
        self.slots
            .iter()
            .map(|slot| {
                let history = *lock(&slot.history);
                TaskStatus {
                    name: slot.name.clone(),
                    trigger: slot.trigger.describe(),
                    last_start: history.last_start,
                    last_end: history.last_end,
                    last_outcome: history.last_outcome,
                    skipped: slot.skipped.load(Ordering::SeqCst),
                }
            })
            .collect()
    }
}

impl Drop for Scheduler {
    /// Aborts the loops and runs of a scheduler dropped without [`Scheduler::stop`].
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        self.shared.permits.close();
        for handle in &lock(&self.lifecycle).loops {
            handle.abort();
        }
        for slot in &self.slots {
            if let Some(handle) = lock(&slot.current).take() {
                handle.abort();
            }
        }
    }
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("tasks", &self.slots.len())
            .field("shutdown_timeout", &self.shutdown_timeout)
            .finish_non_exhaustive()
    }
}

/// Locks a mutex, recovering the data of a poisoned one (no lock is held across user code).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// When to fire next, and the longest real time to wait for it.
struct Wake {
    at: SystemTime,
    /// For fixed-rate and fixed-delay triggers, the wait is capped so a clock that jumps
    /// backwards does not stall the task.
    max_wait: Option<Duration>,
}

/// The loop of one task: sleep until the trigger fires, start or skip a run, repeat.
async fn task_loop(slot: Arc<Slot>, shared: Arc<Shared>) {
    let clock = &shared.clock;
    let start = clock.now();
    let mut wake = start
        .checked_add(slot.initial_delay)
        .and_then(|first| slot.trigger.next(None, None, first))
        .map(|at| Wake {
            at,
            max_wait: match slot.trigger {
                Trigger::Cron(..) => None,
                _ => Some(slot.initial_delay),
            },
        });
    loop {
        let Some(next) = wake else {
            tracing::info!(
                target: TARGET,
                task = %slot.name,
                "scheduled task has no next fire time"
            );
            return;
        };
        sleep_until(clock.as_ref(), &next).await;
        let fired = clock.now();
        let done = if slot.active.swap(true, Ordering::SeqCst) {
            slot.skipped.fetch_add(1, Ordering::SeqCst);
            lock(&slot.history).last_outcome = Some(Outcome::Skipped);
            tracing::warn!(
                target: TARGET,
                task = %slot.name,
                "scheduled task skipped: the previous run is still active"
            );
            None
        } else {
            match spawn_run(&slot, &shared) {
                Some(done) => Some(done),
                None => return,
            }
        };
        wake = match &slot.trigger {
            Trigger::FixedRate(period) => {
                let now = clock.now();
                plan(&slot.trigger, Some(fired), None, now, *period)
            }
            Trigger::FixedDelay(delay) => {
                // The delay counts from the end of the run: wait for it before planning.
                if let Some(done) = done {
                    let _ = done.await;
                }
                let history = *lock(&slot.history);
                let now = clock.now();
                plan(
                    &slot.trigger,
                    history.last_start,
                    history.last_end.or(Some(now)),
                    now,
                    *delay,
                )
            }
            Trigger::Cron(..) => slot
                .trigger
                .next(None, None, clock.now())
                .map(|at| Wake { at, max_wait: None }),
        };
    }
}

/// The next wake-up of a fixed-rate or fixed-delay trigger. The wait is the planned one,
/// clamped to `period` when the reference time lies in the future (the clock went back).
fn plan(
    trigger: &Trigger,
    last_start: Option<SystemTime>,
    last_end: Option<SystemTime>,
    now: SystemTime,
    period: Duration,
) -> Option<Wake> {
    let at = trigger.next(last_start, last_end, now)?;
    let planned = at.duration_since(now).unwrap_or(Duration::ZERO);
    Some(Wake {
        at,
        max_wait: Some(planned.min(period)),
    })
}

/// Sleeps until the clock reaches `wake.at`, in chunks of at most [`MAX_CHUNK`], or until
/// `wake.max_wait` of real time has passed.
async fn sleep_until(clock: &dyn Clock, wake: &Wake) {
    let mut slept = Duration::ZERO;
    loop {
        let Ok(remaining) = wake.at.duration_since(clock.now()) else {
            return;
        };
        let mut chunk = remaining.min(MAX_CHUNK);
        if let Some(max_wait) = wake.max_wait {
            let Some(left) = max_wait.checked_sub(slept).filter(|left| !left.is_zero()) else {
                return;
            };
            chunk = chunk.min(left);
        }
        if chunk.is_zero() {
            return;
        }
        tokio::time::sleep(chunk).await;
        slept = slept.saturating_add(chunk);
    }
}

/// Spawns one run of `slot` and returns a receiver that resolves when it ends, or `None`
/// when the scheduler is stopping.
fn spawn_run(slot: &Arc<Slot>, shared: &Arc<Shared>) -> Option<oneshot::Receiver<()>> {
    let mut current = lock(&slot.current);
    if shared.stopping.load(Ordering::SeqCst) {
        slot.active.store(false, Ordering::SeqCst);
        return None;
    }
    let (done_tx, done_rx) = oneshot::channel();
    *current = Some(tokio::spawn(run_once(
        slot.clone(),
        shared.clone(),
        done_tx,
    )));
    Some(done_rx)
}

/// Clears the active flag of a task when its run ends, however it ends.
struct ActiveGuard<'a>(&'a AtomicBool);

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Aborts the task running the user's future when its supervisor is dropped (on abort).
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One run: wait for a permit, run the work in its own tokio task (so a panic is contained),
/// record and log the outcome.
async fn run_once(slot: Arc<Slot>, shared: Arc<Shared>, done: oneshot::Sender<()>) {
    let active = ActiveGuard(&slot.active);
    let Ok(_permit) = shared.permits.clone().acquire_owned().await else {
        return; // closed: the scheduler is stopping
    };
    let started = shared.clock.now();
    lock(&slot.history).last_start = Some(started);
    let run = slot.run.clone();
    let work = tokio::spawn(async move { run().await });
    let _abort = AbortOnDrop(work.abort_handle());
    let result = work.await;
    let ended = shared.clock.now();
    let outcome = match result {
        Ok(Ok(())) => Outcome::Ok,
        Ok(Err(message)) => {
            tracing::error!(
                target: TARGET,
                task = %slot.name,
                error = %message,
                "scheduled task failed"
            );
            Outcome::Failed
        }
        Err(join_error) if join_error.is_panic() => {
            let message = panic_message(join_error.into_panic());
            tracing::error!(
                target: TARGET,
                task = %slot.name,
                error = %message,
                "scheduled task panicked"
            );
            Outcome::Panicked
        }
        Err(_) => Outcome::Failed,
    };
    let took = ended.duration_since(started).unwrap_or(Duration::ZERO);
    tracing::debug!(
        target: TARGET,
        task = %slot.name,
        duration_ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
        outcome = ?outcome,
        "scheduled task ran"
    );
    {
        let mut history = lock(&slot.history);
        history.last_end = Some(ended);
        history.last_outcome = Some(outcome);
    }
    drop(active);
    let _ = done.send(());
}

/// The message of a panic payload, when it is a string.
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_owned(),
            Err(_) => "panic with a non-string payload".to_owned(),
        },
    }
}
