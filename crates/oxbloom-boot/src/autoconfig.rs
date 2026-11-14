//! Auto-configuration: configurations an application adds as a whole, with class-level
//! conditions and an order among themselves.

use std::any::TypeId;
use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::fmt;

use oxbloom_di::{Component, Conditions, Configuration, ContainerBuilder};

use crate::metadata::PropertyMetadata;

/// A [`Configuration`] that an application adds as a unit: its registrations form one group
/// whose [`conditions`](AutoConfiguration::conditions) are decided once, and its place among the
/// other auto-configurations follows [`after`](AutoConfiguration::after) and
/// [`before`](AutoConfiguration::before).
///
/// The type is also a [`Component`] (usually a `#[component]` struct; a unit struct works):
/// the application registers it with `component::<A>()` as the first registration of the
/// group, so its `#[bean]` methods get `Arc<A>` and it is skipped together with the group.
/// A `#[component]` struct with fields gets them injected, and its bean methods can use them
/// through `&self`.
///
/// Implement it with [`macro@crate::auto_configuration`], or by hand on a type that implements
/// [`Configuration`] and [`Component`]. Every method has a default: no conditions and no
/// ordering constraints.
///
/// # Examples
///
/// ```
/// use oxbloom_boot::{AutoConfiguration, ConfigRef, Conditions, Configuration};
/// use oxbloom_di::{ContainerBuilder, component};
///
/// struct Limits { max: u32 }
///
/// #[component]
/// struct CoreDefaults;
/// impl Configuration for CoreDefaults {
///     fn register(builder: ContainerBuilder) -> ContainerBuilder { builder }
/// }
/// impl AutoConfiguration for CoreDefaults {}
///
/// #[component]
/// struct LimitsDefaults;
/// impl Configuration for LimitsDefaults {
///     fn register(builder: ContainerBuilder) -> ContainerBuilder {
///         builder.bean(|| Limits { max: 10 })
///     }
/// }
/// impl AutoConfiguration for LimitsDefaults {
///     // Back off when the application registered its own `Limits`.
///     fn conditions() -> Conditions {
///         Conditions::new().on_missing_bean::<Limits>()
///     }
///     fn after() -> Vec<ConfigRef> {
///         vec![ConfigRef::of::<CoreDefaults>()]
///     }
/// }
///
/// // The group the application appends: the instance first, then the registrations.
/// let c = ContainerBuilder::new()
///     .instance(Limits { max: 99 })
///     .group("LimitsDefaults", LimitsDefaults::conditions(), |b| {
///         LimitsDefaults::register(b.component::<LimitsDefaults>())
///     })
///     .build()?;
/// assert_eq!(c.get::<Limits>()?.max, 99);
/// assert!(c.get_opt::<LimitsDefaults>()?.is_none()); // skipped with its group
/// assert!(CoreDefaults::before().is_empty());
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
pub trait AutoConfiguration: Configuration + Component {
    /// The class-level conditions: when they do not match, none of the configuration's
    /// registrations is kept. The default is no condition.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfiguration, Conditions, Configuration};
    /// use oxbloom_di::{ContainerBuilder, DiError, component};
    ///
    /// struct Metrics;
    /// #[component]
    /// struct MetricsDefaults;
    /// impl Configuration for MetricsDefaults {
    ///     fn register(builder: ContainerBuilder) -> ContainerBuilder {
    ///         builder.bean(|| Metrics)
    ///     }
    /// }
    /// impl AutoConfiguration for MetricsDefaults {
    ///     fn conditions() -> Conditions {
    ///         Conditions::new().on_property("metrics.enabled")
    ///     }
    /// }
    ///
    /// // No environment, so the property is absent and the group is skipped.
    /// let c = ContainerBuilder::new()
    ///     .group("MetricsDefaults", MetricsDefaults::conditions(), |b| {
    ///         MetricsDefaults::register(b.component::<MetricsDefaults>())
    ///     })
    ///     .build()?;
    /// assert!(c.get_opt::<Metrics>()?.is_none());
    /// assert!(c.get_opt::<MetricsDefaults>()?.is_none());
    /// # Ok::<(), DiError>(())
    /// ```
    fn conditions() -> Conditions {
        Conditions::new()
    }

