//! `#[retryable]`: the generated retry loop, its policy, and how it combines with
//! `#[scheduling]`.

mod common;

use std::fmt;
use std::num::ParseIntError;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use common::{ManualClock, elapse, settle};
use oxbloom_core::Clock;
use oxbloom_tasks::{
    Jitter, RetryDefaults, Scheduler, SchedulerConfig, Scheduling, reset_defaults, retryable,
    scheduling, set_defaults,
};
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::Instant;

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

#[derive(Debug, PartialEq)]
enum MyError {
    Transient,
    Permanent,
    Parse(ParseIntError),
}

impl fmt::Display for MyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MyError::Transient => f.write_str("transient failure"),
            MyError::Permanent => f.write_str("permanent failure"),
            MyError::Parse(error) => write!(f, "not a number: {error}"),
        }
    }
}

impl From<ParseIntError> for MyError {
    fn from(error: ParseIntError) -> Self {
        MyError::Parse(error)
    }
}

fn is_transient(error: &MyError) -> bool {
    *error == MyError::Transient
}

/// Counts the attempts of its methods.
struct Service {
    calls: Arc<AtomicU32>,
    /// How many attempts fail before one succeeds.
    failures: u32,
}

impl Service {
    fn failing(failures: u32) -> Service {
        Service {
            calls: Arc::new(AtomicU32::new(0)),
            failures,
        }
    }

    /// Counts one attempt and returns its number, from 1.
    fn tick(&self) -> u32 {
        self.calls.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    #[retryable(max_attempts = 4, jitter = "none", initial = "10ms")]
    async fn fetch(&self) -> Result<u32, MyError> {
        let attempt = self.tick();
        if attempt <= self.failures {
            Err(MyError::Transient)
        } else {
            Ok(attempt)
        }
    }

    #[retryable(max_attempts = 3)]
    async fn with_key(&self) -> Result<(), MyError> {
        self.tick();
        Err(MyError::Transient)
    }

    #[retryable]
    async fn without_key(&self) -> Result<(), MyError> {
        self.tick();
        Err(MyError::Transient)
    }

    #[retryable(
        max_attempts = 3,
        jitter = "none",
        initial = "1ms",
        retry_if = "is_transient"
    )]
    async fn fail_with(&self, error: &MyError) -> Result<(), MyError> {
        self.tick();
        Err(match error {
            MyError::Transient => MyError::Transient,
            _ => MyError::Permanent,
        })
    }

    #[retryable(max_attempts = 2, jitter = "none", initial = "1ms")]
    async fn parse(&self, text: &str) -> Result<u32, MyError> {
        self.tick();
        let number = text.parse::<u32>()?;
        Ok(number)
    }

    #[retryable(max_attempts = 3, jitter = "none", initial = "1ms")]
    async fn early_return(&self) -> Result<u32, MyError> {
        let attempt = self.tick();
        if attempt < 2 {
            return Err(MyError::Transient);
        }
        Ok(attempt * 10)
    }

    #[retryable(max_attempts = 3, jitter = "none", initial = "1ms")]
    async fn greet(&self, name: String) -> Result<String, MyError> {
        let attempt = self.tick();
        let owned = name.clone();
        if attempt < 2 {
            Err(MyError::Transient)
        } else {
            Ok(format!("hello {owned}"))
        }
    }

    #[retryable(max_attempts = 3, jitter = "none", initial = "1ms")]
    async fn count(&self, items: &[u32]) -> Result<usize, MyError> {
        let attempt = self.tick();
        if attempt < 2 {
            Err(MyError::Transient)
        } else {
            Ok(items.len())
        }
    }

    #[retryable(max_attempts = 3, jitter = "none", initial = "1ms")]
    async fn double(value: u32) -> Result<u32, MyError> {
        let attempt = ASSOCIATED_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt < 3 {
            Err(MyError::Transient)
        } else {
            Ok(value * 2)
        }
    }
}

