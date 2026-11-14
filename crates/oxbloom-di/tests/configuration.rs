//! `#[configuration]` impls and their `#[bean]` methods.

use std::any::type_name;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxbloom_di::{ContainerBuilder, DiError, ErrorSource, component, configuration};

fn name_of<T: ?Sized>() -> String {
    type_name::<T>().to_owned()
}

trait Store: Send + Sync {
    fn name(&self) -> &'static str;
}

// --- every #[bean] method is registered -----------------------------------------------------

struct Url(&'static str);
struct Pool {
    url: &'static str,
}
struct Cache;

#[component]
struct AppConfig;

#[configuration]
impl AppConfig {
    #[bean]
    fn url(&self) -> Url {
        Url("postgres://db/app")
    }

    #[bean]
    fn pool(&self, url: Arc<Url>) -> Pool {
        Pool { url: url.0 }
    }

    /// Not a bean: left alone.
    fn helper(&self) -> u8 {
        7
    }

    #[bean]
    fn cache(&self, _pool: Option<Arc<Pool>>, _all: Vec<Arc<dyn Store>>) -> Cache {
        assert_eq!(self.helper(), 7);
        Cache
    }
}

#[test]
fn configuration_registers_every_bean_method() {
    let c = ContainerBuilder::new()
        .component::<AppConfig>()
        .configuration::<AppConfig>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Url>().unwrap().0, "postgres://db/app");
    assert_eq!(c.get::<Pool>().unwrap().url, "postgres://db/app");
    assert!(c.get::<Cache>().is_ok());
    // One registration each: nothing else was added for the configuration.
    assert_eq!(c.get_all::<Pool>().unwrap().len(), 1);
    assert_eq!(c.get_all::<AppConfig>().unwrap().len(), 1);
}

// --- options ------------------------------------------------------------------------------------

struct Mem(&'static str);
impl Store for Mem {
    fn name(&self) -> &'static str {
        self.0
    }
}
struct Disk;
impl Store for Disk {
    fn name(&self) -> &'static str {
        "disk"
    }
}
struct Remote;
impl Store for Remote {
    fn name(&self) -> &'static str {
        "remote"
    }
}
struct Broken;

static LAZY_CREATED: AtomicUsize = AtomicUsize::new(0);
static BROKEN_RUNS: AtomicUsize = AtomicUsize::new(0);

struct Options {
    fail: bool,
}

#[configuration]
impl Options {
    #[bean(fallible, primary, order = 5, provides(dyn Store))]
    fn mem(&self) -> Result<Mem, DiError> {
        if self.fail {
            return Err(DiError::creation::<Mem>("mem refused"));
        }
        Ok(Mem("mem"))
    }

    #[bean(lazy, provides(dyn Store), order = -1)]
    fn disk(&self) -> Disk {
        LAZY_CREATED.fetch_add(1, Ordering::SeqCst);
        Disk
    }

    #[bean(provides(dyn Store), profile = ["cloud", "edge"])]
    fn remote(&self) -> Remote {
        Remote
    }

    #[bean(fallible, lazy, profile = "!prod")]
    fn broken(&self) -> Result<Broken, DiError> {
        BROKEN_RUNS.fetch_add(1, Ordering::SeqCst);
        Err(DiError::Missing {
            requested: "x".into(),
            required_by: Vec::new(),
        })
    }
}

