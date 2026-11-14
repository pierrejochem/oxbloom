//! Trait provision (`provides`), injectable shapes (`get`, `get_opt`, `get_all`), `primary` and
//! `order`.

use std::any::type_name;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oxbloom_di::{Component, ContainerBuilder, DiError, ErrorSource, Resolver};

trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;
}

struct Mailer;
impl Notifier for Mailer {
    fn name(&self) -> &'static str {
        "mail"
    }
}

struct Sms;
impl Notifier for Sms {
    fn name(&self) -> &'static str {
        "sms"
    }
}

struct Push;
impl Notifier for Push {
    fn name(&self) -> &'static str {
        "push"
    }
}

fn name_of<T: ?Sized>() -> String {
    type_name::<T>().to_owned()
}

fn names(all: &[Arc<dyn Notifier>]) -> Vec<&'static str> {
    all.iter().map(|n| n.name()).collect()
}

#[test]
fn one_provider_resolves_trait() {
    let c = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .build()
        .unwrap();
    assert_eq!(c.get::<dyn Notifier>().unwrap().name(), "mail");
}

#[test]
fn entry_provides_concrete_and_trait() {
    let c = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .build()
        .unwrap();
    let concrete = c.get::<Mailer>().unwrap();
    let as_trait = c.get::<dyn Notifier>().unwrap();
    assert_eq!(
        Arc::as_ptr(&concrete).cast::<()>(),
        Arc::as_ptr(&as_trait).cast::<()>(),
        "the trait Arc shares the allocation of the concrete one"
    );
    let again = c.get::<dyn Notifier>().unwrap();
    assert!(Arc::ptr_eq(&as_trait, &again));
}

#[test]
fn two_providers_without_primary_is_ambiguous_listing_labels() {
    let err = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .factory(|_| Ok(Sms))
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .build()
        .unwrap()
        .get::<dyn Notifier>()
        .err();
    assert_eq!(
        err,
        Some(DiError::Ambiguous {
            requested: name_of::<dyn Notifier>(),
            candidates: vec![
                format!("instance {}", name_of::<Mailer>()),
                format!("factory {}", name_of::<Sms>()),
            ],
        })
    );
}

#[test]
fn primary_resolves_ambiguity() {
    let c = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .instance(Sms)
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .primary()
        .instance(Push)
        .provides(|a: Arc<Push>| -> Arc<dyn Notifier> { a })
        .build()
        .unwrap();
    assert_eq!(c.get::<dyn Notifier>().unwrap().name(), "sms");
    assert_eq!(c.get_opt::<dyn Notifier>().unwrap().unwrap().name(), "sms");
}

#[test]
fn two_primaries_is_ambiguous() {
    let err = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .primary()
        .instance(Sms)
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .primary()
        .build()
        .unwrap()
        .get::<dyn Notifier>()
        .err();
    assert_eq!(
        err,
        Some(DiError::Ambiguous {
            requested: name_of::<dyn Notifier>(),
            candidates: vec![
                format!("instance {}", name_of::<Mailer>()),
                format!("instance {}", name_of::<Sms>()),
            ],
        })
    );
}

#[test]
fn get_all_orders_by_order_then_registration() {
    let c = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .order(5)
        .instance(Sms)
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .instance(Push)
        .provides(|a: Arc<Push>| -> Arc<dyn Notifier> { a })
        .order(-1)
        .build()
        .unwrap();
    // Push (-1), Sms (0, default), Mailer (5).
    assert_eq!(
        names(&c.get_all::<dyn Notifier>().unwrap()),
        ["push", "sms", "mail"]
    );

    // Equal orders keep registration order.
    let c = ContainerBuilder::new()
        .instance(Push)
        .provides(|a: Arc<Push>| -> Arc<dyn Notifier> { a })
        .order(1)
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .order(1)
        .build()
        .unwrap();
    assert_eq!(
        names(&c.get_all::<dyn Notifier>().unwrap()),
        ["push", "mail"]
    );
}

