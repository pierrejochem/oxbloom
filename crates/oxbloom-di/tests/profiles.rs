//! Profile conditions: `active_profiles`, `profile` (any-of, `"!name"`) and `profile_not`.

use std::any::type_name;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxbloom_di::{ContainerBuilder, DiError, Resolver};

trait Store: Send + Sync {
    fn kind(&self) -> &'static str;
}

struct Memory;
impl Store for Memory {
    fn kind(&self) -> &'static str {
        "memory"
    }
}

struct Disk;
impl Store for Disk {
    fn kind(&self) -> &'static str {
        "disk"
    }
}

/// A `u8` instance under `cond` (applied to the registration), built with `active` profiles.
fn kept(active: &[&str], cond: fn(ContainerBuilder) -> ContainerBuilder) -> bool {
    let b = cond(
        ContainerBuilder::new()
            .active_profiles(active.iter().copied())
            .instance(7u8),
    );
    b.build().unwrap().get_opt::<u8>().unwrap().is_some()
}

fn kinds(all: &[Arc<dyn Store>]) -> Vec<&'static str> {
    all.iter().map(|s| s.kind()).collect()
}

#[test]
fn entry_without_condition_always_kept() {
    assert!(kept(&[], |b| b));
    assert!(kept(&["dev"], |b| b));
    assert!(kept(&["dev", "prod"], |b| b));
}

#[test]
fn any_of_profiles() {
    let cond = |b: ContainerBuilder| b.profile("dev").profile("test");
    assert!(kept(&["dev"], cond));
    assert!(kept(&["test"], cond));
    assert!(kept(&["prod", "test"], cond));
    assert!(!kept(&["prod"], cond));
    // Names match exactly.
    assert!(!kept(&["Dev"], cond));
}

#[test]
fn negation_profile() {
    for cond in [
        (|b: ContainerBuilder| b.profile_not("prod")) as fn(_) -> _,
        |b: ContainerBuilder| b.profile("!prod"),
    ] {
        assert!(kept(&[], cond));
        assert!(kept(&["dev"], cond));
        assert!(!kept(&["prod"], cond));
        assert!(!kept(&["dev", "prod"], cond));
    }
    // Conditions are any-of, negations included: dev active OR prod inactive.
    let mixed = |b: ContainerBuilder| b.profile("dev").profile_not("prod");
    assert!(kept(&[], mixed));
    assert!(kept(&["dev", "prod"], mixed));
    assert!(!kept(&["prod"], mixed));
}

#[test]
fn no_active_profiles() {
    // Without `.active_profiles(..)` nothing is active.
    let c = ContainerBuilder::new()
        .instance(1u8)
        .profile("dev")
        .instance(2u16)
        .profile_not("dev")
        .build()
        .unwrap();
    assert!(c.get_opt::<u8>().unwrap().is_none());
    assert_eq!(*c.get::<u16>().unwrap(), 2);
    // An empty list is the same.
    assert!(!kept(&[], |b| b.profile("dev")));
}

#[test]
fn active_profiles_called_again_replaces_the_active_list() {
    let c = ContainerBuilder::new()
        .active_profiles(["dev"])
        .active_profiles(["prod"])
        .instance(1u8)
        .profile("dev")
        .instance(2u16)
        .profile("prod")
        .build()
        .unwrap();
    assert!(c.get_opt::<u8>().unwrap().is_none());
    assert!(c.get_opt::<u16>().unwrap().is_some());
}

#[test]
fn active_profiles_accepts_strings_and_may_follow_registrations() {
    let active: Vec<String> = vec!["dev".to_owned()];
    let c = ContainerBuilder::new()
        .instance(1u8)
        .profile("dev")
        .active_profiles(active)
        .build()
        .unwrap();
    assert_eq!(*c.get::<u8>().unwrap(), 1);
}

