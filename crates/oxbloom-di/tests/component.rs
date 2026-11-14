use std::sync::Arc;

use oxbloom_di::{ContainerBuilder, component};

pub struct Repo;
impl oxbloom_di::Component for Repo {
    fn create(_: &oxbloom_di::Resolver) -> Result<Self, oxbloom_di::DiError> {
        Ok(Repo)
    }
}

#[component]
struct Svc {
    repo: Arc<Repo>,
}

#[component]
struct Empty {}

#[component]
/// Docs and derives must survive.
#[derive(Debug)]
pub struct Fancy {
    pub repo: std::sync::Arc<Repo>,
    svc: Arc<Svc>,
}

impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Repo")
    }
}
impl std::fmt::Debug for Svc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Svc")
    }
}

#[test]
fn component_wires_fields_from_container() {
    let c = ContainerBuilder::new()
        .component::<Repo>()
        .component::<Svc>()
        .build()
        .unwrap();
    assert!(Arc::ptr_eq(
        &c.get::<Svc>().unwrap().repo,
        &c.get::<Repo>().unwrap()
    ));
}

#[test]
fn zero_field_struct_builds() {
    assert!(ContainerBuilder::new().component::<Empty>().build().is_ok());
}

#[test]
fn attributes_and_visibility_are_preserved() {
    let c = ContainerBuilder::new()
        .component::<Fancy>()
        .component::<Svc>()
        .component::<Repo>()
        .build()
        .unwrap();
    let f = c.get::<Fancy>().unwrap();
    assert!(format!("{f:?}").contains("Fancy"));
    assert!(Arc::ptr_eq(&f.repo, &c.get::<Repo>().unwrap()));
    assert!(Arc::ptr_eq(&f.svc, &c.get::<Svc>().unwrap()));
}

mod shadowed {
    use super::*;

    #[allow(dead_code)]
    type Result<T> = std::result::Result<T, String>;
    #[allow(dead_code)]
    struct Component;
    #[allow(dead_code)]
    struct Ok;

    #[component]
    struct Shadow {
        _repo: Arc<Repo>,
    }

    #[test]
    fn works_with_shadowed_names() {
        let c = ContainerBuilder::new()
            .component::<Repo>()
            .component::<Shadow>()
            .build();
        assert!(c.is_ok());
    }
}

// --- shapes, unit structs and options ---------------------------------------------------------

trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;
}
trait Audit: Send + Sync {
    fn audit(&self) -> &'static str;
}

#[component(provides(dyn Notifier, dyn Audit))]
struct Mailer;
impl Notifier for Mailer {
    fn name(&self) -> &'static str {
        "mail"
    }
}
impl Audit for Mailer {
    fn audit(&self) -> &'static str {
        "mail audited"
    }
}

#[component(provides(dyn Notifier), primary)]
struct Sms;
impl Notifier for Sms {
    fn name(&self) -> &'static str {
        "sms"
    }
}

#[component(provides(dyn Notifier), order = -5)]
struct Push {}
impl Notifier for Push {
    fn name(&self) -> &'static str {
        "push"
    }
}

/// Not registered anywhere: an `Option` of it is `None`.
struct Metrics;

#[component]
struct AllShapes {
    repo: Arc<Repo>,
    primary: Arc<dyn Notifier>,
    audit: Option<Arc<dyn Audit>>,
    metrics: Option<Arc<Metrics>>,
    all: Vec<Arc<dyn Notifier>>,
    none: Vec<Arc<Metrics>>,
}

#[component]
struct Unit;

#[test]
fn component_injects_all_shapes() {
    let c = ContainerBuilder::new()
        .component::<AllShapes>()
        .component::<Repo>()
        .component::<Mailer>()
        .component::<Sms>()
        .build()
        .unwrap();
    let s = c.get::<AllShapes>().unwrap();
    assert!(Arc::ptr_eq(&s.repo, &c.get::<Repo>().unwrap()));
    assert_eq!(s.primary.name(), "sms");
    assert_eq!(s.audit.as_ref().map(|a| a.audit()), Some("mail audited"));
    assert!(s.metrics.is_none());
    let names: Vec<_> = s.all.iter().map(|n| n.name()).collect();
    assert_eq!(names, ["mail", "sms"]);
    assert!(s.none.is_empty());
}

#[test]
fn unit_struct_component_builds() {
    let c = ContainerBuilder::new().component::<Unit>().build().unwrap();
    assert!(c.get::<Unit>().is_ok());
}

#[test]
fn component_provides_trait_and_primary() {
    let c = ContainerBuilder::new()
        .component::<Mailer>()
        .component::<Sms>()
        .build()
        .unwrap();
    // `Mailer` provides two traits and stays available as itself.
    assert_eq!(c.get::<dyn Audit>().unwrap().audit(), "mail audited");
    assert!(c.get::<Mailer>().is_ok());
    // Two providers of `dyn Notifier`: the `primary` one wins.
    assert_eq!(c.get::<dyn Notifier>().unwrap().name(), "sms");

    // Without `Sms`, the single provider is chosen.
    let c = ContainerBuilder::new()
        .component::<Mailer>()
        .build()
        .unwrap();
    assert_eq!(c.get::<dyn Notifier>().unwrap().name(), "mail");

    // Two providers, none primary: ambiguous.
    let c = ContainerBuilder::new()
        .component::<Mailer>()
        .component::<Push>()
        .build()
        .unwrap();
    assert!(matches!(
        c.get::<dyn Notifier>(),
        Err(oxbloom_di::DiError::Ambiguous { .. })
    ));
}

