//! Conditions: the [`Environment`] that supplies properties, the [`ConditionContext`] a
//! condition inspects, its [`ConditionOutcome`], and the [`ConditionReport`] of one build.
//!
//! A condition belongs to a registration and is evaluated once by
//! [`ContainerBuilder::build`](crate::ContainerBuilder::build), after profile filtering and
//! before any provider runs, against the registrations kept before it.

use std::any::{TypeId, type_name};
use std::collections::HashSet;
use std::fmt;

/// A source of configuration properties for conditions such as
/// [`ContainerBuilder::on_property`](crate::ContainerBuilder::on_property).
///
/// `property` returns the text of the scalar value at `key`, or `None` when the key is absent
/// or does not hold a scalar. Register one with
/// [`ContainerBuilder::environment`](crate::ContainerBuilder::environment); without one,
/// every property is absent.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use oxbloom_di::{ContainerBuilder, DiError, Environment};
///
/// struct Props(HashMap<&'static str, &'static str>);
/// impl Environment for Props {
///     fn property(&self, key: &str) -> Option<String> {
///         self.0.get(key).map(|v| (*v).to_owned())
///     }
/// }
///
/// struct Metrics;
/// let env = Props(HashMap::from([("metrics.enabled", "true")]));
/// let c = ContainerBuilder::new()
///     .environment(env)
///     .instance(Metrics)
///     .on_property_value("metrics.enabled", "true")
///     .build()?;
/// assert!(c.get_opt::<Metrics>()?.is_some());
///
/// // Without an environment the property is absent and `Metrics` is skipped.
/// let c = ContainerBuilder::new()
///     .instance(Metrics)
///     .on_property_value("metrics.enabled", "true")
///     .build()?;
/// assert!(c.get_opt::<Metrics>()?.is_none());
/// # Ok::<(), DiError>(())
/// ```
pub trait Environment: Send + Sync {
    /// The scalar text at `key`, or `None` when it is absent or not a scalar.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::Environment;
    ///
    /// struct Fixed;
    /// impl Environment for Fixed {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "server.port").then(|| "8080".to_owned())
    ///     }
    /// }
    ///
    /// assert_eq!(Fixed.property("server.port").as_deref(), Some("8080"));
    /// assert_eq!(Fixed.property("server.host"), None);
    /// ```
    fn property(&self, key: &str) -> Option<String>;

    /// Whether `key` is present at all: a scalar, or a value without scalar text such as a
    /// section or a list. [`ContainerBuilder::on_property`](crate::ContainerBuilder::on_property)
    /// asks this, so it also matches a section. The default is `property(key).is_some()`;
    /// override it when the source has sections.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::Environment;
    ///
    /// // `db` is a section: it holds `db.url` but has no text of its own.
    /// struct Fixed;
    /// impl Environment for Fixed {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "db.url").then(|| "mem".to_owned())
    ///     }
    ///     fn contains(&self, key: &str) -> bool {
    ///         key == "db" || self.property(key).is_some()
    ///     }
    /// }
    ///
    /// assert!(Fixed.contains("db"));
    /// assert_eq!(Fixed.property("db"), None);
    /// assert!(Fixed.contains("db.url"));
    /// assert!(!Fixed.contains("cache"));
    /// ```
    fn contains(&self, key: &str) -> bool {
        self.property(key).is_some()
    }
}

/// What a condition sees while the build decides one registration: the registrations kept
/// before it, the properties of the [`Environment`], the active profiles and its own label.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError};
///
/// struct Cache;
/// struct Stats;
///
/// let c = ContainerBuilder::new()
///     .active_profiles(["dev"])
///     .instance(Cache)
///     .instance(Stats)
///     .condition("dev_with_cache", |cx| {
///         if cx.has_bean::<Cache>() && cx.active_profiles().iter().any(|p| p == "dev") {
///             ConditionOutcome::matched()
///         } else {
///             ConditionOutcome::skipped("needs a cache in the dev profile")
///         }
///     })
///     .build()?;
/// assert!(c.get_opt::<Stats>()?.is_some());
/// # Ok::<(), DiError>(())
/// ```
pub struct ConditionContext<'a> {
    kept: &'a HashSet<TypeId>,
    environment: Option<&'a dyn Environment>,
    active: &'a [String],
    registration: &'a str,
}