    /// The configurations processed before this one. A configuration that is not in the
    /// application's list is ignored. The default is none.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfiguration, ConfigRef, auto_configuration};
    /// use oxbloom_di::component;
    ///
    /// #[component]
    /// struct Db;
    /// #[auto_configuration]
    /// impl Db {}
    ///
    /// #[component]
    /// struct Repositories;
    /// #[auto_configuration(after(Db))]
    /// impl Repositories {}
    ///
    /// assert_eq!(Repositories::after(), [ConfigRef::of::<Db>()]);
    /// assert!(Db::after().is_empty());
    /// ```
    fn after() -> Vec<ConfigRef> {
        Vec::new()
    }

    /// The configurations processed after this one; `A::before() = [B]` means the same as
    /// `B::after() = [A]`. The default is none.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfiguration, ConfigRef, auto_configuration};
    /// use oxbloom_di::component;
    ///
    /// #[component]
    /// struct Web;
    /// #[auto_configuration]
    /// impl Web {}
    ///
    /// #[component]
    /// struct Json;
    /// #[auto_configuration(before(Web))]
    /// impl Json {}
    ///
    /// assert_eq!(Json::before(), [ConfigRef::of::<Web>()]);
    /// ```
    fn before() -> Vec<ConfigRef> {
        Vec::new()
    }

    /// The configuration keys this auto-configuration reads, for
    /// [`property_metadata`](crate::OxbloomApplicationBuilder::property_metadata). The default
    /// is none.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfiguration, Configuration, PropertyMetadata};
    /// use oxbloom_di::{ContainerBuilder, component};
    ///
    /// #[component]
    /// struct Mail;
    /// impl Configuration for Mail {
    ///     fn register(builder: ContainerBuilder) -> ContainerBuilder { builder }
    /// }
    /// impl AutoConfiguration for Mail {
    ///     fn properties() -> &'static [PropertyMetadata] {
    ///         static KEYS: [PropertyMetadata; 1] =
    ///             [PropertyMetadata::new("mail.host", "String", None, "The SMTP host.")];
    ///         &KEYS
    ///     }
    /// }
    ///
    /// assert_eq!(Mail::properties()[0].key, "mail.host");
    /// ```
    fn properties() -> &'static [PropertyMetadata] {
        &[]
    }
}

/// Identifies a configuration type in [`AutoConfiguration::after`] and
/// [`AutoConfiguration::before`]: its `TypeId` and type name. Two refs are equal exactly when
/// they name the same type.
///
/// # Examples
///
/// ```
/// use oxbloom_boot::ConfigRef;
///
/// struct DataSourceConfig;
/// struct CacheConfig;
///
/// let r = ConfigRef::of::<DataSourceConfig>();
/// assert_eq!(r, ConfigRef::of::<DataSourceConfig>());
/// assert_ne!(r, ConfigRef::of::<CacheConfig>());
/// assert!(r.name().ends_with("DataSourceConfig"));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConfigRef {
    id: TypeId,
    name: &'static str,
}

impl ConfigRef {
    /// The ref of `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::ConfigRef;
    ///
    /// struct WebConfig;
    /// assert_eq!(ConfigRef::of::<WebConfig>(), ConfigRef::of::<WebConfig>());
    /// ```
    pub fn of<T: 'static>() -> Self {
        ConfigRef {
            id: TypeId::of::<T>(),
            name: std::any::type_name::<T>(),
        }
    }

    /// The type name, as `std::any::type_name` gives it (for messages; not guaranteed to be
    /// unique or stable across compiler versions).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::ConfigRef;
    ///
    /// struct WebConfig;
    /// assert_eq!(ConfigRef::of::<WebConfig>().name(), std::any::type_name::<WebConfig>());
    /// ```
    pub fn name(&self) -> &'static str {
        self.name
    }
}

impl fmt::Debug for ConfigRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ConfigRef").field(&self.name).finish()
    }
}

/// What the application needs of one auto-configuration: its identity, its ordering
/// constraints and its two functions. `register` registers the configuration's instance
/// (`component::<A>()`) and then its registrations, so the application calls it inside the
/// group and the instance is skipped together with it.
#[derive(Clone, Debug)]
pub(crate) struct Descriptor {
    pub(crate) config: ConfigRef,
    pub(crate) after: Vec<ConfigRef>,
    pub(crate) before: Vec<ConfigRef>,
    pub(crate) conditions: fn() -> Conditions,
    pub(crate) register: fn(ContainerBuilder) -> ContainerBuilder,
    pub(crate) properties: fn() -> &'static [PropertyMetadata],
}

