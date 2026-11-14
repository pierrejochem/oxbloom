//! The scheduler runtime against a clock that follows paused tokio time.

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use common::{ManualClock, Recorder, base, elapse, settle};
use oxbloom_core::Clock;
use oxbloom_tasks::{Outcome, Scheduler, SchedulerConfig, Task, Trigger, Zone};
use tracing::Level;

const SECOND: Duration = Duration::from_secs(1);

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A task that counts its runs and finishes at once.
fn counting(name: &str, trigger: Trigger, count: Arc<AtomicUsize>) -> Task {
    Task::new(name, trigger, move || {
        let count = count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    })
}

/// Tracks how many runs are active at once and the highest value seen.
#[derive(Clone, Default)]
struct Concurrency {
    active: Arc<AtomicUsize>,
    max: Arc<AtomicUsize>,
    started: Arc<AtomicUsize>,
}

impl Concurrency {
    /// A task that holds a run for `length` of tokio time.
    fn task(&self, name: &str, trigger: Trigger, length: Duration) -> Task {
        let this = self.clone();
        Task::new(name, trigger, move || {
            let this = this.clone();
            Box::pin(async move {
                this.started.fetch_add(1, Ordering::SeqCst);
                let now = this.active.fetch_add(1, Ordering::SeqCst) + 1;
                this.max.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(length).await;
                this.active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        })
    }
}

fn scheduler(tasks: Vec<Task>, config: SchedulerConfig, clock: &Arc<ManualClock>) -> Scheduler {
    let clock: Arc<dyn Clock> = clock.clone();
    Scheduler::try_new(tasks, config, clock).expect("valid tasks")
}

fn status(scheduler: &Scheduler, name: &str) -> oxbloom_tasks::TaskStatus {
    scheduler
        .tasks()
        .into_iter()
        .find(|status| status.name == name)
        .expect("task exists")
}

#[tokio::test(start_paused = true)]
async fn fixed_rate_runs_every_period() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let task = counting("rate", Trigger::fixed_rate(SECOND).unwrap(), count.clone());
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    settle().await;
    assert_eq!(count.load(Ordering::SeqCst), 1, "first run at start");
    elapse(3_500).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        4,
        "runs at 0, 1, 2 and 3 seconds"
    );
    assert_eq!(
        status(&scheduler, "rate").last_start,
        Some(base() + secs(3))
    );
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn fixed_delay_waits_after_each_run() {
    let clock = ManualClock::new();
    let starts = Arc::new(Mutex::new(Vec::<SystemTime>::new()));
    let task = {
        let (clock, starts) = (clock.clone(), starts.clone());
        Task::new("delay", Trigger::fixed_delay(SECOND).unwrap(), move || {
            let (clock, starts) = (clock.clone(), starts.clone());
            Box::pin(async move {
                starts.lock().unwrap().push(clock.now());
                tokio::time::sleep(secs(2)).await;
                Ok(())
            })
        })
    };
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    elapse(10_000).await;
    let starts = starts.lock().unwrap().clone();
    assert_eq!(
        starts,
        vec![base(), base() + secs(3), base() + secs(6), base() + secs(9)],
        "each run starts one second after the previous one ended"
    );
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn cron_task_fires_at_the_matching_second() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let trigger = Trigger::cron("30 * * * * *", Zone::Utc).unwrap();
    let scheduler = scheduler(
        vec![counting("cron", trigger, count.clone())],
        SchedulerConfig::default(),
        &clock,
    );
    scheduler.start().await;
    elapse(29_900).await;
    assert_eq!(count.load(Ordering::SeqCst), 0, "not before second 30");
    elapse(200).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        status(&scheduler, "cron").last_start,
        Some(base() + secs(30))
    );
    elapse(60_000).await;
    assert_eq!(count.load(Ordering::SeqCst), 2, "once a minute");
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn overlap_is_skipped_and_counted() {
    let clock = ManualClock::new();
    let tracker = Concurrency::default();
    let trigger = Trigger::cron("* * * * * *", Zone::Utc).unwrap();
    let task = tracker.task("slow", trigger, secs(3));
    let recorder = Recorder::capture();
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    elapse(10_500).await;
    let status = status(&scheduler, "slow");
    assert!(status.skipped >= 1, "skipped: {}", status.skipped);
    assert_eq!(
        tracker.max.load(Ordering::SeqCst),
        1,
        "never two runs at once"
    );
    assert!(tracker.started.load(Ordering::SeqCst) >= 2);
    let warnings: Vec<_> = recorder
        .events()
        .into_iter()
        .filter(|e| e.level == Level::WARN && e.target == "oxbloom::tasks")
        .collect();
    assert!(!warnings.is_empty());
    assert_eq!(warnings[0].field("task"), Some("slow"));
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_error_is_recorded_and_the_schedule_continues() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let task = {
        let count = count.clone();
        Task::new("failing", Trigger::fixed_rate(SECOND).unwrap(), move || {
            let count = count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err("database unavailable".to_owned())
            })
        })
    };
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    elapse(2_500).await;
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(
        status(&scheduler, "failing").last_outcome,
        Some(Outcome::Failed)
    );
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_panic_is_recorded_and_the_schedule_continues() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let recorder = Recorder::capture();
    let task = {
        let count = count.clone();
        Task::new(
            "panicking",
            Trigger::fixed_rate(SECOND).unwrap(),
            move || {
                let count = count.clone();
                Box::pin(async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    panic!("deliberate panic in a test task");
                })
            },
        )
    };
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    elapse(2_500).await;
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(
        status(&scheduler, "panicking").last_outcome,
        Some(Outcome::Panicked)
    );
    let errors: Vec<_> = recorder
        .events()
        .into_iter()
        .filter(|e| e.level == Level::ERROR && e.target == "oxbloom::tasks")
        .collect();
    assert_eq!(errors.len(), 3);
    assert_eq!(errors[0].field("task"), Some("panicking"));
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn max_concurrent_bounds_parallel_runs() {
    let clock = ManualClock::new();
    let tracker = Concurrency::default();
    let tasks = ["a", "b", "c"]
        .into_iter()
        .map(|name| tracker.task(name, Trigger::fixed_rate(SECOND).unwrap(), secs(2)))
        .collect();
    let config = SchedulerConfig {
        max_concurrent: 1,
        ..SchedulerConfig::default()
    };
    let scheduler = scheduler(tasks, config, &clock);
    scheduler.start().await;
    elapse(10_000).await;
    assert_eq!(tracker.max.load(Ordering::SeqCst), 1);
    assert!(tracker.started.load(Ordering::SeqCst) >= 4);
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_clock_jump_gives_one_run_not_a_burst() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let task = counting("rate", Trigger::fixed_rate(SECOND).unwrap(), count.clone());
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    settle().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    clock.advance(secs(3_600));
    // The jump is noticed at the end of the current sleep chunk (at most one second).
    elapse(1_500).await;
    assert_eq!(count.load(Ordering::SeqCst), 2, "exactly one extra run");
    assert_eq!(
        status(&scheduler, "rate").last_start,
        Some(base() + secs(3_601)),
        "the run happened on the jumped clock"
    );
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_clock_jump_backwards_does_not_stall_a_fixed_rate() {
    let clock = ManualClock::new();
    // Start an hour ahead so the clock can go back by an hour.
    clock.advance(secs(3_600));
    let count = Arc::new(AtomicUsize::new(0));
    let task = counting("rate", Trigger::fixed_rate(SECOND).unwrap(), count.clone());
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    settle().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    // Without a guard the next run would wait an hour for `last_start + period`.
    clock.rewind(secs(3_600));
    elapse(3_000).await;
    assert_eq!(count.load(Ordering::SeqCst), 4);
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn stop_waits_for_a_run_in_flight() {
    let clock = ManualClock::new();
    let done = Arc::new(AtomicBool::new(false));
    let task = {
        let done = done.clone();
        Task::new("long", Trigger::fixed_rate(secs(60)).unwrap(), move || {
            let done = done.clone();
            Box::pin(async move {
                tokio::time::sleep(secs(3)).await;
                done.store(true, Ordering::SeqCst);
                Ok(())
            })
        })
    };
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    settle().await;
    assert!(!done.load(Ordering::SeqCst));
    scheduler.stop().await;
    assert!(done.load(Ordering::SeqCst), "stop waited for the run");
    assert_eq!(status(&scheduler, "long").last_outcome, Some(Outcome::Ok));
}

#[tokio::test(start_paused = true)]
async fn stop_aborts_at_the_deadline_and_does_not_hang() {
    let clock = ManualClock::new();
    let recorder = Recorder::capture();
    let task = Task::new("stuck", Trigger::fixed_rate(SECOND).unwrap(), || {
        Box::pin(std::future::pending())
    });
    let config = SchedulerConfig {
        shutdown_timeout: SECOND,
        ..SchedulerConfig::default()
    };
    let scheduler = scheduler(vec![task], config, &clock);
    scheduler.start().await;
    settle().await;
    let before = tokio::time::Instant::now();
    scheduler.stop().await;
    assert_eq!(before.elapsed(), SECOND, "stop returned at the deadline");
    let abandoned = recorder
        .events()
        .into_iter()
        .any(|e| e.level == Level::WARN && e.field("task") == Some("stuck"));
    assert!(abandoned, "a warn names the abandoned task");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_returns_with_a_blocked_thread() {
    let clock = ManualClock::new();
    let started = Arc::new(AtomicBool::new(false));
    let task = {
        let started = started.clone();
        Task::new(
            "blocking",
            Trigger::fixed_rate(secs(60)).unwrap(),
            move || {
                let started = started.clone();
                Box::pin(async move {
                    started.store(true, Ordering::SeqCst);
                    // Deliberately blocks the worker thread, which no abort can interrupt.
                    std::thread::sleep(Duration::from_millis(1_500));
                    Ok(())
                })
            },
        )
    };
    let config = SchedulerConfig {
        shutdown_timeout: Duration::from_millis(200),
        ..SchedulerConfig::default()
    };
    let scheduler = scheduler(vec![task], config, &clock);
    scheduler.start().await;
    while !started.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let before = std::time::Instant::now();
    scheduler.stop().await;
    let took = before.elapsed();
    assert!(took < Duration::from_millis(1_000), "stop took {took:?}");
}

#[tokio::test(start_paused = true)]
async fn initial_delay_postpones_the_first_run() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let task = counting(
        "delayed",
        Trigger::fixed_rate(SECOND).unwrap(),
        count.clone(),
    )
    .initial_delay(secs(5));
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    elapse(4_900).await;
    assert_eq!(count.load(Ordering::SeqCst), 0);
    elapse(200).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        status(&scheduler, "delayed").last_start,
        Some(base() + secs(5))
    );
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_configured_initial_delay_applies_to_tasks_without_one() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let task = counting("plain", Trigger::fixed_rate(SECOND).unwrap(), count.clone());
    let config = SchedulerConfig {
        initial_delay: secs(2),
        ..SchedulerConfig::default()
    };
    let scheduler = scheduler(vec![task], config, &clock);
    scheduler.start().await;
    elapse(1_900).await;
    assert_eq!(count.load(Ordering::SeqCst), 0);
    elapse(200).await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn error_with_a_newline_is_one_event() {
    let clock = ManualClock::new();
    let recorder = Recorder::capture();
    let task = Task::new("multiline", Trigger::fixed_rate(secs(60)).unwrap(), || {
        Box::pin(async { Err("first line\nsecond line".to_owned()) })
    });
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    settle().await;
    let errors: Vec<_> = recorder
        .events()
        .into_iter()
        .filter(|e| e.level == Level::ERROR)
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let event = &errors[0];
    assert_eq!(event.target, "oxbloom::tasks");
    assert_eq!(event.field("task"), Some("multiline"));
    assert_eq!(event.field("error"), Some("first line\nsecond line"));
    let message = event.field("message").expect("a message");
    assert!(!message.contains("first line"), "message: {message}");
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn tasks_snapshot_reports_last_outcome() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let ok = counting("ok", Trigger::fixed_rate(secs(10)).unwrap(), count.clone());
    let failing = Task::new("failing", Trigger::fixed_delay(secs(10)).unwrap(), || {
        Box::pin(async { Err("nope".to_owned()) })
    });
    let scheduler = scheduler(vec![ok, failing], SchedulerConfig::default(), &clock);

    let before = scheduler.tasks();
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].name, "ok");
    assert_eq!(before[0].trigger, "fixed rate 10s");
    assert_eq!(before[1].trigger, "fixed delay 10s");
    assert_eq!(before[0].last_start, None);
    assert_eq!(before[0].last_outcome, None);

    scheduler.start().await;
    settle().await;
    let after = scheduler.tasks();
    assert_eq!(after[0].last_outcome, Some(Outcome::Ok));
    assert_eq!(after[0].last_start, Some(base()));
    assert_eq!(after[0].last_end, Some(base()));
    assert_eq!(after[0].skipped, 0);
    assert_eq!(after[1].last_outcome, Some(Outcome::Failed));
    scheduler.stop().await;
}

