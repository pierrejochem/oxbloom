//! Registrations: one [`Entry`] per `component`, `instance`, `factory`, `bean` or `try_bean`
//! call, plus the modifiers (`provides`, `primary`, `order`, `lazy`, profile conditions, entry
//! conditions, lifecycle hooks) applied to it.

use std::any::{Any, TypeId, type_name};
use std::sync::Arc;

use crate::condition::Condition;
use crate::lifecycle::{PostConstruct, PreDestroy};
use crate::{DiError, Resolver};

/// A created value, type-erased: a `Box` holding an `Arc<K>`.
///
/// An entry's stored instance always holds `Arc<C>` for its concrete type `C`. Storing the `Arc`
/// itself (which is `Sized` for every `K`) lets the same shape carry unsized keys too: a
/// [`Provision`] turns the stored `Arc<C>` into another `Instance` holding `Arc<dyn Trait>`,
/// computed once per entry and provision and cached by the build.
pub(crate) type Instance = Box<dyn Any + Send + Sync>;

/// Creates the concrete instance of an entry, requesting dependencies through the resolver.
pub(crate) type Provider = Box<dyn Fn(&Resolver<'_>) -> Result<Instance, DiError> + Send + Sync>;

/// Turns an entry's stored instance (`Arc<C>`) into an instance of a provided key (`Arc<D>`).
pub(crate) type Coerce = Box<dyn Fn(&Instance) -> Instance + Send + Sync>;

/// A key an entry provides besides its concrete type, e.g. `dyn Notifier`.
pub(crate) struct Provision {
    /// `TypeId` of the provided key `D`.
    pub(crate) key: TypeId,
    /// Applies the registered coercion to the stored `Arc<C>`; the result holds an `Arc<D>`.
    coerce: Coerce,
}

/// How an entry gives out the value for one of its keys.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Via {
    /// The concrete key: the stored instance itself.
    Concrete,
    /// The provided key at this index of [`Entry::provided`]; its value is the cached result of
    /// that provision's coercion.
    Provided(usize),
}

/// One registration: its keys, modifiers, a diagnostic source label and how to create it.
pub(crate) struct Entry {
    /// `TypeId` of the concrete type the provider creates.
    pub(crate) key: TypeId,
    /// `type_name` of the concrete type, used in error paths.
    pub(crate) name: &'static str,
    /// Where the registration came from, e.g. `component app::Repo`; used in `Duplicate` and
    /// `Ambiguous`.
    pub(crate) label: String,
    pub(crate) provider: Provider,
    /// Keys provided besides the concrete one, in `provides` order, each at most once.
    pub(crate) provided: Vec<Provision>,
    /// Preferred when a single-value request has several candidates.
    pub(crate) primary: bool,
    /// Sort key for collection requests; ties keep registration order.
    pub(crate) order: i32,
    /// Created on first request instead of by the build loop.
    pub(crate) lazy: bool,
    /// Profile conditions, any-of; empty means unconditional. Evaluated once by `build()`
    /// before indexing, never during creation.
    pub(crate) profiles: Vec<ProfileCondition>,
    /// Entry conditions, all-of, in the order they were added. Evaluated once by `build()`
    /// after profile filtering against the registrations kept before this one, then dropped.
    pub(crate) conditions: Vec<Condition>,
    /// The innermost group of `ContainerBuilder::group` this registration was made in.
    pub(crate) group: Option<u32>,
    /// Run in this order right after the provider succeeds; the first error fails the creation.
    pub(crate) post_construct: Vec<PostConstruct>,
    /// Run in this order when the created entry is destroyed.
    pub(crate) pre_destroy: Vec<PreDestroy>,
}

/// One profile condition: met when `name` is active, or, when `negated`, when it is not.
pub(crate) struct ProfileCondition {
    pub(crate) name: String,
    pub(crate) negated: bool,
}

