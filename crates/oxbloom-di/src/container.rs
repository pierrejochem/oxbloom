//! The public surface: [`Component`], [`ContainerBuilder`] and [`Container`].

use std::any::{TypeId, type_name};
use std::collections::HashSet;
use std::sync::Arc;

use crate::beans::{self, BeanFn, Configuration, TryBeanFn};
use crate::condition::{
    self, Condition, ConditionContext, ConditionOutcome, ConditionReport, Conditions, GroupWalk,
    Groups,
};
use crate::entry::{Entry, ProfileCondition, Provider, erase};
use crate::lifecycle::{self, PostConstruct, PreDestroy};
use crate::resolve::{self, Beans};
use crate::{DiError, Environment, Lifecycle, Resolver, ShutdownReport};

/// A type that can build itself from other components.
///
/// `#[component]` implements it for structs whose fields are all injectable (`Arc<T>`,
/// `Arc<dyn Trait>`, `Option<Arc<..>>`, `Vec<Arc<..>>`) and for unit structs. Implement it by
/// hand when construction needs logic or can fail.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{Component, ContainerBuilder, DiError, Resolver};
///
/// struct Clock { start: std::time::Instant }
///
/// impl Component for Clock {
///     fn create(_: &Resolver) -> Result<Self, DiError> {
///         Ok(Clock { start: std::time::Instant::now() })
///     }
/// }
///
/// let container = ContainerBuilder::new().component::<Clock>().build()?;
/// assert!(container.get::<Clock>()?.start.elapsed().as_secs() < 5);
/// # Ok::<(), DiError>(())
/// ```
pub trait Component: Sized + Send + Sync + 'static {
    /// Builds `Self`, fetching dependencies from `r`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{Component, ContainerBuilder, DiError, Resolver};
    ///
    /// struct Repo;
    /// impl Component for Repo {
    ///     fn create(_: &Resolver) -> Result<Self, DiError> { Ok(Repo) }
    /// }
    ///
    /// struct Service { repo: Arc<Repo> }
    /// impl Component for Service {
    ///     fn create(r: &Resolver) -> Result<Self, DiError> {
    ///         Ok(Service { repo: r.get::<Repo>()? })
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new().component::<Repo>().component::<Service>().build()?;
    /// assert!(c.get::<Service>().is_ok());
    /// # Ok::<(), DiError>(())
    /// ```
    fn create(r: &Resolver) -> Result<Self, DiError>;

    /// Adds modifiers to the registration of `Self`; the default adds none.
    ///
    /// [`ContainerBuilder::component`] pushes the entry for `Self`, then passes the builder
    /// through `configure`, so the builder's last registration is `Self` and modifiers such as
    /// [`ContainerBuilder::provides`], [`ContainerBuilder::primary`] and
    /// [`ContainerBuilder::order`] apply to it. `#[component(..)]` implements it from its
    /// options (`provides`, `primary`, `order`, `lazy`, `lifecycle`, `profile`). Registering further entries here is possible but makes
    /// the component's modifiers order-dependent; keep it to modifiers of `Self`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{Component, ContainerBuilder, DiError, Resolver};
    ///
    /// trait Greeter: Send + Sync { fn greet(&self) -> &'static str; }
    /// struct English;
    /// impl Greeter for English { fn greet(&self) -> &'static str { "hello" } }
    ///
    /// impl Component for English {
    ///     fn create(_: &Resolver) -> Result<Self, DiError> { Ok(English) }
    ///     fn configure(builder: ContainerBuilder) -> ContainerBuilder {
    ///         builder.provides(|e: Arc<English>| -> Arc<dyn Greeter> { e }).primary()
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new().component::<English>().build()?;
    /// assert_eq!(c.get::<dyn Greeter>()?.greet(), "hello");
    /// # Ok::<(), DiError>(())
    /// ```
    fn configure(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}

/// Collects registrations; every error surfaces from [`ContainerBuilder::build`].
///
/// # Examples
///
/// ```
/// use oxbloom_di::ContainerBuilder;
///
/// struct Settings { name: &'static str }
///
/// let container = ContainerBuilder::new()
///     .instance(Settings { name: "demo" })
///     .build()?;
/// assert_eq!(container.get::<Settings>()?.name, "demo");
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
#[derive(Default)]
pub struct ContainerBuilder {
    entries: Vec<Entry>,
    /// The first modifier misuse, reported by `build()` as `Invalid`.
    invalid: Option<String>,
    /// The active profile names, set by [`ContainerBuilder::active_profiles`].
    active: Vec<String>,
    /// The property source of conditions, set by [`ContainerBuilder::environment`].
    environment: Option<Box<dyn Environment>>,
    /// The groups of [`ContainerBuilder::group`]; boxed so a builder without groups stays small.
    groups: Option<Box<Groups>>,
}

