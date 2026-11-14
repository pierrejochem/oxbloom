//! `RetryAutoConfiguration`: the `tasks.retry.*` keys become the process-wide retry defaults,
//! with or without a scheduler, and a bad key or value fails the build naming the key.

#![cfg(feature = "tasks")]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use oxbloom_boot::{
    AutoConfiguration, RetryAutoConfiguration, StartupPhase, TasksAutoConfiguration,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_tasks::{
    Backoff, Jitter, RetryDefaults, Scheduler, Scheduling, TasksSettings, defaults, reset_defaults,
    retryable, scheduling, set_defaults,
};
use oxbloom_test::ContextRunner;
use tokio::sync::{Mutex, MutexGuard};

/// The retry defaults are process-wide, so every test in this file runs alone.
static DEFAULTS_LOCK: Mutex<()> = Mutex::const_new(());

/// Holds the lock and restores the built-in defaults when the test ends, also on a panic.
struct Exclusive {
    _guard: MutexGuard<'static, ()>,
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        reset_defaults();
    }
}

async fn exclusive() -> Exclusive {
    let guard = DEFAULTS_LOCK.lock().await;
    reset_defaults();
    Exclusive { _guard: guard }
}

/// All six keys, each with a value other than its built-in default.
const ALL_KEYS: &str = "[tasks.retry]\n\
    max-attempts = 5\n\
    backoff = \"fixed\"\n\
    initial = \"250ms\"\n\
    multiplier = 3.0\n\
    max-delay = \"10s\"\n\
    jitter = \"equal\"\n";

/// The values of [`ALL_KEYS`].
fn all_keys() -> RetryDefaults {
    RetryDefaults {
        max_attempts: 5,
        backoff: Backoff::Fixed,
        initial: Duration::from_millis(250),
        multiplier: 3.0,
        max_delay: Duration::from_secs(10),
        jitter: Jitter::Equal,
    }
}

/// A bean with one hourly task.
struct Hourly;

#[scheduling]
impl Hourly {
    #[scheduled(fixed_rate = "1h")]
    async fn tick(&self) {}
}

fn hourly(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(Hourly)
        .provides(|h: Arc<Hourly>| -> Arc<dyn Scheduling> { h })
}

#[tokio::test]
async fn the_defaults_come_from_the_config_keys() {
    let _exclusive = exclusive().await;
    ContextRunner::new()
        .config_str(ALL_KEYS)
        .auto_configuration::<RetryAutoConfiguration>()
        .run(|view| {
            assert_eq!(*view.get::<RetryDefaults>().unwrap(), all_keys());
            assert_eq!(defaults(), all_keys());
        })
        .await;
    // The defaults stay set after the run, for the beans that outlive it.
    assert_eq!(defaults(), all_keys());
}

#[tokio::test]
async fn it_runs_without_any_scheduler() {
    let _exclusive = exclusive().await;
    ContextRunner::new()
        .property("tasks.retry.max-attempts", "7")
        .auto_configuration::<TasksAutoConfiguration>()
        .auto_configuration::<RetryAutoConfiguration>()
        .run(|view| {
            assert!(!view.contains::<Scheduler>());
            assert_eq!(view.get::<RetryDefaults>().unwrap().max_attempts, 7);
            assert_eq!(defaults().max_attempts, 7);
        })
        .await;
}

#[tokio::test]
async fn it_runs_when_scheduling_is_disabled() {
    let _exclusive = exclusive().await;
    ContextRunner::new()
        .config_str("[tasks.scheduling]\nenabled = false\n[tasks.retry]\nmax-attempts = 6\n")
        .auto_configuration::<TasksAutoConfiguration>()
        .auto_configuration::<RetryAutoConfiguration>()
        .configure(hourly)
        .run(|view| {
            assert!(!view.contains::<Scheduler>());
            assert!(!view.contains::<TasksSettings>());
            assert_eq!(view.get::<RetryDefaults>().unwrap().max_attempts, 6);
            assert_eq!(defaults().max_attempts, 6);
        })
        .await;
}

#[tokio::test]
async fn a_bad_value_fails_the_build_naming_the_key() {
    let _exclusive = exclusive().await;
    let before = RetryDefaults {
        max_attempts: 9,
        ..RetryDefaults::default()
    };
    set_defaults(before);
    ContextRunner::new()
        .property("tasks.retry.max-attempts", "0")
        .auto_configuration::<RetryAutoConfiguration>()
        .run_failing(|failure| {
            let error = failure.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let di = error.find_cause::<DiError>().expect("a DiError cause");
            assert!(matches!(di, DiError::Creation { .. }), "{di:?}");
            let cause = error
                .find_cause::<ConfigError>()
                .expect("a ConfigError cause");
            let text = cause.to_string();
            assert!(text.contains("tasks.retry.max-attempts"), "{text}");
            assert!(!text.contains('0'), "the value is not echoed: {text}");
        })
        .await;
    assert_eq!(defaults(), before, "the defaults stay unchanged");

    // A bad name is not echoed either.
    ContextRunner::new()
        .property("tasks.retry.backoff", "MARKER-7")
        .auto_configuration::<RetryAutoConfiguration>()
        .run_failing(|failure| {
            let cause = failure
                .error()
                .find_cause::<ConfigError>()
                .expect("a ConfigError cause");
            let text = format!("{} {cause}", failure.error());
            assert!(text.contains("tasks.retry.backoff"), "{text}");
            assert!(
                !text.contains("MARKER-7"),
                "the value is not echoed: {text}"
            );
        })
        .await;
    assert_eq!(defaults(), before);
}

