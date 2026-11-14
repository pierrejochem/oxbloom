//! The provider index and the build algorithm.
//!
//! `build` validates the registrations, then creates every eager entry depth-first in
//! registration order through a [`Resolver`]. Each entry has a once-cell holding its outcome: a
//! created [`Instance`] or the [`DiError`] its creation failed with. Both are final, so a provider
//! runs at most once and a failed entry reports the same error to every later request. The
//! exception is `Cycle` and `TooDeep`, which are never cached: they depend on the requester's
//! stack.
//!
//! The same state lives on in the built container ([`Beans`]): a lazy entry is created by the
//! first request through the same resolver code. Every creation, the eager build included, runs
//! while its thread holds the container-wide reentrant [`CreationLock`], so only one creation
//! chain runs at a time; the resolver stack and the creation order are only touched by the lock
//! holder. Settled outcomes are read without the lock.
//!
//! Creation is recursive (a provider asks the resolver for its dependencies, which runs their
//! providers). The resolver stack is capped at [`MAX_DEPTH`] entries so a deep chain fails with
//! `TooDeep` instead of overflowing the thread stack; the per-level stack use of the functions
//! here is kept small (errors are built in cold, out-of-line functions, hook and lock work is out
//! of line) for the same reason.
//!
//! After a provider returns, its entry's `post_construct` hooks run before the outcome is
//! settled, so no dependent sees an instance whose hooks have not succeeded. Entries whose hooks
//! succeeded are recorded in creation order; a failed build or `shutdown` destroys them in
//! reverse order.

use std::any::{TypeId, type_name};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::DiError;
use crate::entry::{Entry, Instance, Via, read};
use crate::lifecycle::{self, CreationLock, ShutdownReport, locked};

/// Maximum number of entries under creation at once (the depth of the resolver stack).
pub(crate) const MAX_DEPTH: usize = 256;

/// Number of stack entries kept in `TooDeep.path`.
const TOO_DEEP_PATH: usize = 8;

/// Returned by [`Index::pick`] when nothing provides the key.
const NONE: usize = usize::MAX;

/// Returned by [`Index::pick`] when several providers match and none is the single `primary`.
const AMBIGUOUS: usize = usize::MAX - 1;

/// The providers of one key, as entry indices.
#[derive(Default)]
struct Providers {
    /// In registration order: used for single-value selection and `Ambiguous.candidates`.
    registered: Vec<usize>,
    /// Sorted by `order`, ties in registration order: used for collection requests.
    ordered: Vec<usize>,
}

/// Maps a key (a concrete type or a provided `dyn Trait`) to the entries that provide it.
///
/// It stores entries, not values: how an entry gives out a key (its stored instance or a
/// coercion of it) is derived afterwards from the entry and the key ([`Entry::via`]).
pub(crate) struct Index {
    by_key: HashMap<TypeId, Providers>,
}

impl Index {
    /// Indexes `entries` by every key they provide. The first concrete type registered twice (in
    /// order of its second registration) is `Duplicate`, listing the labels of all its
    /// registrations. Only concrete keys are checked: several providers of one trait are normal.
    /// (Provided keys are unsized, so they never coincide with a concrete key.)
    fn new(entries: &[Entry]) -> Result<Self, DiError> {
        let mut by_key: HashMap<TypeId, Providers> = HashMap::with_capacity(entries.len());
        let mut duplicate = None;
        for (i, e) in entries.iter().enumerate() {
            let same_type = &mut by_key.entry(e.key).or_default().registered;
            same_type.push(i);
            if same_type.len() == 2 && duplicate.is_none() {
                duplicate = Some((e.key, e.name));
            }
            for p in &e.provided {
                by_key.entry(p.key).or_default().registered.push(i);
            }
        }
        if let Some((key, name)) = duplicate {
            return Err(DiError::Duplicate {
                type_name: name.to_owned(),
                sources: by_key[&key]
                    .registered
                    .iter()
                    .map(|&i| entries[i].label.clone())
                    .collect(),
            });
        }
        for providers in by_key.values_mut() {
            providers.ordered = providers.registered.clone();
            // Stable: equal orders keep registration order.
            providers.ordered.sort_by_key(|&i| entries[i].order);
        }
        Ok(Index { by_key })
    }

