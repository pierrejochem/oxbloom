//! Test helpers: a clock that follows tokio time and can jump, and an event recorder.
//!
//! `oxbloom-test` has its own `ManualClock`, but it depends on `oxbloom-boot`, which will
//! depend on this crate, so a dev-dependency on it would form a cycle.

#![allow(dead_code)]

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxbloom_core::Clock;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// 2026-11-14 12:00:00 UTC, on a whole minute.
pub fn base() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_794_657_600)
}

/// A clock at [`base`] that moves with tokio time (paused in the tests) and can jump with
/// [`ManualClock::advance`] or back with [`ManualClock::rewind`] without moving tokio time.
pub struct ManualClock {
    origin: tokio::time::Instant,
    /// Jumps forward and back.
    offset: Mutex<(Duration, Duration)>,
}

impl ManualClock {
    pub fn new() -> Arc<ManualClock> {
        Arc::new(ManualClock {
            origin: tokio::time::Instant::now(),
            offset: Mutex::new((Duration::ZERO, Duration::ZERO)),
        })
    }

    /// Moves this clock forward by `by`; tokio time does not move.
    pub fn advance(&self, by: Duration) {
        self.offset.lock().unwrap().0 += by;
    }

    /// Moves this clock back by `by`; tokio time does not move.
    pub fn rewind(&self, by: Duration) {
        self.offset.lock().unwrap().1 += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        let (forward, back) = *self.offset.lock().unwrap();
        base() + self.origin.elapsed() + forward - back
    }
}

/// Lets spawned tasks run without moving tokio time.
pub async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

/// Sleeps `millis` of tokio time (the [`ManualClock`] follows), then settles.
pub async fn elapse(millis: u64) {
    tokio::time::sleep(Duration::from_millis(millis)).await;
    settle().await;
}

/// One recorded tracing event.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub level: Level,
    pub target: String,
    pub fields: Vec<(String, String)>,
}

impl Recorded {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Records the tracing events emitted on the current thread until it is dropped.
///
/// One global subscriber is installed for the whole test binary and writes into the sink
/// of the thread that emitted the event. Thread-local `set_default` subscribers race with
/// the callsite interest cache when tests run in parallel and lose events now and then.
/// The tests that capture use the current-thread runtime, so every event of their
/// scheduler is emitted on the test thread.
pub struct Recorder {
    events: Arc<Mutex<Vec<Recorded>>>,
}

thread_local! {
    static SINK: RefCell<Option<Arc<Mutex<Vec<Recorded>>>>> = const { RefCell::new(None) };
}

impl Recorder {
    pub fn capture() -> Recorder {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            tracing::subscriber::set_global_default(Global::default())
                .expect("no other global subscriber in this test binary");
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        SINK.with(|sink| *sink.borrow_mut() = Some(events.clone()));
        Recorder { events }
    }

    pub fn events(&self) -> Vec<Recorded> {
        self.events.lock().unwrap().clone()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        SINK.with(|sink| *sink.borrow_mut() = None);
    }
}

/// The global subscriber behind [`Recorder`].
#[derive(Default)]
struct Global {
    next_id: AtomicU64,
}

struct Fields<'a>(&'a mut Vec<(String, String)>);

impl Visit for Fields<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push((field.name().to_owned(), value.to_owned()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.push((field.name().to_owned(), format!("{value:?}")));
    }
}

impl Subscriber for Global {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let Some(events) = SINK.with(|sink| sink.borrow().clone()) else {
            return;
        };
        let mut fields = Vec::new();
        event.record(&mut Fields(&mut fields));
        events.lock().unwrap().push(Recorded {
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            fields,
        });
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}