impl<'a> ConditionContext<'a> {
    pub(crate) fn new(
        kept: &'a HashSet<TypeId>,
        environment: Option<&'a dyn Environment>,
        active: &'a [String],
        registration: &'a str,
    ) -> Self {
        ConditionContext {
            kept,
            environment,
            active,
            registration,
        }
    }

    /// Whether a registration kept before this one provides `T`, as its concrete type or
    /// through `provides`. Registrations after this one, and ones skipped by a profile or a
    /// condition, do not count.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError};
    ///
    /// struct Pool;
    /// struct Migrations;
    ///
    /// let needs_pool = |cx: &oxbloom_di::ConditionContext| {
    ///     if cx.has_bean::<Pool>() {
    ///         ConditionOutcome::matched()
    ///     } else {
    ///         ConditionOutcome::skipped("no pool")
    ///     }
    /// };
    /// let c = ContainerBuilder::new()
    ///     .instance(Pool)
    ///     .instance(Migrations).condition("needs_pool", needs_pool)
    ///     .build()?;
    /// assert!(c.get_opt::<Migrations>()?.is_some());
    ///
    /// // `Pool` registered later does not count.
    /// let c = ContainerBuilder::new()
    ///     .instance(Migrations).condition("needs_pool", needs_pool)
    ///     .instance(Pool)
    ///     .build()?;
    /// assert!(c.get_opt::<Migrations>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn has_bean<T: ?Sized + 'static>(&self) -> bool {
        self.kept.contains(&TypeId::of::<T>())
    }

    /// The property `key` of the builder's [`Environment`]; `None` when it is absent or no
    /// environment was set.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError, Environment};
    ///
    /// struct Props;
    /// impl Environment for Props {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "pool.size").then(|| "8".to_owned())
    ///     }
    /// }
    ///
    /// struct Pool;
    /// let c = ContainerBuilder::new()
    ///     .environment(Props)
    ///     .instance(Pool)
    ///     .condition("pool_size_positive", |cx| {
    ///         match cx.property("pool.size").and_then(|v| v.parse::<u32>().ok()) {
    ///             Some(n) if n > 0 => ConditionOutcome::matched(),
    ///             _ => ConditionOutcome::skipped("property \"pool.size\" is not a positive number"),
    ///         }
    ///     })
    ///     .build()?;
    /// assert!(c.get_opt::<Pool>()?.is_some());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn property(&self, key: &str) -> Option<String> {
        self.environment.and_then(|e| e.property(key))
    }

    /// Whether `key` is present in the environment ([`Environment::contains`]), a section or a
    /// list included; `false` without an environment. This is the check of `on_property`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError, Environment};
    ///
    /// // `db` is a section: it holds `db.url` but has no text of its own.
    /// struct Props;
    /// impl Environment for Props {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "db.url").then(|| "mem".to_owned())
    ///     }
    ///     fn contains(&self, key: &str) -> bool {
    ///         key == "db" || self.property(key).is_some()
    ///     }
    /// }
    ///
    /// struct Pool;
    /// let c = ContainerBuilder::new()
    ///     .environment(Props)
    ///     .instance(Pool)
    ///     .condition("has_db_section", |cx| {
    ///         if cx.contains("db") && cx.property("db").is_none() {
    ///             ConditionOutcome::matched()
    ///         } else {
    ///             ConditionOutcome::skipped("no `db` section")
    ///         }
    ///     })
    ///     .build()?;
    /// assert!(c.get_opt::<Pool>()?.is_some());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn contains(&self, key: &str) -> bool {
        self.environment.is_some_and(|e| e.contains(key))
    }

    /// The active profile names set by
    /// [`ContainerBuilder::active_profiles`](crate::ContainerBuilder::active_profiles).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError};
    ///
    /// struct DevTools;
    /// let only_dev = |cx: &oxbloom_di::ConditionContext| {
    ///     if cx.active_profiles() == ["dev"] {
    ///         ConditionOutcome::matched()
    ///     } else {
    ///         ConditionOutcome::skipped("only dev is active")
    ///     }
    /// };
    /// let c = ContainerBuilder::new()
    ///     .active_profiles(["dev"])
    ///     .instance(DevTools).condition("only_dev", only_dev)
    ///     .build()?;
    /// assert!(c.get_opt::<DevTools>()?.is_some());
    ///
    /// let c = ContainerBuilder::new().instance(DevTools).condition("only_dev", only_dev).build()?;
    /// assert!(c.get_opt::<DevTools>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn active_profiles(&self) -> &[String] {
        self.active
    }

    /// The label of the registration being decided, e.g. `instance app::Cache`; the same text
    /// the [`ConditionReport`] shows.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError};
    ///
    /// struct Cache;
    /// let seen = Arc::new(Mutex::new(String::new()));
    /// let slot = Arc::clone(&seen);
    /// ContainerBuilder::new()
    ///     .instance(Cache)
    ///     .condition("log", move |cx| {
    ///         *slot.lock().unwrap() = cx.registration().to_owned();
    ///         ConditionOutcome::matched()
    ///     })
    ///     .build()?;
    /// assert!(seen.lock().unwrap().starts_with("instance "));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn registration(&self) -> &str {
        self.registration
    }
}

