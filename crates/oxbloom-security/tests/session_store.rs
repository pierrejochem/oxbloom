//! Behaviour of the session store trait and the in-memory store.
//!
//! Time is driven by a manual clock, so every timeout is exact. The handle (`Session`), the id
//! generator and the fingerprint are crate-private constructors, so their tests live next to
//! the code in `src/session/handle.rs`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use oxbloom_core::Clock;
use oxbloom_security::{InMemorySessionStore, SessionData, SessionSettings, SessionStore};
use serde_json::json;

/// A clock that moves only when the test says so.
struct ManualClock(Mutex<SystemTime>);

impl ManualClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
        )))
    }

    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

const IDLE: Duration = Duration::from_secs(10);
const ABSOLUTE: Duration = Duration::from_secs(30);

fn settings() -> SessionSettings {
    SessionSettings::default()
        .idle_timeout(IDLE)
        .absolute_timeout(ABSOLUTE)
}

fn store_with(settings: &SessionSettings) -> (InMemorySessionStore, Arc<ManualClock>) {
    let clock = ManualClock::new();
    let store = InMemorySessionStore::new(settings, clock.clone());
    (store, clock)
}

fn data_with(clock: &ManualClock, key: &str, value: serde_json::Value) -> SessionData {
    let mut data = SessionData::new(clock.now());
    data.values_mut().insert(key.to_owned(), value);
    data
}

#[tokio::test]
async fn save_load_delete_round_trip() {
    let (store, clock) = store_with(&settings());
    let data = data_with(&clock, "user", json!("alice"));
    store.save("id-1", &data, IDLE).await.unwrap();

    let loaded = store.load("id-1").await.unwrap().expect("stored");
    assert_eq!(loaded, data);
    assert_eq!(loaded.values().get("user"), Some(&json!("alice")));

    store.delete("id-1").await.unwrap();
    assert_eq!(store.load("id-1").await.unwrap(), None);
    // Deleting an unknown id is not an error.
    store.delete("id-1").await.unwrap();
}

#[tokio::test]
async fn load_unknown_is_none() {
    let (store, _clock) = store_with(&settings());
    assert_eq!(store.load("nobody").await.unwrap(), None);
}

#[tokio::test]
async fn idle_timeout_expires() {
    let (store, clock) = store_with(&settings());
    store
        .save("id", &data_with(&clock, "k", json!(1)), IDLE)
        .await
        .unwrap();

    clock.advance(IDLE - Duration::from_secs(1));
    assert!(store.load("id").await.unwrap().is_some());

    clock.advance(Duration::from_secs(1));
    assert_eq!(store.load("id").await.unwrap(), None);
    assert_eq!(store.len(), 0, "an expired session is removed on access");
}

#[tokio::test]
async fn touch_extends_idle_but_absolute_wins() {
    let (store, clock) = store_with(&settings());
    let mut data = data_with(&clock, "k", json!(1));
    store.save("id", &data, IDLE).await.unwrap();

    // Touch every 8 seconds: the idle timeout never fires, the absolute one does at 30 seconds.
    for _ in 0..3 {
        clock.advance(Duration::from_secs(8));
        let mut loaded = store.load("id").await.unwrap().expect("still alive");
        loaded.touch(clock.now());
        store.save("id", &loaded, IDLE).await.unwrap();
        data = loaded;
    }
    assert_eq!(data.created() + Duration::from_secs(24), data.touched());

    clock.advance(Duration::from_secs(5));
    assert!(store.load("id").await.unwrap().is_some(), "29 seconds old");
    clock.advance(Duration::from_secs(1));
    assert_eq!(
        store.load("id").await.unwrap(),
        None,
        "the absolute timeout counts from creation and a save does not extend it"
    );
}