static ASSOCIATED_CALLS: AtomicU32 = AtomicU32::new(0);

#[tokio::test(start_paused = true)]
async fn a_retryable_method_retries_until_it_succeeds() {
    let _exclusive = exclusive().await;
    let service = Service::failing(2);
    let start = Instant::now();
    assert_eq!(service.fetch().await, Ok(3));
    assert_eq!(service.calls(), 3);
    // Exponential backoff from 10 ms with the default multiplier 2: 10 ms, then 20 ms.
    assert_eq!(start.elapsed(), Duration::from_millis(30));
}

#[tokio::test(start_paused = true)]
async fn the_attribute_overrides_the_global_defaults() {
    let _exclusive = exclusive().await;
    set_defaults(RetryDefaults {
        max_attempts: 1,
        jitter: Jitter::None,
        initial: Duration::from_millis(1),
        ..RetryDefaults::default()
    });
    let service = Service::failing(u32::MAX);
    assert_eq!(service.with_key().await, Err(MyError::Transient));
    assert_eq!(service.calls(), 3, "the attribute's max_attempts wins");

    let service = Service::failing(u32::MAX);
    assert_eq!(service.without_key().await, Err(MyError::Transient));
    assert_eq!(service.calls(), 1, "the global max_attempts applies");
}

#[tokio::test(start_paused = true)]
async fn omitted_fields_follow_the_global_defaults_at_call_time() {
    let _exclusive = exclusive().await;
    let service = Service::failing(u32::MAX);
    let defaults = RetryDefaults {
        jitter: Jitter::None,
        initial: Duration::from_millis(1),
        ..RetryDefaults::default()
    };
    set_defaults(RetryDefaults {
        max_attempts: 2,
        ..defaults
    });
    let _ = service.without_key().await;
    assert_eq!(service.calls(), 2);

    set_defaults(RetryDefaults {
        max_attempts: 4,
        ..defaults
    });
    let _ = service.without_key().await;
    assert_eq!(
        service.calls(),
        2 + 4,
        "the second call reads the new defaults"
    );
}

#[tokio::test(start_paused = true)]
async fn retry_if_stops_on_a_permanent_error() {
    let _exclusive = exclusive().await;
    let service = Service::failing(u32::MAX);
    assert_eq!(
        service.fail_with(&MyError::Permanent).await,
        Err(MyError::Permanent)
    );
    assert_eq!(service.calls(), 1, "a permanent error is not retried");

    let service = Service::failing(u32::MAX);
    assert_eq!(
        service.fail_with(&MyError::Transient).await,
        Err(MyError::Transient)
    );
    assert_eq!(service.calls(), 3, "a transient error is retried");
}

