//! `TasksAutoConfiguration`: the scheduler of the `dyn Scheduling` beans, its settings, its
//! place in the run and its startup errors, run in process through `TestApp` and
//! `ContextRunner` with a `ManualClock`.

#![cfg(feature = "tasks")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxbloom_boot::{
    AutoConfiguration, RunListener, SchedulerService, Service, ServiceError, StartupPhase,
    TasksAutoConfiguration,
};
use oxbloom_config::Config;
use oxbloom_di::{Container, ContainerBuilder};
use oxbloom_tasks::{Scheduler, Scheduling, TaskError, TasksSettings, Zone, scheduling};
use oxbloom_test::{ContextRunner, Log, ManualClock, TestApp};
use oxbloom_web::{Controller, HttpMethod, Route};

const HOUR: Duration = Duration::from_secs(3_600);

/// 2026-01-10 01:30:00 UTC, in winter: Amsterdam is UTC+1.
fn winter_night() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_768_008_600)
}

/// The application's controller, so `TestApp` gets its web server.
struct Ping;
impl Controller for Ping {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    }
}

fn ping(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Ping)
        .provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c })
}

/// The web slice with the tasks auto-configuration on a manual clock at `at`.
fn web(at: SystemTime) -> (TestApp, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at(at));
    let app = TestApp::web()
        .clock(clock.clone())
        .auto_configuration::<TasksAutoConfiguration>()
        .configure(ping);
    (app, clock)
}

/// Polls `done` every few milliseconds for up to five seconds.
async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..1_000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{what}: not reached within five seconds");
}

/// A bean with one hourly task that counts its runs.
#[derive(Clone, Default)]
struct Hourly {
    runs: Arc<AtomicUsize>,
}

#[scheduling]
impl Hourly {
    #[scheduled(fixed_rate = "1h")]
    async fn tick(&self) {
        self.runs.fetch_add(1, Ordering::SeqCst);
    }
}

fn hourly(bean: Hourly) -> impl FnOnce(ContainerBuilder) -> ContainerBuilder + Send + 'static {
    move |b| {
        b.instance(bean)
            .provides(|h: Arc<Hourly>| -> Arc<dyn Scheduling> { h })
    }
}

/// Keeps the `Scheduler` bean once the context is ready.
#[derive(Clone, Default)]
struct Grab(Arc<Mutex<Option<Arc<Scheduler>>>>);

impl Grab {
    fn scheduler(&self) -> Arc<Scheduler> {
        self.0.lock().unwrap().clone().expect("a Scheduler bean")
    }
}

impl RunListener for Grab {
    fn context_ready(&self, c: &Container) {
        *self.0.lock().unwrap() = c.get::<Scheduler>().ok();
    }
}

#[tokio::test]
async fn scheduled_task_runs_after_start_and_stops_at_shutdown() {
    let bean = Hourly::default();
    let runs = bean.runs.clone();
    let grab = Grab::default();
    let (app, clock) = web(winter_night());
    let app = app
        .listener(grab.clone())
        .configure(hourly(bean))
        .start()
        .await;
    wait_until("first run", || runs.load(Ordering::SeqCst) == 1).await;
    let scheduler = grab.scheduler();
    assert!(scheduler.is_running());
    clock.advance(HOUR);
    wait_until("second run", || runs.load(Ordering::SeqCst) == 2).await;
    assert_eq!(scheduler.tasks()[0].name, "Hourly::tick");
    app.stop().await;

    assert!(
        !scheduler.is_running(),
        "the scheduler stopped with the run"
    );
    clock.advance(HOUR);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(runs.load(Ordering::SeqCst), 2, "no run after shutdown");
}

#[tokio::test]
async fn disabled_registers_nothing() {
    ContextRunner::new()
        .property("tasks.scheduling.enabled", "false")
        .auto_configuration::<TasksAutoConfiguration>()
        .configure(hourly(Hourly::default()))
        .run(|view| {
            assert!(!view.contains::<Scheduler>());
            assert!(!view.contains::<SchedulerService>());
            assert!(!view.contains::<TasksSettings>());
        })
        .await;

    // Missing means enabled.
    ContextRunner::new()
        .auto_configuration::<TasksAutoConfiguration>()
        .configure(hourly(Hourly::default()))
        .run(|view| {
            assert!(view.contains::<Scheduler>());
            assert!(view.contains::<SchedulerService>());
        })
        .await;
}