#[tokio::test(start_paused = true)]
async fn start_is_idempotent() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let task = counting(
        "once",
        Trigger::fixed_rate(secs(60)).unwrap(),
        count.clone(),
    );
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    scheduler.start().await;
    scheduler.start().await;
    settle().await;
    assert_eq!(count.load(Ordering::SeqCst), 1);
    scheduler.stop().await;
}

#[test]
fn an_invalid_task_fails_construction_naming_the_task() {
    let error = Trigger::cron("not a cron", Zone::Utc).unwrap_err();
    let tasks = vec![Task::invalid("Reports::nightly", "cron", error)];
    let clock: Arc<dyn Clock> = Arc::new(oxbloom_core::SystemClock);
    let Err(err) = Scheduler::try_new(tasks, SchedulerConfig::default(), clock) else {
        panic!("an invalid task must fail construction");
    };
    let text = err.to_string();
    assert_eq!(err.key(), "cron");
    assert!(text.contains("Reports::nightly"), "{text}");
    assert!(text.contains("key `cron`"), "{text}");
    assert!(
        !text.contains("not a cron"),
        "the value is not echoed: {text}"
    );
}

#[test]
fn the_default_config_matches_the_spec() {
    let config = SchedulerConfig::default();
    assert_eq!(config.max_concurrent, 8);
    assert_eq!(config.shutdown_timeout, secs(10));
    assert_eq!(config.zone, Zone::Utc);
    assert_eq!(config.initial_delay, Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn dropping_stop_aborts_the_runs() {
    /// Sets the flag when the run's future is dropped.
    struct SetOnDrop(Arc<AtomicBool>);
    impl Drop for SetOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let clock = ManualClock::new();
    let dropped = Arc::new(AtomicBool::new(false));
    let task = {
        let dropped = dropped.clone();
        Task::new("stuck", Trigger::fixed_rate(secs(60)).unwrap(), move || {
            let guard = SetOnDrop(dropped.clone());
            Box::pin(async move {
                let _guard = guard;
                std::future::pending::<()>().await;
                Ok(())
            })
        })
    };
    let config = SchedulerConfig {
        shutdown_timeout: secs(3_600),
        ..SchedulerConfig::default()
    };
    let scheduler = scheduler(vec![task], config, &clock);
    scheduler.start().await;
    settle().await;
    assert!(!dropped.load(Ordering::SeqCst), "the run is in flight");
    // Boot abandons a service's `stop` at its own deadline by dropping the future.
    let abandoned = tokio::time::timeout(SECOND, scheduler.stop()).await;
    assert!(abandoned.is_err(), "stop still waits for the run");
    settle().await;
    assert!(
        dropped.load(Ordering::SeqCst),
        "the run is aborted with the dropped stop"
    );
}

#[test]
fn duplicate_task_names_are_rejected() {
    let count = Arc::new(AtomicUsize::new(0));
    let tasks = vec![
        counting("same", Trigger::fixed_rate(SECOND).unwrap(), count.clone()),
        counting("other", Trigger::fixed_rate(SECOND).unwrap(), count.clone()),
        counting("same", Trigger::fixed_delay(SECOND).unwrap(), count),
    ];
    let clock: Arc<dyn Clock> = Arc::new(oxbloom_core::SystemClock);
    let Err(err) = Scheduler::try_new(tasks, SchedulerConfig::default(), clock) else {
        panic!("duplicate names must fail construction");
    };
    assert_eq!(err.task(), "same");
    assert_eq!(err.key(), "name");
    assert!(err.to_string().contains("`same`"), "{err}");
}

#[test]
fn the_configured_zone_applies_to_cron_triggers_without_one() {
    let count = Arc::new(AtomicUsize::new(0));
    let tasks = || {
        vec![
            counting(
                "unzoned",
                Trigger::cron_in_default_zone("0 0 3 * * *").unwrap(),
                count.clone(),
            ),
            counting(
                "zoned",
                Trigger::cron("0 0 3 * * *", Zone::Utc).unwrap(),
                count.clone(),
            ),
        ]
    };
    let clock: Arc<dyn Clock> = Arc::new(oxbloom_core::SystemClock);
    let config = SchedulerConfig {
        zone: Zone::parse("Europe/Amsterdam").unwrap(),
        ..SchedulerConfig::default()
    };
    let amsterdam = Scheduler::try_new(tasks(), config, clock.clone()).unwrap();
    let triggers: Vec<String> = amsterdam.tasks().into_iter().map(|t| t.trigger).collect();
    assert_eq!(
        triggers,
        [
            "cron 0 0 3 * * * (Europe/Amsterdam)",
            "cron 0 0 3 * * * (UTC)"
        ]
    );
    let utc = Scheduler::try_new(tasks(), SchedulerConfig::default(), clock).unwrap();
    assert_eq!(utc.tasks()[0].trigger, "cron 0 0 3 * * * (UTC)");
}

#[tokio::test(start_paused = true)]
async fn is_running_between_start_and_stop() {
    let clock = ManualClock::new();
    let count = Arc::new(AtomicUsize::new(0));
    let task = counting("tick", Trigger::fixed_rate(secs(60)).unwrap(), count);
    let scheduler = scheduler(vec![task], SchedulerConfig::default(), &clock);
    assert!(!scheduler.is_running());
    scheduler.start().await;
    assert!(scheduler.is_running());
    scheduler.stop().await;
    assert!(!scheduler.is_running());
    scheduler.start().await;
    assert!(!scheduler.is_running(), "a stopped scheduler stays stopped");
}