/// The result of one condition: matched, or skipped with a reason.
///
/// The reason appears in the [`ConditionReport`]. Build it from fixed text, type names and
/// key names only: never put configuration values or secrets into it.
///
/// # Examples
///
/// ```
/// use oxbloom_di::ConditionOutcome;
///
/// let yes = ConditionOutcome::matched();
/// assert!(yes.is_matched());
/// assert_eq!(yes.reason(), "matched");
///
/// let no = ConditionOutcome::skipped("the cache is disabled");
/// assert!(!no.is_matched());
/// assert_eq!(no.reason(), "the cache is disabled");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionOutcome {
    matched: bool,
    reason: String,
}

impl ConditionOutcome {
    /// A matching outcome; its reason is `matched`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError};
    ///
    /// struct Always;
    /// let c = ContainerBuilder::new()
    ///     .instance(Always)
    ///     .condition("always", |_| ConditionOutcome::matched())
    ///     .build()?;
    /// assert!(c.get_opt::<Always>()?.is_some());
    /// assert_eq!(c.condition_report().records()[0].reason(), "matched");
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn matched() -> Self {
        Self::with(true, "matched")
    }

    /// A non-matching outcome with `reason`; the registration is skipped.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError};
    ///
    /// struct Never;
    /// let c = ContainerBuilder::new()
    ///     .instance(Never)
    ///     .condition("never", |_| ConditionOutcome::skipped("disabled by design"))
    ///     .build()?;
    /// assert!(c.get_opt::<Never>()?.is_none());
    /// assert_eq!(c.condition_report().records()[0].reason(), "disabled by design");
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self::with(false, reason)
    }

    /// Whether the condition matched.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ConditionOutcome;
    ///
    /// assert!(ConditionOutcome::matched().is_matched());
    /// assert!(!ConditionOutcome::skipped("no").is_matched());
    /// ```
    pub fn is_matched(&self) -> bool {
        self.matched
    }

    /// The reason recorded in the report.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ConditionOutcome;
    ///
    /// assert_eq!(ConditionOutcome::skipped("no cache").reason(), "no cache");
    /// ```
    pub fn reason(&self) -> &str {
        &self.reason
    }

    fn with(matched: bool, reason: impl Into<String>) -> Self {
        ConditionOutcome {
            matched,
            reason: reason.into(),
        }
    }
}