#[test]
fn component_order_attribute_orders_get_all() {
    let c = ContainerBuilder::new()
        .component::<Mailer>()
        .component::<Sms>()
        .component::<Push>()
        .build()
        .unwrap();
    let names: Vec<_> = c
        .get_all::<dyn Notifier>()
        .unwrap()
        .iter()
        .map(|n| n.name())
        .collect();
    assert_eq!(names, ["push", "mail", "sms"]);
}

/// Formerly a compile-fail case: `Arc<dyn Trait>` fields are injectable now.
#[test]
fn arc_dyn_field_compiles() {
    #[component]
    struct Uses {
        n: Arc<dyn Notifier>,
    }
    let c = ContainerBuilder::new()
        .component::<Push>()
        .component::<Uses>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Uses>().unwrap().n.name(), "push");
}

#[component]
struct ShadowsLocals {
    r: Arc<Repo>,
    v: Arc<Unit>,
    e: Option<Arc<Repo>>,
}

#[test]
fn fields_named_like_generated_locals_resolve() {
    let c = ContainerBuilder::new()
        .component::<ShadowsLocals>()
        .component::<Repo>()
        .component::<Unit>()
        .build()
        .unwrap();
    let s = c.get::<ShadowsLocals>().unwrap();
    assert!(Arc::ptr_eq(&s.r, &c.get::<Repo>().unwrap()));
    assert!(Arc::ptr_eq(&s.v, &c.get::<Unit>().unwrap()));
    assert!(s.e.is_some());
}

#[test]
fn hand_written_component_configure_defaults_to_nothing() {
    use oxbloom_di::{Component, DiError};

    // `Repo` implements only `create`; the default `configure` returns the builder unchanged:
    // on an empty builder it registers nothing ...
    let c = <Repo as Component>::configure(ContainerBuilder::new())
        .build()
        .unwrap();
    assert!(matches!(c.get::<Repo>(), Err(DiError::Missing { .. })));
    // ... and after `component::<Repo>()` the last registration is still `Repo` (a modifier for
    // it applies) and the only one.
    let c = ContainerBuilder::new()
        .component::<Repo>()
        .provides(|r: Arc<Repo>| -> Arc<dyn std::fmt::Debug + Send + Sync> { r })
        .build()
        .unwrap();
    assert_eq!(c.get_all::<Repo>().unwrap().len(), 1);
    assert_eq!(
        format!(
            "{:?}",
            c.get::<dyn std::fmt::Debug + Send + Sync>().unwrap()
        ),
        "Repo"
    );
    // Neither primary nor lazy nor profile-conditional: a second provider makes it ambiguous,
    // and it is built eagerly even with profiles active.
    let c = ContainerBuilder::new()
        .active_profiles(["prod"])
        .component::<Repo>()
        .provides(|r: Arc<Repo>| -> Arc<dyn std::fmt::Debug + Send + Sync> { r })
        .instance(7u8)
        .provides(|n: Arc<u8>| -> Arc<dyn std::fmt::Debug + Send + Sync> { n })
        .build()
        .unwrap();
    assert!(matches!(
        c.get::<dyn std::fmt::Debug + Send + Sync>(),
        Err(DiError::Ambiguous { .. })
    ));
}

// --- lifecycle, lazy and profile options --------------------------------------------------------

mod options {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use oxbloom_di::{ContainerBuilder, DiError, Lifecycle, component};

    static STARTED: AtomicUsize = AtomicUsize::new(0);
    static STOPPED: AtomicUsize = AtomicUsize::new(0);
    static LAZY_CREATED: AtomicUsize = AtomicUsize::new(0);

