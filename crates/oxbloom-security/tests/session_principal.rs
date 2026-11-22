//! Behaviour of the per-user session listing and the registry, against the in-memory store
//! with a manual clock.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use oxbloom_core::Clock;
use oxbloom_security::{
    InMemorySessionStore, PrincipalSessions, SessionBackend, SessionData, SessionFilter,
    SessionRegistry, SessionSettings, SessionStore,
};
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

const IDLE: Duration = Duration::from_secs(100);
const ABSOLUTE: Duration = Duration::from_secs(1000);
const KEY: &str = "oxbloom.security.principal";

fn setup() -> (Arc<InMemorySessionStore>, Arc<ManualClock>) {
    let clock = ManualClock::new();
    let settings = SessionSettings::default()
        .idle_timeout(IDLE)
        .absolute_timeout(ABSOLUTE);
    (
        Arc::new(InMemorySessionStore::new(&settings, clock.clone())),
        clock,
    )
}

fn index(store: &Arc<InMemorySessionStore>) -> Arc<dyn PrincipalSessions> {
    store
        .clone()
        .principal_sessions()
        .expect("the memory store has an index")
}

/// Saves a session of `principal` (or without one) and returns after the clock moved a second,
/// so touched times differ.
async fn put(store: &InMemorySessionStore, clock: &ManualClock, id: &str, principal: Option<&str>) {
    let mut data = SessionData::new(clock.now());
    if let Some(name) = principal {
        data.values_mut().insert(KEY.to_owned(), json!(name));
    }
    store.save(id, &data, IDLE).await.unwrap();
    clock.advance(Duration::from_secs(1));
}

async fn ids(index: &dyn PrincipalSessions, name: &str) -> Vec<String> {
    index
        .sessions_of(name)
        .await
        .unwrap()
        .iter()
        .map(|s| s.id().to_owned())
        .collect()
}

#[tokio::test]
async fn lists_live_sessions_of_the_principal_oldest_touched_first() {
    let (store, clock) = setup();
    let index = index(&store);
    put(&store, &clock, "b", Some("alice")).await;
    put(&store, &clock, "x", Some("bob")).await;
    put(&store, &clock, "a", Some("alice")).await;
    put(&store, &clock, "anon", None).await;
    assert_eq!(ids(&*index, "alice").await, ["b", "a"]);
    assert_eq!(ids(&*index, "bob").await, ["x"]);
    assert!(ids(&*index, "carol").await.is_empty());
}

#[tokio::test]
async fn summary_carries_times_and_hides_the_id_in_debug() {
    let (store, clock) = setup();
    let index = index(&store);
    let created = clock.now();
    put(&store, &clock, "secret-id", Some("alice")).await;
    let list = index.sessions_of("alice").await.unwrap();
    assert_eq!(list[0].id(), "secret-id");
    assert_eq!(list[0].created(), created);
    assert_eq!(list[0].touched(), created);
    assert_eq!(format!("{:?}", list[0]), "SessionSummary { .. }");
}

#[tokio::test]
async fn expired_and_deleted_sessions_disappear() {
    let (store, clock) = setup();
    let index = index(&store);
    put(&store, &clock, "old", Some("alice")).await;
    clock.advance(IDLE);
    put(&store, &clock, "new", Some("alice")).await;
    assert_eq!(ids(&*index, "alice").await, ["new"]);
    store.delete("new").await.unwrap();
    assert!(ids(&*index, "alice").await.is_empty());
}

#[tokio::test]
async fn rotated_session_keeps_its_principal() {
    let (store, clock) = setup();
    let index = index(&store);
    put(&store, &clock, "first", Some("alice")).await;
    assert!(store.rotate("first", "second").await.unwrap());
    assert_eq!(ids(&*index, "alice").await, ["second"]);
}

#[tokio::test]
async fn delete_of_removes_all_or_all_but_except() {
    let (store, clock) = setup();
    let index = index(&store);
    for id in ["a", "b", "c"] {
        put(&store, &clock, id, Some("alice")).await;
    }
    put(&store, &clock, "x", Some("bob")).await;
    assert_eq!(index.delete_of("alice", Some("b")).await.unwrap(), 2);
    assert_eq!(ids(&*index, "alice").await, ["b"]);
    assert_eq!(index.delete_of("alice", None).await.unwrap(), 1);
    assert!(ids(&*index, "alice").await.is_empty());
    assert_eq!(ids(&*index, "bob").await, ["x"]);
    assert_eq!(index.delete_of("nobody", None).await.unwrap(), 0);
    assert!(store.load("x").await.unwrap().is_some());
}