fn options(fail: bool, profiles: &[&'static str]) -> ContainerBuilder {
    ContainerBuilder::new()
        .active_profiles(profiles.to_vec())
        .instance(Options { fail })
        .configuration::<Options>()
}

#[test]
fn bean_options_fallible_lazy_primary_order_profile_provides() {
    LAZY_CREATED.store(0, Ordering::SeqCst);
    let c = options(false, &[]).build().unwrap();
    // lazy: not created by build.
    assert_eq!(LAZY_CREATED.load(Ordering::SeqCst), 0);
    // primary wins the single request; order sorts the collection (disk -1, mem 5).
    assert_eq!(c.get::<dyn Store>().unwrap().name(), "mem");
    let names: Vec<_> = c
        .get_all::<dyn Store>()
        .unwrap()
        .iter()
        .map(|s| s.name())
        .collect();
    assert_eq!(names, ["disk", "mem"]); // `remote` is profile-skipped
    assert_eq!(LAZY_CREATED.load(Ordering::SeqCst), 1);

    // profile list is any-of.
    let c = options(false, &["edge"]).build().unwrap();
    assert_eq!(c.get_all::<dyn Store>().unwrap().len(), 3);
    assert!(c.get::<Remote>().is_ok());

    // fallible: an Err fails the creation as `Creation` for the bean type (key = inner type).
    let err = options(true, &[]).build().err().unwrap();
    assert_eq!(
        err,
        DiError::Creation {
            type_name: name_of::<Mem>(),
            message: "mem refused".into(),
            source: ErrorSource::none(),
        }
    );

    // fallible + lazy + "!prod": kept without profiles, fails at first use, other variants
    // become the fixed `bean failed:` text, and the failure is cached.
    BROKEN_RUNS.store(0, Ordering::SeqCst);
    let c = options(false, &[]).build().unwrap();
    let first = c.get::<Broken>().err().unwrap();
    assert_eq!(
        first,
        DiError::Creation {
            type_name: name_of::<Broken>(),
            message: "bean failed: no component registered for x".into(),
            source: ErrorSource::none(),
        }
    );
    assert_eq!(c.get::<Broken>().err(), Some(first));
    assert_eq!(BROKEN_RUNS.load(Ordering::SeqCst), 1);
    // "!prod" with prod active: skipped, so Missing.
    let c = options(false, &["prod"]).build().unwrap();
    assert!(matches!(c.get::<Broken>(), Err(DiError::Missing { .. })));
}

// --- the configuration component --------------------------------------------------------------

struct Greeting(String);

#[component]
struct Name {
    _url: Option<Arc<Url>>,
}

struct Greeter {
    greeting: &'static str,
}

#[configuration]
impl Greeter {
    #[bean]
    fn greeting(&self, name: Arc<Name>) -> Greeting {
        assert!(name._url.is_none());
        Greeting(format!("{} Ada", self.greeting))
    }
}

#[test]
fn configuration_bean_depends_on_the_configuration_component() {
    static CREATED: AtomicUsize = AtomicUsize::new(0);
    let c = ContainerBuilder::new()
        .configuration::<Greeter>()
        .component::<Name>()
        .factory(|_| {
            CREATED.fetch_add(1, Ordering::SeqCst);
            Ok(Greeter { greeting: "hello" })
        })
        .build()
        .unwrap();
    assert_eq!(c.get::<Greeting>().unwrap().0, "hello Ada");
    // The configuration is a singleton like every bean: created once.
    assert_eq!(CREATED.load(Ordering::SeqCst), 1);
}

#[test]
fn forgetting_to_register_the_configuration_component_is_missing_naming_it() {
    let err = ContainerBuilder::new()
        .component::<Name>()
        .configuration::<Greeter>()
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err,
        DiError::Missing {
            requested: name_of::<Greeter>(),
            required_by: vec![name_of::<Greeting>()],
        }
    );
}

// --- duplicates and ambiguity name the methods -------------------------------------------------

struct Twins;

#[configuration]
impl Twins {
    #[bean]
    fn first(&self) -> Mem {
        Mem("first")
    }

    #[bean]
    fn second(&self) -> Mem {
        Mem("second")
    }
}

#[test]
fn duplicate_across_configuration_methods_names_both_labels() {
    let err = ContainerBuilder::new()
        .instance(Twins)
        .configuration::<Twins>()
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err,
        DiError::Duplicate {
            type_name: name_of::<Mem>(),
            sources: vec![
                format!("{}::first", name_of::<Twins>()),
                format!("{}::second", name_of::<Twins>()),
            ],
        }
    );
}

#[test]
fn ambiguous_trait_names_the_bean_method() {
    // `OnlyDisk::disk` and an instance both provide `dyn Store`.
    struct OnlyDisk;
    #[configuration]
    impl OnlyDisk {
        #[bean(provides(dyn Store))]
        fn disk(&self) -> Disk {
            Disk
        }
    }
    let c = ContainerBuilder::new()
        .instance(OnlyDisk)
        .configuration::<OnlyDisk>()
        .instance(Remote)
        .provides(|r: Arc<Remote>| -> Arc<dyn Store> { r })
        .build()
        .unwrap();
    let Err(DiError::Ambiguous { candidates, .. }) = c.get::<dyn Store>() else {
        panic!("expected Ambiguous");
    };
    assert_eq!(
        candidates,
        [
            format!("{}::disk", name_of::<OnlyDisk>()),
            format!("instance {}", name_of::<Remote>()),
        ]
    );
}