#[tokio::test]
async fn an_unknown_key_fails_the_build_by_name() {
    let _exclusive = exclusive().await;
    ContextRunner::new()
        .config_str("[tasks.retry]\nbogus-attempts = 3\n")
        .auto_configuration::<RetryAutoConfiguration>()
        .run_failing(|failure| {
            assert_eq!(failure.error().phase(), StartupPhase::Container);
            let cause = failure
                .error()
                .find_cause::<ConfigError>()
                .expect("a ConfigError cause");
            assert!(cause.to_string().contains("bogus-attempts"), "{cause}");
        })
        .await;
    assert_eq!(defaults(), RetryDefaults::default());
}

#[tokio::test]
async fn scheduling_and_retry_sections_load_together() {
    let _exclusive = exclusive().await;
    ContextRunner::new()
        .config_str("[tasks.scheduling]\nmax-concurrent = 2\n[tasks.retry]\nmax-attempts = 5\n")
        .auto_configuration::<TasksAutoConfiguration>()
        .auto_configuration::<RetryAutoConfiguration>()
        .configure(hourly)
        .run(|view| {
            assert!(view.contains::<Scheduler>());
            assert_eq!(view.get::<TasksSettings>().unwrap().max_concurrent, 2);
            assert_eq!(view.get::<RetryDefaults>().unwrap().max_attempts, 5);
            assert_eq!(defaults().max_attempts, 5);
        })
        .await;
}

/// A sample TOML value for each `tasks.retry` key, in the order of the metadata.
const SAMPLES: [(&str, &str); 6] = [
    ("tasks.retry.max-attempts", "5"),
    ("tasks.retry.backoff", "\"fixed\""),
    ("tasks.retry.initial", "\"250ms\""),
    ("tasks.retry.multiplier", "3.0"),
    ("tasks.retry.max-delay", "\"10s\""),
    ("tasks.retry.jitter", "\"equal\""),
];

#[test]
fn the_metadata_lists_exactly_the_keys_the_reader_accepts() {
    let keys: Vec<_> = RetryAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .collect();
    let expected: Vec<_> = SAMPLES.iter().map(|(k, _)| *k).collect();
    assert_eq!(keys, expected);

    let built_in = RetryDefaults::default();
    let read = |toml: &str| RetryDefaults::from_config(&Config::from_str(toml).unwrap());
    // Every listed key is read: alone, it changes the defaults.
    for (key, value) in SAMPLES {
        let read = read(&format!("{key} = {value}")).unwrap();
        assert_ne!(read, built_in, "{key} is not read");
    }
    // Together they change every field, so no field is read from an unlisted key.
    let all: String = SAMPLES
        .iter()
        .map(|(k, v)| format!("{k} = {v}\n"))
        .collect();
    assert_eq!(read(&all).unwrap(), all_keys());
    // Any other key is rejected.
    assert!(read("tasks.retry.attempts = 2").is_err());

    // The documented types and defaults are the real ones.
    let entry = |key: &str| {
        let p = RetryAutoConfiguration::properties()
            .iter()
            .find(|p| p.key == key)
            .unwrap();
        (p.type_name, p.default)
    };
    assert_eq!(entry("tasks.retry.max-attempts"), ("u32", Some("3")));
    assert_eq!(
        entry("tasks.retry.backoff"),
        ("String", Some("exponential"))
    );
    assert_eq!(entry("tasks.retry.initial"), ("Duration", Some("100ms")));
    assert_eq!(entry("tasks.retry.multiplier"), ("f64", Some("2.0")));
    assert_eq!(entry("tasks.retry.max-delay"), ("Duration", Some("30s")));
    assert_eq!(entry("tasks.retry.jitter"), ("String", Some("full")));
    assert_eq!(built_in.max_attempts, 3);
    assert_eq!(built_in.backoff, Backoff::Exponential);
    assert_eq!(built_in.initial, Duration::from_millis(100));
    assert_eq!(built_in.multiplier, 2.0);
    assert_eq!(built_in.max_delay, Duration::from_secs(30));
    assert_eq!(built_in.jitter, Jitter::Full);
}

#[derive(Debug)]
struct Flaky;

impl fmt::Display for Flaky {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("flaky failure")
    }
}

/// Fails a given number of attempts, then succeeds.
struct Client {
    calls: AtomicU32,
    failures: u32,
}

impl Client {
    #[retryable]
    async fn fetch(&self) -> Result<u32, Flaky> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.failures {
            Err(Flaky)
        } else {
            Ok(attempt)
        }
    }
}

#[tokio::test]
async fn a_retryable_method_uses_the_configured_defaults_end_to_end() {
    let _exclusive = exclusive().await;
    let grabbed: Arc<std::sync::Mutex<Option<Arc<Client>>>> = Arc::default();
    let slot = grabbed.clone();
    ContextRunner::new()
        .config_str("[tasks.retry]\nmax-attempts = 4\ninitial = \"1ms\"\njitter = \"none\"\n")
        .auto_configuration::<RetryAutoConfiguration>()
        .configure(|b| {
            b.instance(Client {
                calls: AtomicU32::new(0),
                failures: 3,
            })
        })
        .run(move |view| *slot.lock().unwrap() = view.get::<Client>().ok())
        .await;
    assert_eq!(defaults().initial, Duration::from_millis(1));
    assert_eq!(defaults().jitter, Jitter::None);

    // The bean of the run: three failures and then a success, which needs the four configured
    // attempts (the built-in default is three).
    let client = grabbed.lock().unwrap().clone().expect("a Client bean");
    assert_eq!(client.fetch().await.unwrap(), 4);
    assert_eq!(client.calls.load(Ordering::SeqCst), 4);

    // Four failures use up the configured attempts.
    let client = Client {
        calls: AtomicU32::new(0),
        failures: 4,
    };
    assert!(client.fetch().await.is_err());
    assert_eq!(client.calls.load(Ordering::SeqCst), 4);
}