    #[component(lifecycle)]
    struct Scheduler;
    impl Lifecycle for Scheduler {
        fn post_construct(&self) -> Result<(), DiError> {
            STARTED.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn pre_destroy(&self) {
            STOPPED.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Lazy, counted through its `post_construct`, and a dependent of the scheduler.
    #[component(lazy, lifecycle)]
    struct Report {
        _scheduler: Arc<Scheduler>,
    }
    impl Lifecycle for Report {
        fn post_construct(&self) -> Result<(), DiError> {
            LAZY_CREATED.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[component(profile = "dev")]
    struct DevOnly;

    #[component(profile = ["dev", "test"])]
    struct DevOrTest;

    #[component(profile = "!prod")]
    struct NotProd;

    fn builder(profiles: &[&'static str]) -> ContainerBuilder {
        ContainerBuilder::new()
            .active_profiles(profiles.to_vec())
            .component::<Report>()
            .component::<Scheduler>()
            .component::<DevOnly>()
            .component::<DevOrTest>()
            .component::<NotProd>()
    }

    #[test]
    fn component_lifecycle_lazy_profile_attributes() {
        STARTED.store(0, Ordering::SeqCst);
        STOPPED.store(0, Ordering::SeqCst);
        LAZY_CREATED.store(0, Ordering::SeqCst);

        let c = builder(&[]).build().unwrap();
        // lifecycle: post_construct ran for the eager scheduler; lazy: the report waits.
        assert_eq!(STARTED.load(Ordering::SeqCst), 1);
        assert_eq!(LAZY_CREATED.load(Ordering::SeqCst), 0);
        assert!(c.get::<Report>().is_ok());
        assert!(c.get::<Report>().is_ok());
        assert_eq!(LAZY_CREATED.load(Ordering::SeqCst), 1);
        // profiles: none active.
        assert!(matches!(c.get::<DevOnly>(), Err(DiError::Missing { .. })));
        assert!(matches!(c.get::<DevOrTest>(), Err(DiError::Missing { .. })));
        assert!(c.get::<NotProd>().is_ok());
        // lifecycle: pre_destroy on shutdown.
        let report = c.shutdown();
        assert_eq!(report.destroyed, 2);
        assert_eq!(STOPPED.load(Ordering::SeqCst), 1);

        let c = builder(&["test", "prod"]).build().unwrap();
        assert!(matches!(c.get::<DevOnly>(), Err(DiError::Missing { .. })));
        assert!(c.get::<DevOrTest>().is_ok());
        assert!(matches!(c.get::<NotProd>(), Err(DiError::Missing { .. })));

        let c = builder(&["dev"]).build().unwrap();
        assert!(c.get::<DevOnly>().is_ok());
        assert!(c.get::<DevOrTest>().is_ok());
    }
}

// --- generated `create`: names in scope and error paths -----------------------------------------

mod names_in_scope {
    #![allow(non_camel_case_types, non_upper_case_globals, dead_code)]
    use std::sync::Arc;

    use oxbloom_di::{ContainerBuilder, component};

    use super::Repo;

    /// A unit struct and a const named like the fields: `let repo = ..` / `let limit = ..` would
    /// be patterns matching them, not bindings.
    struct repo;
    const limit: Option<Arc<Repo>> = None;
    /// Named like identifiers the generated code could use.
    struct __f0;
    struct r;
    struct v;
    struct e;
    struct c;
    struct builder;

    trait Marker: Send + Sync {}
    impl Marker for Uses {}

    #[component(provides(dyn Marker), primary)]
    struct Uses {
        repo: Arc<Repo>,
        limit: Option<Arc<Repo>>,
    }

    #[test]
    fn fields_and_generated_names_like_unit_structs_or_consts_in_scope_resolve() {
        let container = ContainerBuilder::new()
            .component::<Repo>()
            .component::<Uses>()
            .build()
            .unwrap();
        let u = container.get::<Uses>().unwrap();
        assert!(Arc::ptr_eq(&u.repo, &container.get::<Repo>().unwrap()));
        assert!(u.limit.is_some());
        assert!(container.get::<dyn Marker>().is_ok());
    }
}

mod error_paths {
    use std::any::type_name;
    use std::sync::Arc;

    use oxbloom_di::{ContainerBuilder, DiError, ErrorSource, component};

    fn name_of<T: ?Sized>() -> String {
        type_name::<T>().to_owned()
    }

    struct Absent;
    struct Fails;

    #[component]
    struct NeedsAbsent {
        _a: Arc<Absent>,
    }

    #[component]
    struct Left {
        _r: Arc<Right>,
    }
    #[component]
    struct Right {
        _l: Option<Arc<Left>>,
    }

    #[component]
    struct OptionalFails {
        _f: Option<Arc<Fails>>,
    }

    #[test]
    fn generated_create_reports_missing_cycle_and_failing_option() {
        let err = ContainerBuilder::new()
            .component::<NeedsAbsent>()
            .build()
            .err();
        assert_eq!(
            err,
            Some(DiError::Missing {
                requested: name_of::<Absent>(),
                required_by: vec![name_of::<NeedsAbsent>()],
            })
        );

        // A cycle through an `Option` field is still a cycle, not `None`.
        let err = ContainerBuilder::new()
            .component::<Left>()
            .component::<Right>()
            .build()
            .err();
        assert_eq!(
            err,
            Some(DiError::Cycle {
                path: vec![name_of::<Left>(), name_of::<Right>(), name_of::<Left>()],
            })
        );

        // An `Option` whose provider fails is the failure, not `None`.
        let err = ContainerBuilder::new()
            .component::<OptionalFails>()
            .factory(|_| -> Result<Fails, DiError> { Err(DiError::creation::<Fails>("down")) })
            .lazy()
            .build()
            .err();
        assert_eq!(
            err,
            Some(DiError::Creation {
                type_name: name_of::<Fails>(),
                message: "down".into(),
                source: ErrorSource::none(),
            })
        );
    }
}

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