// --- paths, macros and visibility ---------------------------------------------------------------

mod nested {
    use std::sync::Arc;

    pub struct Port(pub u16);

    pub struct Server;

    // Private bean methods, fully qualified attribute paths, and a doc comment.
    #[oxbloom_di::configuration]
    impl Server {
        /// The listening port.
        #[oxbloom_di::bean]
        fn port(&self) -> Port {
            Port(8080)
        }

        #[bean(primary)]
        fn doubled(&self, port: Arc<Port>) -> u32 {
            u32::from(port.0) * 2
        }
    }
}

#[test]
fn bean_methods_may_be_private_and_attributes_qualified() {
    let c = ContainerBuilder::new()
        .instance(nested::Server)
        .configuration::<nested::Server>()
        .build()
        .unwrap();
    assert_eq!(c.get::<nested::Port>().unwrap().0, 8080);
    assert_eq!(*c.get::<u32>().unwrap(), 16160);
}

// --- cfg on methods, `self: &Self` ---------------------------------------------------------------

struct Gone;
struct GoneToo;
struct Kept(u8);
struct Spelled(u8);

struct Conditional;

#[configuration]
impl Conditional {
    #[cfg(any())]
    #[bean]
    fn gone(&self) -> Gone {
        Gone
    }

    #[bean]
    #[cfg_attr(all(), cfg(any()))]
    fn gone_too(&self) -> GoneToo {
        GoneToo
    }

    // A true cfg keeps it; a `cfg_attr` adding a non-cfg attribute stays on the method only.
    #[bean]
    #[cfg(not(any()))]
    #[cfg_attr(all(), inline)]
    fn kept(&self) -> Kept {
        Kept(1)
    }

    #[bean]
    #[allow(clippy::needless_arbitrary_self_type)] // spelled out on purpose
    fn spelled(self: &Self, kept: Arc<Kept>) -> Spelled {
        Spelled(kept.0 + 1)
    }
}

#[test]
fn cfg_on_a_bean_method_also_guards_its_registration() {
    let c = ContainerBuilder::new()
        .instance(Conditional)
        .configuration::<Conditional>()
        .build()
        .unwrap();
    assert!(matches!(c.get::<Gone>(), Err(DiError::Missing { .. })));
    assert!(matches!(c.get::<GoneToo>(), Err(DiError::Missing { .. })));
    assert_eq!(c.get::<Kept>().unwrap().0, 1);
}

#[test]
fn self_ref_self_receiver_is_accepted() {
    let c = ContainerBuilder::new()
        .instance(Conditional)
        .configuration::<Conditional>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Spelled>().unwrap().0, 2);
}

// --- generated names cannot clash with items in scope -----------------------------------------

mod names_in_scope {
    #![allow(non_camel_case_types, dead_code)]
    use std::sync::Arc;

    use oxbloom_di::{ContainerBuilder, DiError, configuration};

    // Unit structs named like identifiers the generated code could use: a binding of that
    // name would become a pattern matching the struct.
    struct r;
    struct v;
    struct e;
    struct this;
    struct c;
    struct builder;
    struct __a0;

    trait Named: Send + Sync {
        fn name(&self) -> &'static str;
    }
    struct Dep;
    struct Made(&'static str);
    impl Named for Made {
        fn name(&self) -> &'static str {
            self.0
        }
    }

    struct Clashing;

    #[configuration]
    impl Clashing {
        #[bean]
        fn dep(&self) -> Dep {
            Dep
        }

        #[bean(fallible, provides(dyn Named))]
        fn made(&self, _dep: Arc<Dep>, _opt: Option<Arc<Dep>>) -> Result<Made, DiError> {
            Ok(Made("made"))
        }
    }

    #[test]
    fn unit_structs_named_like_generated_identifiers_do_not_break_expansion() {
        let container = ContainerBuilder::new()
            .instance(Clashing)
            .configuration::<Clashing>()
            .build()
            .unwrap();
        assert_eq!(container.get::<dyn Named>().unwrap().name(), "made");
    }
}

