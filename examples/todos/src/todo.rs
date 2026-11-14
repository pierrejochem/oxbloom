//! The todo model and its in-memory store.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// One todo item.
///
/// # Examples
///
/// ```
/// use todos::todo::Todo;
///
/// let todo = Todo { id: 1, title: "write docs".into(), done: false };
/// assert_eq!(serde_json::to_string(&todo).unwrap(), r#"{"id":1,"title":"write docs","done":false}"#);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    /// The id, from 1, never reused.
    pub id: u64,
    /// What to do.
    pub title: String,
    /// Whether it is done.
    pub done: bool,
}

/// The in-memory store of todos. Safe to share between threads.
///
/// Ids start at 1 and are never reused, also after a delete. The store is
/// registered as a bean with `.bean(TodoStore::default)`.
///
/// # Examples
///
/// ```
/// use todos::todo::TodoStore;
///
/// let store = TodoStore::default();
/// let first = store.create("a".into());
/// assert_eq!(first.id, 1);
/// assert!(store.delete(1));
/// assert_eq!(store.create("b".into()).id, 2);
/// ```
#[derive(Debug, Default)]
pub struct TodoStore {
    items: Mutex<BTreeMap<u64, Todo>>,
    /// When each todo that is done became done; the cleanup task reads it.
    done_at: Mutex<BTreeMap<u64, SystemTime>>,
    /// The last id handed out.
    last_id: AtomicU64,
}

