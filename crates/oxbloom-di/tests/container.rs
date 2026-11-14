use std::any::type_name;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oxbloom_di::{Component, ContainerBuilder, DiError, Resolver};

#[derive(Default)]
struct Log(Mutex<Vec<&'static str>>);

impl Log {
    fn push(r: &Resolver, name: &'static str) -> Result<(), DiError> {
        r.get::<Log>()?.0.lock().unwrap().push(name);
        Ok(())
    }
}

fn recorded(c: &oxbloom_di::Container) -> Vec<&'static str> {
    c.get::<Log>().unwrap().0.lock().unwrap().clone()
}

// --- build order -------------------------------------------------------

struct Leaf;
impl Component for Leaf {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Log::push(r, "Leaf")?;
        Ok(Leaf)
    }
}

struct Other;
impl Component for Other {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Log::push(r, "Other")?;
        Ok(Other)
    }
}

struct Top {
    _leaf: Arc<Leaf>,
}
impl Component for Top {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        let leaf = r.get::<Leaf>()?;
        Log::push(r, "Top")?;
        Ok(Top { _leaf: leaf })
    }
}

#[test]
fn dependencies_built_before_dependents() {
    let c = ContainerBuilder::new()
        .instance(Log::default())
        .component::<Top>() // dependent registered first
        .component::<Leaf>()
        .build()
        .unwrap();
    assert_eq!(recorded(&c), ["Leaf", "Top"]);
}

#[test]
fn registration_order_is_build_order_for_independents() {
    let c = ContainerBuilder::new()
        .instance(Log::default())
        .component::<Other>()
        .component::<Leaf>()
        .build()
        .unwrap();
    assert_eq!(recorded(&c), ["Other", "Leaf"]);
}

// --- diamond -----------------------------------------------------------

struct D;
impl Component for D {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Log::push(r, "D")?;
        Ok(D)
    }
}
struct B {
    d: Arc<D>,
}
impl Component for B {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(B { d: r.get()? })
    }
}
struct C {
    d: Arc<D>,
}
impl Component for C {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(C { d: r.get()? })
    }
}
struct A {
    b: Arc<B>,
    c: Arc<C>,
}
impl Component for A {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(A {
            b: r.get()?,
            c: r.get()?,
        })
    }
}

#[test]
fn diamond_builds_shared_dependency_once() {
    let c = ContainerBuilder::new()
        .instance(Log::default())
        .component::<A>()
        .component::<B>()
        .component::<C>()
        .component::<D>()
        .build()
        .unwrap();
    assert_eq!(recorded(&c).iter().filter(|n| **n == "D").count(), 1);
    let a = c.get::<A>().unwrap();
    let d = c.get::<D>().unwrap();
    assert!(Arc::ptr_eq(&a.b.d, &a.c.d));
    assert!(Arc::ptr_eq(&a.b.d, &d));
}

// --- missing -----------------------------------------------------------

struct NeverRegistered;
struct NeedsMissing {
    _m: Arc<NeverRegistered>,
}
impl Component for NeedsMissing {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(NeedsMissing { _m: r.get()? })
    }
}
struct NeedsNeedsMissing {
    _n: Arc<NeedsMissing>,
}
impl Component for NeedsNeedsMissing {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(NeedsNeedsMissing { _n: r.get()? })
    }
}

#[test]
fn missing_reports_chain() {
    let err = ContainerBuilder::new()
        .component::<NeedsMissing>()
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err,
        DiError::Missing {
            requested: type_name::<NeverRegistered>().into(),
            required_by: vec![type_name::<NeedsMissing>().into()],
        }
    );
    let err = ContainerBuilder::new()
        .component::<NeedsNeedsMissing>()
        .component::<NeedsMissing>()
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err,
        DiError::Missing {
            requested: type_name::<NeverRegistered>().into(),
            required_by: vec![
                type_name::<NeedsNeedsMissing>().into(),
                type_name::<NeedsMissing>().into()
            ],
        }
    );
}

#[test]
fn container_get_unregistered_is_missing_with_empty_chain() {
    let c = ContainerBuilder::new().build().unwrap();
    assert_eq!(
        c.get::<NeverRegistered>().err().unwrap(),
        DiError::Missing {
            requested: type_name::<NeverRegistered>().into(),
            required_by: vec![]
        }
    );
}

// --- cycles ------------------------------------------------------------