#[tokio::test]
async fn evict_oldest_keeps_the_newest_and_spares_except() {
    let (store, clock) = setup();
    let index = index(&store);
    for id in ["a", "b", "c", "d"] {
        put(&store, &clock, id, Some("alice")).await;
    }
    // `a` is the oldest but protected: the other three shrink to two, so `b` and `c` go.
    assert_eq!(index.evict_oldest("alice", 2, Some("a")).await.unwrap(), 1);
    assert_eq!(ids(&*index, "alice").await, ["a", "c", "d"]);
    assert_eq!(index.evict_oldest("alice", 1, None).await.unwrap(), 2);
    assert_eq!(ids(&*index, "alice").await, ["d"]);
}

#[tokio::test]
async fn evict_oldest_with_nothing_to_do() {
    let (store, clock) = setup();
    let index = index(&store);
    put(&store, &clock, "a", Some("alice")).await;
    put(&store, &clock, "b", Some("alice")).await;
    assert_eq!(index.evict_oldest("alice", 5, None).await.unwrap(), 0);
    assert_eq!(index.evict_oldest("nobody", 0, None).await.unwrap(), 0);
    assert_eq!(ids(&*index, "alice").await, ["a", "b"]);
}

#[tokio::test]
async fn registry_lists_and_invalidates() {
    let (store, clock) = setup();
    let registry = SessionRegistry::new(index(&store));
    for id in ["a", "b", "c"] {
        put(&store, &clock, id, Some("alice")).await;
    }
    assert_eq!(registry.sessions_of("alice").await.unwrap().len(), 3);
    assert_eq!(registry.invalidate_others("alice", "b").await.unwrap(), 2);
    assert_eq!(registry.sessions_of("alice").await.unwrap().len(), 1);
    assert_eq!(registry.invalidate_all("alice").await.unwrap(), 1);
    assert!(registry.sessions_of("alice").await.unwrap().is_empty());
    assert_eq!(format!("{registry:?}"), "SessionRegistry { .. }");
    let _clone = registry.clone();
}

#[test]
fn filter_exposes_the_index_of_a_memory_backend() {
    let (store, _clock) = setup();
    let filter =
        SessionFilter::new(SessionBackend::Server(store), SessionSettings::default()).unwrap();
    assert!(filter.principal_sessions().is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_saves_and_evictions_stay_consistent() {
    let (store, clock) = setup();
    let index = index(&store);
    let mut tasks = Vec::new();
    for n in 0..64 {
        let store = store.clone();
        let index = index.clone();
        let clock = clock.clone();
        tasks.push(tokio::spawn(async move {
            let id = format!("s{n}");
            let mut data = SessionData::new(clock.now());
            data.values_mut().insert(KEY.to_owned(), json!("alice"));
            store.save(&id, &data, IDLE).await.unwrap();
            if n % 2 == 0 {
                index.evict_oldest("alice", 3, Some(&id)).await.unwrap();
            }
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let listed = ids(&*index, "alice").await;
    let mut loadable = Vec::new();
    for n in 0..64 {
        let id = format!("s{n}");
        if store.load(&id).await.unwrap().is_some() {
            loadable.push(id);
        }
    }
    let mut sorted = listed.clone();
    sorted.sort();
    loadable.sort();
    assert_eq!(sorted, loadable);
    index.evict_oldest("alice", 3, None).await.unwrap();
    assert_eq!(ids(&*index, "alice").await.len(), 3);
}

#[tokio::test]
async fn session_past_the_absolute_timeout_is_ignored_while_idle_window_is_open() {
    let (store, clock) = setup();
    let index = index(&store);
    let mut data = SessionData::new(clock.now());
    data.values_mut().insert(KEY.to_owned(), json!("alice"));
    // Keep re-saving so the idle deadline always lies ahead.
    for _ in 0..11 {
        clock.advance(Duration::from_secs(90));
        store.save("old", &data, IDLE).await.unwrap();
    }
    assert_eq!(ids(&*index, "alice").await, ["old"]);
    // 990 s after creation: the idle window is open until 1090 s, the absolute timeout ends at 1000 s.
    clock.advance(Duration::from_secs(11));
    assert!(ids(&*index, "alice").await.is_empty());
    put(&store, &clock, "fresh", Some("alice")).await;
    assert_eq!(
        index.evict_oldest("alice", 0, Some("fresh")).await.unwrap(),
        0
    );
    assert_eq!(index.delete_of("alice", Some("fresh")).await.unwrap(), 0);
    assert_eq!(ids(&*index, "alice").await, ["fresh"]);
}