#[test]
fn get_all_empty_when_none() {
    let c = ContainerBuilder::new().instance(1u8).build().unwrap();
    assert!(c.get_all::<dyn Notifier>().unwrap().is_empty());
    assert!(c.get_all::<Mailer>().unwrap().is_empty());
    assert_eq!(c.get_all::<u8>().unwrap().len(), 1);
}

struct Broken;
struct Probe;

#[test]
fn get_opt_none_when_absent_but_error_when_provider_fails() {
    let boom = DiError::Creation {
        type_name: name_of::<Broken>(),
        message: "boom".into(),
        source: ErrorSource::none(),
    };
    // Inside the build: absent is Ok(None), a failing provider is the provider's error.
    let recorder = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&recorder);
    let result = ContainerBuilder::new()
        .factory(move |r: &Resolver| {
            let mut seen = sink.lock().unwrap();
            seen.push(r.get_opt::<dyn Notifier>().map(|o| o.is_some()));
            seen.push(r.get_opt::<Broken>().map(|o| o.is_some()));
            Ok(Probe)
        })
        .factory(|_| -> Result<Broken, DiError> { Err(DiError::creation::<Broken>("boom")) })
        .build();
    assert_eq!(*recorder.lock().unwrap(), [Ok(false), Err(boom.clone())]);
    // The build loop then reaches Broken itself and gets the cached error.
    assert_eq!(result.err(), Some(boom));

    // On a built container: absent is Ok(None).
    let c = ContainerBuilder::new().instance(1u8).build().unwrap();
    assert!(c.get_opt::<dyn Notifier>().unwrap().is_none());
    assert!(c.get_opt::<Mailer>().unwrap().is_none());
    assert_eq!(*c.get_opt::<u8>().unwrap().unwrap(), 1);
}

// --- cycle through a trait key -----------------------------------------------------------------

struct NeedsNotifier;
impl Component for NeedsNotifier {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        r.get::<dyn Notifier>()?;
        Ok(NeedsNotifier)
    }
}

struct LoopBack;
impl Notifier for LoopBack {
    fn name(&self) -> &'static str {
        "loop"
    }
}
impl Component for LoopBack {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        r.get::<NeedsNotifier>()?;
        Ok(LoopBack)
    }
}

#[test]
fn cycle_through_trait_key_names_concrete_types() {
    let err = ContainerBuilder::new()
        .component::<NeedsNotifier>()
        .component::<LoopBack>()
        .provides(|a: Arc<LoopBack>| -> Arc<dyn Notifier> { a })
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Cycle {
            path: vec![
                name_of::<NeedsNotifier>(),
                name_of::<LoopBack>(),
                name_of::<NeedsNotifier>(),
            ],
        })
    );
}

// --- builder misuse ----------------------------------------------------------------------------

#[test]
fn provides_with_wrong_type_is_invalid_at_build() {
    let err = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .build()
        .err();
    match err {
        Some(DiError::Invalid { message }) => {
            assert!(message.contains(type_name::<Sms>()), "{message}");
            assert!(message.contains(type_name::<Mailer>()), "{message}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn modifier_before_any_registration_is_invalid() {
    let provides = ContainerBuilder::new()
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .instance(Mailer)
        .build()
        .err();
    assert!(
        matches!(provides, Some(DiError::Invalid { .. })),
        "{provides:?}"
    );
    let primary = ContainerBuilder::new()
        .primary()
        .instance(1u8)
        .build()
        .err();
    assert!(
        matches!(primary, Some(DiError::Invalid { .. })),
        "{primary:?}"
    );
    let order = ContainerBuilder::new().order(3).build().err();
    assert!(matches!(order, Some(DiError::Invalid { .. })), "{order:?}");
}

// --- shapes inside a provider ------------------------------------------------------------------

trait Absent: Send + Sync {}

struct Dispatcher {
    single: Arc<dyn Notifier>,
    absent: Option<Arc<dyn Absent>>,
    all: Vec<Arc<dyn Notifier>>,
}
impl Component for Dispatcher {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Dispatcher {
            single: r.get::<dyn Notifier>()?,
            absent: r.get_opt::<dyn Absent>()?,
            all: r.get_all::<dyn Notifier>()?,
        })
    }
}

#[test]
fn resolver_shapes_inside_a_component_create() {
    let c = ContainerBuilder::new()
        .component::<Dispatcher>() // dependent first: its requests create the providers
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .order(2)
        .instance(Sms)
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .primary()
        .build()
        .unwrap();
    let d = c.get::<Dispatcher>().unwrap();
    assert_eq!(d.single.name(), "sms");
    assert!(d.absent.is_none());
    assert_eq!(names(&d.all), ["sms", "mail"]);
    assert_eq!(
        Arc::as_ptr(&d.single).cast::<()>(),
        Arc::as_ptr(&c.get::<Sms>().unwrap()).cast::<()>()
    );
}

#[test]
fn duplicate_concrete_still_detected_when_both_provide_the_same_trait() {
    let err = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .factory(|_| Ok(Mailer))
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Duplicate {
            type_name: name_of::<Mailer>(),
            sources: vec![
                format!("instance {}", name_of::<Mailer>()),
                format!("factory {}", name_of::<Mailer>()),
            ],
        })
    );
}