    /// The entry a single-value request for `key` uses: the only provider, else the only
    /// `primary` one; otherwise [`NONE`] or [`AMBIGUOUS`].
    #[inline(never)]
    fn pick(&self, entries: &[Entry], key: TypeId) -> usize {
        let Some(providers) = self.by_key.get(&key) else {
            return NONE;
        };
        match providers.registered.as_slice() {
            &[only] => only,
            several => {
                let mut primaries = several.iter().filter(|&&i| entries[i].primary);
                match (primaries.next(), primaries.next()) {
                    (Some(&primary), None) => primary,
                    _ => AMBIGUOUS,
                }
            }
        }
    }

    /// `Ambiguous` for `key`, listing every provider's label in registration order.
    #[cold]
    #[inline(never)]
    fn ambiguous<T>(
        &self,
        entries: &[Entry],
        key: TypeId,
        requested: &'static str,
    ) -> Result<T, DiError> {
        Err(DiError::Ambiguous {
            requested: requested.to_owned(),
            candidates: self.by_key[&key]
                .registered
                .iter()
                .map(|&i| entries[i].label.clone())
                .collect(),
        })
    }

    /// Whether any entry provides `key`.
    fn provides(&self, key: TypeId) -> bool {
        self.by_key.contains_key(&key)
    }

    /// Every entry providing `key`, sorted by `order` then registration order.
    fn ordered(&self, key: TypeId) -> &[usize] {
        self.by_key.get(&key).map_or(&[], |p| p.ordered.as_slice())
    }
}

/// Per entry and provision: unset until first read, then the coercion result (an `Arc<D>` for
/// the provision's key).
type Coerced = Vec<Box<[OnceLock<Instance>]>>;

/// The registrations of a container and what became of them: built by [`build`], then owned by
/// `Container`, which reads settled outcomes freely and creates lazy entries on first request.
pub(crate) struct Beans {
    entries: Vec<Entry>,
    index: Index,
    /// Per entry: unset until its creation finishes, then the final outcome. Set only by the
    /// holder of `lock`.
    outcomes: Vec<OnceLock<Result<Instance, DiError>>>,
    coerced: Coerced,
    /// Serializes every creation chain (the eager build, each lazy creation) and `shutdown`.
    lock: CreationLock,
    /// Entries under creation, outermost first. Only the holder of `lock` touches it.
    stack: Mutex<Vec<usize>>,
    /// Entries created so far (provider and `post_construct` succeeded), in creation order;
    /// emptied by `shutdown`. Only the holder of `lock` touches it.
    created: Mutex<Vec<usize>>,
    /// Set by the first `shutdown`.
    closed: AtomicBool,
}

/// Validates `entries` and creates every eager one; the first error aborts the build.
///
/// When creation fails, the `pre_destroy` hooks of the entries created so far run in reverse
/// creation order (panics caught) before the error is returned. On success every created entry's
/// coercions are run (if no request ran them yet), so a panicking coercion surfaces here, and the
/// providers and `post_construct` hooks of settled entries are dropped.
pub(crate) fn build(entries: Vec<Entry>) -> Result<Beans, DiError> {
    let index = Index::new(&entries)?;
    let mut beans = Beans {
        outcomes: (0..entries.len()).map(|_| OnceLock::new()).collect(),
        coerced: entries
            .iter()
            .map(|e| e.provided.iter().map(|_| OnceLock::new()).collect())
            .collect(),
        stack: Mutex::new(Vec::new()),
        created: Mutex::new(Vec::with_capacity(entries.len())),
        lock: CreationLock::default(),
        closed: AtomicBool::new(false),
        entries,
        index,
    };
    beans.create_eager()?;
    beans.finish_build();
    Ok(beans)
}

