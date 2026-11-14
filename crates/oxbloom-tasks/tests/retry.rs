//! Retry: the attempt count, the delays between attempts with paused tokio time, the last
//! error, panics, cancellation, events and the process-wide defaults.

mod common;

use std::cell::Cell;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use common::Recorder;
use oxbloom_tasks::{
    Backoff, Jitter, RetryDefaults, RetryPolicy, defaults, reset_defaults, set_defaults,
};
use tokio::time::Instant;
use tracing::Level;

/// The built-in policy without jitter, so the delays are 100, 200, 400 ms and so on.
fn policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        max_attempts,
        jitter: Jitter::None,
        ..RetryPolicy::default()
    }
}

#[tokio::test(start_paused = true)]
async fn run_succeeds_on_third_attempt() {
    let calls = Cell::new(0_u32);
    let start = Instant::now();
    let result = policy(3)
        .run(
            "charge",
            || {
                calls.set(calls.get() + 1);
                let n = calls.get();
                async move { if n < 3 { Err("boom") } else { Ok(7) } }
            },
            |_| true,
        )
        .await;
    assert_eq!(result, Ok(7));
    assert_eq!(calls.get(), 3);
    assert_eq!(start.elapsed(), Duration::from_millis(100 + 200));
}

#[tokio::test(start_paused = true)]
async fn run_returns_the_last_error_when_attempts_run_out() {
    let calls = Cell::new(0_u32);
    let result: Result<(), String> = policy(3)
        .run(
            "charge",
            || {
                calls.set(calls.get() + 1);
                let n = calls.get();
                async move { Err(format!("failure {n}")) }
            },
            |_| true,
        )
        .await;
    assert_eq!(result, Err("failure 3".to_owned()));
    assert_eq!(calls.get(), 3);
}

#[tokio::test(start_paused = true)]
async fn run_with_one_attempt_does_not_sleep() {
    let calls = Cell::new(0_u32);
    let start = Instant::now();
    let result: Result<(), &str> = policy(1)
        .run(
            "charge",
            || {
                calls.set(calls.get() + 1);
                async { Err("boom") }
            },
            |_| true,
        )
        .await;
    assert_eq!(result, Err("boom"));
    assert_eq!(calls.get(), 1);
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn retry_if_false_returns_at_once() {
    let calls = Cell::new(0_u32);
    let start = Instant::now();
    let result: Result<(), &str> = policy(5)
        .run(
            "charge",
            || {
                calls.set(calls.get() + 1);
                async { Err("declined") }
            },
            |error| *error != "declined",
        )
        .await;
    assert_eq!(result, Err("declined"));
    assert_eq!(calls.get(), 1);
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn run_does_not_catch_a_panic() {
    let calls = Cell::new(0_u32);
    let policy = policy(3);
    let mut future = pin!(policy.run(
        "charge",
        || {
            calls.set(calls.get() + 1);
            async {
                if true {
                    panic!("the op panicked");
                }
                Ok::<(), &str>(())
            }
        },
        |_| true,
    ));
    let mut cx = Context::from_waker(Waker::noop());
    let polled = catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(&mut cx)));
    assert!(polled.is_err(), "the panic reaches the caller");
    assert_eq!(calls.get(), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_future_during_sleep_stops_the_retries() {
    let calls = Cell::new(0_u32);
    let policy = policy(5);
    {
        let mut future = Box::pin(policy.run(
            "charge",
            || {
                calls.set(calls.get() + 1);
                async { Err::<(), &str>("boom") }
            },
            |_| true,
        ));
        std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert_eq!(calls.get(), 1);
        tokio::time::advance(Duration::from_millis(50)).await;
        drop(future);
    }
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(calls.get(), 1);
}

#[tokio::test(start_paused = true)]
async fn huge_max_attempts_is_fine() {
    let calls = Cell::new(0_u32);
    let result = policy(u32::MAX)
        .run(
            "charge",
            || {
                calls.set(calls.get() + 1);
                let n = calls.get();
                async move { if n < 2 { Err("boom") } else { Ok(n) } }
            },
            |_| true,
        )
        .await;
    assert_eq!(result, Ok(2));
    assert_eq!(calls.get(), 2);
}

#[tokio::test(start_paused = true)]
async fn run_with_random_feeds_the_jitter() {
    let start = Instant::now();
    let policy = RetryPolicy {
        jitter: Jitter::Full,
        ..RetryPolicy::default()
    };
    let calls = Cell::new(0_u32);
    let result: Result<(), &str> = policy
        .run_with_random(
            "charge",
            || {
                calls.set(calls.get() + 1);
                async { Err("boom") }
            },
            |_| true,
            || 0.5,
        )
        .await;
    assert_eq!(result, Err("boom"));
    assert_eq!(calls.get(), 3);
    assert_eq!(start.elapsed(), Duration::from_millis(50 + 100));
}

#[tokio::test(start_paused = true)]
async fn events_log_each_retry_and_exhaustion() {
    let recorder = Recorder::capture();
    let result: Result<(), &str> = policy(3)
        .run("charge", || async { Err("boom") }, |_| true)
        .await;
    assert_eq!(result, Err("boom"));
    let events: Vec<_> = recorder
        .events()
        .into_iter()
        .filter(|e| e.target == "oxbloom::tasks")
        .collect();
    let warns: Vec<_> = events.iter().filter(|e| e.level == Level::WARN).collect();
    let errors: Vec<_> = events.iter().filter(|e| e.level == Level::ERROR).collect();
    assert_eq!(warns.len(), 2, "{events:?}");
    assert_eq!(errors.len(), 1, "{events:?}");
    assert_eq!(warns[0].field("method"), Some("charge"));
    assert_eq!(warns[0].field("attempt"), Some("1"));
    assert_eq!(warns[0].field("delay_ms"), Some("100"));
    assert_eq!(warns[0].field("error"), Some("boom"));
    assert_eq!(warns[1].field("attempt"), Some("2"));
    assert_eq!(warns[1].field("delay_ms"), Some("200"));
    assert_eq!(errors[0].field("method"), Some("charge"));
    assert_eq!(errors[0].field("attempts"), Some("3"));
    assert_eq!(errors[0].field("error"), Some("boom"));
    drop(recorder);

    let recorder = Recorder::capture();
    let result: Result<u32, &str> = policy(3).run("charge", || async { Ok(1) }, |_| true).await;
    assert_eq!(result, Ok(1));
    assert!(recorder.events().is_empty(), "{:?}", recorder.events());
}

#[test]
fn defaults_round_trip() {
    // The only test in this binary that touches the process-wide defaults.
    assert_eq!(defaults(), RetryDefaults::default());
    let custom = RetryDefaults {
        max_attempts: 7,
        backoff: Backoff::Fixed,
        initial: Duration::from_millis(250),
        multiplier: 3.0,
        max_delay: Duration::from_secs(5),
        jitter: Jitter::Equal,
    };
    set_defaults(custom);
    assert_eq!(defaults(), custom);
    assert_eq!(
        defaults().policy(),
        RetryPolicy {
            max_attempts: 7,
            backoff: Backoff::Fixed,
            initial: Duration::from_millis(250),
            multiplier: 3.0,
            max_delay: Duration::from_secs(5),
            jitter: Jitter::Equal,
        }
    );
    reset_defaults();
    assert_eq!(defaults(), RetryDefaults::default());
    assert_eq!(defaults().policy(), RetryPolicy::default());
}
