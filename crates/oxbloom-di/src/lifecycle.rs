//! Lifecycle hooks, lazy creation and shutdown: the [`Lifecycle`] trait, the type-erased hooks
//! an [`Entry`] carries, the container-wide reentrant [`CreationLock`] that serializes lazy
//! creation and shutdown, and the reverse-order destroy run shared by `Container::shutdown` and
//! the cleanup of a failed build.

use std::any::type_name;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};

use crate::entry::{Entry, Instance, read};
use crate::{DiError, ErrorSource};

/// Runs after an entry's instance is created; an error fails the entry's creation.
pub(crate) type PostConstruct = Box<dyn Fn(&Instance) -> Result<(), DiError> + Send + Sync>;

/// Runs when a created entry is destroyed (by `shutdown`, or by a failed build's cleanup).
pub(crate) type PreDestroy = Box<dyn Fn(&Instance) + Send + Sync>;

/// Fixed `DestroyFailure.message` for a panicking `pre_destroy` hook.
const PANICKED: &str = "pre_destroy panicked";

/// Lifecycle callbacks a bean implements on itself; register them with
/// [`ContainerBuilder::lifecycle`](crate::ContainerBuilder::lifecycle).
///
/// Both methods have empty defaults, so a type implements only what it needs. For hooks
/// defined outside the type, use [`ContainerBuilder::post_construct`](crate::ContainerBuilder::post_construct)
/// and [`ContainerBuilder::pre_destroy`](crate::ContainerBuilder::pre_destroy).
///
/// # Examples
///
/// ```
/// use std::sync::atomic::{AtomicBool, Ordering};
/// use oxbloom_di::{ContainerBuilder, DiError, Lifecycle};
///
/// #[derive(Default)]
/// struct Pool { open: AtomicBool }
///
/// impl Lifecycle for Pool {
///     fn post_construct(&self) -> Result<(), DiError> {
///         self.open.store(true, Ordering::SeqCst);
///         Ok(())
///     }
///     fn pre_destroy(&self) {
///         self.open.store(false, Ordering::SeqCst);
///     }
/// }
///
/// let c = ContainerBuilder::new().instance(Pool::default()).lifecycle::<Pool>().build()?;
/// let pool = c.get::<Pool>()?;
/// assert!(pool.open.load(Ordering::SeqCst));
/// c.shutdown();
/// assert!(!pool.open.load(Ordering::SeqCst));
/// # Ok::<(), DiError>(())
/// ```
pub trait Lifecycle: Send + Sync + 'static {
    /// Runs once, right after the bean is created and after the `post_construct` hooks of its
    /// dependencies; no dependent receives the bean before it returns `Ok`.
    ///
    /// An error fails the bean's creation with [`DiError::Creation`] (another variant is
    /// wrapped into `Creation` for this type) and aborts `build()`. The default does nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError, Lifecycle};
    ///
    /// struct Settings { port: u16 }
    ///
    /// impl Lifecycle for Settings {
    ///     fn post_construct(&self) -> Result<(), DiError> {
    ///         if self.port == 0 {
    ///             return Err(DiError::creation::<Settings>("port must not be 0"));
    ///         }
    ///         Ok(())
    ///     }
    /// }
    ///
    /// let ok = ContainerBuilder::new().instance(Settings { port: 8080 }).lifecycle::<Settings>();
    /// assert!(ok.build().is_ok());
    ///
    /// let bad = ContainerBuilder::new().instance(Settings { port: 0 }).lifecycle::<Settings>();
    /// assert!(matches!(bad.build(), Err(DiError::Creation { .. })));
    /// ```
    fn post_construct(&self) -> Result<(), DiError> {
        Ok(())
    }

    /// Runs once, when [`Container::shutdown`](crate::Container::shutdown) destroys the bean, or
    /// when a later failure aborts `build()` after the bean's `post_construct` succeeded.
    /// Beans are destroyed in reverse creation order, so dependencies are still usable here.
    ///
    /// A panic is caught. During `shutdown` it is reported in the [`ShutdownReport`]; during the
    /// cleanup of a failed `build()` it is caught and dropped (the build's error is what the
    /// caller sees). The default does nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Mutex;
    /// use oxbloom_di::{ContainerBuilder, Lifecycle};
    ///
    /// #[derive(Default)]
    /// struct Buffer { pending: Mutex<Vec<String>>, flushed: Mutex<usize> }
    ///
    /// impl Lifecycle for Buffer {
    ///     fn pre_destroy(&self) {
    ///         let mut pending = self.pending.lock().unwrap();
    ///         *self.flushed.lock().unwrap() += pending.len();
    ///         pending.clear();
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new().instance(Buffer::default()).lifecycle::<Buffer>().build()?;
    /// let buffer = c.get::<Buffer>()?;
    /// buffer.pending.lock().unwrap().push("event".into());
    /// c.shutdown();
    /// assert_eq!(*buffer.flushed.lock().unwrap(), 1);
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    fn pre_destroy(&self) {}
}