struct Cy1 {
    _o: Arc<Cy2>,
}
struct Cy2 {
    _o: Arc<Cy1>,
}
impl Component for Cy1 {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Cy1 { _o: r.get()? })
    }
}
impl Component for Cy2 {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Cy2 { _o: r.get()? })
    }
}
struct SelfDep {
    _s: Arc<SelfDep>,
}
impl Component for SelfDep {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(SelfDep { _s: r.get()? })
    }
}

#[test]
fn cycle_of_two() {
    let err = ContainerBuilder::new()
        .component::<Cy1>()
        .component::<Cy2>()
        .build()
        .err()
        .unwrap();
    let a = type_name::<Cy1>().to_owned();
    let b = type_name::<Cy2>().to_owned();
    assert_eq!(
        err,
        DiError::Cycle {
            path: vec![a.clone(), b, a]
        }
    );
}

#[test]
fn self_cycle() {
    let err = ContainerBuilder::new()
        .component::<SelfDep>()
        .build()
        .err()
        .unwrap();
    let n = type_name::<SelfDep>().to_owned();
    assert_eq!(
        err,
        DiError::Cycle {
            path: vec![n.clone(), n]
        }
    );
}

// --- duplicate ---------------------------------------------------------

static DUP_CREATED: AtomicUsize = AtomicUsize::new(0);
struct Dup;
impl Component for Dup {
    fn create(_: &Resolver) -> Result<Self, DiError> {
        DUP_CREATED.fetch_add(1, Ordering::SeqCst);
        Ok(Dup)
    }
}

#[test]
fn duplicate_component_and_instance() {
    let err = ContainerBuilder::new()
        .component::<Dup>()
        .instance(Dup)
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err,
        DiError::Duplicate {
            type_name: type_name::<Dup>().into(),
            sources: vec![
                format!("component {}", type_name::<Dup>()),
                format!("instance {}", type_name::<Dup>()),
            ],
        }
    );
    assert_eq!(
        DUP_CREATED.load(Ordering::SeqCst),
        0,
        "no provider may run before duplicate check"
    );
}

// --- instance / factory ------------------------------------------------

struct Uses {
    name: Arc<String>,
    n: Arc<u32>,
}
impl Component for Uses {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Uses {
            name: r.get()?,
            n: r.get()?,
        })
    }
}

#[test]
fn instance_and_factory_are_injectable() {
    let c = ContainerBuilder::new()
        .component::<Uses>() // before its dependencies
        .instance(String::from("hi"))
        .factory(|_r| Ok(7u32))
        .build()
        .unwrap();
    let u = c.get::<Uses>().unwrap();
    assert_eq!((u.name.as_str(), *u.n), ("hi", 7));
}

#[test]
fn factory_error_is_returned() {
    let err = ContainerBuilder::new()
        .factory::<u32, _>(|_r| Err(DiError::creation::<u32>("nope")))
        .build()
        .err()
        .unwrap();
    assert_eq!(err, DiError::creation::<u32>("nope"));
}

// --- misc --------------------------------------------------------------

#[test]
fn container_is_send_sync() {
    fn assert<T: Send + Sync>() {}
    assert::<oxbloom_di::Container>();
}

#[test]
fn zero_dependency_component_builds() {
    let c = ContainerBuilder::new()
        .instance(Log::default())
        .component::<Leaf>()
        .build()
        .unwrap();
    assert!(c.get::<Leaf>().is_ok());
}

#[test]
fn panicking_provider_propagates_out_of_build() {
    let result = std::panic::catch_unwind(|| {
        let _ = ContainerBuilder::new()
            .factory::<u32, _>(|_| panic!("provider exploded"))
            .build();
    });
    assert!(result.is_err(), "build() must not swallow provider panics");
}

// --- error caching -----------------------------------------------------

struct Fragile;

/// Errors seen by the requesters below, shared with the test through a clone.
#[derive(Default, Clone)]
struct Seen(Arc<Mutex<Vec<DiError>>>);

/// Asks for `Fragile`, records the error and carries on without it.
struct Requester<const N: u8>;
impl<const N: u8> Component for Requester<N> {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        if let Err(e) = r.get::<Fragile>() {
            r.get::<Seen>()?.0.lock().unwrap().push(e);
        }
        Ok(Requester)
    }
}

#[test]
fn failed_creation_is_cached_not_rerun() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let seen = Seen::default();
    let err = ContainerBuilder::new()
        .instance(seen.clone())
        .component::<Requester<1>>() // swallows the first error
        .component::<Requester<2>>() // asks again
        .factory(move |_| -> Result<Fragile, DiError> {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(DiError::creation::<Fragile>("unavailable"))
        })
        .build()
        .err()
        .unwrap();
    let expected = DiError::creation::<Fragile>("unavailable");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "provider must run once");
    assert_eq!(
        *seen.0.lock().unwrap(),
        [expected.clone(), expected.clone()]
    );
    assert_eq!(err, expected, "build reports the cached error of the entry");
}