/// The check of a [`Condition`].
pub(crate) type Check = Box<dyn Fn(&ConditionContext<'_>) -> ConditionOutcome + Send + Sync>;

/// One named condition of a registration or of a group.
pub(crate) struct Condition {
    /// Shown in the report, e.g. `on_bean` or the name passed to `condition`.
    pub(crate) name: &'static str,
    pub(crate) check: Check,
}

impl Condition {
    /// A condition named `name` that runs `check`.
    pub(crate) fn new(
        name: &'static str,
        check: impl Fn(&ConditionContext<'_>) -> ConditionOutcome + Send + Sync + 'static,
    ) -> Self {
        Condition {
            name,
            check: Box::new(check),
        }
    }

    /// Matches when a registration kept before provides `T`.
    pub(crate) fn on_bean<T: ?Sized + 'static>() -> Self {
        Self::new("on_bean", |cx| {
            if cx.has_bean::<T>() {
                ConditionOutcome::matched_because(format!(
                    "an earlier registration provides `{}`",
                    type_name::<T>()
                ))
            } else {
                ConditionOutcome::skipped(format!(
                    "no earlier registration provides `{}`",
                    type_name::<T>()
                ))
            }
        })
    }

    /// Matches when no registration kept before provides `T`.
    pub(crate) fn on_missing_bean<T: ?Sized + 'static>() -> Self {
        Self::new("on_missing_bean", |cx| {
            if cx.has_bean::<T>() {
                ConditionOutcome::skipped(format!(
                    "an earlier registration already provides `{}`",
                    type_name::<T>()
                ))
            } else {
                ConditionOutcome::matched_because(format!(
                    "no earlier registration provides `{}`",
                    type_name::<T>()
                ))
            }
        })
    }

    /// Matches when the property `key` is present ([`Environment::contains`]).
    pub(crate) fn on_property(key: &str) -> Self {
        let key = key.to_owned();
        Self::new("on_property", move |cx| {
            if cx.contains(&key) {
                ConditionOutcome::matched_because(format!("property \"{key}\" is set"))
            } else {
                ConditionOutcome::skipped(format!("property \"{key}\" is not set"))
            }
        })
    }

    /// Matches when the property `key` equals `value` exactly; an absent key matches only when
    /// `or_missing`. The reasons name the key, never a value.
    pub(crate) fn on_property_value(key: &str, value: &str, or_missing: bool) -> Self {
        let (key, value) = (key.to_owned(), value.to_owned());
        let name = if or_missing {
            "on_property_value_or_missing"
        } else {
            "on_property_value"
        };
        Self::new(name, move |cx| match cx.property(&key) {
            Some(actual) if actual == value => ConditionOutcome::matched_because(format!(
                "property \"{key}\" has the expected value"
            )),
            Some(_) => ConditionOutcome::skipped(format!(
                "property \"{key}\" does not have the expected value"
            )),
            None if or_missing => {
                ConditionOutcome::matched_because(format!("property \"{key}\" is not set"))
            }
            None => ConditionOutcome::skipped(format!("property \"{key}\" is not set")),
        })
    }
}

impl ConditionOutcome {
    /// A matching outcome with a fixed explanatory reason (built-in conditions only).
    fn matched_because(reason: String) -> Self {
        Self::with(true, reason)
    }
}

/// The conditions of a group of registrations, for
/// [`ContainerBuilder::group`](crate::ContainerBuilder::group): the same conditions as the
/// builder's modifiers ([`condition`](crate::ContainerBuilder::condition),
/// [`on_bean`](crate::ContainerBuilder::on_bean), and so on), collected in a value.
///
/// Several conditions are combined with AND and run in the order they were added; evaluation
/// stops at the first one that does not match. `Conditions::new()` (or `default()`) holds none
/// and always matches.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{Conditions, ContainerBuilder, DiError};
///
/// struct Pool;
/// struct Migrations;
/// struct HealthCheck;
///
/// let needs_pool = || Conditions::new().on_bean::<Pool>();
///
/// let c = ContainerBuilder::new()
///     .instance(Pool)
///     .group("database", needs_pool(), |b| b.instance(Migrations).instance(HealthCheck))
///     .build()?;
/// assert!(c.get_opt::<Migrations>()?.is_some());
/// assert!(c.get_opt::<HealthCheck>()?.is_some());
///
/// // Without a pool, the whole group is skipped.
/// let c = ContainerBuilder::new()
///     .group("database", needs_pool(), |b| b.instance(Migrations).instance(HealthCheck))
///     .build()?;
/// assert!(c.get_opt::<Migrations>()?.is_none());
/// assert!(c.get_opt::<HealthCheck>()?.is_none());
/// # Ok::<(), DiError>(())
/// ```
#[derive(Default)]
pub struct Conditions {
    pub(crate) list: Vec<Condition>,
    /// The first misuse (an empty property key); the group that receives these conditions
    /// records it, so its build returns `Invalid`.
    pub(crate) invalid: Option<String>,
}

