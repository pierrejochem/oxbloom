//! `#[scheduling]` and `#[scheduled]`: the tasks they generate and how those tasks run.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use common::Recorder;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_tasks::{
    Outcome, Scheduler, SchedulerConfig, Scheduling, Task, Trigger, Zone, scheduling,
};

fn try_scheduler(tasks: Vec<Task>) -> Result<Scheduler, oxbloom_tasks::TaskError> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    Scheduler::try_new(tasks, SchedulerConfig::default(), clock)
}

/// Polls `done` every few milliseconds of real time for up to five seconds.
async fn wait_until(mut done: impl FnMut() -> bool) {
    for _ in 0..1_000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition not reached within five seconds");
}

struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(cron = "0 0 3 * * *", zone = "Europe/Amsterdam")]
    async fn nightly(&self) {}

    #[scheduled(fixed_rate = "30s", initial_delay = "5s")]
    fn refresh(&self) {}

    #[scheduled(fixed_delay = "250ms", name = "reports-cleanup")]
    async fn cleanup(&self) -> Result<(), String> {
        Ok(())
    }

    /// Not scheduled: left alone.
    #[allow(dead_code)]
    fn helper(&self) -> u32 {
        7
    }
}

#[test]
fn generates_one_task_per_method() {
    let tasks = Arc::new(Reports).scheduled();
    let names: Vec<&str> = tasks.iter().map(Task::name).collect();
    assert_eq!(
        names,
        ["Reports::nightly", "Reports::refresh", "reports-cleanup"]
    );

    let amsterdam = Zone::parse("Europe/Amsterdam").unwrap();
    assert_eq!(
        tasks[0].trigger(),
        Some(&Trigger::cron("0 0 3 * * *", amsterdam).unwrap())
    );
    assert_eq!(tasks[0].delay(), None);
    assert_eq!(
        tasks[1].trigger(),
        Some(&Trigger::fixed_rate(Duration::from_secs(30)).unwrap())
    );
    assert_eq!(tasks[1].delay(), Some(Duration::from_secs(5)));
    assert_eq!(
        tasks[2].trigger(),
        Some(&Trigger::fixed_delay(Duration::from_millis(250)).unwrap())
    );
}

#[test]
fn a_cron_trigger_without_a_zone_takes_the_scheduler_zone() {
    struct Nightly;

    #[scheduling]
    impl Nightly {
        #[scheduled(cron = "0 0 3 * * *")]
        async fn run(&self) {}
    }

    let tasks = Arc::new(Nightly).scheduled();
    assert_eq!(
        tasks[0].trigger(),
        Some(&Trigger::cron_in_default_zone("0 0 3 * * *").unwrap())
    );
    // UTC unless the scheduler is configured with another zone.
    let utc = try_scheduler(Arc::new(Nightly).scheduled()).unwrap();
    assert_eq!(utc.tasks()[0].trigger, "cron 0 0 3 * * * (UTC)");
    let config = SchedulerConfig {
        zone: Zone::parse("Europe/Amsterdam").unwrap(),
        ..SchedulerConfig::default()
    };
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let amsterdam = Scheduler::try_new(tasks, config, clock).unwrap();
    assert_eq!(
        amsterdam.tasks()[0].trigger,
        "cron 0 0 3 * * * (Europe/Amsterdam)"
    );
}

#[test]
fn a_repeated_name_fails_the_scheduler() {
    struct Twice;

    #[scheduling]
    impl Twice {
        #[scheduled(fixed_rate = "1s", name = "twice")]
        async fn one(&self) {}

        #[scheduled(fixed_rate = "2s", name = "twice")]
        async fn two(&self) {}
    }

    let err = try_scheduler(Arc::new(Twice).scheduled()).expect_err("duplicate name");
    assert_eq!((err.task(), err.key()), ("twice", "name"));
}

/// Records where each method ran.
#[derive(Default)]
struct Probe {
    async_runs: AtomicUsize,
    plain_runs: AtomicUsize,
    async_thread: Mutex<Option<ThreadId>>,
    plain_thread: Mutex<Option<ThreadId>>,
}

#[scheduling]
impl Probe {
    #[scheduled(fixed_rate = "1h")]
    async fn in_the_runtime(&self) {
        tokio::task::yield_now().await;
        *self.async_thread.lock().unwrap() = Some(std::thread::current().id());
        self.async_runs.fetch_add(1, Ordering::SeqCst);
    }

    #[scheduled(fixed_rate = "1h")]
    fn blocking(&self) -> Result<(), std::io::Error> {
        *self.plain_thread.lock().unwrap() = Some(std::thread::current().id());
        self.plain_runs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn async_and_plain_methods_run() {
    let probe = Arc::new(Probe::default());
    let scheduler = try_scheduler(probe.clone().scheduled()).expect("valid tasks");
    scheduler.start().await;
    wait_until(|| {
        probe.async_runs.load(Ordering::SeqCst) == 1 && probe.plain_runs.load(Ordering::SeqCst) == 1
    })
    .await;
    scheduler.stop().await;

    // The current-thread runtime runs every async task on the test thread; a plain `fn` runs
    // on a thread of the blocking pool.
    let test_thread = std::thread::current().id();
    assert_eq!(*probe.async_thread.lock().unwrap(), Some(test_thread));
    let plain = probe
        .plain_thread
        .lock()
        .unwrap()
        .expect("plain method ran");
    assert_ne!(plain, test_thread, "a plain fn runs in spawn_blocking");
    for status in scheduler.tasks() {
        assert_eq!(status.last_outcome, Some(Outcome::Ok), "{}", status.name);
    }
}

struct Failing;

#[derive(Debug)]
struct Quota(u32);

impl std::fmt::Display for Quota {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "quota of {} exceeded", self.0)
    }
}

