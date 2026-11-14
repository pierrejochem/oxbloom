//! Reloading bundles when their files change: the file stamps, the rebuild that the polling
//! task and [`SslBundles::reload`](crate::SslBundles::reload) share, and the polling task.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use oxbloom_core::Clock;
use tracing::Dispatch;

use crate::SslBundle;
use crate::bundle;
use crate::error::SslError;
use crate::settings::BundleSource;
use crate::validity;

/// The log target of every reload event.
const TARGET: &str = "oxbloom::ssl";

/// How often [`reload_now`] reads a bundle whose files keep changing during the read.
const ATTEMPTS: usize = 3;

/// A test seam that runs between the read of a bundle's files and the second metadata check.
pub(crate) type ReadHook = dyn Fn(&str) + Send + Sync;

/// The slot that holds the optional [`ReadHook`], shared by every clone of the bundles.
pub(crate) type HookSlot = Arc<Mutex<Option<Arc<ReadHook>>>>;

/// Locks `mutex`, also when a panicking holder poisoned it: the data stays consistent because
/// every holder only replaces whole values.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the metadata of one file says: its size, its modification time and, on Unix, its
/// device and inode, so a file replaced by a rename is seen even within one clock tick.
/// `fs::metadata` follows symlinks, so a swapped symlink changes the stamp.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: (u64, u64),
}

/// The stamps of every watched file of a bundle, in the order of
/// [`bundle::watched_files`]; `None` for a file whose metadata cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snapshot(Vec<Option<Stamp>>);