// --- duplicate sources -------------------------------------------------

struct Svc;
impl Component for Svc {
    fn create(_: &Resolver) -> Result<Self, DiError> {
        Ok(Svc)
    }
}

#[test]
fn duplicate_names_both_sources() {
    let err = ContainerBuilder::new()
        .component::<Svc>()
        .instance(Svc)
        .build()
        .err()
        .unwrap();
    let n = type_name::<Svc>();
    assert_eq!(
        err,
        DiError::Duplicate {
            type_name: n.into(),
            sources: vec![format!("component {n}"), format!("instance {n}")],
        }
    );
}

#[test]
fn duplicate_lists_every_source() {
    let err = ContainerBuilder::new()
        .component::<Svc>()
        .instance(Svc)
        .factory(|_| Ok(Svc))
        .build()
        .err()
        .unwrap();
    let n = type_name::<Svc>();
    assert_eq!(
        err,
        DiError::Duplicate {
            type_name: n.into(),
            sources: vec![
                format!("component {n}"),
                format!("instance {n}"),
                format!("factory {n}")
            ],
        }
    );
}

// --- stack-relative errors are not cached -------------------------------

static SWALLOW_B_RUNS: AtomicUsize = AtomicUsize::new(0);

/// Asks for `SwallowB` and ignores the error.
struct SwallowA;
impl Component for SwallowA {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        let _ = r.get::<SwallowB>();
        Ok(SwallowA)
    }
}
/// Needs `SwallowA`: a cycle while `SwallowA` is under creation, fine afterwards.
struct SwallowB(#[allow(dead_code)] Arc<SwallowA>);
impl Component for SwallowB {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        SWALLOW_B_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(SwallowB(r.get()?))
    }
}

#[test]
fn swallowed_cycle_error_does_not_fail_the_build() {
    let c = ContainerBuilder::new()
        .component::<SwallowA>()
        .component::<SwallowB>()
        .build()
        .unwrap();
    assert!(c.get::<SwallowA>().is_ok());
    assert!(Arc::ptr_eq(
        &c.get::<SwallowB>().unwrap().0,
        &c.get::<SwallowA>().unwrap()
    ));
}

static CYCLE_RUNS: AtomicUsize = AtomicUsize::new(0);
struct CountedA;
impl Component for CountedA {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        let first = r.get::<CountedB>().err();
        assert!(matches!(first, Some(DiError::Cycle { .. })), "{first:?}");
        Ok(CountedA)
    }
}
struct CountedB(#[allow(dead_code)] Arc<CountedA>);
impl Component for CountedB {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        CYCLE_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(CountedB(r.get()?))
    }
}

#[test]
fn cycle_error_is_not_cached() {
    let c = ContainerBuilder::new()
        .component::<CountedA>()
        .component::<CountedB>()
        .build()
        .unwrap();
    // First run inside CountedA's creation failed with Cycle; the build loop ran it again.
    assert_eq!(CYCLE_RUNS.load(Ordering::SeqCst), 2);
    assert!(c.get::<CountedB>().is_ok());
}

struct Absent;
/// Fails with `Missing` (a stack-independent error), counted.
struct NeedsAbsent;
static NEEDS_ABSENT_RUNS: AtomicUsize = AtomicUsize::new(0);
impl Component for NeedsAbsent {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        NEEDS_ABSENT_RUNS.fetch_add(1, Ordering::SeqCst);
        r.get::<Absent>()?;
        Ok(NeedsAbsent)
    }
}
struct IgnoresNeedsAbsent;
impl Component for IgnoresNeedsAbsent {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        let _ = r.get::<NeedsAbsent>();
        Ok(IgnoresNeedsAbsent)
    }
}

#[test]
fn cached_creation_error_is_still_not_rerun() {
    let err = ContainerBuilder::new()
        .component::<IgnoresNeedsAbsent>()
        .component::<NeedsAbsent>()
        .build()
        .err()
        .unwrap();
    assert_eq!(NEEDS_ABSENT_RUNS.load(Ordering::SeqCst), 1);
    // The cached error keeps the chain of its first creation.
    assert_eq!(
        err,
        DiError::Missing {
            requested: type_name::<Absent>().into(),
            required_by: vec![
                type_name::<IgnoresNeedsAbsent>().into(),
                type_name::<NeedsAbsent>().into()
            ],
        }
    );
}
