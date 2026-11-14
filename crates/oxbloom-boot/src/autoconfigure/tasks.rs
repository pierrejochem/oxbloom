//! `TasksAutoConfiguration`: the `oxbloom-tasks` scheduler of every `dyn Scheduling` bean,
//! started and stopped as a boot service. `RetryAutoConfiguration`: the `tasks.retry.*` keys as
//! the process-wide retry defaults.

use std::sync::Arc;

use async_trait::async_trait;
use oxbloom_config::Config;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_tasks::{RetryDefaults, Scheduler, Scheduling, TasksSettings, set_defaults};

use crate::autoconfig::AutoConfiguration;
use crate::error::ServiceError;
use crate::metadata::PropertyMetadata;
use crate::service::Service;

/// Registers a [`Scheduler`] for the tasks of every `dyn Scheduling` bean (a `#[scheduling]`
/// impl registered with `provides`), and a [`SchedulerService`] that starts and stops it with
/// the run.
///
/// Available with the cargo feature `tasks`. The group runs when the application registers at
/// least one `dyn Scheduling` bean (the condition `on_bean`) and `tasks.scheduling.enabled` is
/// missing or `true` (the condition `on_property_value_or_missing`); the conditions report
/// names the reason for a skip. It registers:
///
/// - the [`TasksSettings`] of the `tasks.scheduling.*` keys; an unknown key directly under
///   `tasks` or `tasks.scheduling`, or a bad value, fails the build with `DiError::Creation`
///   whose source is the `ConfigError` naming the key, never the value;
/// - the [`Scheduler`] (unless the application registered one), built with
///   [`Scheduler::try_new`] from the tasks of every `dyn Scheduling` bean, the settings, and
///   the `dyn Clock` bean (the system clock without one). `tasks.scheduling.zone` applies to
///   cron tasks without a `zone`, `tasks.scheduling.initial-delay` to tasks without an
///   `initial_delay`. A bad cron expression, zone or duration, a zero `fixed_rate` or
///   `fixed_delay`, or two tasks with the same name fail the build with `DiError::Creation`
///   for `Scheduler` whose source is the `oxbloom_tasks::TaskError` naming the task and the
///   key, never the value;
/// - the [`SchedulerService`] as a `dyn Service`.
///
/// Scheduler events use the log target `oxbloom::tasks`.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{AutoConfiguration, Banner, OxbloomApplication, RunListener, TasksAutoConfiguration};
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_tasks::{Scheduler, Scheduling, scheduling};
///
/// struct Reports;
///
/// #[scheduling]
/// impl Reports {
///     #[scheduled(fixed_rate = "1h", name = "refresh")]
///     async fn refresh(&self) {}
/// }
///
/// #[derive(Clone, Default)]
/// struct Seen(Arc<Mutex<Vec<String>>>);
/// impl RunListener for Seen {
///     fn context_ready(&self, c: &Container) {
///         let names = c.get::<Scheduler>().unwrap().tasks().into_iter().map(|t| t.name);
///         self.0.lock().unwrap().extend(names);
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let seen = Seen::default();
/// let builder = OxbloomApplication::builder()
///     .with_config(Config::from_str("[tasks.scheduling]\nmax-concurrent = 2")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(seen.clone())
///     .auto_configuration::<TasksAutoConfiguration>()
///     .configure(|b| b.instance(Reports).provides(|r: Arc<Reports>| -> Arc<dyn Scheduling> { r }));
/// // The scheduler is a service, so the run waits for shutdown; trigger it up front.
/// builder.shutdown_handle().trigger();
/// builder.run().await?;
/// assert_eq!(*seen.0.lock().unwrap(), ["refresh"]);
///
/// let keys: Vec<_> = TasksAutoConfiguration::properties().iter().map(|p| p.key).collect();
/// assert!(keys.contains(&"tasks.scheduling.zone"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct TasksAutoConfiguration;

impl Configuration for TasksAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(tasks_settings)
            .try_bean(scheduler)
            .on_missing_bean::<Scheduler>()
            .bean(SchedulerService::new)
            // Same phase as the web server: the highest order puts it after the server (and
            // after every other service of that phase) at start, so it stops first.
            .order(i32::MAX)
            .provides(|s: Arc<SchedulerService>| -> Arc<dyn Service> { s })
    }
}

impl AutoConfiguration for TasksAutoConfiguration {
    /// Runs with at least one `dyn Scheduling` bean, and backs off when
    /// `tasks.scheduling.enabled` is set and is not `true`.
    fn conditions() -> Conditions {
        Conditions::new()
            .on_property_value_or_missing("tasks.scheduling.enabled", "true")
            .on_bean::<dyn Scheduling>()
    }