impl TodoStore {
    fn items(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Todo>> {
        // The map stays consistent even if a holder panicked.
        self.items.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn done_at(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, SystemTime>> {
        self.done_at.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Adds a todo that is not done and returns it.
    ///
    /// # Examples
    ///
    /// ```
    /// use todos::todo::TodoStore;
    ///
    /// let todo = TodoStore::default().create("water plants".into());
    /// assert_eq!((todo.id, todo.done), (1, false));
    /// ```
    pub fn create(&self, title: String) -> Todo {
        let id = self.last_id.fetch_add(1, Ordering::Relaxed) + 1;
        let todo = Todo {
            id,
            title,
            done: false,
        };
        self.items().insert(id, todo.clone());
        todo
    }

    /// The todo with `id`, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use todos::todo::TodoStore;
    ///
    /// let store = TodoStore::default();
    /// store.create("a".into());
    /// assert_eq!(store.get(1).unwrap().title, "a");
    /// assert!(store.get(2).is_none());
    /// ```
    pub fn get(&self, id: u64) -> Option<Todo> {
        self.items().get(&id).cloned()
    }

    /// All todos by id, only those with `done == filter` when given.
    ///
    /// # Examples
    ///
    /// ```
    /// use todos::todo::TodoStore;
    ///
    /// let store = TodoStore::default();
    /// store.create("a".into());
    /// store.create("b".into());
    /// store.patch(2, None, Some(true));
    /// assert_eq!(store.list(None).len(), 2);
    /// assert_eq!(store.list(Some(true))[0].id, 2);
    /// ```
    pub fn list(&self, done: Option<bool>) -> Vec<Todo> {
        self.items()
            .values()
            .filter(|t| done.is_none_or(|d| t.done == d))
            .cloned()
            .collect()
    }

    /// Replaces title and done flag of an existing todo. `None` for an
    /// unknown id: it never creates one.
    ///
    /// # Examples
    ///
    /// ```
    /// use todos::todo::TodoStore;
    ///
    /// let store = TodoStore::default();
    /// store.create("a".into());
    /// assert_eq!(store.replace(1, "b".into(), true).unwrap().title, "b");
    /// assert!(store.replace(9, "x".into(), false).is_none());
    /// assert!(store.get(9).is_none());
    /// ```
    pub fn replace(&self, id: u64, title: String, done: bool) -> Option<Todo> {
        self.patch(id, Some(title), Some(done))
    }

    /// Changes only the given fields of an existing todo. `None` for an
    /// unknown id.
    ///
    /// # Examples
    ///
    /// ```
    /// use todos::todo::TodoStore;
    ///
    /// let store = TodoStore::default();
    /// store.create("a".into());
    /// let todo = store.patch(1, None, Some(true)).unwrap();
    /// assert_eq!((todo.title.as_str(), todo.done), ("a", true));
    /// ```
    pub fn patch(&self, id: u64, title: Option<String>, done: Option<bool>) -> Option<Todo> {
        self.patch_at(id, title, done, SystemTime::now())
    }

    /// Like [`patch`](TodoStore::patch), with `at` as the moment a todo becomes done. A todo
    /// that is already done keeps its earlier moment, and one that is no longer done has none.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use todos::todo::TodoStore;
    ///
    /// let store = TodoStore::default();
    /// store.create("a".into());
    /// let long_ago = SystemTime::UNIX_EPOCH;
    /// store.patch_at(1, None, Some(true), long_ago);
    /// assert_eq!(store.remove_done_before(SystemTime::now()), 1);
    /// ```
    pub fn patch_at(
        &self,
        id: u64,
        title: Option<String>,
        done: Option<bool>,
        at: SystemTime,
    ) -> Option<Todo> {
        let mut items = self.items();
        let todo = items.get_mut(&id)?;
        if let Some(title) = title {
            todo.title = title;
        }
        if let Some(done) = done {
            todo.done = done;
            let mut done_at = self.done_at();
            if done {
                done_at.entry(id).or_insert(at);
            } else {
                done_at.remove(&id);
            }
        }
        Some(todo.clone())
    }

    /// Removes every done todo that became done before `cutoff` and returns how many.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use todos::todo::TodoStore;
    ///
    /// let store = TodoStore::default();
    /// store.create("open".into());
    /// store.create("done".into());
    /// store.patch(2, None, Some(true));
    /// assert_eq!(store.remove_done_before(SystemTime::UNIX_EPOCH), 0);
    /// assert_eq!(store.remove_done_before(SystemTime::now() + std::time::Duration::from_secs(1)), 1);
    /// assert_eq!(store.list(None).len(), 1);
    /// ```
    pub fn remove_done_before(&self, cutoff: SystemTime) -> usize {
        let mut items = self.items();
        let mut done_at = self.done_at();
        let expired: Vec<u64> = done_at
            .iter()
            .filter(|(_, at)| **at < cutoff)
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            items.remove(id);
            done_at.remove(id);
        }
        expired.len()
    }

    /// Removes the todo; `true` if it existed.
    ///
    /// # Examples
    ///
    /// ```
    /// use todos::todo::TodoStore;
    ///
    /// let store = TodoStore::default();
    /// store.create("a".into());
    /// assert!(store.delete(1));
    /// assert!(!store.delete(1));
    /// ```
    pub fn delete(&self, id: u64) -> bool {
        self.done_at().remove(&id);
        self.items().remove(&id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use super::*;

    #[test]
    fn concurrent_creates_get_distinct_ids() {
        let store = Arc::new(TodoStore::default());
        let threads: Vec<_> = (0..32)
            .map(|i| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || store.create(format!("t{i}")).id)
            })
            .collect();
        let ids: BTreeSet<u64> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(ids.len(), 32);
        assert_eq!(ids, (1..=32).collect());
        assert_eq!(store.list(None).len(), 32);
    }

    #[test]
    fn ids_are_not_reused_after_delete() {
        let store = TodoStore::default();
        store.create("a".into());
        store.delete(1);
        assert_eq!(store.create("b".into()).id, 2);
    }

    #[test]
    fn replace_and_patch_never_upsert() {
        let store = TodoStore::default();
        assert!(store.replace(1, "x".into(), true).is_none());
        assert!(store.patch(1, None, Some(true)).is_none());
        assert!(store.list(None).is_empty());
    }
}