#[tokio::test(start_paused = true)]
async fn question_mark_with_from_conversion_works() {
    let _exclusive = exclusive().await;
    let service = Service::failing(0);
    assert!(matches!(service.parse("x").await, Err(MyError::Parse(_))));
    assert_eq!(service.calls(), 2, "the converted error is retried");

    let service = Service::failing(0);
    assert_eq!(service.parse("7").await, Ok(7));
    assert_eq!(service.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn early_return_inside_the_body_ends_the_attempt() {
    let _exclusive = exclusive().await;
    let service = Service::failing(0);
    assert_eq!(service.early_return().await, Ok(20));
    assert_eq!(service.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_string_argument_cloned_inside_the_body_works() {
    let _exclusive = exclusive().await;
    let service = Service::failing(0);
    assert_eq!(
        service.greet("ada".to_owned()).await,
        Ok("hello ada".to_owned())
    );
    assert_eq!(service.calls(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_borrowed_argument_and_an_associated_fn_without_self_work() {
    let _exclusive = exclusive().await;
    let service = Service::failing(0);
    let items = vec![1, 2, 3];
    assert_eq!(service.count(&items).await, Ok(3));
    assert_eq!(service.calls(), 2);

    ASSOCIATED_CALLS.store(0, Ordering::SeqCst);
    assert_eq!(Service::double(21).await, Ok(42));
    assert_eq!(ASSOCIATED_CALLS.load(Ordering::SeqCst), 3);
}

/// Calls the method like any other async method; it would not compile if the signature had
/// changed.
async fn call_through(service: &Service) -> Result<u32, MyError> {
    service.fetch().await
}

fn assert_send<T: Send>(value: T) -> T {
    value
}

#[tokio::test(start_paused = true)]
async fn the_signature_is_unchanged() {
    let _exclusive = exclusive().await;
    let service = Service::failing(0);
    assert_eq!(call_through(&service).await, Ok(1));
    // The future stays `Send`, so it can be spawned.
    assert_eq!(assert_send(service.fetch()).await, Ok(2));
}

/// A scheduled job whose retries take longer than its period.
#[derive(Default)]
struct Job {
    sync_calls: AtomicU32,
    swapped_calls: AtomicU32,
}

#[scheduling]
impl Job {
    #[scheduled(fixed_rate = "1s")]
    #[retryable(max_attempts = 3, backoff = "fixed", initial = "2s", jitter = "none")]
    async fn sync(&self) -> Result<(), MyError> {
        self.sync_calls.fetch_add(1, Ordering::SeqCst);
        Err(MyError::Transient)
    }

    #[retryable(max_attempts = 3, backoff = "fixed", initial = "2s", jitter = "none")]
    #[scheduled(fixed_rate = "1s")]
    async fn swapped(&self) -> Result<(), MyError> {
        self.swapped_calls.fetch_add(1, Ordering::SeqCst);
        Err(MyError::Transient)
    }
}

#[tokio::test(start_paused = true)]
async fn retries_outlasting_the_period_skip_the_next_trigger() {
    let _exclusive = exclusive().await;
    let job = Arc::new(Job::default());
    let clock = ManualClock::new();
    let clock: Arc<dyn Clock> = clock;
    let scheduler = Scheduler::try_new(job.clone().scheduled(), SchedulerConfig::default(), clock)
        .expect("valid tasks");
    scheduler.start().await;
    settle().await;
    // Attempts at 0 s and 2 s; the third one would come at 4 s.
    elapse(3_500).await;
    assert_eq!(job.sync_calls.load(Ordering::SeqCst), 2);
    assert_eq!(job.swapped_calls.load(Ordering::SeqCst), 2);
    for status in scheduler.tasks() {
        assert_eq!(status.last_end, None, "{}: one run in flight", status.name);
        assert!(
            status.skipped > 0,
            "{}: skipped {}",
            status.name,
            status.skipped
        );
    }
    scheduler.stop().await;
}

/// Every key at once: the generated `Overrides` has no struct update left, which Clippy
/// would otherwise report.
struct Configured;

impl Configured {
    #[retryable(
        max_attempts = 2,
        backoff = "fixed",
        initial = "5ms",
        multiplier = 1.5,
        max_delay = "1s",
        jitter = "none",
        retry_if = "is_transient"
    )]
    async fn call(&self, error: &MyError) -> Result<(), MyError> {
        Err(match error {
            MyError::Transient => MyError::Transient,
            _ => MyError::Permanent,
        })
    }
}

#[tokio::test(start_paused = true)]
async fn every_key_at_once_fixes_the_whole_policy() {
    let _exclusive = exclusive().await;
    set_defaults(RetryDefaults {
        max_attempts: 9,
        ..RetryDefaults::default()
    });
    let start = Instant::now();
    assert_eq!(
        Configured.call(&MyError::Transient).await,
        Err(MyError::Transient)
    );
    // Two attempts with one fixed 5 ms delay between them, whatever the defaults say.
    assert_eq!(start.elapsed(), Duration::from_millis(5));
}
