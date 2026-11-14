//! The cleanup task of the todos example: what it removes, and that the scheduler knows it.
//! The 1 minute period of the task is not waited for; the removal is driven through the
//! bean's own method with a fixed `now`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use oxbloom_boot::RunListener;
use oxbloom_di::Container;
use oxbloom_tasks::Scheduler;
use todos::cleanup::TodoCleanup;
use todos::todo::TodoStore;

/// Records what the container holds once the context is ready.
#[derive(Clone, Default)]
struct Seen {
    task_names: Arc<Mutex<Vec<String>>>,
    removed_with: Arc<Mutex<Option<Duration>>>,
}

impl RunListener for Seen {
    fn context_ready(&self, container: &Container) {
        let scheduler = container.get::<Scheduler>().unwrap();
        let names = scheduler.tasks().into_iter().map(|t| t.name);
        self.task_names.lock().unwrap().extend(names);
        let cleanup = container.get::<TodoCleanup>().unwrap();
        *self.removed_with.lock().unwrap() = Some(cleanup.max_age());
    }
}

async fn run_with(args: &[&str]) -> Seen {
    let seen = Seen::default();
    let builder = todos::application(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(["--server.port=0"].into_iter().chain(args.iter().copied()))
        .logging(false)
        .output(std::io::sink())
        .listener(seen.clone());
    builder.shutdown_handle().trigger();
    let status = tokio::time::timeout(Duration::from_secs(5), builder.run())
        .await
        .expect("test timed out")
        .unwrap();
    assert_eq!(status.code(), 0);
    seen
}

#[test]
fn cleanup_removes_only_done_todos_older_than_the_age() {
    let store = Arc::new(TodoStore::default());
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000);
    let cleanup = TodoCleanup::new(Arc::clone(&store), Duration::from_secs(3600));

    let old = store.create("old and done".into()).id;
    let recent = store.create("recent and done".into()).id;
    let open = store.create("still open".into()).id;
    store.patch_at(old, None, Some(true), now - Duration::from_secs(7200));
    store.patch_at(recent, None, Some(true), now - Duration::from_secs(60));

    assert_eq!(cleanup.cleanup_at(now), 1);
    let left: Vec<u64> = store.list(None).iter().map(|t| t.id).collect();
    assert_eq!(left, [recent, open]);
}

#[tokio::test]
async fn the_scheduler_knows_the_cleanup_task() {
    let seen = run_with(&[]).await;
    assert_eq!(*seen.task_names.lock().unwrap(), ["TodoCleanup::cleanup"]);
    // The default of application.toml.
    assert_eq!(
        *seen.removed_with.lock().unwrap(),
        Some(Duration::from_secs(24 * 3600))
    );
}

#[tokio::test]
async fn the_max_age_comes_from_configuration() {
    let seen = run_with(&["--todos.cleanup.max-age=250ms"]).await;
    assert_eq!(
        *seen.removed_with.lock().unwrap(),
        Some(Duration::from_millis(250))
    );
}

#[tokio::test]
async fn a_store_that_fails_twice_is_purged_on_the_third_attempt() {
    let store = Arc::new(TodoStore::default());
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000);
    let cleanup = TodoCleanup::new(Arc::clone(&store), Duration::from_secs(3600));
    let old = store.create("old and done".into()).id;
    store.patch_at(old, None, Some(true), now - Duration::from_secs(7200));

    store.fail_next(2);

    assert_eq!(cleanup.purge_at(now).await.unwrap(), 1);
    assert!(store.list(None).is_empty());
}

#[tokio::test]
async fn a_store_that_keeps_failing_returns_the_last_error() {
    let store = Arc::new(TodoStore::default());
    let cleanup = TodoCleanup::new(Arc::clone(&store), Duration::from_secs(3600));
    let old = store.create("old and done".into()).id;
    store.patch_at(old, None, Some(true), SystemTime::UNIX_EPOCH);

    store.fail_next(3);

    let error = cleanup.purge_at(SystemTime::now()).await.unwrap_err();
    assert_eq!(error.to_string(), "the todo store is unavailable");
    assert_eq!(store.list(None).len(), 1);
}