#[test]
fn skipped_entry_is_not_a_candidate_and_dependency_on_it_is_missing() {
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let b = || {
        ContainerBuilder::new()
            .active_profiles(["prod"])
            .factory(|_: &Resolver| {
                RUNS.fetch_add(1, Ordering::SeqCst);
                Ok(Memory)
            })
            .provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
            .profile("dev")
            .instance(Disk)
            .provides(|s: Arc<Disk>| -> Arc<dyn Store> { s })
    };
    // Two providers of `dyn Store`, one skipped: not ambiguous, and the skipped one never runs.
    let c = b().build().unwrap();
    assert_eq!(c.get::<dyn Store>().unwrap().kind(), "disk");
    assert!(matches!(c.get::<Memory>(), Err(DiError::Missing { .. })));
    assert_eq!(RUNS.load(Ordering::SeqCst), 0);

    // A kept entry that depends on a skipped one fails the build with `Missing`.
    struct Cache;
    let err = b()
        .factory(|r: &Resolver| r.get::<Memory>().map(|_| Cache))
        .build()
        .err()
        .unwrap();
    match err {
        DiError::Missing {
            requested,
            required_by,
        } => {
            assert_eq!(requested, type_name::<Memory>());
            assert_eq!(required_by, [type_name::<Cache>()]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(RUNS.load(Ordering::SeqCst), 0);
}

#[test]
fn duplicate_not_raised_when_one_is_skipped() {
    let b = |active: &[&'static str]| {
        ContainerBuilder::new()
            .active_profiles(active.to_vec())
            .instance(1u8)
            .profile("dev")
            .instance(2u8)
            .profile_not("dev")
    };
    assert_eq!(*b(&["dev"]).build().unwrap().get::<u8>().unwrap(), 1);
    assert_eq!(*b(&[]).build().unwrap().get::<u8>().unwrap(), 2);

    // Both kept: `Duplicate` as before.
    let err = ContainerBuilder::new()
        .active_profiles(["dev"])
        .instance(1u8)
        .profile("dev")
        .instance(2u8)
        .build()
        .err()
        .unwrap();
    assert!(matches!(err, DiError::Duplicate { .. }), "{err:?}");
}

#[test]
fn get_all_ignores_skipped_entries() {
    // The only provider is skipped: empty.
    let c = ContainerBuilder::new()
        .instance(Memory)
        .provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
        .profile("dev")
        .build()
        .unwrap();
    assert!(c.get_all::<dyn Store>().unwrap().is_empty());
    assert!(c.get_opt::<dyn Store>().unwrap().is_none());

    // Some skipped: the kept ones in order.
    let c = ContainerBuilder::new()
        .active_profiles(["dev"])
        .instance(Memory)
        .provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
        .order(5)
        .profile("dev")
        .instance(Disk)
        .provides(|s: Arc<Disk>| -> Arc<dyn Store> { s })
        .profile("!dev")
        .build()
        .unwrap();
    assert_eq!(kinds(&c.get_all::<dyn Store>().unwrap()), ["memory"]);

    // A resolver collection inside a provider skips them too.
    struct Count(usize);
    let c = ContainerBuilder::new()
        .instance(Memory)
        .provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
        .profile("dev")
        .instance(Disk)
        .provides(|s: Arc<Disk>| -> Arc<dyn Store> { s })
        .factory(|r: &Resolver| Ok(Count(r.get_all::<dyn Store>()?.len())))
        .build()
        .unwrap();
    assert_eq!(c.get::<Count>().unwrap().0, 1);
}

#[test]
fn profile_modifier_before_registration_is_invalid() {
    let err = ContainerBuilder::new()
        .profile("dev")
        .build()
        .err()
        .unwrap();
    assert!(matches!(err, DiError::Invalid { .. }), "{err:?}");
    let err = ContainerBuilder::new()
        .profile_not("dev")
        .instance(1u8)
        .build()
        .err()
        .unwrap();
    assert!(matches!(err, DiError::Invalid { .. }), "{err:?}");
    // The message names the modifier, not a profile name.
    let DiError::Invalid { message } = err else {
        unreachable!()
    };
    assert!(message.contains("profile_not"), "{message}");
    assert!(!message.contains("dev"), "{message}");

    // `active_profiles` is not a modifier: any position is fine.
    assert!(
        ContainerBuilder::new()
            .active_profiles(["dev"])
            .build()
            .is_ok()
    );
}

#[test]
fn modifiers_on_a_skipped_entry_are_still_checked() {
    // A misused modifier after a skipped registration is still `Invalid`.
    let err = ContainerBuilder::new()
        .instance(Memory)
        .profile("dev")
        .provides(|s: Arc<Disk>| -> Arc<dyn Store> { s })
        .build()
        .err()
        .unwrap();
    assert!(matches!(err, DiError::Invalid { .. }), "{err:?}");
}

#[test]
fn empty_profile_name_is_invalid() {
    let message = |b: ContainerBuilder| match b.build().err() {
        Some(DiError::Invalid { message }) => message,
        other => panic!("expected Invalid, got {other:?}"),
    };
    assert_eq!(
        message(ContainerBuilder::new().instance(1u8).profile("")),
        "`profile` needs a name"
    );
    assert_eq!(
        message(ContainerBuilder::new().instance(1u8).profile("!")),
        "`profile` needs a name"
    );
    assert_eq!(
        message(ContainerBuilder::new().instance(1u8).profile_not("")),
        "`profile_not` needs a name"
    );
    // `profile_not` takes its name literally, so "!" is a (strange but) non-empty name.
    assert!(
        ContainerBuilder::new()
            .instance(1u8)
            .profile_not("!")
            .build()
            .is_ok()
    );
}

#[test]
fn hooks_on_a_skipped_registration_never_run() {
    let runs = Arc::new(AtomicUsize::new(0));
    let (pc, pd) = (Arc::clone(&runs), Arc::clone(&runs));
    let c = ContainerBuilder::new()
        .instance(Memory)
        .profile("prod")
        .post_construct(move |_: &Memory| {
            pc.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .pre_destroy(move |_: &Memory| {
            pd.fetch_add(1, Ordering::SeqCst);
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Memory>().unwrap().is_none());
    assert_eq!(c.shutdown(), oxbloom_di::ShutdownReport::default());
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}