impl Entry {
    /// An entry for the concrete type `T`, labelled `"{kind} {type name}"`.
    pub(crate) fn new<T: Send + Sync + 'static>(kind: &str, provider: Provider) -> Self {
        Self::labelled::<T>(format!("{kind} {}", type_name::<T>()), provider)
    }

    /// An entry for the concrete type `T` with the source label `label`.
    pub(crate) fn labelled<T: Send + Sync + 'static>(label: String, provider: Provider) -> Self {
        Entry {
            key: TypeId::of::<T>(),
            name: type_name::<T>(),
            label,
            provider,
            provided: Vec::new(),
            primary: false,
            order: 0,
            lazy: false,
            profiles: Vec::new(),
            conditions: Vec::new(),
            group: None,
            post_construct: Vec::new(),
            pre_destroy: Vec::new(),
        }
    }

    /// Adds the provided key `D`, produced from the stored `Arc<T>` by `coerce`.
    ///
    /// The caller has checked that `T` is this entry's concrete type and that `D` is unsized, so
    /// a provided key never equals a concrete key. Providing a key twice is ignored: the first
    /// coercion wins.
    pub(crate) fn provide<T, D, F>(&mut self, coerce: F)
    where
        T: Send + Sync + 'static,
        D: ?Sized + Send + Sync + 'static,
        F: Fn(Arc<T>) -> Arc<D> + Send + Sync + 'static,
    {
        debug_assert_eq!(self.key, TypeId::of::<T>());
        let key = TypeId::of::<D>();
        if self.provided.iter().any(|p| p.key == key) {
            return;
        }
        self.provided.push(Provision {
            key,
            coerce: Box::new(move |instance| Box::new(coerce(read::<T>(instance)))),
        });
    }

    /// Drops the provider and the `post_construct` hooks (and what they captured) once the
    /// entry's outcome is settled: a settled entry is never created again. The `pre_destroy`
    /// hooks stay for `shutdown`.
    pub(crate) fn release(&mut self) {
        self.provider = Box::new(|_| released());
        self.post_construct = Vec::new();
    }

    /// Whether the entry takes part in a build with the `active` profiles: it has no condition,
    /// or any of its conditions is met.
    pub(crate) fn is_kept(&self, active: &[String]) -> bool {
        self.profiles.is_empty()
            || self
                .profiles
                .iter()
                .any(|c| active.contains(&c.name) != c.negated)
    }

    /// How this entry gives out `key`. The index lists an entry under a key only when the key
    /// is its concrete type or one of its provisions.
    pub(crate) fn via(&self, key: TypeId) -> Via {
        if key == self.key {
            return Via::Concrete;
        }
        match self.provided.iter().position(|p| p.key == key) {
            Some(j) => Via::Provided(j),
            None => key_mismatch(),
        }
    }

    /// Runs provision `j`'s coercion on the stored `instance`; the result holds an `Arc<D>` for
    /// that provision's key `D`. Called once per (entry, provision); callers cache the result.
    pub(crate) fn coerce(&self, j: usize, instance: &Instance) -> Instance {
        (self.provided[j].coerce)(instance)
    }
}

/// Erases a concrete value handle into an [`Instance`] stored under the key `T`.
pub(crate) fn erase<T: Send + Sync + 'static>(value: Arc<T>) -> Instance {
    Box::new(value)
}

/// Recovers the `Arc<K>` an [`Instance`] holds.
///
/// Callers only pass an instance together with the key it holds: an entry's stored instance
/// for its concrete key ([`Via::Concrete`]), or a cached coercion result for its provided key
/// ([`Via::Provided`]). A mismatch is an internal invariant violation.
pub(crate) fn read<K: ?Sized + Send + Sync + 'static>(instance: &Instance) -> Arc<K> {
    match instance.downcast_ref::<Arc<K>>() {
        Some(arc) => Arc::clone(arc),
        None => key_mismatch(),
    }
}

#[cold]
#[inline(never)]
fn released() -> ! {
    unreachable!("a settled entry's provider is never run again")
}

#[cold]
#[inline(never)]
fn key_mismatch() -> ! {
    unreachable!("an instance is only read through a key it provides")
}