impl Conditions {
    /// No conditions: a group with them is always kept (unless an enclosing group is skipped).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{Conditions, ContainerBuilder, DiError};
    ///
    /// struct Clock;
    /// let c = ContainerBuilder::new()
    ///     .group("clock", Conditions::new(), |b| b.instance(Clock))
    ///     .build()?;
    /// assert!(c.get_opt::<Clock>()?.is_some());
    /// assert!(c.condition_report().records().is_empty());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    fn with(mut self, condition: Condition) -> Self {
        self.list.push(condition);
        self
    }

    /// Adds the property condition `make` builds, or records the misuse of `modifier` when
    /// `key` is empty.
    fn with_property(
        mut self,
        modifier: &str,
        key: &str,
        make: impl FnOnce() -> Condition,
    ) -> Self {
        if key.is_empty() {
            self.invalid
                .get_or_insert_with(|| format!("`{modifier}` needs a non-empty key"));
            self
        } else {
            self.with(make())
        }
    }

    /// Adds a custom condition named `name`, like
    /// [`ContainerBuilder::condition`](crate::ContainerBuilder::condition); its
    /// [`ConditionContext::registration`] is the group's name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ConditionOutcome, Conditions, ContainerBuilder, DiError};
    ///
    /// struct Tracing;
    /// let only_dev = Conditions::new().condition("only_dev", |cx| {
    ///     if cx.active_profiles().iter().any(|p| p == "dev") {
    ///         ConditionOutcome::matched()
    ///     } else {
    ///         ConditionOutcome::skipped("the dev profile is not active")
    ///     }
    /// });
    /// let c = ContainerBuilder::new()
    ///     .group("tracing", only_dev, |b| b.instance(Tracing))
    ///     .build()?;
    /// assert!(c.get_opt::<Tracing>()?.is_none());
    /// let record = &c.condition_report().records()[0];
    /// assert_eq!((record.registration(), record.condition()), ("tracing", "only_dev"));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn condition(
        self,
        name: &'static str,
        f: impl Fn(&ConditionContext) -> ConditionOutcome + Send + Sync + 'static,
    ) -> Self {
        self.with(Condition::new(name, f))
    }

    /// Matches when a registration kept before the group provides `T`, like
    /// [`ContainerBuilder::on_bean`](crate::ContainerBuilder::on_bean). The group's own
    /// registrations do not count.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{Conditions, ContainerBuilder, DiError};
    ///
    /// struct Pool;
    /// struct Migrations;
    /// let c = ContainerBuilder::new()
    ///     .instance(Pool)
    ///     .group("migrations", Conditions::new().on_bean::<Pool>(), |b| b.instance(Migrations))
    ///     .build()?;
    /// assert!(c.get_opt::<Migrations>()?.is_some());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_bean<T: ?Sized + 'static>(self) -> Self {
        self.with(Condition::on_bean::<T>())
    }

    /// Matches when no registration kept before the group provides `T`, like
    /// [`ContainerBuilder::on_missing_bean`](crate::ContainerBuilder::on_missing_bean): the
    /// back-off rule for a group of defaults. The group's own registrations do not count, so a
    /// group may register the `T` its condition asks about.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{Conditions, ContainerBuilder, DiError, Resolver};
    ///
    /// struct Pool(&'static str);
    /// struct Stats { pool: Arc<Pool> }
    ///
    /// let defaults = |b: ContainerBuilder| {
    ///     b.group("pool defaults", Conditions::new().on_missing_bean::<Pool>(), |b| {
    ///         b.instance(Pool("default"))
    ///             .factory(|r: &Resolver| Ok(Stats { pool: r.get::<Pool>()? }))
    ///     })
    /// };
    /// let c = defaults(ContainerBuilder::new()).build()?;
    /// assert_eq!(c.get::<Stats>()?.pool.0, "default");
    ///
    /// // The application's pool comes first: the whole group backs off.
    /// let c = defaults(ContainerBuilder::new().instance(Pool("app"))).build()?;
    /// assert!(c.get_opt::<Stats>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_missing_bean<T: ?Sized + 'static>(self) -> Self {
        self.with(Condition::on_missing_bean::<T>())
    }

    /// Matches when the property `key` is present, like
    /// [`ContainerBuilder::on_property`](crate::ContainerBuilder::on_property), whatever its
    /// value (a section or a list counts when [`Environment::contains`] says so): also when it
    /// is `false`, unlike Spring's `@ConditionalOnProperty`. For a switch use
    /// [`Conditions::on_property_value`] with `"true"`. An empty `key` makes the build that
    /// receives these conditions return `Invalid`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{Conditions, ContainerBuilder, DiError};
    ///
    /// struct Mailer;
    /// let c = ContainerBuilder::new()
    ///     .group("mail", Conditions::new().on_property("smtp.host"), |b| b.instance(Mailer))
    ///     .build()?;
    /// assert!(c.get_opt::<Mailer>()?.is_none()); // no environment: absent
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_property(self, key: &str) -> Self {
        self.with_property("on_property", key, || Condition::on_property(key))
    }

    /// Matches when the property `key` equals `value` exactly and case-sensitively, like
    /// [`ContainerBuilder::on_property_value`](crate::ContainerBuilder::on_property_value). Never
    /// pass a secret as `value`: it sits in the source code. An empty `key` makes the build that
    /// receives these conditions return `Invalid`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{Conditions, ContainerBuilder, DiError, Environment};
    ///
    /// struct Props;
    /// impl Environment for Props {
    ///     fn property(&self, key: &str) -> Option<String> {
    ///         (key == "cache.kind").then(|| "redis".to_owned())
    ///     }
    /// }
    /// struct RedisCache;
    /// let redis = Conditions::new().on_property_value("cache.kind", "redis");
    /// let c = ContainerBuilder::new()
    ///     .environment(Props)
    ///     .group("redis", redis, |b| b.instance(RedisCache))
    ///     .build()?;
    /// assert!(c.get_opt::<RedisCache>()?.is_some());
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_property_value(self, key: &str, value: &str) -> Self {
        self.with_property("on_property_value", key, || {
            Condition::on_property_value(key, value, false)
        })
    }

    /// Like [`Conditions::on_property_value`], but also matches when `key` is absent, like
    /// [`ContainerBuilder::on_property_value_or_missing`](crate::ContainerBuilder::on_property_value_or_missing).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{Conditions, ContainerBuilder, DiError};
    ///
    /// struct MemoryCache;
    /// let memory = Conditions::new().on_property_value_or_missing("cache.kind", "memory");
    /// let c = ContainerBuilder::new()
    ///     .group("memory cache", memory, |b| b.instance(MemoryCache))
    ///     .build()?;
    /// assert!(c.get_opt::<MemoryCache>()?.is_some()); // absent: the default applies
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn on_property_value_or_missing(self, key: &str, value: &str) -> Self {
        self.with_property("on_property_value_or_missing", key, || {
            Condition::on_property_value(key, value, true)
        })
    }
}