impl Beans {
    /// Creates every non-lazy entry in registration order (lazy ones only when an eager one
    /// depends on them); on failure destroys what was created and returns the error.
    fn create_eager(&self) -> Result<(), DiError> {
        let _guard = self.lock.lock();
        let resolver = Resolver::new(self);
        for i in 0..self.entries.len() {
            if self.entries[i].lazy {
                continue;
            }
            if let Err(e) = resolver.create(i) {
                self.clean_up();
                return Err(e);
            }
        }
        Ok(())
    }

    /// Runs the coercions of every created entry, then drops the providers and `post_construct`
    /// hooks of every settled entry: they never run again (a settled outcome is final), and
    /// dropping them releases what they captured, such as the extra handle an `instance` keeps.
    fn finish_build(&mut self) {
        for (idx, cells) in self.coerced.iter().enumerate() {
            if let Some(Ok(instance)) = self.outcomes[idx].get() {
                for (j, cell) in cells.iter().enumerate() {
                    cell.get_or_init(|| self.entries[idx].coerce(j, instance));
                }
            }
        }
        for (entry, cell) in self.entries.iter_mut().zip(&self.outcomes) {
            if cell.get().is_some() {
                entry.release();
            }
        }
    }

    /// Destroys the entries created so far, last created first; the report is dropped (the
    /// build's error is what the caller sees).
    #[cold]
    #[inline(never)]
    fn clean_up(&self) {
        let created = std::mem::take(&mut *locked(&self.created));
        lifecycle::destroy(&self.entries, &created, |idx| self.created_instance(idx));
    }

    /// Whether `shutdown` has started.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Closes the container and destroys every created entry once, last created first. Takes the
    /// creation lock, so it waits for a lazy creation in flight on another thread and destroys
    /// that bean too. Later calls (also from a `pre_destroy` hook, on the same thread) find the
    /// container closed and return an empty report.
    pub(crate) fn shutdown(&self) -> ShutdownReport {
        let _guard = self.lock.lock();
        if self.closed.swap(true, Ordering::AcqRel) {
            return ShutdownReport::default();
        }
        let created = std::mem::take(&mut *locked(&self.created));
        lifecycle::destroy(&self.entries, &created, |idx| self.created_instance(idx))
    }

    /// The single `K`: `Ok(None)` when nothing provides it, `Ambiguous` as for
    /// [`Resolver::get`]. A lazy provider is created first; its failure is the error.
    pub(crate) fn one<K: ?Sized + Send + Sync + 'static>(&self) -> Result<Option<Arc<K>>, DiError> {
        let key = TypeId::of::<K>();
        match self.index.pick(&self.entries, key) {
            NONE => Ok(None),
            AMBIGUOUS => self.index.ambiguous(&self.entries, key, type_name::<K>()),
            entry => {
                let instance = self.instance(entry)?;
                Ok(Some(self.view::<K>(Slot { entry, instance })))
            }
        }
    }

    /// Every `K`, sorted by `order` then registration order; lazy providers are created first
    /// and the first failure is returned.
    pub(crate) fn all<K: ?Sized + Send + Sync + 'static>(&self) -> Result<Vec<Arc<K>>, DiError> {
        self.index
            .ordered(TypeId::of::<K>())
            .iter()
            .map(|&entry| {
                let instance = self.instance(entry)?;
                Ok(self.view::<K>(Slot { entry, instance }))
            })
            .collect()
    }

    /// The settled outcome of entry `idx`, creating it first when it is a lazy entry not yet
    /// created.
    fn instance(&self, idx: usize) -> Result<&Instance, DiError> {
        let cell = &self.outcomes[idx];
        if cell.get().is_some() {
            return outcome(cell);
        }
        self.create_lazy(idx)
    }

    /// Creates entry `idx` on behalf of a container request: takes the creation lock (waiting
    /// for another thread's chain; re-entering when this thread already holds it), then creates
    /// the entry unless the container was shut down meanwhile. A chain started inside another
    /// one (a provider calling back into the container) continues its resolver stack, so a
    /// cycle across both is still a `Cycle` and the depth limit still holds.
    #[cold]
    #[inline(never)]
    fn create_lazy(&self, idx: usize) -> Result<&Instance, DiError> {
        let _guard = self.lock.lock();
        if self.is_closed() {
            return Err(DiError::ShutDown);
        }
        let _mark = StackMark {
            beans: self,
            len: locked(&self.stack).len(),
        };
        Resolver::new(self).create(idx)
    }

    /// The `Arc<K>` `slot`'s entry gives for its key `K`: the stored instance for the concrete
    /// key, else the provision's coercion, run on first use and cached.
    #[inline(never)]
    fn view<K: ?Sized + Send + Sync + 'static>(&self, slot: Slot<'_>) -> Arc<K> {
        let entry = &self.entries[slot.entry];
        match entry.via(TypeId::of::<K>()) {
            Via::Concrete => read::<K>(slot.instance),
            Via::Provided(j) => read::<K>(
                self.coerced[slot.entry][j].get_or_init(|| entry.coerce(j, slot.instance)),
            ),
        }
    }

    /// The instance of an entry recorded as created.
    fn created_instance(&self, idx: usize) -> &Instance {
        match self.outcomes[idx].get() {
            Some(Ok(instance)) => instance,
            _ => invariant_broken(),
        }
    }
}