/// An ordered list of auto-configurations, the unit a crate that ships them exposes, e.g. as
/// `pub fn auto_configurations() -> AutoConfigurations`.
///
/// A configuration appears at most once: adding one that is already in the list (directly or
/// through [`extend`](AutoConfigurations::extend)) keeps the first entry and its position.
///
/// # Examples
///
/// ```
/// use oxbloom_boot::{AutoConfigurations, auto_configuration};
/// use oxbloom_di::component;
///
/// #[component]
/// pub struct JsonAutoConfig;
/// #[auto_configuration]
/// impl JsonAutoConfig {}
///
/// #[component]
/// pub struct WebAutoConfig;
/// #[auto_configuration(after(JsonAutoConfig))]
/// impl WebAutoConfig {}
///
/// /// What a starter crate exposes.
/// pub fn auto_configurations() -> AutoConfigurations {
///     AutoConfigurations::new()
///         .add::<JsonAutoConfig>()
///         .add::<WebAutoConfig>()
/// }
///
/// let list = auto_configurations().add::<JsonAutoConfig>(); // already present
/// assert_eq!(list.len(), 2);
/// ```
#[derive(Clone, Default, Debug)]
pub struct AutoConfigurations {
    descriptors: Vec<Descriptor>,
}

impl AutoConfigurations {
    /// An empty list.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::AutoConfigurations;
    ///
    /// assert!(AutoConfigurations::new().is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `A`, unless it is already in the list (then the list is unchanged).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfigurations, auto_configuration};
    /// use oxbloom_di::component;
    ///
    /// #[component]
    /// pub struct MailAutoConfig;
    /// #[auto_configuration(on_property("mail.host"))]
    /// impl MailAutoConfig {}
    ///
    /// let list = AutoConfigurations::new().add::<MailAutoConfig>();
    /// assert_eq!(list.len(), 1);
    /// ```
    pub fn add<A: AutoConfiguration>(mut self) -> Self {
        self.push(Descriptor {
            config: ConfigRef::of::<A>(),
            after: A::after(),
            before: A::before(),
            conditions: A::conditions,
            register: register_with_instance::<A>,
            properties: A::properties,
        });
        self
    }

    /// Appends the entries of `other` in its order, skipping those already in this list.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfigurations, auto_configuration};
    /// use oxbloom_di::component;
    ///
    /// #[component]
    /// pub struct A;
    /// #[auto_configuration]
    /// impl A {}
    /// #[component]
    /// pub struct B;
    /// #[auto_configuration]
    /// impl B {}
    ///
    /// let mut list = AutoConfigurations::new().add::<A>();
    /// list.extend(AutoConfigurations::new().add::<A>().add::<B>());
    /// assert_eq!(list.len(), 2);
    /// ```
    pub fn extend(&mut self, other: AutoConfigurations) {
        for descriptor in other.descriptors {
            self.push(descriptor);
        }
    }

    /// The number of configurations in the list.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfigurations, auto_configuration};
    /// use oxbloom_di::component;
    ///
    /// #[component]
    /// pub struct A;
    /// #[auto_configuration]
    /// impl A {}
    ///
    /// assert_eq!(AutoConfigurations::new().add::<A>().len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    /// Whether the list has no configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::AutoConfigurations;
    ///
    /// assert!(AutoConfigurations::default().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// The entries in list order, one per configuration.
    pub(crate) fn descriptors(&self) -> &[Descriptor] {
        &self.descriptors
    }

    /// Appends `descriptor` unless its configuration is already present (the first one wins).
    fn push(&mut self, descriptor: Descriptor) {
        // Linear: lists are short, and the order must stay the insertion order.
        if !self
            .descriptors
            .iter()
            .any(|d| d.config == descriptor.config)
        {
            self.descriptors.push(descriptor);
        }
    }
}

/// The group body of `A`: its instance, visible to its beans as `Arc<A>`, then its
/// registrations.
fn register_with_instance<A: AutoConfiguration>(builder: ContainerBuilder) -> ContainerBuilder {
    A::register(builder.component::<A>())
}