// --- condition options ---------------------------------------------------------------------------

mod conditions {
    use std::any::type_name;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use oxbloom_di::{ConditionReport, ContainerBuilder, DiError, Environment, configuration};

    use super::{Mem, Remote, Store};

    /// A fixed property source.
    struct Props(HashMap<&'static str, &'static str>);

    impl Props {
        fn of(pairs: &[(&'static str, &'static str)]) -> Self {
            Props(pairs.iter().copied().collect())
        }
    }

    impl Environment for Props {
        fn property(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|v| (*v).to_owned())
        }
    }

    /// The `(condition, matched, reason)` records of `registration`.
    fn records(report: &ConditionReport, registration: &str) -> Vec<(String, bool, String)> {
        report
            .records()
            .iter()
            .filter(|r| r.registration() == registration)
            .map(|r| (r.condition().to_owned(), r.matched(), r.reason().to_owned()))
            .collect()
    }

    fn label<C>(method: &str) -> String {
        format!("{}::{method}", type_name::<C>())
    }

    // --- on_missing_bean -------------------------------------------------------------------------

    struct Cache(&'static str);
    struct Pool;
    struct PoolStub;

    struct Defaults;

    #[configuration]
    impl Defaults {
        #[bean(on_missing_bean)]
        fn cache(&self) -> Cache {
            Cache("default")
        }

        #[bean(on_missing_bean(Pool))]
        fn pool_stub(&self) -> PoolStub {
            PoolStub
        }

        #[bean(on_missing_bean(dyn Store), provides(dyn Store))]
        fn store(&self) -> Mem {
            Mem("default")
        }
    }

    #[test]
    fn bean_on_missing_bean_without_argument_uses_return_type() {
        // Nothing registered before: the default is kept.
        let c = ContainerBuilder::new()
            .instance(Defaults)
            .configuration::<Defaults>()
            .build()
            .unwrap();
        assert_eq!(c.get::<Cache>().unwrap().0, "default");
        assert_eq!(
            records(c.condition_report(), &label::<Defaults>("cache")),
            [(
                "on_missing_bean".to_owned(),
                true,
                format!(
                    "no earlier registration provides `{}`",
                    type_name::<Cache>()
                )
            )]
        );

        // A user `Cache` registered first: the default backs off, no `Duplicate`.
        let c = ContainerBuilder::new()
            .instance(Cache("user"))
            .instance(Defaults)
            .configuration::<Defaults>()
            .build()
            .unwrap();
        assert_eq!(c.get::<Cache>().unwrap().0, "user");
        assert_eq!(
            records(c.condition_report(), &label::<Defaults>("cache")),
            [(
                "on_missing_bean".to_owned(),
                false,
                format!(
                    "an earlier registration already provides `{}`",
                    type_name::<Cache>()
                )
            )]
        );
    }

    #[test]
    fn bean_on_missing_bean_with_explicit_type() {
        let c = ContainerBuilder::new()
            .instance(Defaults)
            .configuration::<Defaults>()
            .build()
            .unwrap();
        assert!(c.get_opt::<PoolStub>().unwrap().is_some());

        // The explicit type decides, not the return type.
        let c = ContainerBuilder::new()
            .instance(Pool)
            .instance(Defaults)
            .configuration::<Defaults>()
            .build()
            .unwrap();
        assert!(c.get_opt::<PoolStub>().unwrap().is_none());
        assert_eq!(
            records(c.condition_report(), &label::<Defaults>("pool_stub")),
            [(
                "on_missing_bean".to_owned(),
                false,
                format!(
                    "an earlier registration already provides `{}`",
                    type_name::<Pool>()
                )
            )]
        );
    }

    #[test]
    fn bean_on_missing_bean_with_dyn_trait() {
        let c = ContainerBuilder::new()
            .instance(Defaults)
            .configuration::<Defaults>()
            .build()
            .unwrap();
        assert_eq!(c.get::<dyn Store>().unwrap().name(), "default");

        // An earlier `dyn Store` provider (through `provides`) makes the default back off.
        let c = ContainerBuilder::new()
            .instance(Remote)
            .provides(|r: Arc<Remote>| -> Arc<dyn Store> { r })
            .instance(Defaults)
            .configuration::<Defaults>()
            .build()
            .unwrap();
        assert_eq!(c.get::<dyn Store>().unwrap().name(), "remote");
        assert!(c.get_opt::<Mem>().unwrap().is_none());
    }

    // --- on_bean -----------------------------------------------------------------------------------

    struct DataSource;
    struct HealthCheck;

    struct Health;

    #[configuration]
    impl Health {
        #[bean(on_bean(DataSource))]
        fn health(&self) -> HealthCheck {
            HealthCheck
        }
    }

    #[test]
    fn bean_on_bean_requires_earlier_registration() {
        let c = ContainerBuilder::new()
            .instance(DataSource)
            .instance(Health)
            .configuration::<Health>()
            .build()
            .unwrap();
        assert!(c.get_opt::<HealthCheck>().unwrap().is_some());

        // Registered after the bean: it does not count.
        let c = ContainerBuilder::new()
            .instance(Health)
            .configuration::<Health>()
            .instance(DataSource)
            .build()
            .unwrap();
        assert!(c.get_opt::<HealthCheck>().unwrap().is_none());
        assert_eq!(
            records(c.condition_report(), &label::<Health>("health")),
            [(
                "on_bean".to_owned(),
                false,
                format!(
                    "no earlier registration provides `{}`",
                    type_name::<DataSource>()
                )
            )]
        );
    }

    // --- on_property -------------------------------------------------------------------------------

    struct Metrics;
    struct FastPath;

    struct Features;

    #[configuration]
    impl Features {
        #[bean(on_property("metrics.enabled"))]
        fn metrics(&self) -> Metrics {
            Metrics
        }

        #[bean(on_property("mode", "fast"))]
        fn fast(&self) -> FastPath {
            FastPath
        }
    }

    fn features(props: &[(&'static str, &'static str)]) -> oxbloom_di::Container {
        ContainerBuilder::new()
            .environment(Props::of(props))
            .instance(Features)
            .configuration::<Features>()
            .build()
            .unwrap()
    }

    #[test]
    fn bean_on_property_forms() {
        // Key only: any value counts, absence does not.
        let c = features(&[("metrics.enabled", "false")]);
        assert!(c.get_opt::<Metrics>().unwrap().is_some());
        let c = features(&[]);
        assert!(c.get_opt::<Metrics>().unwrap().is_none());
        assert_eq!(
            records(c.condition_report(), &label::<Features>("metrics")),
            [(
                "on_property".to_owned(),
                false,
                "property \"metrics.enabled\" is not set".to_owned()
            )]
        );

        // Key and value: an exact, case-sensitive match.
        let c = features(&[("mode", "fast")]);
        assert!(c.get_opt::<FastPath>().unwrap().is_some());
        assert_eq!(
            records(c.condition_report(), &label::<Features>("fast")),
            [(
                "on_property_value".to_owned(),
                true,
                "property \"mode\" has the expected value".to_owned()
            )]
        );
        for value in ["fastest", "fas", "Fast", ""] {
            let c = features(&[("mode", value)]);
            assert!(c.get_opt::<FastPath>().unwrap().is_none(), "{value:?}");
            assert_eq!(
                records(c.condition_report(), &label::<Features>("fast")),
                [(
                    "on_property_value".to_owned(),
                    false,
                    "property \"mode\" does not have the expected value".to_owned()
                )]
            );
        }
        let c = features(&[]);
        assert!(c.get_opt::<FastPath>().unwrap().is_none());
    }

    // --- several conditions, other options -------------------------------------------------------

    struct Auditor;

    struct Audit;

    #[configuration]
    impl Audit {
        #[bean(on_property("audit"), on_bean(DataSource), on_missing_bean)]
        fn auditor(&self) -> Auditor {
            Auditor
        }
    }

    fn audit(props: &[(&'static str, &'static str)], data: bool, user: bool) -> bool {
        let mut b = ContainerBuilder::new().environment(Props::of(props));
        if data {
            b = b.instance(DataSource);
        }
        if user {
            b = b.instance(Auditor);
        }
        let c = b.instance(Audit).configuration::<Audit>().build().unwrap();
        c.condition_report()
            .records()
            .iter()
            .filter(|r| r.registration() == label::<Audit>("auditor"))
            .all(|r| r.matched())
    }

    #[test]
    fn bean_conditions_and_with_each_other() {
        assert!(audit(&[("audit", "on")], true, false));
        assert!(!audit(&[], true, false));
        assert!(!audit(&[("audit", "on")], false, false));
        assert!(!audit(&[("audit", "on")], true, true));

        // Evaluated in source order; the first failure stops the evaluation.
        let c = ContainerBuilder::new()
            .environment(Props::of(&[("audit", "on")]))
            .instance(Audit)
            .configuration::<Audit>()
            .build()
            .unwrap();
        let conditions: Vec<_> = records(c.condition_report(), &label::<Audit>("auditor"))
            .into_iter()
            .map(|(name, matched, _)| (name, matched))
            .collect();
        assert_eq!(
            conditions,
            [
                ("on_property".to_owned(), true),
                ("on_bean".to_owned(), false)
            ]
        );
        assert!(c.get_opt::<Auditor>().unwrap().is_none());
    }

    static TUNED_CREATED: AtomicUsize = AtomicUsize::new(0);

    struct Tuned;

    #[configuration]
    impl Tuned {
        #[bean(lazy, on_property("tuned"), primary, order = 3, profile = "dev", provides(dyn Store))]
        fn tuned(&self) -> Mem {
            TUNED_CREATED.fetch_add(1, Ordering::SeqCst);
            Mem("tuned")
        }
    }

    fn tuned(
        props: &[(&'static str, &'static str)],
        profiles: &[&'static str],
    ) -> oxbloom_di::Container {
        ContainerBuilder::new()
            .active_profiles(profiles.to_vec())
            .environment(Props::of(props))
            .instance(Remote)
            .provides(|r: Arc<Remote>| -> Arc<dyn Store> { r })
            .order(10)
            .instance(Tuned)
            .configuration::<Tuned>()
            .build()
            .unwrap()
    }

    #[test]
    fn bean_condition_with_lazy_primary_order_profile() {
        TUNED_CREATED.store(0, Ordering::SeqCst);
        // Profile and condition both pass: kept, lazy, primary and ordered first.
        let c = tuned(&[("tuned", "yes")], &["dev"]);
        assert_eq!(TUNED_CREATED.load(Ordering::SeqCst), 0);
        assert_eq!(c.get::<dyn Store>().unwrap().name(), "tuned");
        let names: Vec<_> = c
            .get_all::<dyn Store>()
            .unwrap()
            .iter()
            .map(|s| s.name())
            .collect();
        assert_eq!(names, ["tuned", "remote"]);
        assert_eq!(TUNED_CREATED.load(Ordering::SeqCst), 1);

        // The condition fails: skipped, only `remote` remains.
        let c = tuned(&[], &["dev"]);
        assert_eq!(c.get::<dyn Store>().unwrap().name(), "remote");
        assert_eq!(
            records(c.condition_report(), &label::<Tuned>("tuned")).len(),
            1
        );

        // The profile fails: skipped by the profile, so no condition record.
        let c = tuned(&[("tuned", "yes")], &[]);
        assert_eq!(c.get::<dyn Store>().unwrap().name(), "remote");
        assert!(records(c.condition_report(), &label::<Tuned>("tuned")).is_empty());
        assert_eq!(TUNED_CREATED.load(Ordering::SeqCst), 1);
    }

    // --- fallible ------------------------------------------------------------------------------------

    static CLIENT_RUNS: AtomicUsize = AtomicUsize::new(0);

    struct Client(&'static str);

    struct Clients;

    #[configuration]
    impl Clients {
        #[bean(fallible, on_missing_bean)]
        fn client(&self) -> Result<Client, DiError> {
            CLIENT_RUNS.fetch_add(1, Ordering::SeqCst);
            Ok(Client("default"))
        }
    }

    #[test]
    fn fallible_bean_on_missing_bean_uses_ok_type() {
        CLIENT_RUNS.store(0, Ordering::SeqCst);
        let c = ContainerBuilder::new()
            .instance(Client("user"))
            .instance(Clients)
            .configuration::<Clients>()
            .build()
            .unwrap();
        assert_eq!(c.get::<Client>().unwrap().0, "user");
        assert_eq!(CLIENT_RUNS.load(Ordering::SeqCst), 0);
        assert_eq!(
            records(c.condition_report(), &label::<Clients>("client")),
            [(
                "on_missing_bean".to_owned(),
                false,
                format!(
                    "an earlier registration already provides `{}`",
                    type_name::<Client>()
                )
            )]
        );

        let c = ContainerBuilder::new()
            .instance(Clients)
            .configuration::<Clients>()
            .build()
            .unwrap();
        assert_eq!(c.get::<Client>().unwrap().0, "default");
        assert_eq!(CLIENT_RUNS.load(Ordering::SeqCst), 1);
    }

    // --- cfg ------------------------------------------------------------------------------------------

    // Only exists when the method exists: a condition emitted outside the method's cfg group
    // would name a missing type and fail to compile.
    #[cfg(any())]
    struct Ghost;
    struct Visible;

    struct Gated;

    #[configuration]
    impl Gated {
        #[cfg(any())]
        #[bean(on_bean(Ghost), on_property("ghost"))]
        fn ghost(&self) -> Ghost {
            Ghost
        }

        #[bean(on_missing_bean)]
        #[cfg_attr(any(), cfg(any()))]
        fn visible(&self) -> Visible {
            Visible
        }
    }

    #[test]
    fn bean_condition_skipped_together_with_cfg_false_method() {
        let c = ContainerBuilder::new()
            .instance(Gated)
            .configuration::<Gated>()
            .build()
            .unwrap();
        assert!(c.get_opt::<Visible>().unwrap().is_some());
        assert!(records(c.condition_report(), &label::<Gated>("ghost")).is_empty());
        assert_eq!(
            records(c.condition_report(), &label::<Gated>("visible")).len(),
            1
        );
    }

    // --- siblings -------------------------------------------------------------------------------------

    struct Earlier;
    struct Later;
    struct Third;

    struct Siblings;

    #[configuration]
    impl Siblings {
        #[bean]
        fn earlier(&self) -> Earlier {
            Earlier
        }

        #[bean(on_missing_bean(Earlier))]
        fn later(&self) -> Later {
            Later
        }

        #[bean(on_bean(Earlier))]
        fn third(&self) -> Third {
            Third
        }
    }

    /// Each `#[bean]` is its own registration, so a later bean sees the beans its own
    /// configuration registered before it. A condition meant for the whole configuration
    /// therefore belongs on the configuration itself (`#[auto_configuration]`, evaluated once as
    /// a group), not on each method.
    #[test]
    fn sibling_beans_in_one_configuration_see_each_other() {
        let c = ContainerBuilder::new()
            .instance(Siblings)
            .configuration::<Siblings>()
            .build()
            .unwrap();
        assert!(c.get_opt::<Earlier>().unwrap().is_some());
        assert!(c.get_opt::<Later>().unwrap().is_none());
        assert!(c.get_opt::<Third>().unwrap().is_some());
    }

    // --- trailing commas ---------------------------------------------------------------------

    struct Commas;
    struct WithComma;
    struct BackOff;

    // A trailing comma is accepted in every condition option, as in `on_property("k",)`.
    #[configuration]
    impl Commas {
        #[bean(on_bean(Commas,), on_property("comma.key",))]
        fn with_comma(&self) -> WithComma {
            WithComma
        }

        #[bean(on_missing_bean(WithComma,))]
        fn back_off(&self) -> BackOff {
            BackOff
        }
    }

    #[test]
    fn condition_types_accept_a_trailing_comma() {
        let c = ContainerBuilder::new()
            .environment(Props::of(&[("comma.key", "x")]))
            .instance(Commas)
            .configuration::<Commas>()
            .build()
            .unwrap();
        assert!(c.get_opt::<WithComma>().unwrap().is_some());
        assert!(c.get_opt::<BackOff>().unwrap().is_none());

        let c = ContainerBuilder::new()
            .instance(Commas)
            .configuration::<Commas>()
            .build()
            .unwrap();
        assert!(c.get_opt::<WithComma>().unwrap().is_none());
        assert!(c.get_opt::<BackOff>().unwrap().is_some());
    }
}

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/configuration/*.rs");
}