#[tokio::test]
async fn rotate_moves_data_and_deletes_old() {
    let (store, clock) = store_with(&settings());
    let data = data_with(&clock, "k", json!("v"));
    store.save("old", &data, IDLE).await.unwrap();

    store.rotate("old", "new").await.unwrap();
    assert_eq!(store.load("old").await.unwrap(), None);
    assert_eq!(store.load("new").await.unwrap(), Some(data));
    assert_eq!(store.len(), 1);

    // Rotating an unknown id is a no-op.
    store.rotate("gone", "other").await.unwrap();
    assert_eq!(store.load("other").await.unwrap(), None);
}

#[tokio::test]
async fn rotate_keeps_created() {
    let (store, clock) = store_with(&settings());
    let data = data_with(&clock, "k", json!("v"));
    let created = data.created();
    store.save("old", &data, IDLE).await.unwrap();

    // Keep the session alive past the idle timeout, then rotate at 16 seconds.
    clock.advance(Duration::from_secs(8));
    let mut touched = store.load("old").await.unwrap().expect("alive");
    touched.touch(clock.now());
    store.save("old", &touched, IDLE).await.unwrap();
    clock.advance(Duration::from_secs(8));
    store.rotate("old", "new").await.unwrap();
    let loaded = store.load("new").await.unwrap().expect("rotated");
    assert_eq!(loaded.created(), created);

    // The absolute timeout still counts from the original creation: touched at 24 seconds,
    // the session is gone at 30.
    clock.advance(Duration::from_secs(8));
    let mut touched = loaded;
    touched.touch(clock.now());
    store.save("new", &touched, IDLE).await.unwrap();
    clock.advance(Duration::from_secs(5));
    assert!(store.load("new").await.unwrap().is_some());
    clock.advance(Duration::from_secs(1));
    assert_eq!(store.load("new").await.unwrap(), None);
}

#[tokio::test]
async fn cap_evicts_the_oldest_touched() {
    let settings = settings().max_sessions(3);
    let (store, clock) = store_with(&settings);

    for id in ["a", "b", "c"] {
        store
            .save(id, &data_with(&clock, "id", json!(id)), IDLE)
            .await
            .unwrap();
        clock.advance(Duration::from_secs(1));
    }
    // Touch "a" so that "b" becomes the least recently touched.
    let mut a = store.load("a").await.unwrap().unwrap();
    a.touch(clock.now());
    store.save("a", &a, IDLE).await.unwrap();
    clock.advance(Duration::from_secs(1));

    store
        .save("d", &data_with(&clock, "id", json!("d")), IDLE)
        .await
        .unwrap();
    assert_eq!(store.len(), 3);
    assert_eq!(store.load("b").await.unwrap(), None, "b was evicted");
    for id in ["a", "c", "d"] {
        assert!(store.load(id).await.unwrap().is_some(), "{id} survives");
    }

    // Saving an id that already exists never evicts.
    store
        .save("c", &data_with(&clock, "id", json!("c2")), IDLE)
        .await
        .unwrap();
    assert_eq!(store.len(), 3);
    assert!(store.load("a").await.unwrap().is_some());
}