impl Snapshot {
    fn take(files: &[(&'static str, String)]) -> Snapshot {
        Snapshot(
            files
                .iter()
                .map(|(_, path)| {
                    std::fs::metadata(path).ok().map(|meta| Stamp {
                        len: meta.len(),
                        modified: meta.modified().ok(),
                        #[cfg(unix)]
                        inode: {
                            use std::os::unix::fs::MetadataExt;
                            (meta.dev(), meta.ino())
                        },
                    })
                })
                .collect(),
        )
    }

    /// The index of the first file whose stamp differs from `other`.
    fn first_difference(&self, other: &Snapshot) -> usize {
        self.0
            .iter()
            .zip(&other.0)
            .position(|(a, b)| a != b)
            .unwrap_or(0)
    }
}

/// The reload state of one bundle: the files it watches, the stamps of the last successful
/// load, and the stamps of the last failed reload, so one bad change warns once.
pub(crate) struct ReloadState {
    files: Vec<(&'static str, String)>,
    last_good: Snapshot,
    last_failed: Option<Snapshot>,
}

impl ReloadState {
    /// The state before the first load of `source`. The stamps are taken before the files are
    /// read, so a change during the first load is picked up by the next tick.
    pub(crate) fn new(source: &BundleSource) -> ReloadState {
        let files = bundle::watched_files(source);
        let last_good = Snapshot::take(&files);
        ReloadState {
            files,
            last_good,
            last_failed: None,
        }
    }
}

/// The result of one rebuild attempt.
pub(crate) enum Outcome {
    /// No watched file changed since the last success or the last failure.
    Unchanged,
    /// The bundle was rebuilt and swapped in.
    Reloaded,
    /// The rebuild failed; the old material stays.
    Failed(SslError),
    /// The file named by `key` changed while it was read; nothing was swapped.
    Changed { key: String },
}

/// Rebuilds `bundle` from all its sources and swaps the result in.
///
/// Without `force`, nothing is read when the watched files still have the stamps of the last
/// success or of the last failure. The stamps are taken before and after the read; when they
/// differ, the attempt is dropped. A failure without `force` records the stamps, so the same
/// bad files are not tried, and not reported, again.
pub(crate) fn rebuild(bundle: &SslBundle, force: bool, hook: Option<&ReadHook>) -> Outcome {
    let inner = &bundle.inner;
    let mut state = lock(&inner.reload);
    let before = Snapshot::take(&state.files);
    if !force && (before == state.last_good || state.last_failed.as_ref() == Some(&before)) {
        return Outcome::Unchanged;
    }
    let loaded = bundle::load(&inner.source);
    if let Some(hook) = hook {
        hook(bundle.name());
    }
    let after = Snapshot::take(&state.files);
    if before != after {
        let leaf = state.files[before.first_difference(&after)].0;
        return Outcome::Changed {
            key: inner.source.key(leaf),
        };
    }
    match loaded {
        Ok(material) => {
            inner.material.store(Arc::new(material));
            state.last_good = after;
            state.last_failed = None;
            Outcome::Reloaded
        }
        Err(e) => {
            if !force {
                state.last_failed = Some(after);
            }
            Outcome::Failed(e)
        }
    }
}

/// Rebuilds `bundle` now, reading it again up to [`ATTEMPTS`] times while its files change
/// during the read.
pub(crate) fn reload_now(bundle: &SslBundle, hook: Option<&ReadHook>) -> Result<(), SslError> {
    let mut changed = String::new();
    for _ in 0..ATTEMPTS {
        match rebuild(bundle, true, hook) {
            Outcome::Reloaded | Outcome::Unchanged => return Ok(()),
            Outcome::Failed(e) => return Err(e),
            Outcome::Changed { key } => changed = key,
        }
    }
    Err(SslError::Io {
        bundle: bundle.name().to_string(),
        key: changed,
        kind: std::io::ErrorKind::Interrupted,
    })
}

/// A short name for the kind of `error`, for the reload warning. It never holds a path, a key
/// or a password.
fn kind(error: &SslError) -> String {
    match error {
        SslError::Io { kind, .. } => format!("io ({kind})"),
        SslError::Pem { .. } => "pem".to_string(),
        SslError::KeyMismatch { .. } => "key-mismatch".to_string(),
        SslError::Pkcs12 { .. } => "pkcs12".to_string(),
        SslError::EmptyBundle { .. } => "empty-bundle".to_string(),
        _ => "other".to_string(),
    }
}

/// Warns about the certificates of `bundle` that are not valid at the time of `clock` or
/// expire within `window`, following the 24 hour repeat rule of [`validity::check`].
pub(crate) fn check_validity(bundle: &SslBundle, clock: &dyn Clock, window: Duration) {
    let material = bundle.inner.material.load();
    validity::check(bundle.name(), &material, clock.now(), window);
}

/// What the polling task works on: the bundles, the clock and expiry warning time for the
/// validity checks, and the test seam. It holds no [`ReloadTask`], so the task never keeps
/// itself alive.
#[derive(Clone)]
pub(crate) struct Poller {
    pub(crate) bundles: Arc<Vec<SslBundle>>,
    pub(crate) hook: HookSlot,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) expiry_warning: Duration,
}

impl Poller {
    /// One pass of the polling task: reloads every bundle whose files changed, logs the
    /// outcome on `oxbloom::ssl`, then checks the validity of every bundle's certificates. A
    /// reloaded bundle warns at once; any other repeats a warning every 24 hours by the clock.
    fn tick(&self) {
        let hook = lock(&self.hook).clone();
        for bundle in self.bundles.iter() {
            let name = bundle.name();
            match rebuild(bundle, false, hook.as_deref()) {
                Outcome::Unchanged => {}
                Outcome::Reloaded => tracing::info!(
                    target: TARGET,
                    bundle = name,
                    "reloaded the SSL bundle after its files changed"
                ),
                Outcome::Changed { key } => tracing::debug!(
                    target: TARGET,
                    bundle = name,
                    key = %key,
                    "a file of the SSL bundle changed during the read; retrying on the next tick"
                ),
                Outcome::Failed(error) => tracing::warn!(
                    target: TARGET,
                    bundle = name,
                    error = %kind(&error),
                    "cannot reload the SSL bundle; keeping the previous material"
                ),
            }
            check_validity(bundle, &*self.clock, self.expiry_warning);
        }
    }
}

/// The polling task that reloads SSL bundles when their files change, from
/// [`SslBundles::start_reload`](crate::SslBundles::start_reload).
///
/// [`ReloadTask::stop`] ends it, and so does dropping it. Once either returns, no further
/// reload runs. A task started with `ssl.reload-interval = "0s"` does nothing.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_ssl::SslBundles;
///
/// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
/// # let ca_pem = generated.cert.pem();
/// let config = Config::from_str(&format!(
///     "[ssl]\nreload-interval = \"10s\"\n[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"
/// ))?;
/// let task = SslBundles::from_config(&config)?.start_reload();
/// assert!(task.is_running());
/// task.stop();
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct ReloadTask {
    running: Option<Running>,
}

struct Running {
    /// Set by `stop`; the task checks it, under this lock, before each pass.
    stopped: Arc<Mutex<bool>>,
    abort: tokio::task::AbortHandle,
}

impl ReloadTask {
    /// Starts polling every `interval`. Zero, or an interval too long for the clock, starts
    /// nothing. Inside a Tokio runtime (which needs its time driver) the task runs on that
    /// runtime; outside one it runs on its own thread.
    pub(crate) fn start(poller: Poller, interval: Duration) -> ReloadTask {
        if interval.is_zero() {
            return ReloadTask { running: None };
        }
        let Some(first) = tokio::time::Instant::now().checked_add(interval) else {
            return ReloadTask { running: None };
        };
        let stopped = Arc::new(Mutex::new(false));
        let dispatch = current_subscriber();
        let task = poll(poller, first, interval, Arc::clone(&stopped), dispatch);
        let abort = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime.spawn(task).abort_handle(),
            Err(_) => match own_thread(task) {
                Some(abort) => abort,
                None => {
                    tracing::warn!(
                        target: TARGET,
                        "cannot start the SSL reload task; bundle files are not watched"
                    );
                    return ReloadTask { running: None };
                }
            },
        };
        ReloadTask {
            running: Some(Running { stopped, abort }),
        }
    }

    /// Whether the task is polling: false for a zero interval, after the task ended, or when
    /// its runtime shut down.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// let bundles = SslBundles::from_config(&Config::from_str("[ssl]\nreload-interval = \"0s\"")?)?;
    /// assert!(!bundles.start_reload().is_running());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn is_running(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| !running.abort.is_finished())
    }