#[tokio::test]
async fn no_scheduling_bean_registers_nothing() {
    ContextRunner::new()
        .auto_configuration::<TasksAutoConfiguration>()
        .run(|view| {
            assert!(!view.contains::<Scheduler>());
            assert!(!view.contains::<dyn Service>());
            assert!(
                view.conditions().records().iter().any(|r| {
                    r.registration().contains("TasksAutoConfiguration") && !r.matched()
                })
            );
        })
        .await;
}

struct BadCron;

#[scheduling]
impl BadCron {
    #[scheduled(cron = "MARKER-5 * *")]
    async fn nightly(&self) {}
}

#[tokio::test]
async fn bad_cron_fails_startup_naming_the_task() {
    let (app, _clock) = web(winter_night());
    let error = app
        .configure(|b| {
            b.instance(BadCron)
                .provides(|s: Arc<BadCron>| -> Arc<dyn Scheduling> { s })
        })
        .start_failing()
        .await;
    assert_eq!(error.phase(), StartupPhase::Container);
    let cause = error.find_cause::<TaskError>().expect("a TaskError cause");
    assert_eq!((cause.task(), cause.key()), ("BadCron::nightly", "cron"));
    let text = format!("{error} {cause}");
    assert!(text.contains("BadCron::nightly"), "{text}");
    assert!(
        !text.contains("MARKER-5"),
        "the value is not echoed: {text}"
    );
}

struct ZeroRate;

#[scheduling]
impl ZeroRate {
    #[scheduled(fixed_rate = "0s")]
    async fn spin(&self) {}
}

#[tokio::test]
async fn zero_fixed_rate_fails_startup() {
    let (app, _clock) = web(winter_night());
    let error = app
        .configure(|b| {
            b.instance(ZeroRate)
                .provides(|s: Arc<ZeroRate>| -> Arc<dyn Scheduling> { s })
        })
        .start_failing()
        .await;
    assert_eq!(error.phase(), StartupPhase::Container);
    let cause = error.find_cause::<TaskError>().expect("a TaskError cause");
    assert_eq!(
        (cause.task(), cause.key()),
        ("ZeroRate::spin", "fixed_rate")
    );
}

/// Records whether the scheduler runs at each event of the run.
#[derive(Clone)]
struct Order {
    log: Log,
    scheduler: Arc<Mutex<Option<Arc<Scheduler>>>>,
    services: Arc<Mutex<Vec<&'static str>>>,
}

impl Order {
    fn record(&self, event: &str) {
        let running = self
            .scheduler
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .is_running();
        self.log
            .push(format!("{event}: scheduler running {running}"));
    }
}

impl RunListener for Order {
    fn context_ready(&self, c: &Container) {
        *self.scheduler.lock().unwrap() = Some(c.get::<Scheduler>().unwrap());
    }

    fn started(&self, c: &Container) {
        // Boot starts the services in ascending phase, ties in DI order (a stable sort).
        let mut services = c.get_all::<dyn Service>().unwrap();
        services.sort_by_key(|s| s.phase());
        *self.services.lock().unwrap() = services.iter().map(|s| s.name()).collect();
        self.record("started");
    }

    fn ready(&self, _c: &Container) {
        self.record("ready");
    }

    fn stopping(&self) {
        self.record("stopping");
    }
}

/// A service in the web server's phase with the default order, like the server itself.
struct Neighbour(Order);

#[oxbloom_boot::async_trait]
impl Service for Neighbour {
    async fn start(&self) -> Result<(), ServiceError> {
        self.0.record("neighbour start");
        Ok(())
    }

    async fn stop(&self) {
        self.0.record("neighbour stop");
    }

    fn phase(&self) -> i32 {
        i32::MAX
    }

    fn name(&self) -> &'static str {
        "Neighbour"
    }
}

#[tokio::test]
async fn tasks_start_before_ready_and_stop_before_the_server() {
    let order = Order {
        log: Log::new(),
        scheduler: Arc::default(),
        services: Arc::default(),
    };
    let neighbour = Neighbour(order.clone());
    let (app, _clock) = web(winter_night());
    let app = app
        .listener(order.clone())
        .configure(hourly(Hourly::default()))
        .configure(move |b| {
            b.instance(neighbour)
                .provides(|n: Arc<Neighbour>| -> Arc<dyn Service> { n })
        })
        .start()
        .await;
    app.stop().await;

    assert_eq!(
        order.log.entries(),
        [
            "neighbour start: scheduler running false",
            "started: scheduler running true",
            "ready: scheduler running true",
            "stopping: scheduler running true",
            "neighbour stop: scheduler running false",
        ]
    );
    // The scheduler starts after the web server (same phase, later in order) and so stops
    // before it.
    let services = order.services.lock().unwrap().clone();
    let position = |name: &str| services.iter().position(|s| *s == name).unwrap();
    assert!(
        position("TaskScheduler") > position("WebServer"),
        "{services:?}"
    );
    assert_eq!(services.last(), Some(&"TaskScheduler"), "{services:?}");
}