    fn properties() -> &'static [PropertyMetadata] {
        &SCHEDULING
    }
}

/// The settings of the `tasks` keys; the `ConfigError` stays the typed source.
fn tasks_settings(config: Arc<Config>) -> Result<TasksSettings, DiError> {
    TasksSettings::from_config(&config).map_err(DiError::creation_from::<TasksSettings>)
}

/// The scheduler of every source's tasks; the `TaskError` stays the typed source.
fn scheduler(
    settings: Arc<TasksSettings>,
    sources: Vec<Arc<dyn Scheduling>>,
    clock: Option<Arc<dyn Clock>>,
) -> Result<Scheduler, DiError> {
    let tasks = sources
        .into_iter()
        .flat_map(|source| source.scheduled())
        .collect();
    let clock = clock.unwrap_or_else(|| Arc::new(SystemClock));
    Scheduler::try_new(tasks, settings.scheduler_config(), clock)
        .map_err(DiError::creation_from::<Scheduler>)
}

/// Reads the `tasks.retry.*` keys and stores them as the process-wide retry defaults with
/// [`set_defaults`], so every `#[retryable]` method without its own value for a setting uses
/// the configured one.
///
/// Available with the cargo feature `tasks`. It has no conditions: it runs whether or not a
/// scheduler exists, so the retry keys apply when `tasks.scheduling.enabled` is `false` or no
/// `dyn Scheduling` bean is registered. It registers the [`RetryDefaults`] of the
/// `tasks.retry.*` keys as a bean, read with [`RetryDefaults::from_config`]. An unknown key
/// directly under `tasks` or `tasks.retry`, or a bad value, fails the build with
/// `DiError::Creation` for `RetryDefaults` whose source is the `ConfigError` naming the key,
/// never the value, and the defaults stay as they were.
///
/// The defaults are process-wide and stay set after the run ends. A test that changes them
/// restores the built-in values with `oxbloom_tasks::reset_defaults`.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_boot::{AutoConfiguration, Banner, OxbloomApplication, RetryAutoConfiguration};
/// use oxbloom_config::Config;
/// use oxbloom_tasks::{Backoff, defaults, reset_defaults};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("[tasks.retry]\nmax-attempts = 5\nbackoff = \"fixed\"\ninitial = \"250ms\"")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .auto_configuration::<RetryAutoConfiguration>()
///     .run()
///     .await?;
/// assert_eq!(defaults().max_attempts, 5);
/// assert_eq!(defaults().backoff, Backoff::Fixed);
/// assert_eq!(defaults().initial, Duration::from_millis(250));
/// reset_defaults();
///
/// let keys: Vec<_> = RetryAutoConfiguration::properties().iter().map(|p| p.key).collect();
/// assert!(keys.contains(&"tasks.retry.max-attempts"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct RetryAutoConfiguration;

impl Configuration for RetryAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.try_bean(retry_defaults)
    }
}

impl AutoConfiguration for RetryAutoConfiguration {
    /// None: the retry keys apply with or without a scheduler.
    fn conditions() -> Conditions {
        Conditions::new()
    }

    fn properties() -> &'static [PropertyMetadata] {
        &RETRY
    }
}

/// The retry defaults of the `tasks.retry` keys, stored process-wide; the `ConfigError` stays
/// the typed source.
fn retry_defaults(config: Arc<Config>) -> Result<RetryDefaults, DiError> {
    let value =
        RetryDefaults::from_config(&config).map_err(DiError::creation_from::<RetryDefaults>)?;
    set_defaults(value);
    Ok(value)
}

/// The boot [`Service`] that starts the [`Scheduler`] bean with the run and stops it at
/// shutdown, registered by [`TasksAutoConfiguration`].
///
/// It starts late: its [`phase`](Service::phase) is `i32::MAX`, the web server's (the highest
/// phase there is), and its registration has the highest DI `order`, so among the services of
/// that phase it comes last. The web server and every other service are therefore up before
/// the first task fires, and at shutdown the scheduler stops first, before the server. Boot
/// fires `ready` after all services started, so scheduled work begins before readiness flips
/// to accepting traffic; give a task an `initial_delay` (or set
/// `tasks.scheduling.initial-delay`) when it should wait for warm-up.
///
/// `stop` stops new triggers and waits up to `tasks.scheduling.shutdown-timeout` for runs in
/// flight, then aborts the rest. Boot's own stop deadline still bounds it: when boot abandons
/// the `stop` first, the runs in flight are aborted with it.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_boot::{SchedulerService, Service};
/// use oxbloom_core::{Clock, SystemClock};
/// use oxbloom_tasks::{Scheduler, SchedulerConfig};
///
/// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
/// let scheduler = Arc::new(Scheduler::try_new(Vec::new(), SchedulerConfig::default(), clock)?);
/// let service = SchedulerService::new(scheduler.clone());
/// assert_eq!(service.phase(), i32::MAX);
/// assert_eq!(service.name(), "TaskScheduler");
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// service.start().await?;
/// assert!(scheduler.is_running());
/// service.stop().await;
/// assert!(!scheduler.is_running());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct SchedulerService {
    scheduler: Arc<Scheduler>,
}