    /// Ends the polling task. When it returns, a reload in progress has finished and no other
    /// one starts, so the call may wait for a pass that is reading files. Dropping the task
    /// does the same.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca_pem = generated.cert.pem();
    /// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
    /// let bundles = SslBundles::from_config(&config)?;
    /// bundles.start_reload().stop();
    /// drop(bundles.start_reload()); // the same as `stop`
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        if let Some(running) = self.running.take() {
            *lock(&running.stopped) = true;
            running.abort.abort();
        }
    }
}

/// Stops the task, like [`ReloadTask::stop`].
impl Drop for ReloadTask {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Shows whether the task is running.
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_ssl::SslBundles;
///
/// let bundles = SslBundles::from_config(&Config::from_str("[ssl]\nreload-interval = \"0s\"")?)?;
/// assert_eq!(format!("{:?}", bundles.start_reload()), "ReloadTask { running: false }");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl fmt::Debug for ReloadTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReloadTask")
            .field("running", &self.is_running())
            .finish()
    }
}

/// Runs `task` on a current-thread runtime on its own thread. `None` when the runtime or the
/// thread cannot be created.
fn own_thread(
    task: impl std::future::Future<Output = ()> + Send + 'static,
) -> Option<tokio::task::AbortHandle> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .ok()?;
    let handle = runtime.spawn(task);
    let abort = handle.abort_handle();
    std::thread::Builder::new()
        .name("oxbloom-ssl-reload".to_string())
        .spawn(move || {
            // An aborted task ends `block_on` with a cancelled error, which is the normal end.
            let _ = runtime.block_on(handle);
        })
        .ok()?;
    Some(abort)
}

/// The subscriber that is current on this thread, or `None` when there is none. Forcing the
/// no-op subscriber on the task would hide the global subscriber installed later, so the
/// task then reports to whatever default exists when each event is emitted.
fn current_subscriber() -> Option<Dispatch> {
    tracing::dispatcher::get_default(|dispatch| {
        (!dispatch.is::<tracing::subscriber::NoSubscriber>()).then(|| dispatch.clone())
    })
}

/// The polling loop: from `first` on, every `interval`, one [`Poller::tick`] on the blocking
/// pool, reported to `dispatch` (the subscriber that was current when the task started) or,
/// without one, to the default subscriber at the time of each event.
async fn poll(
    poller: Poller,
    first: tokio::time::Instant,
    interval: Duration,
    stopped: Arc<Mutex<bool>>,
    dispatch: Option<Dispatch>,
) {
    let mut ticker = tokio::time::interval_at(first, interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let (poller, stopped, pass_dispatch) =
            (poller.clone(), Arc::clone(&stopped), dispatch.clone());
        let pass = tokio::task::spawn_blocking(move || {
            let stopped = lock(&stopped);
            if *stopped {
                return false;
            }
            match &pass_dispatch {
                Some(dispatch) => tracing::dispatcher::with_default(dispatch, || poller.tick()),
                None => poller.tick(),
            }
            true
        })
        .await;
        match pass {
            Ok(true) => {}
            Ok(false) => break,
            Err(e) if e.is_cancelled() => break,
            // A panicking pass is skipped; the next tick tries again.
            Err(_) => {
                let warn = || {
                    tracing::warn!(
                        target: TARGET,
                        "a pass of the SSL reload task panicked; trying again on the next tick"
                    )
                };
                match &dispatch {
                    Some(dispatch) => tracing::dispatcher::with_default(dispatch, warn),
                    None => warn(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warning_kind_names_no_key_or_path() {
        let io = SslError::Io {
            bundle: "web".into(),
            key: "ssl.bundle.pem.web.private-key".into(),
            kind: std::io::ErrorKind::NotFound,
        };
        assert_eq!(kind(&io), "io (entity not found)");
        assert_eq!(
            kind(&SslError::KeyMismatch {
                bundle: "web".into()
            }),
            "key-mismatch"
        );
    }

    #[test]
    fn only_a_real_subscriber_is_captured_for_the_task() {
        assert!(current_subscriber().is_none());
        let scoped = Dispatch::new(tracing::subscriber::NoSubscriber::default());
        tracing::dispatcher::with_default(&scoped, || assert!(current_subscriber().is_none()));

        struct Real;
        impl tracing::Subscriber for Real {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, _: &tracing::Event<'_>) {}
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }
        let real = Dispatch::new(Real);
        tracing::dispatcher::with_default(&real, || {
            assert!(current_subscriber().is_some_and(|d| d.is::<Real>()));
        });
    }

    #[test]
    fn a_missing_file_has_no_stamp() {
        let files = [("certificate", "/nonexistent/oxbloom/web.crt".to_string())];
        assert_eq!(Snapshot::take(&files), Snapshot(vec![None]));
    }
}