impl ContainerBuilder {
    /// An empty builder.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// let container = ContainerBuilder::new().build()?;
    /// # let _ = container;
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    fn push<T: Send + Sync + 'static>(mut self, kind: &str, provider: Provider) -> Self {
        self.entries.push(Entry::new::<T>(kind, provider));
        self.grouped()
    }

    /// Registers `T`, built through [`Component::create`].
    ///
    /// Registration order does not matter; dependencies are built first. After pushing the
    /// entry, the builder goes through [`Component::configure`], which applies the modifiers a
    /// `#[component(..)]` declares.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, component};
    ///
    /// #[component]
    /// struct UserRepo {}
    ///
    /// #[component]
    /// struct UserService { repo: Arc<UserRepo> }
    ///
    /// let container = ContainerBuilder::new()
    ///     .component::<UserService>()
    ///     .component::<UserRepo>()
    ///     .build()?;
    /// # let _ = container;
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn component<T: Component>(self) -> Self {
        T::configure(self.push::<T>(
            "component",
            Box::new(|r| T::create(r).map(|v| erase(Arc::new(v)))),
        ))
    }

    /// Registers a ready-made singleton, for values you create yourself (clients, pools, settings).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// let container = ContainerBuilder::new().instance(42u32).build()?;
    /// assert_eq!(*container.get::<u32>()?, 42);
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn instance<T: Send + Sync + 'static>(self, value: T) -> Self {
        let value = Arc::new(value);
        self.push::<T>("instance", Box::new(move |_| Ok(erase(Arc::clone(&value)))))
    }

    /// Registers a closure that builds `T`. The closure may request other singletons.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, Resolver};
    ///
    /// struct Settings { name: &'static str }
    /// struct Banner(String);
    ///
    /// let container = ContainerBuilder::new()
    ///     .instance(Settings { name: "demo" })
    ///     .factory(|r: &Resolver| {
    ///         let s = r.get::<Settings>()?;
    ///         Ok(Banner(format!("== {} ==", s.name)))
    ///     })
    ///     .build()?;
    /// assert_eq!(container.get::<Banner>()?.0, "== demo ==");
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn factory<T, F>(self, f: F) -> Self
    where
        T: Send + Sync + 'static,
        F: Fn(&Resolver) -> Result<T, DiError> + Send + Sync + 'static,
    {
        self.push::<T>(
            "factory",
            Box::new(move |r| f(r).map(|v| erase(Arc::new(v)))),
        )
    }

    /// Registers the result of `f` as the bean `T`. `f` is a closure or `fn` item with up to 12
    /// parameters, each an injectable shape ([`Inject`](crate::Inject): `Arc<T>`,
    /// `Arc<dyn Trait>`, `Option<Arc<..>>`, `Vec<Arc<..>>`) resolved like a `#[component]`
    /// field. Use it for third-party types and computed values; for a fallible function use
    /// [`ContainerBuilder::try_bean`].
    ///
    /// The bean is keyed by its return type `T`, and every modifier ([`provides`],
    /// [`primary`](ContainerBuilder::primary), [`lazy`](ContainerBuilder::lazy), profile
    /// conditions, lifecycle hooks) applies to it. Its source label, used in
    /// [`DiError::Duplicate`] and [`DiError::Ambiguous`], is `closure at file:line` of this
    /// call. A parameter that is not an injectable shape is a compile error.
    ///
    /// Note that a function returning `Result<T, DiError>` registered here is a bean of type
    /// `Result<T, DiError>`; that is what `try_bean` is for. Likewise a function returning
    /// `Arc<X>` registers the bean `Arc<X>`, not `X`: return the value itself.
    ///
    /// [`provides`]: ContainerBuilder::provides
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Settings { db_url: &'static str }
    /// // A third-party type: no `#[component]` possible.
    /// struct Pool { url: String, size: u32 }
    /// trait Clock: Send + Sync { fn now(&self) -> u64; }
    /// struct Fixed;
    /// impl Clock for Fixed { fn now(&self) -> u64 { 42 } }
    /// struct Audit { pool: Arc<Pool>, clock: Option<Arc<dyn Clock>> }
    ///
    /// fn pool(settings: Arc<Settings>) -> Pool {
    ///     Pool { url: settings.db_url.to_owned(), size: 4 }
    /// }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Settings { db_url: "postgres://db/app" })
    ///     .bean(pool)
    ///     .bean(|pool: Arc<Pool>, clock: Option<Arc<dyn Clock>>| Audit { pool, clock })
    ///     .build()?;
    /// let audit = c.get::<Audit>()?;
    /// assert_eq!((audit.pool.url.as_str(), audit.pool.size), ("postgres://db/app", 4));
    /// assert!(audit.clock.is_none());
    ///
    /// // A missing dependency names the chain that needed it.
    /// let err = ContainerBuilder::new().bean(pool).build();
    /// assert!(matches!(err, Err(DiError::Missing { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    #[track_caller]
    pub fn bean<T, Args, F>(self, f: F) -> Self
    where
        F: BeanFn<Args, T>,
        T: Send + Sync + 'static,
    {
        let label = closure_label();
        self.push_labelled::<T>(label, beans::provider(f))
    }

    /// Registers the result of the fallible `f` as the bean `T`: like
    /// [`ContainerBuilder::bean`], for a function returning `Result<T, DiError>`.
    ///
    /// An `Err` from `f` fails the bean's creation with [`DiError::Creation`] for `T` (a
    /// `Creation` keeps its message and source; another variant becomes the fixed text
    /// `bean failed: {error}`), aborts `build()` for an eager bean, and is cached like every
    /// creation failure. Errors resolving the parameters (missing, ambiguous, cycle) are
    /// reported as they are. The source label is `closure at file:line` of this call.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Port(u16);
    /// struct Listener { port: u16 }
    ///
    /// fn listener(port: Arc<Port>) -> Result<Listener, DiError> {
    ///     if port.0 == 0 {
    ///         return Err(DiError::creation::<Listener>("port must not be 0"));
    ///     }
    ///     Ok(Listener { port: port.0 })
    /// }
    ///
    /// let c = ContainerBuilder::new().instance(Port(8080)).try_bean(listener).build()?;
    /// assert_eq!(c.get::<Listener>()?.port, 8080);
    ///
    /// let failed = ContainerBuilder::new().instance(Port(0)).try_bean(listener).build();
    /// assert!(matches!(failed, Err(DiError::Creation { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    #[track_caller]
    pub fn try_bean<T, Args, F>(self, f: F) -> Self
    where
        F: TryBeanFn<Args, T>,
        T: Send + Sync + 'static,
    {
        let label = closure_label();
        self.push_labelled::<T>(label, beans::try_provider(f))
    }

    /// Adds the registrations of the [`Configuration`] `C`, as `C::register(self)`.
    ///
    /// A modifier written after `.configuration::<C>()` (such as `.primary()` or `.lazy()`)
    /// applies only to the last registration `C` made, not to all of them; put modifiers inside
    /// `register`, next to the registration they belong to.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{Configuration, ContainerBuilder, DiError};
    ///
    /// trait Mailer: Send + Sync { fn send(&self) -> &'static str; }
    /// struct Smtp;
    /// impl Mailer for Smtp { fn send(&self) -> &'static str { "smtp" } }
    ///
    /// struct MailConfig;
    /// impl Configuration for MailConfig {
    ///     fn register(builder: ContainerBuilder) -> ContainerBuilder {
    ///         builder.bean(|| Smtp).provides(|s: Arc<Smtp>| -> Arc<dyn Mailer> { s })
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new().configuration::<MailConfig>().build()?;
    /// assert_eq!(c.get::<dyn Mailer>()?.send(), "smtp");
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn configuration<C: Configuration>(self) -> Self {
        C::register(self)
    }

    pub(crate) fn push_labelled<T: Send + Sync + 'static>(
        mut self,
        label: String,
        provider: Provider,
    ) -> Self {
        self.entries.push(Entry::labelled::<T>(label, provider));
        self.grouped()
    }

    /// The last registration a modifier applies to: `None` before any registration, and also
    /// at the start of a group, so a modifier never reaches across a group's start.
    fn last_mut(&mut self) -> Option<&mut Entry> {
        let floor = self.groups.as_ref().map_or(0, |g| g.floor());
        if self.entries.len() <= floor {
            return None;
        }
        self.entries.last_mut()
    }

    /// Records the innermost open group on the registration just pushed.
    fn grouped(mut self) -> Self {
        if let (Some(groups), Some(last)) = (&self.groups, self.entries.last_mut()) {
            last.group = groups.current();
        }
        self
    }

    /// Makes the last registration also available as `D`, typically a trait object.
    ///
    /// `coerce` turns the registered `Arc<T>` into an `Arc<D>`; for a trait it is the unsizing
    /// coercion `|a: Arc<T>| -> Arc<dyn Trait> { a }`, so `get::<dyn Trait>()` and `get::<T>()`
    /// share one allocation. `coerce` runs once per registration, by the end of `build()`; every
    /// request for `D` gets a clone of that one `Arc<D>` (a decorator such as
    /// `|a| -> Arc<dyn Trait> { Arc::new(Logged(a)) }` is created once, too).
    ///
    /// A provided trait must be `Send + Sync` (`trait Notifier: Send + Sync`). The key is the
    /// exact type: `dyn Notifier` and `dyn Notifier + Send + Sync` are different keys, so request
    /// the one you provide. Several registrations may provide the same trait: see
    /// [`ContainerBuilder::primary`] and [`Container::get_all`]. Providing the same `D` twice
    /// keeps the first coercion.
    ///
    /// Misuse does not fail here: `provides` before any registration, with a `T` that is not
    /// the type of the last registration, or with a sized `D` (a concrete type, not a trait
    /// object or slice), makes [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// trait Notifier: Send + Sync { fn notify(&self) -> String; }
    /// struct Mailer;
    /// impl Notifier for Mailer { fn notify(&self) -> String { "mail sent".into() } }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Mailer)
    ///     .provides(|m: Arc<Mailer>| -> Arc<dyn Notifier> { m })
    ///     .build()?;
    /// assert_eq!(c.get::<dyn Notifier>()?.notify(), "mail sent");
    /// assert!(c.get::<Mailer>().is_ok()); // still available as itself
    ///
    /// // The closure must take the type of the last registration.
    /// struct Sms;
    /// impl Notifier for Sms { fn notify(&self) -> String { "sms sent".into() } }
    /// let wrong = ContainerBuilder::new()
    ///     .instance(Mailer)
    ///     .provides(|s: Arc<Sms>| -> Arc<dyn Notifier> { s })
    ///     .build();
    /// assert!(matches!(wrong, Err(DiError::Invalid { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn provides<T, D, F>(mut self, coerce: F) -> Self
    where
        T: Send + Sync + 'static,
        D: ?Sized + Send + Sync + 'static,
        F: Fn(Arc<T>) -> Arc<D> + Send + Sync + 'static,
    {
        match self.last_mut() {
            // `Arc<D>` is a thin pointer exactly when `D` is sized.
            Some(_) if size_of::<Arc<D>>() == size_of::<usize>() => {
                let message = format!(
                    "`provides` needs an unsized key such as `dyn Trait`, got `{}`",
                    type_name::<D>()
                );
                self.misuse(message);
            }
            Some(last) if last.key == TypeId::of::<T>() => last.provide(coerce),
            Some(last) => {
                let message = format!(
                    "`provides` takes `Arc<{}>` but the last registration is `{}`",
                    type_name::<T>(),
                    last.name
                );
                self.misuse(message);
            }
            None => self.misuse("`provides` must follow a registration".to_owned()),
        }
        self
    }

    /// Prefers the last registration when a single value of one of its keys has several
    /// providers. Two `primary` providers of one key are [`DiError::Ambiguous`].
    ///
    /// Before any registration it makes [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// trait Store: Send + Sync { fn kind(&self) -> &'static str; }
    /// struct Memory;
    /// impl Store for Memory { fn kind(&self) -> &'static str { "memory" } }
    /// struct Disk;
    /// impl Store for Disk { fn kind(&self) -> &'static str { "disk" } }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Memory).provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
    ///     .instance(Disk).provides(|s: Arc<Disk>| -> Arc<dyn Store> { s }).primary()
    ///     .build()?;
    /// assert_eq!(c.get::<dyn Store>()?.kind(), "disk");
    ///
    /// // Without `primary`, two providers are ambiguous.
    /// let c = ContainerBuilder::new()
    ///     .instance(Memory).provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
    ///     .instance(Disk).provides(|s: Arc<Disk>| -> Arc<dyn Store> { s })
    ///     .build()?;
    /// assert!(matches!(c.get::<dyn Store>(), Err(DiError::Ambiguous { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn primary(mut self) -> Self {
        match self.last_mut() {
            Some(last) => last.primary = true,
            None => self.misuse("`primary` must follow a registration".to_owned()),
        }
        self
    }

    /// Sets the position of the last registration in collection requests
    /// ([`Container::get_all`], [`Resolver::get_all`]): ascending `n`, then registration order.
    /// The default is `0`; calling it again replaces the value.
    ///
    /// Before any registration it makes [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// trait Step: Send + Sync { fn name(&self) -> &'static str; }
    /// struct Auth;
    /// impl Step for Auth { fn name(&self) -> &'static str { "auth" } }
    /// struct Log;
    /// impl Step for Log { fn name(&self) -> &'static str { "log" } }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Auth).provides(|s: Arc<Auth>| -> Arc<dyn Step> { s }).order(10)
    ///     .instance(Log).provides(|s: Arc<Log>| -> Arc<dyn Step> { s }).order(-10)
    ///     .build()?;
    /// let steps: Vec<_> = c.get_all::<dyn Step>()?.iter().map(|s| s.name()).collect();
    /// assert_eq!(steps, ["log", "auth"]);
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn order(mut self, n: i32) -> Self {
        match self.last_mut() {
            Some(last) => last.order = n,
            None => self.misuse("`order` must follow a registration".to_owned()),
        }
        self
    }

    /// Creates the last registration on first request instead of during
    /// [`ContainerBuilder::build`]: on the first [`Container::get`], [`Container::get_opt`] or
    /// [`Container::get_all`] that selects it, or when another bean is created that depends on
    /// it. An eager bean that depends on a lazy one therefore creates it during `build()`.
    ///
    /// Errors of a lazy bean's creation (a missing or ambiguous dependency, a cycle, a failing
    /// provider or `post_construct`) appear at first use, not at `build()`, and a failure is
    /// cached like any other: later requests get the same error. An `Option` request whose only
    /// provider is lazy and fails is that error, never `None`. The exception is a lazy bean in a
    /// dependency cycle: `Cycle` (like `TooDeep`) is never cached, so every request for it runs
    /// the providers of the cycle again before reporting `Cycle`.
    ///
    /// Lazy creation is serialized by one container-wide lock: concurrent first requests create
    /// the bean once (the others wait), and two threads never deadlock on each other's lazy
    /// chains. The lock is reentrant for the creating thread, so a provider may call back into
    /// the container; it must not wait for another thread that itself creates a lazy bean of
    /// the same container. [`Container::shutdown`] takes the same lock: it waits for a creation
    /// in flight and destroys the bean too. A lazy bean that was never created is not
    /// destroyed; one that was is destroyed at the position where it was really created.
    ///
    /// Before any registration it makes [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::sync::atomic::{AtomicUsize, Ordering};
    /// use oxbloom_di::{ContainerBuilder, DiError, Resolver};
    ///
    /// struct SearchIndex { docs: usize }
    ///
    /// let loads = Arc::new(AtomicUsize::new(0));
    /// let counter = Arc::clone(&loads);
    /// let c = ContainerBuilder::new()
    ///     .factory(move |_: &Resolver| {
    ///         counter.fetch_add(1, Ordering::SeqCst); // expensive: runs only when needed
    ///         Ok(SearchIndex { docs: 3 })
    ///     })
    ///     .lazy()
    ///     .build()?;
    /// assert_eq!(loads.load(Ordering::SeqCst), 0);
    /// assert_eq!(c.get::<SearchIndex>()?.docs, 3);
    /// assert_eq!(c.get::<SearchIndex>()?.docs, 3);
    /// assert_eq!(loads.load(Ordering::SeqCst), 1);
    ///
    /// // A lazy bean's failure shows up at first use, not at build.
    /// struct Remote;
    /// let c = ContainerBuilder::new()
    ///     .factory(|_: &Resolver| -> Result<Remote, DiError> {
    ///         Err(DiError::creation::<Remote>("unreachable"))
    ///     })
    ///     .lazy()
    ///     .build()?;
    /// assert!(matches!(c.get::<Remote>(), Err(DiError::Creation { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn lazy(mut self) -> Self {
        match self.last_mut() {
            Some(last) => last.lazy = true,
            None => self.misuse("`lazy` must follow a registration".to_owned()),
        }
        self
    }

    /// Sets the active profile names; registrations with profile conditions
    /// ([`ContainerBuilder::profile`], [`ContainerBuilder::profile_not`]) are kept or skipped by
    /// them. The default is none active. Calling it again replaces the list; it is not a
    /// modifier, so it may come anywhere in the chain.
    ///
    /// The container does not read configuration itself: pass the names your application
    /// activated, e.g. `Config::active_profiles` from `oxbloom-config`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// struct DbUrl(&'static str);
    ///
    /// let c = ContainerBuilder::new()
    ///     .active_profiles(["dev"])
    ///     .instance(DbUrl("sqlite::memory:")).profile("dev")
    ///     .instance(DbUrl("postgres://db/app")).profile("prod")
    ///     .build()?;
    /// assert_eq!(c.get::<DbUrl>()?.0, "sqlite::memory:");
    ///
    /// // Owned names work too, e.g. a list read from configuration.
    /// let active: Vec<String> = vec!["prod".to_owned()];
    /// let c = ContainerBuilder::new()
    ///     .active_profiles(active)
    ///     .instance(DbUrl("sqlite::memory:")).profile("dev")
    ///     .instance(DbUrl("postgres://db/app")).profile("prod")
    ///     .build()?;
    /// assert_eq!(c.get::<DbUrl>()?.0, "postgres://db/app");
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn active_profiles<I, S>(mut self, active: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.active = active.into_iter().map(Into::into).collect();
        self
    }

    /// Keeps the last registration only when the profile `name` is active; a name starting with
    /// `!` keeps it only when the rest of the name is inactive (as [`ContainerBuilder::profile_not`]).
    ///
    /// It sets a condition on the last registration; it does not activate `name`. Which profiles
    /// are active is set once for the whole builder by [`ContainerBuilder::active_profiles`].
    ///
    /// Repeated `profile` and `profile_not` calls on one registration are any-of: it is kept when
    /// at least one condition holds. A skipped registration is invisible to the build: it is not
    /// created, not a candidate for any key, never [`DiError::Duplicate`], and a dependency on it
    /// is [`DiError::Missing`]. Names match exactly.
    ///
    /// Before any registration, or with an empty name (also `"!"`), it makes
    /// [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// trait Mailer: Send + Sync { fn kind(&self) -> &'static str; }
    /// struct Smtp;
    /// impl Mailer for Smtp { fn kind(&self) -> &'static str { "smtp" } }
    /// struct Console;
    /// impl Mailer for Console { fn kind(&self) -> &'static str { "console" } }
    ///
    /// let builder = |active: &[&'static str]| {
    ///     ContainerBuilder::new()
    ///         .active_profiles(active.to_vec())
    ///         .instance(Smtp).provides(|m: Arc<Smtp>| -> Arc<dyn Mailer> { m }).profile("prod")
    ///         .instance(Console).provides(|m: Arc<Console>| -> Arc<dyn Mailer> { m })
    ///         .profile("dev").profile("test") // dev or test
    /// };
    /// assert_eq!(builder(&["prod"]).build()?.get::<dyn Mailer>()?.kind(), "smtp");
    /// assert_eq!(builder(&["test"]).build()?.get::<dyn Mailer>()?.kind(), "console");
    /// // Neither active: nothing provides `dyn Mailer`.
    /// let c = builder(&[]).build()?;
    /// assert!(matches!(c.get::<dyn Mailer>(), Err(DiError::Missing { .. })));
    ///
    /// // `!name` negates.
    /// let c = ContainerBuilder::new().instance(Console).profile("!prod").build()?;
    /// assert!(c.get::<Console>().is_ok());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn profile(self, name: &str) -> Self {
        match name.strip_prefix('!') {
            Some(rest) => self.profile_condition("profile", rest, true),
            None => self.profile_condition("profile", name, false),
        }
    }

    /// Keeps the last registration only when the profile `name` is not active; the name is
    /// taken as written (no `!` prefix handling). Combines any-of with
    /// [`ContainerBuilder::profile`] conditions on the same registration.
    ///
    /// It sets a condition on the last registration; which profiles are active is set by
    /// [`ContainerBuilder::active_profiles`].
    ///
    /// Before any registration, or with an empty name, it makes [`ContainerBuilder::build`]
    /// return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct DemoData;
    ///
    /// let c = ContainerBuilder::new().instance(DemoData).profile_not("prod").build()?;
    /// assert!(c.get::<DemoData>().is_ok()); // no profile active
    ///
    /// let c = ContainerBuilder::new()
    ///     .active_profiles(["prod"])
    ///     .instance(DemoData).profile_not("prod")
    ///     .build()?;
    /// assert!(matches!(c.get::<DemoData>(), Err(DiError::Missing { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn profile_not(self, name: &str) -> Self {
        self.profile_condition("profile_not", name, true)
    }

    /// Adds a profile condition to the last registration, or records the misuse of `modifier`
    /// (no registration yet, or an empty name).
    fn profile_condition(mut self, modifier: &str, name: &str, negated: bool) -> Self {
        match self.last_mut() {
            Some(_) if name.is_empty() => self.misuse(format!("`{modifier}` needs a name")),
            Some(last) => last.profiles.push(ProfileCondition {
                name: name.to_owned(),
                negated,
            }),
            None => self.misuse(format!("`{modifier}` must follow a registration")),
        }
        self
    }

    /// Keeps the last registration only when `f` matches; `name` labels the condition in the
    /// [`ConditionReport`].
    ///
    /// Conditions are evaluated once by [`ContainerBuilder::build`], after profile filtering and
    /// before any provider runs, in registration order. `f` gets a [`ConditionContext`] that
    /// sees the registrations kept before this one (never this one, never later ones), the
    /// [`environment`](ContainerBuilder::environment) and the active profiles. Several
    /// conditions on one registration are combined with AND (profile conditions are any-of):
    /// they run in the order they were added and evaluation stops at the first one that does not
    /// match. A skipped registration is invisible, exactly like one skipped by a profile: it is
    /// not a candidate for any key, never [`DiError::Duplicate`], a dependency on it is
    /// [`DiError::Missing`], and its provider and hooks never run.
    ///
    /// The reason of a [`ConditionOutcome`] appears in the report: build it from fixed text,
    /// type names and key names, never from configuration values. A condition that panics is
    /// not caught: the panic propagates out of `build()`.
    ///
    /// Before any registration it makes [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError};
    ///
    /// struct Cache;
    /// struct Warmup;
    /// let needs_cache = |cx: &oxbloom_di::ConditionContext| {
    ///     if cx.has_bean::<Cache>() {
    ///         ConditionOutcome::matched()
    ///     } else {
    ///         ConditionOutcome::skipped("no cache to warm up")
    ///     }
    /// };
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Cache)
    ///     .instance(Warmup).condition("needs_cache", needs_cache)
    ///     .build()?;
    /// assert!(c.get_opt::<Warmup>()?.is_some());
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Warmup).condition("needs_cache", needs_cache)
    ///     .build()?;
    /// assert!(c.get_opt::<Warmup>()?.is_none());
    /// assert_eq!(c.condition_report().records()[0].reason(), "no cache to warm up");
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn condition(
        self,
        name: &'static str,
        f: impl Fn(&ConditionContext) -> ConditionOutcome + Send + Sync + 'static,
    ) -> Self {
        self.entry_condition("condition", Condition::new(name, f))
    }

    /// Keeps the last registration only when a registration kept before it provides `T`, as its
    /// concrete type or through [`provides`](ContainerBuilder::provides).
    ///
    /// Only earlier registrations count: `on_bean::<Later>()` placed before `Later` is
    /// registered does not match. Registrations skipped by a profile or by a condition do not
    /// count either. Combines with the other conditions of the registration by AND (see
    /// [`ContainerBuilder::condition`]).
    ///
    /// Before any registration it makes [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Pool;
    /// struct HealthCheck;
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Pool)
    ///     .instance(HealthCheck).on_bean::<Pool>()
    ///     .build()?;
    /// assert!(c.get_opt::<HealthCheck>()?.is_some());
    ///
    /// // `Pool` registered after the condition does not count.
    /// let c = ContainerBuilder::new()
    ///     .instance(HealthCheck).on_bean::<Pool>()
    ///     .instance(Pool)
    ///     .build()?;
    /// assert!(c.get_opt::<HealthCheck>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_bean<T: ?Sized + 'static>(self) -> Self {
        self.entry_condition("on_bean", Condition::on_bean::<T>())
    }

    /// Keeps the last registration only when no registration kept before it provides `T`: the
    /// back-off rule for defaults. Register the application's own beans first; a default
    /// registered after them with `on_missing_bean` steps aside.
    ///
    /// The same rules as [`ContainerBuilder::on_bean`] apply: only earlier, kept registrations
    /// count, trait objects count through `provides`, and the conditions of one registration
    /// combine by AND.
    ///
    /// Before any registration it makes [`ContainerBuilder::build`] return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// trait Clock: Send + Sync { fn now(&self) -> u64; }
    /// struct SystemClock;
    /// impl Clock for SystemClock { fn now(&self) -> u64 { 1 } }
    /// struct FixedClock;
    /// impl Clock for FixedClock { fn now(&self) -> u64 { 42 } }
    ///
    /// let with_default = |b: ContainerBuilder| {
    ///     b.instance(SystemClock)
    ///         .provides(|c: Arc<SystemClock>| -> Arc<dyn Clock> { c })
    ///         .on_missing_bean::<dyn Clock>()
    /// };
    ///
    /// // Nothing provides `dyn Clock` yet: the default is kept.
    /// let c = with_default(ContainerBuilder::new()).build()?;
    /// assert_eq!(c.get::<dyn Clock>()?.now(), 1);
    ///
    /// // The application's clock comes first: the default steps aside.
    /// let user = ContainerBuilder::new()
    ///     .instance(FixedClock)
    ///     .provides(|c: Arc<FixedClock>| -> Arc<dyn Clock> { c });
    /// let c = with_default(user).build()?;
    /// assert_eq!(c.get::<dyn Clock>()?.now(), 42);
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_missing_bean<T: ?Sized + 'static>(self) -> Self {
        self.entry_condition("on_missing_bean", Condition::on_missing_bean::<T>())
    }

    /// Keeps the last registration only when the property `key` of the
    /// [`environment`](ContainerBuilder::environment) is present, whatever its value; a section
    /// or a list counts when [`Environment::contains`] says so. Without an environment every
    /// property is absent. The report names the key, never the value.
    ///
    /// It also matches when the value is `false`, unlike Spring's `@ConditionalOnProperty`. For
    /// an on/off switch use [`on_property_value`](ContainerBuilder::on_property_value) with
    /// `"true"`.
    ///
    /// Before any registration, or with an empty `key`, it makes [`ContainerBuilder::build`]
    /// return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError, Environment};
    ///
    /// struct Props;
    /// impl Environment for Props {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "smtp.host").then(|| "mail.internal".to_owned())
    ///     }
    /// }
    /// struct Mailer;
    ///
    /// let c = ContainerBuilder::new()
    ///     .environment(Props)
    ///     .instance(Mailer).on_property("smtp.host")
    ///     .build()?;
    /// assert!(c.get_opt::<Mailer>()?.is_some());
    ///
    /// let c = ContainerBuilder::new().instance(Mailer).on_property("smtp.host").build()?;
    /// assert!(c.get_opt::<Mailer>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_property(self, key: &str) -> Self {
        self.property_condition("on_property", key, || Condition::on_property(key))
    }

    /// Keeps the last registration only when the property `key` equals `value` exactly
    /// (case-sensitive, no trimming, no substring match). An absent key does not match; see
    /// [`ContainerBuilder::on_property_value_or_missing`] for the variant that does.
    ///
    /// When the value differs, the report records `property "<key>" does not have the expected
    /// value`: neither the actual nor the expected value appears.
    ///
    /// The comparison is case-sensitive, unlike Spring's `havingValue`: `TRUE` does not match
    /// `"true"`. `value` is part of the source code and is compared as plain text, so never pass
    /// a secret as the expected value.
    ///
    /// Before any registration, or with an empty `key`, it makes [`ContainerBuilder::build`]
    /// return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError, Environment};
    ///
    /// struct Props(&'static str);
    /// impl Environment for Props {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "cache.kind").then(|| self.0.to_owned())
    ///     }
    /// }
    /// struct RedisCache;
    ///
    /// let build = |kind| {
    ///     ContainerBuilder::new()
    ///         .environment(Props(kind))
    ///         .instance(RedisCache).on_property_value("cache.kind", "redis")
    ///         .build()
    /// };
    /// assert!(build("redis")?.get_opt::<RedisCache>()?.is_some());
    /// assert!(build("redis-cluster")?.get_opt::<RedisCache>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_property_value(self, key: &str, value: &str) -> Self {
        self.property_condition("on_property_value", key, || {
            Condition::on_property_value(key, value, false)
        })
    }

    /// Like [`ContainerBuilder::on_property_value`], but also matches when the property `key`
    /// is absent: for a default that applies unless configured otherwise. On a `Config` table or
    /// list the key still counts as not set (only [`ContainerBuilder::on_property`] counts a
    /// section as present), so the condition matches.
    ///
    /// Before any registration, or with an empty `key`, it makes [`ContainerBuilder::build`]
    /// return [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError, Environment};
    ///
    /// struct Props(Option<&'static str>);
    /// impl Environment for Props {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         if key == "cache.kind" { self.0.map(str::to_owned) } else { None }
    ///     }
    /// }
    /// struct MemoryCache;
    ///
    /// let build = |kind| {
    ///     ContainerBuilder::new()
    ///         .environment(Props(kind))
    ///         .instance(MemoryCache).on_property_value_or_missing("cache.kind", "memory")
    ///         .build()
    /// };
    /// assert!(build(None)?.get_opt::<MemoryCache>()?.is_some());
    /// assert!(build(Some("memory"))?.get_opt::<MemoryCache>()?.is_some());
    /// assert!(build(Some("redis"))?.get_opt::<MemoryCache>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_property_value_or_missing(self, key: &str, value: &str) -> Self {
        self.property_condition("on_property_value_or_missing", key, || {
            Condition::on_property_value(key, value, true)
        })
    }

    /// Sets the property source of the property conditions and of
    /// [`ConditionContext::property`]. Without one, every property is absent. Calling it again
    /// replaces it; it is not a modifier, so it may come anywhere in the chain.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError, Environment};
    ///
    /// struct Props;
    /// impl Environment for Props {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "feature.audit").then(|| "on".to_owned())
    ///     }
    /// }
    /// struct Audit;
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Audit).on_property_value("feature.audit", "on")
    ///     .environment(Props) // position in the chain does not matter
    ///     .build()?;
    /// assert!(c.get_opt::<Audit>()?.is_some());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn environment(mut self, environment: impl Environment + 'static) -> Self {
        self.environment = Some(Box::new(environment));
        self
    }

    /// Runs `f` and treats every registration it makes as one group named `name`, kept only
    /// when `conditions` match.
    ///
    /// The group's conditions are evaluated once by [`ContainerBuilder::build`], at the
    /// position of the group's first registration that a profile keeps, against the
    /// registrations kept before it: earlier registrations count, the group's own do not. When
    /// they do not match, every registration of the group is skipped, including those of nested
    /// groups, without evaluating their own conditions; the providers and hooks of skipped
    /// registrations never run. When they match, each registration's own conditions are
    /// evaluated as usual and combined with the group's by AND. Groups nest: the conditions of
    /// all enclosing groups are ANDed, the outermost decided first. A group with no
    /// registrations is a no-op and leaves no record. A group whose registrations are all
    /// skipped by profiles is never decided either: its conditions do not run and it leaves no
    /// record.
    ///
    /// The [`ConditionReport`] records a group's conditions with the group's name as
    /// registration, before the records of the registration that decided it.
    ///
    /// The start of a group is a barrier for modifiers: a modifier written inside `f` before
    /// its first registration finds no registration, so it never changes the registration
    /// before the group, and [`ContainerBuilder::build`] returns [`DiError::Invalid`] (the same
    /// as a modifier before any registration). A modifier written after `group(..)` applies to
    /// the last registration made inside it (or, for an empty group, to the one before it),
    /// like after [`ContainerBuilder::configuration`].
    ///
    /// A property condition in `conditions` with an empty key makes [`ContainerBuilder::build`]
    /// return [`DiError::Invalid`].
    ///
    /// `f` should extend the builder it is given. If it returns a different builder instead,
    /// that builder is used as it is, with its own groups, and this group has no effect.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{Conditions, ContainerBuilder, DiError, Resolver};
    ///
    /// trait Cache: Send + Sync { fn kind(&self) -> &'static str; }
    /// struct MemoryCache;
    /// impl Cache for MemoryCache { fn kind(&self) -> &'static str { "memory" } }
    /// struct Redis;
    /// impl Cache for Redis { fn kind(&self) -> &'static str { "redis" } }
    /// struct CacheStats { cache: Arc<dyn Cache> }
    ///
    /// // Defaults that back off as a whole when the application has its own cache.
    /// let defaults = |b: ContainerBuilder| {
    ///     b.group("CacheDefaults", Conditions::new().on_missing_bean::<dyn Cache>(), |b| {
    ///         b.instance(MemoryCache)
    ///             .provides(|c: Arc<MemoryCache>| -> Arc<dyn Cache> { c })
    ///             .factory(|r: &Resolver| Ok(CacheStats { cache: r.get::<dyn Cache>()? }))
    ///     })
    /// };
    ///
    /// let c = defaults(ContainerBuilder::new()).build()?;
    /// assert_eq!(c.get::<CacheStats>()?.cache.kind(), "memory");
    ///
    /// let user = ContainerBuilder::new()
    ///     .instance(Redis)
    ///     .provides(|c: Arc<Redis>| -> Arc<dyn Cache> { c });
    /// let c = defaults(user).build()?;
    /// assert_eq!(c.get::<dyn Cache>()?.kind(), "redis");
    /// assert!(c.get_opt::<CacheStats>()?.is_none());
    /// let record = &c.condition_report().records()[0];
    /// assert_eq!((record.registration(), record.matched()), ("CacheDefaults", false));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn group(
        mut self,
        name: &'static str,
        mut conditions: Conditions,
        f: impl FnOnce(ContainerBuilder) -> ContainerBuilder,
    ) -> Self {
        if let Some(message) = conditions.invalid.take() {
            self.misuse(message);
        }
        let registrations = self.entries.len();
        let id =
            self.groups
                .get_or_insert_with(Default::default)
                .open(name, conditions, registrations);
        let mut this = f(self);
        let registrations = this.entries.len();
        if let Some(groups) = &mut this.groups {
            groups.close(id, registrations);
        }
        this
    }

    /// Adds the property condition `make` builds to the last registration, or records the
    /// misuse of `modifier` when `key` is empty (the text never contains a value).
    fn property_condition(
        mut self,
        modifier: &str,
        key: &str,
        make: impl FnOnce() -> Condition,
    ) -> Self {
        if key.is_empty() {
            self.misuse(format!("`{modifier}` needs a non-empty key"));
            return self;
        }
        self.entry_condition(modifier, make())
    }

    /// Adds `condition` to the last registration, or records the misuse of `modifier`.
    fn entry_condition(mut self, modifier: &str, condition: Condition) -> Self {
        match self.last_mut() {
            Some(last) => last.conditions.push(condition),
            None => self.misuse(format!("`{modifier}` needs a registration before it")),
        }
        self
    }

    /// Runs `post_construct` after the last registration's instance is created, before any
    /// dependent receives it, and after the hooks of its own dependencies.
    ///
    /// `f` gets the bean (`&T`); an `Err` fails the bean's creation and aborts
    /// [`ContainerBuilder::build`] with [`DiError::Creation`] (another error variant is wrapped
    /// into `Creation` for `T`). Beans created before the failure are then destroyed in reverse
    /// creation order: their [`pre_destroy`](ContainerBuilder::pre_destroy) hooks run, the failed
    /// bean's do not. A hook that panics is not caught. Calling it again adds another hook; the
    /// hooks run in the order they were added.
    ///
    /// Misuse does not fail here: before any registration, or with a `T` that is not the type
    /// of the last registration, [`ContainerBuilder::build`] returns [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Limits { max_users: u32 }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Limits { max_users: 100 })
    ///     .post_construct(|l: &Limits| {
    ///         if l.max_users == 0 {
    ///             return Err(DiError::creation::<Limits>("max_users must be positive"));
    ///         }
    ///         Ok(())
    ///     })
    ///     .build()?;
    /// assert_eq!(c.get::<Limits>()?.max_users, 100);
    ///
    /// let failed = ContainerBuilder::new()
    ///     .instance(Limits { max_users: 0 })
    ///     .post_construct(|l: &Limits| {
    ///         if l.max_users == 0 {
    ///             return Err(DiError::creation::<Limits>("max_users must be positive"));
    ///         }
    ///         Ok(())
    ///     })
    ///     .build();
    /// assert!(matches!(failed, Err(DiError::Creation { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn post_construct<T, F>(self, f: F) -> Self
    where
        T: Send + Sync + 'static,
        F: Fn(&T) -> Result<(), DiError> + Send + Sync + 'static,
    {
        self.hooks::<T>(
            "post_construct",
            Some(lifecycle::post_construct::<T, F>(f)),
            None,
        )
    }

    /// Runs `pre_destroy` when the last registration's bean is destroyed: by
    /// [`Container::shutdown`], or by a failed [`ContainerBuilder::build`] after the bean was
    /// created. Beans are destroyed in reverse creation order, so a bean's dependencies are
    /// still intact in its hook. A panic is caught: `shutdown` reports it in the
    /// [`ShutdownReport`]; the cleanup of a failed build drops it.
    /// Calling it again adds another hook; the hooks run in the order they were added.
    ///
    /// Misuse does not fail here: before any registration, or with a `T` that is not the type
    /// of the last registration, [`ContainerBuilder::build`] returns [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::atomic::{AtomicBool, Ordering};
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// #[derive(Default)]
    /// struct Connection { closed: AtomicBool }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Connection::default())
    ///     .pre_destroy(|conn: &Connection| conn.closed.store(true, Ordering::SeqCst))
    ///     .build()?;
    /// let conn = c.get::<Connection>()?;
    /// assert_eq!(c.shutdown().destroyed, 1);
    /// assert!(conn.closed.load(Ordering::SeqCst));
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn pre_destroy<T, F>(self, f: F) -> Self
    where
        T: Send + Sync + 'static,
        F: Fn(&T) + Send + Sync + 'static,
    {
        self.hooks::<T>("pre_destroy", None, Some(lifecycle::pre_destroy::<T, F>(f)))
    }

    /// Registers both hooks of `T`'s [`Lifecycle`] implementation for the last registration, as
    /// [`ContainerBuilder::post_construct`] and [`ContainerBuilder::pre_destroy`] would.
    ///
    /// Misuse does not fail here: before any registration, or with a `T` that is not the type
    /// of the last registration, [`ContainerBuilder::build`] returns [`DiError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Mutex;
    /// use oxbloom_di::{ContainerBuilder, DiError, Lifecycle, Resolver};
    ///
    /// struct Scheduler { jobs: Mutex<Vec<&'static str>> }
    ///
    /// impl Lifecycle for Scheduler {
    ///     fn post_construct(&self) -> Result<(), DiError> {
    ///         self.jobs.lock().unwrap().push("cleanup");
    ///         Ok(())
    ///     }
    ///     fn pre_destroy(&self) {
    ///         self.jobs.lock().unwrap().clear();
    ///     }
    /// }
    ///
    /// let c = ContainerBuilder::new()
    ///     .factory(|_: &Resolver| Ok(Scheduler { jobs: Mutex::new(Vec::new()) }))
    ///     .lifecycle::<Scheduler>()
    ///     .build()?;
    /// let scheduler = c.get::<Scheduler>()?;
    /// assert_eq!(*scheduler.jobs.lock().unwrap(), ["cleanup"]);
    /// c.shutdown();
    /// assert!(scheduler.jobs.lock().unwrap().is_empty());
    ///
    /// // The type must be the last registration's.
    /// struct Other;
    /// let wrong = ContainerBuilder::new().instance(Other).lifecycle::<Scheduler>().build();
    /// assert!(matches!(wrong, Err(DiError::Invalid { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn lifecycle<T: Lifecycle>(self) -> Self {
        self.hooks::<T>(
            "lifecycle",
            Some(lifecycle::post_construct::<T, _>(T::post_construct)),
            Some(lifecycle::pre_destroy::<T, _>(T::pre_destroy)),
        )
    }

    /// Adds hooks for `T` to the last registration, or records the misuse of `modifier`.
    fn hooks<T: 'static>(
        mut self,
        modifier: &str,
        post: Option<PostConstruct>,
        pre: Option<PreDestroy>,
    ) -> Self {
        match self.last_mut() {
            Some(last) if last.key == TypeId::of::<T>() => {
                last.post_construct.extend(post);
                last.pre_destroy.extend(pre);
            }
            Some(last) => {
                let message = format!(
                    "`{modifier}` takes `{}` but the last registration is `{}`",
                    type_name::<T>(),
                    last.name
                );
                self.misuse(message);
            }
            None => self.misuse(format!("`{modifier}` must follow a registration")),
        }
        self
    }

    /// Records a modifier misuse; the first one is reported by `build()`.
    #[cold]
    fn misuse(&mut self, message: String) {
        self.invalid.get_or_insert(message);
    }

    /// Builds every eager singleton in dependency order (registration order among independents);
    /// [`lazy`](ContainerBuilder::lazy) ones wait for their first request unless an eager one
    /// depends on them.
    ///
    /// First, registrations whose profile conditions do not hold for the active
    /// [`active_profiles`](ContainerBuilder::active_profiles) are dropped; the rest of the build never sees
    /// them. Next, the entry conditions ([`ContainerBuilder::condition`],
    /// [`ContainerBuilder::on_bean`], [`ContainerBuilder::on_missing_bean`], the property
    /// conditions) are evaluated once in registration order against the registrations kept so
    /// far, and registrations whose conditions do not all match are dropped too; a condition that
    /// panics propagates out of `build()`. Builder misuse is reported as [`DiError::Invalid`]
    /// even when it concerns a dropped registration. `build()` is
    /// [`ContainerBuilder::build_with_report`] without the report.
    ///
    /// Each provider runs at most once per outcome. A failed creation is remembered: every later
    /// request for that type gets the same error, even when an earlier requester ignored it.
    /// `Cycle` and `TooDeep` are never cached: they describe the stack of the request, not the
    /// type, so a later request from another stack runs the provider again. A provider that
    /// wraps a dependency's error into its own `Creation` (for example with `map_err`) makes a
    /// `Cycle` or `TooDeep` an ordinary, cached failure. A provider that panics is not caught:
    /// the panic propagates out of `build()`.
    ///
    /// Creation is recursive, and a dependency chain more than 256 creations deep is
    /// [`DiError::TooDeep`]. The limit counts depth, not bytes: the stack a 256-deep chain needs
    /// grows with the stack frames of its providers. Measured for a 256-deep chain on x86_64
    /// Linux: the simplest `factory` closures need about 206 KB (debug) or 79 KB (release);
    /// `#[component]` structs with four `Arc` fields need about 278 KB (debug) or 119 KB
    /// (release); `bean` factories need about 194 KB (debug) or 79 KB (release) with one `Arc`
    /// parameter, about 298 KB (debug) or 111 KB (release) with four; `#[bean]` methods of a
    /// `#[configuration]` need about 230 KB (debug) or 95 KB (release) with one parameter
    /// besides `&self`, about 322 KB (debug) or 95 KB (release) with four. Large values built by
    /// value in a provider add their size per level. Build deep graphs on a thread with enough
    /// stack (the main thread has 8 MB on Linux).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError, component};
    ///
    /// #[component]
    /// struct UserRepo {}
    ///
    /// #[component]
    /// struct UserService { repo: Arc<UserRepo> }
    ///
    /// // UserRepo was never registered, so wiring fails here, not at first use.
    /// let result = ContainerBuilder::new().component::<UserService>().build();
    /// assert!(matches!(result, Err(DiError::Missing { .. })));
    /// ```
    pub fn build(mut self) -> Result<Container, DiError> {
        // The same result as `build_with_report().0`, written out because this frame stays
        // live during the recursive creation in `resolve::build`: no returned tuple, no error
        // temporaries (see `misused`), so the depth-probe stack minima stay unchanged.
        if self.invalid.is_some() {
            return misused(&mut self.invalid);
        }
        let mut entries = self.entries;
        let mut report = filter(
            &mut entries,
            &self.active,
            &self.environment,
            &mut self.groups,
        );
        resolve::build(entries).map(|beans| Container {
            beans,
            report: std::mem::take(&mut report),
        })
    }

    /// Like [`ContainerBuilder::build`], and also returns the [`ConditionReport`] of the
    /// conditions evaluated, also when the build fails.
    ///
    /// The steps are: builder misuse is [`DiError::Invalid`] with an empty report (no condition
    /// runs, so a printed report shows `(none)` in both sections); profile
    /// filtering; the condition walk in registration order (see
    /// [`ContainerBuilder::condition`]); then provider resolution of the kept registrations. A
    /// failure in the last step still returns the report of the walk. After a successful build
    /// the same report is also available from [`Container::condition_report`]. A condition that
    /// panics is not caught: the panic propagates out of this call.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError, Resolver};
    ///
    /// struct Server;
    /// struct Db;
    /// struct Repo { _db: Arc<Db> }
    ///
    /// let (built, report) = ContainerBuilder::new()
    ///     .instance(Server).on_missing_bean::<Server>()
    ///     .factory(|r: &Resolver| Ok(Repo { _db: r.get::<Db>()? }))
    ///     .build_with_report();
    /// // The build fails (nothing provides `Db`), but the report is there.
    /// assert!(matches!(built, Err(DiError::Missing { .. })));
    /// assert_eq!(report.records().len(), 1);
    /// assert!(report.records()[0].matched());
    /// ```
    pub fn build_with_report(mut self) -> (Result<Container, DiError>, ConditionReport) {
        if let Some(message) = self.invalid {
            return (
                Err(DiError::Invalid { message }),
                ConditionReport::default(),
            );
        }
        let mut entries = self.entries;
        let report = filter(
            &mut entries,
            &self.active,
            &self.environment,
            &mut self.groups,
        );
        finish(resolve::build(entries), report)
    }
}