/// What [`Container::shutdown`](crate::Container::shutdown) did.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{ContainerBuilder, ShutdownReport};
///
/// struct Cache;
/// let c = ContainerBuilder::new().instance(Cache).pre_destroy(|_: &Cache| {}).build()?;
/// let report = c.shutdown();
/// assert_eq!(report.destroyed, 1);
/// assert!(report.failures.is_empty());
/// assert_eq!(c.shutdown(), ShutdownReport::default()); // a second call does nothing
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
///
/// The struct is `#[non_exhaustive]`: read its fields, or compare with
/// `ShutdownReport::default()`; it cannot be built literally outside `oxbloom-di`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShutdownReport {
    /// Beans whose `pre_destroy` hooks all ran without panicking. Beans without a
    /// `pre_destroy` hook are not counted.
    pub destroyed: usize,
    /// One entry per panicking `pre_destroy` hook, in the order they ran.
    pub failures: Vec<DestroyFailure>,
}

/// A `pre_destroy` hook that panicked during [`Container::shutdown`](crate::Container::shutdown).
///
/// The message is fixed text (`"pre_destroy panicked"`); the panic payload is not kept, so no
/// value from the hook can leak into the report.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{ContainerBuilder, DestroyFailure};
///
/// struct Socket;
/// let c = ContainerBuilder::new()
///     .instance(Socket)
///     .pre_destroy(|_: &Socket| panic!("already closed"))
///     .build()?;
/// # std::panic::set_hook(Box::new(|_| {})); // keep the doctest output quiet
/// let report = c.shutdown();
/// assert_eq!(report.destroyed, 0);
/// let failure: &DestroyFailure = &report.failures[0];
/// assert_eq!(failure.type_name, std::any::type_name::<Socket>());
/// assert_eq!(failure.message, "pre_destroy panicked");
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
///
/// The struct is `#[non_exhaustive]`: read its fields; it cannot be built literally outside
/// `oxbloom-di`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DestroyFailure {
    /// The bean's concrete type name.
    pub type_name: String,
    /// Fixed text describing the failure.
    pub message: String,
}

/// Wraps `f` as the post-construct hook of an entry whose concrete type is `T`.
pub(crate) fn post_construct<T, F>(f: F) -> PostConstruct
where
    T: Send + Sync + 'static,
    F: Fn(&T) -> Result<(), DiError> + Send + Sync + 'static,
{
    Box::new(move |instance| f(&read::<T>(instance)).map_err(as_creation::<T>))
}

/// Wraps `f` as the pre-destroy hook of an entry whose concrete type is `T`.
pub(crate) fn pre_destroy<T, F>(f: F) -> PreDestroy
where
    T: Send + Sync + 'static,
    F: Fn(&T) + Send + Sync + 'static,
{
    Box::new(move |instance| f(&read::<T>(instance)))
}