/// One group of [`ContainerBuilder::group`](crate::ContainerBuilder::group): its name, its
/// conditions and the group it is nested in.
pub(crate) struct Group {
    name: &'static str,
    parent: Option<u32>,
    conditions: Vec<Condition>,
}

/// The group tree of a builder, the stack of groups whose `f` is running and the modifier
/// barrier of the innermost one.
#[derive(Default)]
pub(crate) struct Groups {
    tree: Vec<Group>,
    /// Open groups, innermost last, each with the floor of the group around it.
    open: Vec<(u32, usize)>,
    /// The number of registrations when the innermost open group started: a modifier finds no
    /// registration at or below it, so it never reaches across the start of a group.
    floor: usize,
}

impl Groups {
    /// The innermost open group: the group of a registration made now.
    pub(crate) fn current(&self) -> Option<u32> {
        self.open.last().map(|&(id, _)| id)
    }

    /// The modifier barrier: registrations at an index below it belong outside the innermost
    /// open group.
    pub(crate) fn floor(&self) -> usize {
        self.floor
    }

    /// Opens a new group nested in the current one, with `registrations` made so far, and
    /// returns its id.
    pub(crate) fn open(
        &mut self,
        name: &'static str,
        conditions: Conditions,
        registrations: usize,
    ) -> u32 {
        let id = u32::try_from(self.tree.len()).expect("fewer than 2^32 groups");
        self.tree.push(Group {
            name,
            parent: self.current(),
            conditions: conditions.list,
        });
        self.open.push((id, self.floor));
        self.floor = registrations;
        id
    }

    /// Closes the group `id` when it is the innermost open group of this builder and restores
    /// the outer floor; a group with no registration (`registrations` equals its floor) is
    /// dropped with the groups nested in it, so an empty group leaves nothing behind. A
    /// builder whose innermost open group is not `id` (a replacement builder returned by the
    /// group's `f`) is left unchanged.
    pub(crate) fn close(&mut self, id: u32, registrations: usize) {
        if self.current() != Some(id) {
            return;
        }
        let empty = registrations == self.floor;
        if let Some((_, outer)) = self.open.pop() {
            self.floor = outer;
        }
        if empty {
            self.tree.truncate(id as usize);
        }
    }
}