/// The `Invalid` error of the first recorded misuse. Kept out of `build()` so the error's
/// temporaries are not part of its frame, which stays live during the recursive creation.
#[cold]
#[inline(never)]
fn misused(invalid: &mut Option<String>) -> Result<Container, DiError> {
    Err(DiError::Invalid {
        message: invalid.take().unwrap_or_default(),
    })
}

/// Pairs the outcome of `resolve::build` with the report; a container keeps a copy of it.
#[inline(never)]
fn finish(
    built: Result<Beans, DiError>,
    report: ConditionReport,
) -> (Result<Container, DiError>, ConditionReport) {
    match built {
        Ok(beans) => {
            let container = Container {
                beans,
                report: report.clone(),
            };
            (Ok(container), report)
        }
        Err(e) => (Err(e), report),
    }
}

/// Drops the registrations skipped by a profile (`active`), then walks the rest in registration
/// order and drops every one whose conditions do not all match, evaluating each entry's
/// conditions against the entries kept before it. Returns the report of every evaluated
/// condition. Profile and condition filtering happen once, here, so the recursive creation path
/// stays unchanged.
///
/// The kept-so-far index holds the concrete key and every provided key of each kept entry and
/// grows during the walk, so the cost is linear in entries plus conditions. Not inlined, so
/// its locals are gone from the stack before the recursive creation starts.
#[inline(never)]
fn filter(
    entries: &mut Vec<Entry>,
    active: &[String],
    environment: &Option<Box<dyn Environment>>,
    groups: &mut Option<Box<Groups>>,
) -> ConditionReport {
    entries.retain(|e| e.is_kept(active));
    let environment = environment.as_deref();
    let mut kept: HashSet<TypeId> = HashSet::new();
    let mut records = Vec::new();
    let mut groups = groups.take();
    let mut walk = groups.as_deref().map(GroupWalk::new);
    entries.retain_mut(|entry| {
        let conditions = std::mem::take(&mut entry.conditions);
        if let (Some(group), Some(groups), Some(walk)) = (entry.group, &mut groups, &mut walk)
            && !walk.admits(groups, group, (&kept, environment, active), &mut records)
        {
            return false;
        }
        let cx = ConditionContext::new(&kept, environment, active, &entry.label);
        if !condition::evaluate(&conditions, &cx, &mut records) {
            return false;
        }
        kept.insert(entry.key);
        kept.extend(entry.provided.iter().map(|p| p.key));
        true
    });
    ConditionReport::new(records)
}