/// The auto-configurations of `list` without those in `excluded`, in processing order.
///
/// `A::after() = [B]` and `B::before() = [A]` both put B before A; a reference to a
/// configuration that is not in the (filtered) list, and a reference of a configuration to
/// itself, are ignored. The order is a stable topological sort (Kahn's algorithm that always
/// takes the ready configuration earliest in the list), so ties keep list order.
pub(crate) fn resolve<'a>(
    list: &'a AutoConfigurations,
    excluded: &[ConfigRef],
) -> Result<Vec<&'a Descriptor>, AutoConfigurationCycle> {
    let nodes: Vec<&Descriptor> = list
        .descriptors()
        .iter()
        .filter(|d| !excluded.contains(&d.config))
        .collect();
    let index: HashMap<ConfigRef, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, d)| (d.config, i))
        .collect();
    // `preds[i]`: the configurations processed before `i`; `succs[i]`: those after it. Sets,
    // so an edge stated by both `after` and `before` counts once.
    let mut preds = vec![BTreeSet::new(); nodes.len()];
    let mut succs = vec![BTreeSet::new(); nodes.len()];
    let mut edge = |first: usize, then: usize| {
        if first != then {
            succs[first].insert(then);
            preds[then].insert(first);
        }
    };
    for (i, d) in nodes.iter().enumerate() {
        for r in &d.after {
            if let Some(&j) = index.get(r) {
                edge(j, i);
            }
        }
        for r in &d.before {
            if let Some(&j) = index.get(r) {
                edge(i, j);
            }
        }
    }

    let mut waiting: Vec<usize> = preds.iter().map(BTreeSet::len).collect();
    let mut ready: BTreeSet<usize> = (0..nodes.len()).filter(|&i| waiting[i] == 0).collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(i) = ready.pop_first() {
        order.push(nodes[i]);
        for &s in &succs[i] {
            waiting[s] -= 1;
            if waiting[s] == 0 {
                ready.insert(s);
            }
        }
    }
    if order.len() == nodes.len() {
        return Ok(order);
    }

    // Every configuration left still waits on another one that is left, so following the
    // "processed after" edges from any of them ends on a cycle.
    let left = |i: usize| waiting[i] > 0;
    let first_left = |i: usize| preds[i].iter().copied().find(|&p| left(p));
    let start = (0..nodes.len()).find(|&i| left(i)).expect("a node is left");
    let mut walk = vec![start];
    let mut current = start;
    let path = loop {
        current = first_left(current).expect("a left node waits on a left node");
        if let Some(pos) = walk.iter().position(|&n| n == current) {
            let mut cycle = walk.split_off(pos);
            cycle.push(current);
            break cycle;
        }
        walk.push(current);
    };
    Err(AutoConfigurationCycle {
        path: path.into_iter().map(|i| nodes[i].config.name()).collect(),
    })
}

/// The auto-configurations cannot be ordered: their `after` and `before` entries form a cycle.
/// Its text names type names only.
pub(crate) struct AutoConfigurationCycle {
    /// One cycle, each configuration followed by one it is processed after, ending on the
    /// first again.
    pub(crate) path: Vec<&'static str>,
}

impl fmt::Display for AutoConfigurationCycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "auto-configurations form an ordering cycle: {}",
            self.path.join(" -> ")
        )
    }
}

impl fmt::Debug for AutoConfigurationCycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AutoConfigurationCycle")
            .field("path", &self.path)
            .finish()
    }
}

impl Error for AutoConfigurationCycle {}

#[cfg(test)]
mod tests {
    use oxbloom_di::{DiError, Resolver};

    use super::*;