/// A bean whose run never ends and that notes when it is dropped.
#[derive(Clone, Default)]
struct Stuck {
    started: Arc<AtomicBool>,
    dropped: Arc<AtomicBool>,
}

struct SetOnDrop(Arc<AtomicBool>);
impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[scheduling]
impl Stuck {
    #[scheduled(fixed_rate = "1h")]
    async fn hang(&self) {
        let _guard = SetOnDrop(self.dropped.clone());
        self.started.store(true, Ordering::SeqCst);
        std::future::pending::<()>().await;
    }
}

#[tokio::test]
async fn shutdown_timeout_is_applied() {
    let bean = Stuck::default();
    let (started, dropped) = (bean.started.clone(), bean.dropped.clone());
    let (app, _clock) = web(winter_night());
    let app = app
        .property("tasks.scheduling.shutdown-timeout", "200ms")
        .configure(move |b| {
            b.instance(bean)
                .provides(|s: Arc<Stuck>| -> Arc<dyn Scheduling> { s })
        })
        .start()
        .await;
    wait_until("run started", || started.load(Ordering::SeqCst)).await;
    let before = std::time::Instant::now();
    app.stop().await;
    let took = before.elapsed();
    assert!(took >= Duration::from_millis(200), "stop took {took:?}");
    // Boot's own stop deadline in tests is 5 seconds: the scheduler's came first.
    assert!(took < Duration::from_secs(4), "stop took {took:?}");
    wait_until("run aborted", || dropped.load(Ordering::SeqCst)).await;
}

/// A sample TOML value for each `tasks.scheduling` key, in the order of the metadata.
const SAMPLES: [(&str, &str); 5] = [
    ("tasks.scheduling.enabled", "false"),
    ("tasks.scheduling.max-concurrent", "2"),
    ("tasks.scheduling.shutdown-timeout", "\"3s\""),
    ("tasks.scheduling.zone", "\"Europe/Amsterdam\""),
    ("tasks.scheduling.initial-delay", "\"5s\""),
];

#[test]
fn metadata_matches_keys_read() {
    let keys: Vec<_> = TasksAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .collect();
    let expected: Vec<_> = SAMPLES.iter().map(|(k, _)| *k).collect();
    assert_eq!(keys, expected);

    let defaults = TasksSettings::default();
    let read = |toml: &str| TasksSettings::from_config(&Config::from_str(toml).unwrap());
    // Every listed key is read: alone, it changes the settings.
    for (key, value) in SAMPLES {
        let settings = read(&format!("{key} = {value}")).unwrap();
        assert_ne!(settings, defaults, "{key} is not read");
    }
    // Together they change every field, so no field is read from an unlisted key.
    let all: String = SAMPLES
        .iter()
        .map(|(k, v)| format!("{k} = {v}\n"))
        .collect();
    let s = read(&all).unwrap();
    assert!(!s.enabled);
    assert_eq!(s.max_concurrent, 2);
    assert_eq!(s.shutdown_timeout, Duration::from_secs(3));
    assert_eq!(s.zone, Zone::parse("Europe/Amsterdam").unwrap());
    assert_eq!(s.initial_delay, Duration::from_secs(5));
    // Any other key is rejected.
    assert!(read("tasks.scheduling.pool-size = 2").is_err());
    assert!(read("tasks.retry = true").is_err());
    // The documented defaults are the real ones.
    let default_of = |key: &str| {
        TasksAutoConfiguration::properties()
            .iter()
            .find(|p| p.key == key)
            .unwrap()
            .default
    };
    assert_eq!(default_of("tasks.scheduling.enabled"), Some("true"));
    assert_eq!(default_of("tasks.scheduling.max-concurrent"), Some("8"));
    assert_eq!(default_of("tasks.scheduling.shutdown-timeout"), Some("10s"));
    assert_eq!(default_of("tasks.scheduling.zone"), Some("UTC"));
    assert_eq!(default_of("tasks.scheduling.initial-delay"), Some("0s"));
    assert!(defaults.enabled);
    assert_eq!(defaults.max_concurrent, 8);
    assert_eq!(defaults.shutdown_timeout, Duration::from_secs(10));
    assert_eq!(defaults.zone, Zone::Utc);
    assert_eq!(defaults.initial_delay, Duration::ZERO);
}