/// `closure at file:line` for the caller of a `#[track_caller]` builder method.
#[track_caller]
fn closure_label() -> String {
    let at = std::panic::Location::caller();
    format!("closure at {}:{}", at.file(), at.line())
}

/// All singletons, shareable across threads. Eager beans were created by
/// [`ContainerBuilder::build`]; [`lazy`](ContainerBuilder::lazy) ones are created by their first
/// request.
///
/// Call [`Container::shutdown`] to run the beans' `pre_destroy` hooks; dropping the container
/// does not run them.
///
/// # Examples
///
/// ```
/// use oxbloom_di::ContainerBuilder;
///
/// let container = ContainerBuilder::new().instance("shared").build()?;
/// std::thread::scope(|s| {
///     s.spawn(|| assert_eq!(*container.get::<&str>().unwrap(), "shared"));
/// });
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
pub struct Container {
    beans: Beans,
    /// The conditions the build evaluated.
    report: ConditionReport,
}

impl Container {
    /// The [`ConditionReport`] of the build that created this container: every condition
    /// evaluated, in evaluation order. For the report of a failed build use
    /// [`ContainerBuilder::build_with_report`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Server;
    /// struct DefaultServer;
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Server)
    ///     .instance(DefaultServer).on_missing_bean::<Server>()
    ///     .build()?;
    /// let record = &c.condition_report().records()[0];
    /// assert!(!record.matched());
    /// assert!(record.reason().starts_with("an earlier registration already provides"));
    /// assert!(c.get_opt::<DefaultServer>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn condition_report(&self) -> &ConditionReport {
        &self.report
    }

    /// Returns the singleton `T`. Eager beans are already built; a
    /// [`lazy`](ContainerBuilder::lazy) one is created by its first request (and its creation
    /// error, if any, is returned from then on).
    ///
    /// `T` is a registered type or a provided key such as `dyn Trait`; selection follows
    /// [`Resolver::get`]: one provider, else the single `primary` one, else
    /// [`DiError::Ambiguous`]. Nothing providing `T` is [`DiError::Missing`] with an empty
    /// `required_by`. After [`Container::shutdown`] it is [`DiError::ShutDown`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// let container = ContainerBuilder::new().instance(1u8).build()?;
    /// let a = container.get::<u8>()?;
    /// let b = container.get::<u8>()?;
    /// assert!(Arc::ptr_eq(&a, &b));                       // same singleton
    /// assert!(matches!(container.get::<u16>(), Err(DiError::Missing { .. })));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>, DiError> {
        self.open()?;
        match self.beans.one::<T>()? {
            Some(value) => Ok(value),
            None => Err(DiError::Missing {
                requested: type_name::<T>().to_owned(),
                required_by: Vec::new(),
            }),
        }
    }

    /// Returns the singleton `T`, or `None` when nothing provides `T`.
    ///
    /// Only the absence of a provider is `None`: a lazy provider is created first, and its
    /// failure is the error.
    ///
    /// Several providers without a single `primary` are still [`DiError::Ambiguous`]. After
    /// [`Container::shutdown`] it is [`DiError::ShutDown`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// struct Metrics;
    /// let c = ContainerBuilder::new().instance(7u32).build()?;
    /// assert_eq!(c.get_opt::<u32>()?.as_deref(), Some(&7));
    /// assert!(c.get_opt::<Metrics>()?.is_none());
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn get_opt<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Option<Arc<T>>, DiError> {
        self.open()?;
        self.beans.one::<T>()
    }

    /// Returns every provider of `T`, sorted by `order` then registration order; empty when
    /// there is none. Lazy providers are created first, and the first failure is the error.
    /// After [`Container::shutdown`] it is [`DiError::ShutDown`].
    ///
    /// Unlike [`Resolver::get_all`], it never leaves out the requester: called from inside a
    /// provider of `T` (through a captured container), it includes that provider and reports
    /// [`DiError::Cycle`]. Inside a provider, use the `Resolver` it was given.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::ContainerBuilder;
    ///
    /// trait Plugin: Send + Sync { fn name(&self) -> &'static str; }
    /// struct Gzip;
    /// impl Plugin for Gzip { fn name(&self) -> &'static str { "gzip" } }
    /// struct Cors;
    /// impl Plugin for Cors { fn name(&self) -> &'static str { "cors" } }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Gzip).provides(|p: Arc<Gzip>| -> Arc<dyn Plugin> { p })
    ///     .instance(Cors).provides(|p: Arc<Cors>| -> Arc<dyn Plugin> { p })
    ///     .build()?;
    /// let names: Vec<_> = c.get_all::<dyn Plugin>()?.iter().map(|p| p.name()).collect();
    /// assert_eq!(names, ["gzip", "cors"]);
    /// assert!(c.get_all::<u8>()?.is_empty());
    /// # Ok::<(), oxbloom_di::DiError>(())
    /// ```
    pub fn get_all<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Vec<Arc<T>>, DiError> {
        self.open()?;
        self.beans.all::<T>()
    }

    /// Runs the `pre_destroy` hooks of every created bean, last created first, and closes the
    /// container.
    ///
    /// A bean is destroyed after every bean that depends on it, so its dependencies are still
    /// intact in its hook. A panicking hook is caught and recorded as a [`DestroyFailure`]
    /// with fixed text; the remaining hooks still run. Afterwards every `get*` returns
    /// [`DiError::ShutDown`]; `Arc` handles already handed out stay valid. A second call (also
    /// from another thread, which waits for the first, or from a `pre_destroy` hook) returns an
    /// empty report.
    ///
    /// Shutdown takes the lock that serializes lazy creation: a lazy bean being created on
    /// another thread is finished first and then destroyed with the others, at the position
    /// where it was created. A lazy bean that was never created is not destroyed.
    ///
    /// Dropping a container does not shut it down: call this explicitly.
    ///
    /// Called from inside a provider or a `post_construct` hook, on the thread that is creating
    /// a lazy bean, it closes the container at once: the rest of that chain creates nothing
    /// (its requests are `ShutDown`), the bean being created is not handed out, and a bean whose
    /// own `post_construct` called `shutdown` has its `pre_destroy` hooks run right after.
    ///
    /// [`DestroyFailure`]: crate::DestroyFailure
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use oxbloom_di::{ContainerBuilder, DiError, Resolver};
    ///
    /// struct Db;
    /// struct Repo { _db: Arc<Db> }
    ///
    /// let log = Arc::new(Mutex::new(Vec::new()));
    /// let (l1, l2) = (log.clone(), log.clone());
    /// let c = ContainerBuilder::new()
    ///     .instance(Db).pre_destroy(move |_: &Db| l1.lock().unwrap().push("db closed"))
    ///     .factory(|r: &Resolver| Ok(Repo { _db: r.get::<Db>()? }))
    ///     .pre_destroy(move |_: &Repo| l2.lock().unwrap().push("repo flushed"))
    ///     .build()?;
    /// let repo = c.get::<Repo>()?;
    ///
    /// let report = c.shutdown();
    /// assert_eq!(report.destroyed, 2);
    /// assert_eq!(*log.lock().unwrap(), ["repo flushed", "db closed"]);
    /// assert!(matches!(c.get::<Repo>(), Err(DiError::ShutDown)));
    /// # drop(repo); // the handle stays usable after shutdown
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn shutdown(&self) -> ShutdownReport {
        self.beans.shutdown()
    }

    /// `ShutDown` once [`Container::shutdown`] has started.
    fn open(&self) -> Result<(), DiError> {
        if self.beans.is_closed() {
            return Err(DiError::ShutDown);
        }
        Ok(())
    }
}