/// A hook error as `Creation` for `T`, the bean whose creation failed, so a failing hook never
/// looks like a wiring error (and is cached like any creation failure). A `Creation` keeps its
/// message and source, whatever type it named. Any other variant becomes the fixed text
/// `post_construct failed: {error}` (type names and fixed text only, no values) with an empty
/// source.
#[cold]
#[inline(never)]
fn as_creation<T>(error: DiError) -> DiError {
    let (message, source) = match error {
        DiError::Creation {
            message, source, ..
        } => (message, source),
        other => (
            format!("post_construct failed: {other}"),
            ErrorSource::none(),
        ),
    };
    DiError::Creation {
        type_name: type_name::<T>().to_owned(),
        message,
        source,
    }
}

/// Runs the `post_construct` hooks of `entry` on its fresh `instance`, in registration order;
/// the first error stops them.
pub(crate) fn construct(entry: &Entry, instance: &Instance) -> Result<(), DiError> {
    entry
        .post_construct
        .iter()
        .try_for_each(|hook| hook(instance))
}

/// Runs the `pre_destroy` hooks of the entries in `created`, last created first. Each hook runs
/// under `catch_unwind`; a panic becomes a [`DestroyFailure`] and the run continues.
pub(crate) fn destroy<'i>(
    entries: &[Entry],
    created: &[usize],
    instance: impl Fn(usize) -> &'i Instance,
) -> ShutdownReport {
    let mut report = ShutdownReport::default();
    for &idx in created.iter().rev() {
        let entry = &entries[idx];
        if entry.pre_destroy.is_empty() {
            continue;
        }
        let instance = instance(idx);
        let mut clean = true;
        for hook in &entry.pre_destroy {
            if catch_unwind(AssertUnwindSafe(|| hook(instance))).is_err() {
                clean = false;
                report.failures.push(DestroyFailure {
                    type_name: entry.name.to_owned(),
                    message: PANICKED.to_owned(),
                });
            }
        }
        if clean {
            report.destroyed += 1;
        }
    }
    report
}

/// The container-wide creation lock: a mutex whose owner is tracked by [`ThreadId`] with a
/// recursion count, so the owning thread can take it again (a lazy provider that calls back into
/// the container, a `pre_destroy` hook that calls `shutdown`) while other threads wait.
///
/// Lazy creation and `shutdown` hold it; reads of created beans do not. The inner mutex is only
/// held for the bookkeeping, never across user code, so it cannot be poisoned by a hook or a
/// provider.
#[derive(Default)]
pub(crate) struct CreationLock {
    owner: Mutex<Owner>,
    released: Condvar,
}

#[derive(Default)]
struct Owner {
    thread: Option<ThreadId>,
    depth: usize,
}

impl CreationLock {
    /// Takes the lock, waiting while another thread holds it; re-entering on the owning thread
    /// only counts up. Released when the guard drops (also while unwinding).
    #[inline(never)]
    pub(crate) fn lock(&self) -> CreationGuard<'_> {
        let me = thread::current().id();
        let mut owner = locked(&self.owner);
        loop {
            match owner.thread {
                None => {
                    owner.thread = Some(me);
                    break;
                }
                Some(thread) if thread == me => break,
                Some(_) => {
                    owner = self
                        .released
                        .wait(owner)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
        }
        owner.depth += 1;
        CreationGuard { lock: self }
    }
}

/// Holds the [`CreationLock`]; dropping it releases one level.
pub(crate) struct CreationGuard<'a> {
    lock: &'a CreationLock,
}

impl Drop for CreationGuard<'_> {
    fn drop(&mut self) {
        let mut owner = locked(&self.lock.owner);
        owner.depth -= 1;
        if owner.depth == 0 {
            owner.thread = None;
            drop(owner);
            self.lock.released.notify_one();
        }
    }
}

/// Locks `mutex`, ignoring poison: the data it guards here is only touched by short, non-panicking
/// bookkeeping.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