/// Restores the resolver stack to `len` entries when dropped. A chain that completes leaves the
/// stack as it found it anyway; this matters when a provider panics, so the next chain does not
/// see the aborted chain's entries (which would look like a cycle).
struct StackMark<'a> {
    beans: &'a Beans,
    len: usize,
}

impl Drop for StackMark<'_> {
    fn drop(&mut self) {
        locked(&self.beans.stack).truncate(self.len);
    }
}

/// A created entry: its index and its stored instance.
#[derive(Clone, Copy)]
struct Slot<'a> {
    entry: usize,
    instance: &'a Instance,
}

/// Handed to providers so they can request their dependencies.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{ContainerBuilder, Resolver};
///
/// let container = ContainerBuilder::new()
///     .instance(2u32)
///     .factory(|r: &Resolver| Ok(*r.get::<u32>()? as u64 * 10))
///     .build()?;
/// assert_eq!(*container.get::<u64>()?, 20);
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
pub struct Resolver<'a> {
    beans: &'a Beans,
    /// Keeps `Resolver` `!Send` and `!Sync`: it is used only on the thread that holds the
    /// creation lock.
    _single_thread: PhantomData<*const ()>,
}

impl<'a> Resolver<'a> {
    /// A resolver over `beans`; the caller holds the creation lock.
    fn new(beans: &'a Beans) -> Self {
        Resolver {
            beans,
            _single_thread: PhantomData,
        }
    }

    /// Returns the singleton `D`, building it first if needed.
    ///
    /// `D` is a registered type or a key one registration provides (`dyn Trait`, see
    /// [`ContainerBuilder::provides`](crate::ContainerBuilder::provides)). Nothing providing `D`
    /// is [`DiError::Missing`]; several providers are [`DiError::Ambiguous`] unless exactly one is
    /// [`primary`](crate::ContainerBuilder::primary).
    ///
    /// A failed creation is remembered: asking again returns the same error and does not run the
    /// provider a second time. A chain of more than 256 nested creations is
    /// [`DiError::TooDeep`]. `Cycle` and `TooDeep` are never cached: they describe the stack of
    /// the request, not the entry, so a later request from another stack runs the provider again.
    /// A provider that turns a dependency's error into its own `Creation` error (for example
    /// with `map_err`) turns a `Cycle` or `TooDeep` into an ordinary failure, which is cached.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{Component, ContainerBuilder, DiError, Resolver};
    ///
    /// struct Name(&'static str);
    /// struct Hello(String);
    /// impl Component for Hello {
    ///     fn create(r: &Resolver) -> Result<Self, DiError> {
    ///         Ok(Hello(format!("hello {}", r.get::<Name>()?.0)))
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new().instance(Name("Ada")).component::<Hello>().build()?;
    /// assert_eq!(c.get::<Hello>()?.0, "hello Ada");
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn get<D: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<D>, DiError> {
        self.resolve(TypeId::of::<D>(), type_name::<D>())
            .map(|_| self.picked::<D>())
    }