#[scheduling]
impl Failing {
    #[scheduled(fixed_rate = "1h")]
    async fn upload(&self) -> Result<(), Quota> {
        Err(Quota(3))
    }

    #[scheduled(fixed_rate = "1h")]
    fn sync(&self) -> Result<(), String> {
        Err("remote unreachable".to_owned())
    }
}

#[tokio::test]
async fn result_error_is_a_failed_run_with_the_message() {
    let recorder = Recorder::capture();
    let scheduler = try_scheduler(Arc::new(Failing).scheduled()).expect("valid tasks");
    scheduler.start().await;
    wait_until(|| {
        scheduler
            .tasks()
            .iter()
            .all(|status| status.last_outcome.is_some())
    })
    .await;
    scheduler.stop().await;

    for status in scheduler.tasks() {
        assert_eq!(
            status.last_outcome,
            Some(Outcome::Failed),
            "{}",
            status.name
        );
    }
    let failures: Vec<(String, String)> = recorder
        .events()
        .into_iter()
        .filter(|event| event.field("message") == Some("scheduled task failed"))
        .map(|event| {
            (
                event.field("task").unwrap_or_default().to_owned(),
                event.field("error").unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert!(
        failures.contains(&(
            "Failing::upload".to_owned(),
            "quota of 3 exceeded".to_owned()
        )),
        "{failures:?}"
    );
    assert!(
        failures.contains(&("Failing::sync".to_owned(), "remote unreachable".to_owned())),
        "{failures:?}"
    );
}

#[test]
fn bad_cron_literal_surfaces_as_a_startup_error_naming_the_task() {
    struct Jobs;

    #[scheduling]
    impl Jobs {
        #[scheduled(fixed_rate = "1s")]
        async fn fine(&self) {}

        #[scheduled(cron = "every night at three")]
        async fn nightly(&self) {}
    }

    let tasks = Arc::new(Jobs).scheduled();
    assert_eq!(tasks.len(), 2, "the invalid task is still listed");
    assert!(tasks[1].trigger().is_none());
    let Err(err) = try_scheduler(tasks) else {
        panic!("a bad cron literal must fail startup");
    };
    assert_eq!(err.task(), "Jobs::nightly");
    assert_eq!(err.key(), "cron");
    let text = err.to_string();
    assert!(text.contains("Jobs::nightly"), "{text}");
    assert!(text.contains("`cron`"), "{text}");
    assert!(
        !text.contains("every night"),
        "the value is not echoed: {text}"
    );
}

#[test]
fn zero_fixed_rate_surfaces_as_a_startup_error() {
    struct Poller;

    #[scheduling]
    impl Poller {
        #[scheduled(fixed_rate = "0s")]
        async fn poll(&self) {}
    }

    let Err(err) = try_scheduler(Arc::new(Poller).scheduled()) else {
        panic!("a zero fixed rate must fail startup");
    };
    assert_eq!(err.task(), "Poller::poll");
    assert_eq!(err.key(), "fixed_rate");
    assert_eq!(
        err.to_string(),
        "scheduled task `Poller::poll`, key `fixed_rate`: fixed rate must be greater than zero"
    );
}

#[test]
fn each_key_is_named_in_its_startup_error() {
    struct Keys;

    #[scheduling]
    impl Keys {
        #[scheduled(fixed_delay = "soon")]
        async fn delay(&self) {}

        #[scheduled(fixed_rate = "1s", initial_delay = "-5s")]
        async fn warm(&self) {}

        #[scheduled(cron = "0 0 3 * * *", zone = "Mars/Olympus_Mons")]
        async fn zoned(&self) {}
    }

    let tasks = Arc::new(Keys).scheduled();
    let keys: Vec<(String, &str)> = tasks
        .into_iter()
        .map(|task| {
            let err = try_scheduler(vec![task]).expect_err("invalid task");
            (err.task().to_owned(), err.key())
        })
        .collect();
    assert_eq!(
        keys,
        [
            ("Keys::delay".to_owned(), "fixed_delay"),
            ("Keys::warm".to_owned(), "initial_delay"),
            ("Keys::zoned".to_owned(), "zone"),
        ]
    );
}

#[test]
fn the_generated_impl_is_a_dyn_scheduling_source() {
    let source: Arc<dyn Scheduling> = Arc::new(Reports);
    assert_eq!(source.scheduled().len(), 3);
}

#[test]
fn spelled_out_self_and_result_aliases_are_accepted() {
    struct Shapes;

    #[scheduling]
    impl Shapes {
        #[allow(clippy::needless_arbitrary_self_type)]
        #[scheduled(fixed_rate = "1s")]
        fn spelled(self: &Self) -> std::io::Result<()> {
            Ok(())
        }

        #[allow(clippy::unused_unit)]
        #[scheduled(fixed_rate = "1s")]
        fn unit(&self) -> () {}
    }

    assert_eq!(Arc::new(Shapes).scheduled().len(), 2);
}