    struct Base;
    impl Component for Base {
        fn create(_: &Resolver) -> Result<Self, DiError> {
            Ok(Base)
        }
    }
    impl Configuration for Base {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder.bean(|| 1_u8)
        }
    }
    impl AutoConfiguration for Base {}

    struct Top;
    impl Component for Top {
        fn create(_: &Resolver) -> Result<Self, DiError> {
            Ok(Top)
        }
    }
    impl Configuration for Top {
        fn register(builder: ContainerBuilder) -> ContainerBuilder {
            builder.bean(|| 2_u16)
        }
    }
    impl AutoConfiguration for Top {
        fn conditions() -> Conditions {
            Conditions::new().on_bean::<u8>()
        }
        fn after() -> Vec<ConfigRef> {
            vec![ConfigRef::of::<Base>()]
        }
        fn before() -> Vec<ConfigRef> {
            vec![ConfigRef::of::<u32>()]
        }
    }

    fn configs(list: &AutoConfigurations) -> Vec<ConfigRef> {
        list.descriptors().iter().map(|d| d.config).collect()
    }

    #[test]
    fn descriptor_holds_the_configuration_functions() {
        let list = AutoConfigurations::new().add::<Top>();
        let d = &list.descriptors()[0];
        assert_eq!(d.config, ConfigRef::of::<Top>());
        assert_eq!(d.after, [ConfigRef::of::<Base>()]);
        assert_eq!(d.before, [ConfigRef::of::<u32>()]);
        let c = (d.register)(ContainerBuilder::new().bean(|| 1_u8))
            .build()
            .unwrap();
        assert_eq!(*c.get::<u16>().unwrap(), 2);
        assert!(
            c.get_opt::<Top>().unwrap().is_some(),
            "register adds the instance"
        );
        let c = ContainerBuilder::new()
            .group("Top", (d.conditions)(), d.register)
            .build()
            .unwrap();
        assert!(c.get_opt::<u16>().unwrap().is_none());
        assert!(
            c.get_opt::<Top>().unwrap().is_none(),
            "skipped with its group"
        );
    }

    #[test]
    fn duplicates_keep_the_first_position() {
        let mut list = AutoConfigurations::new()
            .add::<Top>()
            .add::<Base>()
            .add::<Top>();
        assert_eq!(
            configs(&list),
            [ConfigRef::of::<Top>(), ConfigRef::of::<Base>()]
        );
        list.extend(AutoConfigurations::new().add::<Base>().add::<Top>());
        assert_eq!(
            configs(&list),
            [ConfigRef::of::<Top>(), ConfigRef::of::<Base>()]
        );
    }

    #[test]
    fn descriptors_are_send_and_sync() {
        fn check<T: Send + Sync + Clone>() {}
        check::<AutoConfigurations>();
        check::<Descriptor>();
    }

    struct N1;
    struct N2;
    struct N3;
    struct N4;

    fn r<T: 'static>() -> ConfigRef {
        ConfigRef::of::<T>()
    }

    fn node<T: 'static>(after: Vec<ConfigRef>, before: Vec<ConfigRef>) -> Descriptor {
        Descriptor {
            config: r::<T>(),
            after,
            before,
            conditions: Conditions::new,
            register: |b| b,
            properties: || &[],
        }
    }

    fn list(nodes: Vec<Descriptor>) -> AutoConfigurations {
        AutoConfigurations { descriptors: nodes }
    }

    fn resolved(list: &AutoConfigurations, excluded: &[ConfigRef]) -> Vec<ConfigRef> {
        resolve(list, excluded)
            .unwrap()
            .iter()
            .map(|d| d.config)
            .collect()
    }

    #[test]
    fn resolve_sorts_stably_and_ignores_unknown_and_self() {
        let l = list(vec![
            node::<N1>(vec![r::<N3>(), r::<N1>(), r::<u8>()], vec![]),
            node::<N2>(vec![], vec![r::<N2>()]),
            node::<N3>(vec![], vec![r::<N1>()]), // the same edge as N1's `after`
            node::<N4>(vec![], vec![]),
        ]);
        assert_eq!(
            resolved(&l, &[]),
            [r::<N2>(), r::<N3>(), r::<N1>(), r::<N4>()]
        );
        assert_eq!(
            resolved(&l, &[r::<N3>(), r::<u16>()]),
            [r::<N1>(), r::<N2>(), r::<N4>()]
        );
        assert!(resolved(&list(vec![]), &[]).is_empty());
    }

    #[test]
    fn cycle_names_the_configurations_on_it() {
        // N1 waits on the cycle N2 -> N3 -> N4 -> N2 but is not on it.
        let l = list(vec![
            node::<N1>(vec![r::<N2>()], vec![]),
            node::<N2>(vec![r::<N3>()], vec![r::<N4>()]),
            node::<N3>(vec![r::<N4>()], vec![]),
            node::<N4>(vec![], vec![]),
        ]);
        let cycle = resolve(&l, &[]).unwrap_err();
        let names = |v: &[ConfigRef]| v.iter().map(|c| c.name()).collect::<Vec<_>>();
        assert_eq!(
            cycle.path,
            names(&[r::<N2>(), r::<N3>(), r::<N4>(), r::<N2>()])
        );
        assert_eq!(
            cycle.to_string(),
            format!(
                "auto-configurations form an ordering cycle: {} -> {} -> {} -> {}",
                r::<N2>().name(),
                r::<N3>().name(),
                r::<N4>().name(),
                r::<N2>().name()
            )
        );
        let debug = format!("{cycle:?}");
        assert!(
            debug.starts_with("AutoConfigurationCycle { path: ["),
            "{debug}"
        );
        // Excluding one configuration of the cycle resolves it.
        assert_eq!(
            resolved(&l, &[r::<N3>()]),
            [r::<N2>(), r::<N1>(), r::<N4>()]
        );
    }

    #[test]
    fn config_ref_debug_names_the_type() {
        let text = format!("{:?}", ConfigRef::of::<Base>());
        assert!(
            text.starts_with("ConfigRef(") && text.contains("Base"),
            "{text}"
        );
    }
}