    /// Returns the singleton `D`, or `None` when nothing provides `D`.
    ///
    /// Only the absence of a provider is `None`: a provider that exists but fails to create is
    /// its error, and several providers without a single `primary` are
    /// [`DiError::Ambiguous`], as for [`Resolver::get`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{Component, ContainerBuilder, DiError, Resolver};
    ///
    /// struct Cache;
    /// struct Repo { cache: Option<Arc<Cache>> }
    /// impl Component for Repo {
    ///     fn create(r: &Resolver) -> Result<Self, DiError> {
    ///         Ok(Repo { cache: r.get_opt::<Cache>()? })
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new().component::<Repo>().build()?;
    /// assert!(c.get::<Repo>()?.cache.is_none());
    ///
    /// let c = ContainerBuilder::new().component::<Repo>().instance(Cache).build()?;
    /// assert!(c.get::<Repo>()?.cache.is_some());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn get_opt<D: ?Sized + Send + Sync + 'static>(&self) -> Result<Option<Arc<D>>, DiError> {
        let key = TypeId::of::<D>();
        if !self.beans.index.provides(key) {
            return Ok(None);
        }
        self.resolve(key, type_name::<D>())
            .map(|_| Some(self.picked::<D>()))
    }

    /// Returns every provider of `D`, sorted by `order` then registration order; empty when
    /// there is none. Each is built first if needed, and the first failure is returned.
    ///
    /// The requester itself is left out, so a composite that provides `D` can collect all other
    /// providers of `D`. Only the requester: a provider of `D` that is waiting further up the
    /// chain for the requester (A provides `D` and needs B; B asks for all `D`) is still a
    /// [`DiError::Cycle`], whatever the registration order. The skip applies only here, to the
    /// resolver handed to the provider: [`Container::get_all`](crate::Container::get_all) called
    /// from inside a provider (through a captured container) does not skip the requester and
    /// may report `Cycle`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{Component, ContainerBuilder, DiError, Resolver};
    ///
    /// trait Check: Send + Sync { fn name(&self) -> &'static str; }
    /// struct Disk;
    /// impl Check for Disk { fn name(&self) -> &'static str { "disk" } }
    /// struct Db;
    /// impl Check for Db { fn name(&self) -> &'static str { "db" } }
    ///
    /// struct Health { checks: Vec<Arc<dyn Check>> }
    /// impl Component for Health {
    ///     fn create(r: &Resolver) -> Result<Self, DiError> {
    ///         Ok(Health { checks: r.get_all::<dyn Check>()? })
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new()
    ///     .component::<Health>()
    ///     .instance(Disk).provides(|d: Arc<Disk>| -> Arc<dyn Check> { d })
    ///     .instance(Db).provides(|d: Arc<Db>| -> Arc<dyn Check> { d }).order(-1)
    ///     .build()?;
    /// let names: Vec<_> = c.get::<Health>()?.checks.iter().map(|c| c.name()).collect();
    /// assert_eq!(names, ["db", "disk"]);
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn get_all<D: ?Sized + Send + Sync + 'static>(&self) -> Result<Vec<Arc<D>>, DiError> {
        let providers = self.beans.index.ordered(TypeId::of::<D>());
        let mut all = Vec::with_capacity(providers.len());
        for &entry in providers {
            if self.is_requester(entry) {
                continue;
            }
            let instance = self.instance(entry)?;
            all.push(self.view::<D>(Slot { entry, instance }));
        }
        Ok(all)
    }

    // The functions below sit on the recursion path (`get` -> `resolve` -> `create` -> `run` ->
    // provider -> `get` ...). Debug builds give every temporary its own stack slot, so these frames
    // hold only indices and references: anything that returns a `Result` writes it straight into
    // the caller's return slot, and errors are built by the cold functions further down.

    /// The provider a single-value request of `key` selects, created if needed; `Missing` when
    /// nothing provides `key`. `pick` returns a bare index (or a sentinel), and the error cases
    /// are cold, so this frame holds little more than `create`'s `Result`.
    #[inline(never)]
    fn resolve(&self, key: TypeId, name: &'static str) -> Result<&'a Instance, DiError> {
        let entry = self.beans.index.pick(&self.beans.entries, key);
        if entry < AMBIGUOUS {
            self.create(entry)
        } else if entry == NONE {
            self.missing(name)
        } else {
            self.beans.index.ambiguous(&self.beans.entries, key, name)
        }
    }

    /// The instance of entry `idx`, creating it if needed (`get_all`'s out-of-line `create`).
    #[inline(never)]
    fn instance(&self, idx: usize) -> Result<&'a Instance, DiError> {
        self.create(idx)
    }

    /// The `Arc<D>` after a successful [`Resolver::resolve`] for `D`: selects the same entry again
    /// (selection is deterministic) instead of carrying it through the recursive frames.
    #[inline(never)]
    fn picked<D: ?Sized + Send + Sync + 'static>(&self) -> Arc<D> {
        let entry = self
            .beans
            .index
            .pick(&self.beans.entries, TypeId::of::<D>());
        self.view::<D>(Slot {
            entry,
            instance: self.beans.created_instance(entry),
        })
    }

    /// Whether entry `idx` is the one whose provider is making the current request (the top of
    /// the resolver stack). Entries further down the stack are not matched.
    #[inline(never)]
    fn is_requester(&self, idx: usize) -> bool {
        locked(&self.beans.stack).last() == Some(&idx)
    }

    /// The `Arc<D>` `slot`'s entry gives for its key `D` (see `Beans::view`).
    #[inline(always)]
    fn view<D: ?Sized + Send + Sync + 'static>(&self, slot: Slot<'_>) -> Arc<D> {
        self.beans.view::<D>(slot)
    }

    /// The outcome of entry `idx`, running its provider if it has no final outcome yet.
    #[inline(always)]
    fn create(&self, idx: usize) -> Result<&'a Instance, DiError> {
        if is_settled(&self.beans.outcomes[idx]) {
            return outcome(&self.beans.outcomes[idx]);
        }
        if !self.enter(idx) {
            // A cycle or depth failure belongs to the requester, not to `idx`: not cached.
            return self.refusal(idx);
        }
        self.run(idx)
    }

    /// Runs the provider of `idx` (already pushed by `enter`), then hands its outcome to
    /// `finish`. Nothing but the outcome lives across the provider call: hook and bookkeeping
    /// work stays in `finish`, off the recursion path's stack frames.
    #[inline(never)]
    fn run(&self, idx: usize) -> Result<&'a Instance, DiError> {
        let outcome = (self.beans.entries[idx].provider)(self);
        self.finish(idx, outcome)
    }

    /// Pops `idx` off the resolver stack, runs its `post_construct` hooks on a created
    /// instance, records it as created when they succeed, and settles the outcome.
    #[inline(never)]
    fn finish(
        &self,
        idx: usize,
        outcome: Result<Instance, DiError>,
    ) -> Result<&'a Instance, DiError> {
        self.leave();
        let outcome = match outcome {
            Ok(instance) => self.construct(idx, instance),
            Err(e) => Err(e),
        };
        settle(&self.beans.outcomes[idx], outcome)
    }

    /// Runs the `post_construct` hooks of the fresh `instance` of `idx` and records it as
    /// created. The container may have been shut down by this thread meanwhile (by the provider
    /// or a hook calling `Container::shutdown`): an instance finished after that is never handed
    /// out and never left undestroyed.
    #[inline(never)]
    fn construct(&self, idx: usize, instance: Instance) -> Result<Instance, DiError> {
        if self.beans.is_closed() {
            // Closed while the provider ran: drop the instance, run no hooks.
            return shut_down();
        }
        lifecycle::construct(&self.beans.entries[idx], &instance)?;
        if self.beans.is_closed() {
            return self.destroy_late(idx, &instance);
        }
        locked(&self.beans.created).push(idx);
        Ok(instance)
    }

    /// A `post_construct` hook of `idx` shut the container down: `shutdown` did not see this
    /// entry (it was not yet recorded as created), so its `pre_destroy` hooks run here.
    #[cold]
    #[inline(never)]
    fn destroy_late(&self, idx: usize, instance: &Instance) -> Result<Instance, DiError> {
        lifecycle::destroy(&self.beans.entries, &[idx], |_| instance);
        shut_down()
    }

    /// Pushes `idx` onto the resolver stack unless the container is closed, or that would repeat
    /// an entry or exceed the depth limit.
    #[inline(never)]
    fn enter(&self, idx: usize) -> bool {
        if self.beans.is_closed() {
            return false;
        }
        let mut stack = locked(&self.beans.stack);
        if stack.len() >= MAX_DEPTH || stack.contains(&idx) {
            return false;
        }
        stack.push(idx);
        true
    }

    #[inline(never)]
    fn leave(&self) {
        locked(&self.beans.stack).pop();
    }

    fn names(&self, idxs: &[usize]) -> Vec<String> {
        idxs.iter()
            .map(|&i| self.beans.entries[i].name.to_owned())
            .collect()
    }

    #[cold]
    #[inline(never)]
    fn missing<T>(&self, requested: &'static str) -> Result<T, DiError> {
        Err(DiError::Missing {
            requested: requested.to_owned(),
            required_by: self.names(&locked(&self.beans.stack)),
        })
    }

    /// Why `enter(idx)` refused: `ShutDown` once the container is closed, else a cycle if `idx`
    /// is under creation, otherwise the depth limit.
    #[cold]
    #[inline(never)]
    fn refusal<T>(&self, idx: usize) -> Result<T, DiError> {
        if self.beans.is_closed() {
            return shut_down();
        }
        let stack = locked(&self.beans.stack);
        Err(match stack.iter().position(|&i| i == idx) {
            Some(start) => {
                let mut path = self.names(&stack[start..]);
                path.push(self.beans.entries[idx].name.to_owned());
                DiError::Cycle { path }
            }
            None => DiError::TooDeep {
                limit: MAX_DEPTH,
                path: self.names(&stack[..stack.len().min(TOO_DEEP_PATH)]),
            },
        })
    }
}