#[test]
fn get_dyn_on_container_after_build() {
    let c = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .build()
        .unwrap();
    let handle = std::thread::spawn(move || c.get::<dyn Notifier>().map(|n| n.name()));
    assert_eq!(handle.join().unwrap(), Ok("mail"));

    let empty = ContainerBuilder::new().build().unwrap();
    assert_eq!(
        empty.get::<dyn Notifier>().err(),
        Some(DiError::Missing {
            requested: name_of::<dyn Notifier>(),
            required_by: Vec::new(),
        })
    );
}

#[test]
fn trait_object_must_be_send_sync_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/shapes/*.rs");
}

// --- fix round 1 -------------------------------------------------------------------------------

/// A decorator: `provides` may build a new value around the registered one.
struct Loud(#[allow(dead_code)] Arc<Mailer>);
impl Notifier for Loud {
    fn name(&self) -> &'static str {
        "loud mail"
    }
}

static LOUD_COERCIONS: AtomicUsize = AtomicUsize::new(0);

struct UsesNotifierTwice;
impl Component for UsesNotifierTwice {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        let a = r.get::<dyn Notifier>()?;
        let b = r.get::<dyn Notifier>()?;
        let all = r.get_all::<dyn Notifier>()?;
        assert!(Arc::ptr_eq(&a, &b));
        assert!(Arc::ptr_eq(&a, &all[0]));
        Ok(UsesNotifierTwice)
    }
}

#[test]
fn decorator_coercion_runs_once_and_gets_share_the_arc() {
    let c = ContainerBuilder::new()
        .component::<UsesNotifierTwice>()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> {
            LOUD_COERCIONS.fetch_add(1, Ordering::SeqCst);
            Arc::new(Loud(a))
        })
        .build()
        .unwrap();
    let first = c.get::<dyn Notifier>().unwrap();
    let second = c.get::<dyn Notifier>().unwrap();
    assert_eq!(first.name(), "loud mail");
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(
        &first,
        &c.get_all::<dyn Notifier>().unwrap()[0]
    ));
    assert!(Arc::ptr_eq(
        &first,
        &c.get_opt::<dyn Notifier>().unwrap().unwrap()
    ));
    assert_eq!(LOUD_COERCIONS.load(Ordering::SeqCst), 1);
}

static EAGER_COERCIONS: AtomicUsize = AtomicUsize::new(0);

#[test]
fn coercion_runs_at_build_even_when_never_requested() {
    let _c = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> {
            EAGER_COERCIONS.fetch_add(1, Ordering::SeqCst);
            a
        })
        .build()
        .unwrap();
    assert_eq!(EAGER_COERCIONS.load(Ordering::SeqCst), 1);
}

/// Provides `dyn Notifier` and collects every other provider of it.
struct Composite(Vec<Arc<dyn Notifier>>);
impl Notifier for Composite {
    fn name(&self) -> &'static str {
        "composite"
    }
}
impl Component for Composite {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Composite(r.get_all::<dyn Notifier>()?))
    }
}

#[test]
fn composite_collects_all_other_providers() {
    let c = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .component::<Composite>()
        .provides(|a: Arc<Composite>| -> Arc<dyn Notifier> { a })
        .primary()
        .instance(Sms)
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .build()
        .unwrap();
    assert_eq!(names(&c.get::<Composite>().unwrap().0), ["mail", "sms"]);
    assert_eq!(c.get::<dyn Notifier>().unwrap().name(), "composite");
    // After the build nothing is under creation: the container lists all three.
    assert_eq!(
        names(&c.get_all::<dyn Notifier>().unwrap()),
        ["mail", "composite", "sms"]
    );
}

/// Asks for a single `dyn Notifier`, which only it provides.
struct SelfOnly;
impl Notifier for SelfOnly {
    fn name(&self) -> &'static str {
        "self"
    }
}
impl Component for SelfOnly {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        r.get::<dyn Notifier>()?;
        Ok(SelfOnly)
    }
}

#[test]
fn single_get_of_own_trait_is_a_cycle() {
    let err = ContainerBuilder::new()
        .component::<SelfOnly>()
        .provides(|a: Arc<SelfOnly>| -> Arc<dyn Notifier> { a })
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Cycle {
            path: vec![name_of::<SelfOnly>(), name_of::<SelfOnly>()],
        })
    );
}

#[test]
fn provides_with_a_sized_key_is_invalid_at_build() {
    let err = ContainerBuilder::new()
        .instance(Sms)
        .provides(|_: Arc<Sms>| -> Arc<Mailer> { Arc::new(Mailer) })
        .build()
        .err();
    match err {
        Some(DiError::Invalid { message }) => {
            assert!(message.contains(type_name::<Mailer>()), "{message}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    // Providing the concrete type itself is a sized key, too.
    let err = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<Mailer> { a })
        .build()
        .err();
    assert!(matches!(err, Some(DiError::Invalid { .. })), "{err:?}");
}

#[test]
fn ambiguous_candidates_use_registration_order_not_order() {
    let err = ContainerBuilder::new()
        .instance(Mailer)
        .provides(|a: Arc<Mailer>| -> Arc<dyn Notifier> { a })
        .order(9)
        .instance(Sms)
        .provides(|a: Arc<Sms>| -> Arc<dyn Notifier> { a })
        .order(-9)
        .build()
        .unwrap()
        .get::<dyn Notifier>()
        .err();
    assert_eq!(
        err,
        Some(DiError::Ambiguous {
            requested: name_of::<dyn Notifier>(),
            candidates: vec![
                format!("instance {}", name_of::<Mailer>()),
                format!("instance {}", name_of::<Sms>()),
            ],
        })
    );
}

// --- fix round 2: only the requester is skipped ------------------------------------------------

/// Provides `dyn Notifier` and needs `Collector`.
struct Upstream;
impl Notifier for Upstream {
    fn name(&self) -> &'static str {
        "upstream"
    }
}
impl Component for Upstream {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        r.get::<Collector>()?;
        Ok(Upstream)
    }
}

/// Collects every `dyn Notifier`, including `Upstream`, which waits for it.
struct Collector;
impl Component for Collector {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        r.get_all::<dyn Notifier>()?;
        Ok(Collector)
    }
}

#[test]
fn get_all_through_an_ancestor_provider_is_a_cycle_in_both_orders() {
    let upstream_first = ContainerBuilder::new()
        .component::<Upstream>()
        .provides(|a: Arc<Upstream>| -> Arc<dyn Notifier> { a })
        .component::<Collector>()
        .build()
        .err();
    assert_eq!(
        upstream_first,
        Some(DiError::Cycle {
            path: vec![
                name_of::<Upstream>(),
                name_of::<Collector>(),
                name_of::<Upstream>(),
            ],
        })
    );

    let collector_first = ContainerBuilder::new()
        .component::<Collector>()
        .component::<Upstream>()
        .provides(|a: Arc<Upstream>| -> Arc<dyn Notifier> { a })
        .build()
        .err();
    assert_eq!(
        collector_first,
        Some(DiError::Cycle {
            path: vec![
                name_of::<Collector>(),
                name_of::<Upstream>(),
                name_of::<Collector>(),
            ],
        })
    );
}
