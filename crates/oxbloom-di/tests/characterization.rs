//! Pins the container behavior that must survive the v2 refactor (build order, sharing,
//! error shapes). These tests describe the CURRENT behavior and must keep passing.

use std::any::type_name;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oxbloom_di::{Component, Container, ContainerBuilder, DiError, ErrorSource, Resolver};

#[derive(Default)]
struct Log(Mutex<Vec<&'static str>>);

fn note(r: &Resolver, name: &'static str) -> Result<(), DiError> {
    r.get::<Log>()?.0.lock().unwrap().push(name);
    Ok(())
}

fn recorded(c: &Container) -> Vec<&'static str> {
    c.get::<Log>().unwrap().0.lock().unwrap().clone()
}

macro_rules! logged {
    ($name:ident $(, $dep:ident)*) => {
        struct $name;
        impl Component for $name {
            fn create(r: &Resolver) -> Result<Self, DiError> {
                $(r.get::<$dep>()?;)*
                note(r, stringify!($name))?;
                Ok($name)
            }
        }
    };
}

logged!(Base);
logged!(Mid, Base);
logged!(Side);
logged!(Top, Mid, Side);
logged!(Free);

#[test]
fn build_order_follows_registration_then_dependency_depth_first() {
    let c = ContainerBuilder::new()
        .instance(Log::default())
        .component::<Top>()
        .component::<Free>()
        .component::<Mid>()
        .component::<Base>()
        .component::<Side>()
        .build()
        .unwrap();
    // Top is first: its dependencies are built depth-first (Mid -> Base, then Side) before it;
    // the remaining entries follow in registration order.
    assert_eq!(recorded(&c), ["Base", "Mid", "Side", "Top", "Free"]);
}

static SHARED_BUILT: AtomicUsize = AtomicUsize::new(0);
struct Shared;
impl Component for Shared {
    fn create(_: &Resolver) -> Result<Self, DiError> {
        SHARED_BUILT.fetch_add(1, Ordering::SeqCst);
        Ok(Shared)
    }
}
struct Left(Arc<Shared>);
impl Component for Left {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Left(r.get()?))
    }
}
struct Right(Arc<Shared>);
impl Component for Right {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Right(r.get()?))
    }
}
struct Apex(Arc<Left>, Arc<Right>);
impl Component for Apex {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Apex(r.get()?, r.get()?))
    }
}

#[test]
fn diamond_builds_each_once() {
    let c = ContainerBuilder::new()
        .component::<Apex>()
        .component::<Left>()
        .component::<Right>()
        .component::<Shared>()
        .build()
        .unwrap();
    assert_eq!(SHARED_BUILT.load(Ordering::SeqCst), 1);
    let apex = c.get::<Apex>().unwrap();
    assert!(Arc::ptr_eq(&apex.0.0, &apex.1.0));
    assert!(Arc::ptr_eq(&apex.0.0, &c.get::<Shared>().unwrap()));
}

struct DupA;
impl Component for DupA {
    fn create(_: &Resolver) -> Result<Self, DiError> {
        Ok(DupA)
    }
}
struct DupB;

#[test]
fn duplicate_reports_first_in_registration_order() {
    // DupB is duplicated first in registration order (its second registration comes before
    // DupA's second one), so DupB is reported.
    let err = ContainerBuilder::new()
        .instance(DupA)
        .instance(DupB)
        .instance(DupB)
        .component::<DupA>()
        .build()
        .err()
        .unwrap();
    match err {
        DiError::Duplicate { type_name, .. } => assert_eq!(type_name, type_name_of::<DupB>()),
        other => panic!("expected Duplicate, got {other:?}"),
    }
}

fn type_name_of<T>() -> String {
    type_name::<T>().to_owned()
}

struct CycA(#[allow(dead_code)] Arc<CycB>);
struct CycB(#[allow(dead_code)] Arc<CycA>);
impl Component for CycA {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(CycA(r.get()?))
    }
}
impl Component for CycB {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(CycB(r.get()?))
    }
}

#[test]
fn cycle_path_a_b_a() {
    let err = ContainerBuilder::new()
        .component::<CycA>()
        .component::<CycB>()
        .build()
        .err()
        .unwrap();
    let (a, b) = (type_name_of::<CycA>(), type_name_of::<CycB>());
    assert_eq!(
        err,
        DiError::Cycle {
            path: vec![a.clone(), b, a]
        }
    );
}

struct Absent;
struct NeedsAbsent(#[allow(dead_code)] Arc<Absent>);
impl Component for NeedsAbsent {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(NeedsAbsent(r.get()?))
    }
}
struct NeedsNeedsAbsent(#[allow(dead_code)] Arc<NeedsAbsent>);
impl Component for NeedsNeedsAbsent {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(NeedsNeedsAbsent(r.get()?))
    }
}

#[test]
fn missing_required_by_chain() {
    let err = ContainerBuilder::new()
        .component::<NeedsNeedsAbsent>()
        .component::<NeedsAbsent>()
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err,
        DiError::Missing {
            requested: type_name_of::<Absent>(),
            required_by: vec![
                type_name_of::<NeedsNeedsAbsent>(),
                type_name_of::<NeedsAbsent>()
            ],
        }
    );
}

struct Flaky;

#[test]
fn factory_error_is_creation_naming_the_type() {
    let err = ContainerBuilder::new()
        .factory(|_| -> Result<Flaky, DiError> { Err(DiError::creation::<Flaky>("down")) })
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err,
        DiError::Creation {
            type_name: type_name_of::<Flaky>(),
            message: "down".into(),
            source: ErrorSource::none(),
        }
    );
}