/// Records the outcome of a provider run as final and returns it, except `Cycle` and `TooDeep`.
/// Those describe the stack the provider ran on, not the entry, so they are passed on and the
/// cell stays unset: a request from another stack runs the provider again.
///
/// The entry was on the stack while its provider ran, so no nested call can have set the cell (a
/// nested request for it is a cycle).
#[inline(never)]
fn settle(
    cell: &OnceLock<Result<Instance, DiError>>,
    outcome: Result<Instance, DiError>,
) -> Result<&Instance, DiError> {
    if let Err(e @ (DiError::Cycle { .. } | DiError::TooDeep { .. })) = outcome {
        return Err(e);
    }
    if cell.set(outcome).is_err() {
        invariant_broken();
    }
    self::outcome(cell)
}

/// Whether `cell` holds a final outcome (out of line: keeps the `OnceLock` check's temporaries
/// off the recursion path's frames in debug builds).
#[inline(never)]
fn is_settled(cell: &OnceLock<Result<Instance, DiError>>) -> bool {
    cell.get().is_some()
}

/// A finished outcome: the instance, or a clone of the cached error.
#[inline(never)]
fn outcome(cell: &OnceLock<Result<Instance, DiError>>) -> Result<&Instance, DiError> {
    match cell.get() {
        Some(Ok(instance)) => Ok(instance),
        Some(Err(e)) => Err(e.clone()),
        None => invariant_broken(),
    }
}

/// `ShutDown`, built out of line.
#[cold]
#[inline(never)]
fn shut_down<T>() -> Result<T, DiError> {
    Err(DiError::ShutDown)
}

#[cold]
#[inline(never)]
fn invariant_broken() -> ! {
    unreachable!("an entry's outcome is set exactly once, after its provider returns")
}
