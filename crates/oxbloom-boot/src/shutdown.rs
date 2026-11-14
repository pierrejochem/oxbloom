use std::fmt;
use std::sync::Arc;

use tokio::sync::watch;

/// Requests the shutdown of a running Oxbloom application without a signal.
///
/// Clones share one state: triggering any clone triggers them all. Triggering is
/// idempotent and remembered, so a handle triggered before `run()` reaches its
/// wait step makes `run()` skip the wait. Get the handle of an application with
/// `OxbloomApplicationBuilder::shutdown_handle`, or inject it as
/// `Arc<ShutdownHandle>`: boot registers it in the container.
///
/// ```
/// use oxbloom_boot::ShutdownHandle;
///
/// let handle = ShutdownHandle::new();
/// let clone = handle.clone();
/// assert!(!handle.is_triggered());
/// clone.trigger();
/// assert!(handle.is_triggered());
/// ```
#[derive(Clone)]
pub struct ShutdownHandle {
    state: Arc<watch::Sender<bool>>,
}

impl ShutdownHandle {
    /// Creates a handle that has not been triggered.
    ///
    /// ```
    /// use oxbloom_boot::ShutdownHandle;
    ///
    /// assert!(!ShutdownHandle::new().is_triggered());
    /// ```
    pub fn new() -> Self {
        Self {
            state: Arc::new(watch::Sender::new(false)),
        }
    }

    /// Requests shutdown. Calling it again has no further effect.
    ///
    /// ```
    /// use oxbloom_boot::ShutdownHandle;
    ///
    /// let handle = ShutdownHandle::new();
    /// handle.trigger();
    /// handle.trigger();
    /// assert!(handle.is_triggered());
    /// ```
    pub fn trigger(&self) {
        self.state.send_replace(true);
    }

    /// Whether shutdown has been requested.
    ///
    /// ```
    /// use oxbloom_boot::ShutdownHandle;
    ///
    /// let handle = ShutdownHandle::new();
    /// assert!(!handle.is_triggered());
    /// handle.trigger();
    /// assert!(handle.is_triggered());
    /// ```
    pub fn is_triggered(&self) -> bool {
        *self.state.borrow()
    }

    /// Waits until shutdown is requested; returns at once if it already was.
    ///
    /// ```
    /// use oxbloom_boot::ShutdownHandle;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let handle = ShutdownHandle::new();
    /// let waiter = handle.clone();
    /// let task = tokio::spawn(async move { waiter.wait().await });
    /// handle.trigger();
    /// task.await?;
    /// # Ok::<(), tokio::task::JoinError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn wait(&self) {
        let mut rx = self.state.subscribe();
        // The sender lives in `self`, so `wait_for` cannot fail with a closed channel.
        let _ = rx.wait_for(|triggered| *triggered).await;
    }
}

impl Default for ShutdownHandle {
    /// Same as [`ShutdownHandle::new`].
    ///
    /// ```
    /// use oxbloom_boot::ShutdownHandle;
    ///
    /// assert!(!ShutdownHandle::default().is_triggered());
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ShutdownHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShutdownHandle")
            .field("triggered", &self.is_triggered())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn within<F: std::future::Future>(f: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(2), f)
            .await
            .expect("test timed out")
    }

    #[test]
    fn trigger_is_idempotent() {
        let h = ShutdownHandle::new();
        let c = h.clone();
        assert!(!c.is_triggered());
        h.trigger();
        h.trigger();
        c.trigger();
        assert!(h.is_triggered() && c.is_triggered());
        assert_eq!(format!("{h:?}"), "ShutdownHandle { triggered: true }");
    }

    #[tokio::test]
    async fn wait_returns_immediately_when_triggered() {
        let h = ShutdownHandle::new();
        h.trigger();
        within(h.wait()).await;
        within(h.wait()).await;
    }

    #[tokio::test]
    async fn wait_wakes_on_trigger_from_a_clone() {
        let h = ShutdownHandle::new();
        let c = h.clone();
        let task = tokio::spawn(async move { c.wait().await });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        h.trigger();
        within(task).await.unwrap();
    }

    #[test]
    fn separate_handles_are_independent() {
        let a = ShutdownHandle::new();
        let b = ShutdownHandle::new();
        a.trigger();
        assert!(!b.is_triggered());
    }
}