/// The group decisions of one condition walk: each group is decided at most once, enclosing
/// groups first, against the registrations kept before the visit that decides it.
pub(crate) struct GroupWalk {
    /// `Some(matched)` once a group is decided.
    decided: Vec<Option<bool>>,
    /// Scratch: the undecided groups from a registration's group up to the first decided one.
    path: Vec<u32>,
}

impl GroupWalk {
    pub(crate) fn new(groups: &Groups) -> Self {
        GroupWalk {
            decided: vec![None; groups.tree.len()],
            path: Vec::new(),
        }
    }

    /// Whether every group from `group` outwards matches, deciding the undecided ones from the
    /// outermost inwards. A group that does not match decides the groups nested in it on the
    /// path as not matching, without evaluating them. Each group is pushed onto the path and
    /// evaluated at most once per walk, so the total cost is linear.
    #[inline(never)]
    pub(crate) fn admits(
        &mut self,
        groups: &mut Groups,
        group: u32,
        cx: (&HashSet<TypeId>, Option<&dyn Environment>, &[String]),
        records: &mut Vec<ConditionRecord>,
    ) -> bool {
        let (kept, environment, active) = cx;
        self.path.clear();
        let mut at = Some(group);
        let mut matched = true;
        while let Some(g) = at {
            match self.decided[g as usize] {
                Some(m) => {
                    matched = m;
                    break;
                }
                None => {
                    self.path.push(g);
                    at = groups.tree[g as usize].parent;
                }
            }
        }
        while let Some(g) = self.path.pop() {
            let node = &mut groups.tree[g as usize];
            if matched {
                let conditions = std::mem::take(&mut node.conditions);
                let cx = ConditionContext::new(kept, environment, active, node.name);
                matched = evaluate(&conditions, &cx, records);
            }
            self.decided[g as usize] = Some(matched);
        }
        matched
    }
}

/// Evaluates `conditions` in order for the registration `cx.registration()`, appending one
/// record per evaluated condition to `records`. Stops at the first condition that does not
/// match, so later ones are neither run nor recorded. Returns whether all matched.
///
/// A panicking condition is not caught.
pub(crate) fn evaluate(
    conditions: &[Condition],
    cx: &ConditionContext<'_>,
    records: &mut Vec<ConditionRecord>,
) -> bool {
    for condition in conditions {
        let outcome = (condition.check)(cx);
        let matched = outcome.matched;
        records.push(ConditionRecord {
            registration: cx.registration.to_owned(),
            condition: condition.name,
            matched,
            reason: outcome.reason,
        });
        if !matched {
            return false;
        }
    }
    true
}

/// One evaluated condition: the registration it belongs to, the condition's name, whether it
/// matched, and why.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{ContainerBuilder, DiError};
///
/// struct Server;
/// struct Fallback;
///
/// let c = ContainerBuilder::new()
///     .instance(Server)
///     .instance(Fallback).on_bean::<Server>()
///     .build()?;
/// let record = &c.condition_report().records()[0];
/// assert!(record.registration().ends_with("Fallback"));
/// assert_eq!(record.condition(), "on_bean");
/// assert!(record.matched());
/// assert!(record.reason().starts_with("an earlier registration provides"));
/// # Ok::<(), DiError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConditionRecord {
    registration: String,
    condition: &'static str,
    matched: bool,
    reason: String,
}

impl ConditionRecord {
    /// The label of the registration, e.g. `instance app::Server` or `closure at src/main.rs:12`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Cache;
    /// let c = ContainerBuilder::new().instance(Cache).on_missing_bean::<Cache>().build()?;
    /// assert!(c.condition_report().records()[0].registration().starts_with("instance "));
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn registration(&self) -> &str {
        &self.registration
    }

    /// The condition's name: `on_bean`, `on_missing_bean`, `on_property`,
    /// `on_property_value`, `on_property_value_or_missing`, or the name given to
    /// [`ContainerBuilder::condition`](crate::ContainerBuilder::condition).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Cache;
    /// let c = ContainerBuilder::new().instance(Cache).on_property("cache.size").build()?;
    /// assert_eq!(c.condition_report().records()[0].condition(), "on_property");
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn condition(&self) -> &str {
        self.condition
    }