#[tokio::test]
async fn app_clock_bean_is_used() {
    let at = UNIX_EPOCH + Duration::from_secs(1_900_000_000);
    let grab = Grab::default();
    let (app, _clock) = web(at);
    let app = app
        .listener(grab.clone())
        .configure(hourly(Hourly::default()))
        .start()
        .await;
    let scheduler = grab.scheduler();
    wait_until("first run", || scheduler.tasks()[0].last_end.is_some()).await;
    // The manual clock does not move by itself: the run started and ended at its time.
    assert_eq!(scheduler.tasks()[0].last_start, Some(at));
    assert_eq!(scheduler.tasks()[0].last_end, Some(at));
    app.stop().await;
}

/// Two nightly tasks at 03:00, one without a zone and one in UTC.
#[derive(Clone, Default)]
struct Nightly {
    unzoned: Arc<AtomicUsize>,
    utc: Arc<AtomicUsize>,
}

#[scheduling]
impl Nightly {
    #[scheduled(cron = "0 0 3 * * *")]
    async fn unzoned(&self) {
        self.unzoned.fetch_add(1, Ordering::SeqCst);
    }

    #[scheduled(cron = "0 0 3 * * *", zone = "UTC")]
    async fn utc(&self) {
        self.utc.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn the_configured_zone_applies_to_cron_tasks_without_a_zone() {
    let bean = Nightly::default();
    let (unzoned, utc) = (bean.unzoned.clone(), bean.utc.clone());
    let grab = Grab::default();
    let (app, clock) = web(winter_night());
    let app = app
        .property("tasks.scheduling.zone", "Europe/Amsterdam")
        .listener(grab.clone())
        .configure(move |b| {
            b.instance(bean)
                .provides(|n: Arc<Nightly>| -> Arc<dyn Scheduling> { n })
        })
        .start()
        .await;
    let triggers: Vec<String> = grab
        .scheduler()
        .tasks()
        .into_iter()
        .map(|t| t.trigger)
        .collect();
    assert_eq!(
        triggers,
        [
            "cron 0 0 3 * * * (Europe/Amsterdam)",
            "cron 0 0 3 * * * (UTC)"
        ]
    );
    // 03:00 in Amsterdam is 02:00 UTC in winter: only the zone-less task fires.
    clock.advance(Duration::from_secs(30 * 60 + 1));
    wait_until("the zone-less task ran", || {
        unzoned.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(utc.load(Ordering::SeqCst), 0);
    // An hour later it is 03:00 UTC: the task with its own zone fires.
    clock.advance(HOUR);
    wait_until("the UTC task ran", || utc.load(Ordering::SeqCst) == 1).await;
    assert_eq!(unzoned.load(Ordering::SeqCst), 1);
    app.stop().await;
}

/// Two hourly tasks: one with its own zero initial delay, one without.
#[derive(Clone, Default)]
struct Warm {
    own: Arc<AtomicUsize>,
    plain: Arc<AtomicUsize>,
}

#[scheduling]
impl Warm {
    #[scheduled(fixed_rate = "1h", initial_delay = "0s")]
    async fn own(&self) {
        self.own.fetch_add(1, Ordering::SeqCst);
    }

    #[scheduled(fixed_rate = "1h")]
    async fn plain(&self) {
        self.plain.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn configured_initial_delay_applies_to_tasks_without_one() {
    let bean = Warm::default();
    let (own, plain) = (bean.own.clone(), bean.plain.clone());
    let (app, clock) = web(winter_night());
    let app = app
        .property("tasks.scheduling.initial-delay", "30m")
        .configure(move |b| {
            b.instance(bean)
                .provides(|w: Arc<Warm>| -> Arc<dyn Scheduling> { w })
        })
        .start()
        .await;
    wait_until("the task with its own delay ran", || {
        own.load(Ordering::SeqCst) == 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        plain.load(Ordering::SeqCst),
        0,
        "postponed by the global delay"
    );
    clock.advance(Duration::from_secs(30 * 60));
    wait_until("the plain task ran", || plain.load(Ordering::SeqCst) == 1).await;
    app.stop().await;
}