impl SchedulerService {
    /// A service for `scheduler`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::SchedulerService;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig};
    ///
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let scheduler = Arc::new(Scheduler::try_new(Vec::new(), SchedulerConfig::default(), clock)?);
    /// let service = SchedulerService::new(scheduler.clone());
    /// assert!(Arc::ptr_eq(service.scheduler(), &scheduler));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(scheduler: Arc<Scheduler>) -> SchedulerService {
        SchedulerService { scheduler }
    }

    /// The scheduler this service starts and stops.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::SchedulerService;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_tasks::{Scheduler, SchedulerConfig};
    ///
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let scheduler = Arc::new(Scheduler::try_new(Vec::new(), SchedulerConfig::default(), clock)?);
    /// assert!(SchedulerService::new(scheduler).scheduler().tasks().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn scheduler(&self) -> &Arc<Scheduler> {
        &self.scheduler
    }
}

#[async_trait]
impl Service for SchedulerService {
    /// Starts the scheduler's loops; never fails.
    async fn start(&self) -> Result<(), ServiceError> {
        self.scheduler.start().await;
        Ok(())
    }

    /// Stops the scheduler, waiting up to `tasks.scheduling.shutdown-timeout` for runs in
    /// flight.
    async fn stop(&self) {
        self.scheduler.stop().await;
    }

    /// `i32::MAX`, the web server's phase; the registration's order puts it after the server.
    fn phase(&self) -> i32 {
        i32::MAX
    }

    fn name(&self) -> &'static str {
        "TaskScheduler"
    }
}

/// Exactly the keys `TasksSettings::from_config` reads (pinned by a test).
static SCHEDULING: [PropertyMetadata; 5] = [
    PropertyMetadata::new(
        "tasks.scheduling.enabled",
        "bool",
        Some("true"),
        "`false` turns the task scheduler off: no scheduler, no service, no task runs.",
    ),
    PropertyMetadata::new(
        "tasks.scheduling.max-concurrent",
        "usize",
        Some("8"),
        "How many scheduled runs execute at the same time across all tasks; at least 1.",
    ),
    PropertyMetadata::new(
        "tasks.scheduling.shutdown-timeout",
        "Duration",
        Some("10s"),
        "How long shutdown waits for runs in flight before it aborts them.",
    ),
    PropertyMetadata::new(
        "tasks.scheduling.zone",
        "String",
        Some("UTC"),
        "The zone of cron tasks without a `zone` of their own: `UTC` or an IANA zone name such \
         as `Europe/Amsterdam`.",
    ),
    PropertyMetadata::new(
        "tasks.scheduling.initial-delay",
        "Duration",
        Some("0s"),
        "The delay before the first run of tasks without an `initial_delay` of their own.",
    ),
];

/// Exactly the keys `RetryDefaults::from_config` reads (pinned by a test).
static RETRY: [PropertyMetadata; 6] = [
    PropertyMetadata::new(
        "tasks.retry.max-attempts",
        "u32",
        Some("3"),
        "How many times a `#[retryable]` method runs at most, the first call included; at \
         least 1.",
    ),
    PropertyMetadata::new(
        "tasks.retry.backoff",
        "String",
        Some("exponential"),
        "How the delay between attempts grows: `fixed` or `exponential`.",
    ),
    PropertyMetadata::new(
        "tasks.retry.initial",
        "Duration",
        Some("100ms"),
        "The delay before the first retry; at most `tasks.retry.max-delay`.",
    ),
    PropertyMetadata::new(
        "tasks.retry.multiplier",
        "f64",
        Some("2.0"),
        "The factor of each next delay with `exponential` backoff; finite and at least 1.0.",
    ),
    PropertyMetadata::new(
        "tasks.retry.max-delay",
        "Duration",
        Some("30s"),
        "The cap on every delay between attempts.",
    ),
    PropertyMetadata::new(
        "tasks.retry.jitter",
        "String",
        Some("full"),
        "The randomness added to each delay: `none`, `full` or `equal`.",
    ),
];