    /// Whether the condition matched.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Cache;
    /// let c = ContainerBuilder::new().instance(Cache).on_property("cache.size").build()?;
    /// assert!(!c.condition_report().records()[0].matched()); // no environment
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn matched(&self) -> bool {
        self.matched
    }

    /// Why the condition matched or not; fixed text, type names and key names only.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Cache;
    /// let c = ContainerBuilder::new().instance(Cache).on_property("cache.size").build()?;
    /// assert_eq!(
    ///     c.condition_report().records()[0].reason(),
    ///     "property \"cache.size\" is not set"
    /// );
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// Every condition one build evaluated, in evaluation order.
///
/// `Display` renders the report as a block with a `Matched:` and a `Skipped:` section, one
/// line `  <registration>: <condition> - <reason>` per record and `  (none)` for an empty
/// section, without a trailing newline. Registrations skipped by a profile have no record.
///
/// # Examples
///
/// ```
/// use oxbloom_di::{ContainerBuilder, DiError};
///
/// struct Metrics;
///
/// let (built, report) = ContainerBuilder::new()
///     .instance(Metrics).on_property("metrics.enabled")
///     .build_with_report();
/// assert!(built.is_ok());
/// let label = format!("instance {}", std::any::type_name::<Metrics>());
/// assert_eq!(
///     report.to_string(),
///     format!(
///         "CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n  \
///          {label}: on_property - property \"metrics.enabled\" is not set"
///     )
/// );
/// # Ok::<(), DiError>(())
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConditionReport {
    records: Vec<ConditionRecord>,
}

impl ConditionReport {
    pub(crate) fn new(records: Vec<ConditionRecord>) -> Self {
        ConditionReport { records }
    }

    /// The records in evaluation order: registration order, and within a registration the
    /// order of its conditions up to the first one that did not match.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct A;
    /// struct B;
    /// let c = ContainerBuilder::new()
    ///     .instance(A).on_missing_bean::<B>()
    ///     .instance(B).on_bean::<A>()
    ///     .build()?;
    /// let names: Vec<_> = c.condition_report().records().iter().map(|r| r.condition()).collect();
    /// assert_eq!(names, ["on_missing_bean", "on_bean"]);
    /// # Ok::<(), DiError>(())
    /// ```
    pub fn records(&self) -> &[ConditionRecord] {
        &self.records
    }
}

impl fmt::Display for ConditionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CONDITIONS EVALUATION REPORT\n\nMatched:")?;
        self.section(f, true)?;
        f.write_str("\n\nSkipped:")?;
        self.section(f, false)
    }
}

impl ConditionReport {
    fn section(&self, f: &mut fmt::Formatter<'_>, matched: bool) -> fmt::Result {
        let mut any = false;
        for r in self.records.iter().filter(|r| r.matched == matched) {
            any = true;
            write!(f, "\n  {}: {} - {}", r.registration, r.condition, r.reason)?;
        }
        if !any {
            f.write_str("\n  (none)")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        registration: &str,
        condition: &'static str,
        matched: bool,
        reason: &str,
    ) -> ConditionRecord {
        ConditionRecord {
            registration: registration.to_owned(),
            condition,
            matched,
            reason: reason.to_owned(),
        }
    }

    #[test]
    fn display_format_is_exact() {
        let report = ConditionReport::new(vec![
            record(
                "instance app::Server",
                "on_missing_bean",
                true,
                "no earlier registration provides `app::Server`",
            ),
            record(
                "closure at src/main.rs:9",
                "on_property",
                false,
                "property \"cache.kind\" is not set",
            ),
            record("instance app::Cache", "custom", true, "matched"),
        ]);
        assert_eq!(
            report.to_string(),
            "CONDITIONS EVALUATION REPORT\n\
             \n\
             Matched:\n  \
             instance app::Server: on_missing_bean - no earlier registration provides `app::Server`\n  \
             instance app::Cache: custom - matched\n\
             \n\
             Skipped:\n  \
             closure at src/main.rs:9: on_property - property \"cache.kind\" is not set"
        );
    }

    #[test]
    fn empty_sections_print_none() {
        assert_eq!(
            ConditionReport::default().to_string(),
            "CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n  (none)"
        );
        let only_skipped =
            ConditionReport::new(vec![record("instance a::B", "on_bean", false, "no")]);
        assert_eq!(
            only_skipped.to_string(),
            "CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n  instance a::B: on_bean - no"
        );
    }
}