#[tokio::test]
async fn sweep_removes_expired() {
    let (store, clock) = store_with(&settings());
    store
        .save(
            "short",
            &data_with(&clock, "k", json!(1)),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
    store
        .save("long", &data_with(&clock, "k", json!(2)), IDLE)
        .await
        .unwrap();
    assert_eq!(store.len(), 2);

    clock.advance(Duration::from_secs(5));
    store.sweep();
    assert_eq!(store.len(), 1);
    assert!(store.load("long").await.unwrap().is_some());

    // `maintain` is the trait hook the filter calls; it sweeps.
    clock.advance(IDLE);
    store.maintain().await;
    assert_eq!(store.len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_saves_of_one_id_do_not_panic() {
    let (store, clock) = store_with(&settings().max_sessions(10));
    let store = Arc::new(store);
    let mut tasks = Vec::new();
    for i in 0..50 {
        let store = store.clone();
        let data = data_with(&clock, "n", json!(i));
        tasks.push(tokio::spawn(async move {
            store.save("shared", &data, IDLE).await.unwrap();
            store.load("shared").await.unwrap().expect("present");
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(store.len(), 1);
}

#[tokio::test]
async fn store_is_usable_as_a_trait_object() {
    let (store, clock) = store_with(&settings());
    let store: Arc<dyn SessionStore> = Arc::new(store);
    store
        .save("id", &data_with(&clock, "k", json!(true)), IDLE)
        .await
        .unwrap();
    assert!(store.load("id").await.unwrap().is_some());
}

#[tokio::test]
async fn debug_shows_no_id_or_value() {
    let (store, clock) = store_with(&settings());
    let data = data_with(&clock, "secret-key", json!("secret-value"));
    store.save("secret-id", &data, IDLE).await.unwrap();

    for text in [format!("{store:?}"), format!("{data:?}")] {
        assert!(!text.contains("secret"), "leaked: {text}");
    }
}

#[test]
fn session_data_is_empty() {
    let now = SystemTime::UNIX_EPOCH;
    let mut data = SessionData::new(now);
    assert!(data.is_empty());
    assert_eq!(data.created(), now);
    assert_eq!(data.touched(), now);
    data.values_mut().insert("k".to_owned(), json!(1));
    assert!(!data.is_empty());
}

/// Saves a session, advances the clock by a day in steps, and checks it survives every load,
/// a sweep and a rotation.
async fn assert_never_expires(settings: &SessionSettings, ttl: Duration) {
    let (store, clock) = store_with(settings);
    let data = data_with(&clock, "k", json!("v"));
    store.save("id", &data, ttl).await.unwrap();
    assert_eq!(store.load("id").await.unwrap(), Some(data.clone()));

    clock.advance(Duration::from_secs(24 * 60 * 60));
    store.sweep();
    assert_eq!(store.load("id").await.unwrap(), Some(data.clone()));

    store.rotate("id", "id2").await.unwrap();
    clock.advance(Duration::from_secs(24 * 60 * 60));
    store.sweep();
    assert_eq!(store.load("id2").await.unwrap(), Some(data));
}

#[tokio::test]
async fn max_durations_mean_no_limit() {
    // Both timeouts at `Duration::MAX`, passed as the ttl too: no deadline at all.
    let unlimited = SessionSettings::default()
        .idle_timeout(Duration::MAX)
        .absolute_timeout(Duration::MAX);
    assert_never_expires(&unlimited, Duration::MAX).await;
}

#[tokio::test]
async fn max_ttl_leaves_only_the_absolute_timeout() {
    let settings = SessionSettings::default()
        .idle_timeout(Duration::MAX)
        .absolute_timeout(Duration::from_secs(3 * 24 * 60 * 60));
    assert_never_expires(&settings, Duration::MAX).await;

    // The absolute timeout still applies when the ttl overflows.
    let (store, clock) = store_with(&settings);
    store
        .save("id", &data_with(&clock, "k", json!(1)), Duration::MAX)
        .await
        .unwrap();
    clock.advance(Duration::from_secs(3 * 24 * 60 * 60));
    assert_eq!(store.load("id").await.unwrap(), None);
}

#[tokio::test]
async fn max_absolute_timeout_leaves_only_the_idle_timeout() {
    let settings = SessionSettings::default()
        .idle_timeout(Duration::from_secs(2 * 24 * 60 * 60))
        .absolute_timeout(Duration::MAX);
    let (store, clock) = store_with(&settings);
    store
        .save(
            "id",
            &data_with(&clock, "k", json!(1)),
            settings.idle_timeout,
        )
        .await
        .unwrap();
    clock.advance(Duration::from_secs(24 * 60 * 60));
    store.sweep();
    assert!(store.load("id").await.unwrap().is_some());

    // The idle timeout still applies when the absolute deadline overflows.
    clock.advance(Duration::from_secs(24 * 60 * 60));
    assert_eq!(store.load("id").await.unwrap(), None);
}
