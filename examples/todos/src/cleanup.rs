//! The scheduled cleanup of done todos.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use oxbloom_config::Config;
use oxbloom_di::DiError;
use oxbloom_tasks::scheduling;
use serde::Deserialize;

use crate::todo::TodoStore;

/// The `todos.cleanup.*` keys.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct CleanupSettings {
    /// `todos.cleanup.max-age`: how long a todo stays after it is done. Default 24 hours.
    max_age: Duration,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        CleanupSettings {
            max_age: Duration::from_secs(24 * 3600),
        }
    }
}

/// Removes done todos that have been done for longer than `todos.cleanup.max-age`.
///
/// The task runs one minute after the end of its previous run (`fixed_delay`), starting one
/// minute after the application starts. Boot's `tasks` feature collects it because
/// `lib.rs` registers the bean `provides` as a `dyn Scheduling`. The period is a literal of
/// the attribute; the age is configuration. The task is named `TodoCleanup::cleanup`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::{Duration, SystemTime};
/// use todos::cleanup::TodoCleanup;
/// use todos::todo::TodoStore;
///
/// let store = Arc::new(TodoStore::default());
/// let cleanup = TodoCleanup::new(Arc::clone(&store), Duration::from_secs(3600));
/// store.create("done long ago".into());
/// store.patch_at(1, None, Some(true), SystemTime::UNIX_EPOCH);
/// assert_eq!(cleanup.cleanup_at(SystemTime::now()), 1);
/// assert!(store.list(None).is_empty());
/// ```
pub struct TodoCleanup {
    store: Arc<TodoStore>,
    max_age: Duration,
}

impl TodoCleanup {
    /// A cleanup of `store` for todos done longer than `max_age` ago.
    pub fn new(store: Arc<TodoStore>, max_age: Duration) -> Self {
        TodoCleanup { store, max_age }
    }

    /// The configured age.
    pub fn max_age(&self) -> Duration {
        self.max_age
    }

    /// Removes the todos done more than the age before `now` and returns how many.
    pub fn cleanup_at(&self, now: SystemTime) -> usize {
        let cutoff = now
            .checked_sub(self.max_age)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        self.store.remove_done_before(cutoff)
    }
}

#[scheduling]
impl TodoCleanup {
    #[scheduled(fixed_delay = "1m")]
    async fn cleanup(&self) {
        let removed = self.cleanup_at(SystemTime::now());
        if removed > 0 {
            tracing::info!(target: "todos", removed, "removed done todos");
        }
    }
}

/// The bean: the store and the `todos.cleanup.*` settings. A bad value fails the start.
///
/// # Errors
///
/// A [`DiError`] whose source is the `ConfigError` naming the key.
pub fn cleanup_bean(config: Arc<Config>, store: Arc<TodoStore>) -> Result<TodoCleanup, DiError> {
    let settings: CleanupSettings = config
        .get("todos.cleanup")
        .map_err(DiError::creation_from::<TodoCleanup>)?;
    Ok(TodoCleanup::new(store, settings.max_age))
}
