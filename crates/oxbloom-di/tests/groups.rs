//! Groups: `Conditions` and `ContainerBuilder::group`, evaluated once per group at its first
//! kept registration, ANDed along nested groups.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use oxbloom_di::{ConditionOutcome, Conditions, ContainerBuilder, DiError, Resolver};

struct Foo;
struct NeedsFoo {
    _foo: Arc<Foo>,
}
struct Other;
struct Later;
struct Inner;
struct Outer;

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

/// `(registration, condition, matched)` of every record.
fn records(b: ContainerBuilder) -> Vec<(String, String, bool)> {
    let (_, report) = b.build_with_report();
    report
        .records()
        .iter()
        .map(|r| {
            (
                r.registration().to_owned(),
                r.condition().to_owned(),
                r.matched(),
            )
        })
        .collect()
}

fn never() -> Conditions {
    Conditions::new().condition("never", |_| ConditionOutcome::skipped("never"))
}

fn always() -> Conditions {
    Conditions::new().condition("always", |_| ConditionOutcome::matched())
}

/// A factory that counts its runs.
fn counted<T: Send + Sync + 'static>(
    b: ContainerBuilder,
    runs: &Arc<AtomicUsize>,
    make: fn() -> T,
) -> ContainerBuilder {
    let runs = Arc::clone(runs);
    b.factory(move |_: &Resolver| {
        runs.fetch_add(1, Ordering::SeqCst);
        Ok(make())
    })
}

#[test]
fn class_level_on_missing_bean_does_not_hide_siblings() {
    // The group's first registration provides `Foo`; the group is decided before it, so the
    // condition matches and both registrations are kept.
    let c = ContainerBuilder::new()
        .group(
            "FooConfig",
            Conditions::new().on_missing_bean::<Foo>(),
            |b| {
                b.instance(Foo).factory(|r: &Resolver| {
                    Ok(NeedsFoo {
                        _foo: r.get::<Foo>()?,
                    })
                })
            },
        )
        .build()
        .unwrap();
    assert!(c.get_opt::<Foo>().unwrap().is_some());
    assert!(c.get_opt::<NeedsFoo>().unwrap().is_some());
    let records = c.condition_report().records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].registration(), "FooConfig");
    assert!(records[0].matched());

    // A user `Foo` before the group makes it back off as a whole.
    let c = ContainerBuilder::new()
        .instance(Foo)
        .group(
            "FooConfig",
            Conditions::new().on_missing_bean::<Foo>(),
            |b| b.instance(Foo).instance(Other),
        )
        .build()
        .unwrap();
    assert!(c.get_opt::<Other>().unwrap().is_none());
}

#[test]
fn failing_group_skips_all_its_registrations_and_providers_never_run() {
    let runs = Arc::new(AtomicUsize::new(0));
    let c = ContainerBuilder::new()
        .group("Disabled", never(), |b| {
            let b = counted(b, &runs, || Foo);
            let b = counted(b, &runs, || Other).lazy();
            b.group("Nested", always(), |b| counted(b, &runs, || Inner))
        })
        .build()
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert!(c.get_opt::<Foo>().unwrap().is_none());
    assert!(c.get_opt::<Other>().unwrap().is_none());
    assert!(c.get_opt::<Inner>().unwrap().is_none());
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    // One record: the failing group; the nested group was never evaluated.
    let names: Vec<_> = c
        .condition_report()
        .records()
        .iter()
        .map(|r| (r.registration(), r.matched()))
        .collect();
    assert_eq!(names, [("Disabled", false)]);
}

#[test]
fn other_groups_unaffected() {
    let c = ContainerBuilder::new()
        .group("Off", never(), |b| b.instance(Foo))
        .group("On", always(), |b| b.instance(Other))
        .instance(Later)
        .build()
        .unwrap();
    assert!(c.get_opt::<Foo>().unwrap().is_none());
    assert!(c.get_opt::<Other>().unwrap().is_some());
    assert!(c.get_opt::<Later>().unwrap().is_some());
}

#[test]
fn nested_groups_and_together() {
    let build = |outer: Conditions, inner: Conditions| {
        ContainerBuilder::new()
            .group("Outer", outer, |b| {
                b.instance(Outer)
                    .group("Inner", inner, |b| b.instance(Inner))
            })
            .build()
            .unwrap()
    };
    let c = build(always(), always());
    assert!(c.get_opt::<Outer>().unwrap().is_some());
    assert!(c.get_opt::<Inner>().unwrap().is_some());

    let c = build(never(), always());
    assert!(c.get_opt::<Outer>().unwrap().is_none());
    assert!(c.get_opt::<Inner>().unwrap().is_none());

    // The outer group is decided first even when the inner registration is the first one.
    let r = records(ContainerBuilder::new().group("Outer", always(), |b| {
        b.group("Inner", always(), |b| b.instance(Inner))
            .instance(Outer)
    }));
    assert_eq!(
        r,
        [
            ("Outer".to_owned(), "always".to_owned(), true),
            ("Inner".to_owned(), "always".to_owned(), true),
        ]
    );

    // A failing outer group short-circuits: the inner group is not evaluated.
    let r = records(ContainerBuilder::new().group("Outer", never(), |b| {
        b.group("Inner", always(), |b| b.instance(Inner))
    }));
    assert_eq!(r, [("Outer".to_owned(), "never".to_owned(), false)]);
}

#[test]
fn nested_failing_inner_keeps_outer_siblings() {
    let c = ContainerBuilder::new()
        .group("Outer", always(), |b| {
            b.instance(Outer)
                .group("Inner", never(), |b| b.instance(Inner))
                .instance(Other)
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Outer>().unwrap().is_some());
    assert!(c.get_opt::<Inner>().unwrap().is_none());
    assert!(c.get_opt::<Other>().unwrap().is_some());
}

#[test]
fn empty_group_is_noop() {
    let c = ContainerBuilder::new()
        .instance(Foo)
        .group("Empty", never(), |b| b)
        .group("EmptyNested", never(), |b| b.group("Inner", never(), |b| b))
        .primary() // still applies to `Foo`
        .build()
        .unwrap();
    assert!(c.get_opt::<Foo>().unwrap().is_some());
    assert!(c.condition_report().records().is_empty());
}

#[test]
fn group_conditions_see_only_registrations_before_the_group() {
    // `Later` comes after the group: it does not count.
    let c = ContainerBuilder::new()
        .group("NeedsLater", Conditions::new().on_bean::<Later>(), |b| {
            b.instance(Other)
        })
        .instance(Later)
        .build()
        .unwrap();
    assert!(c.get_opt::<Other>().unwrap().is_none());

    // The group's own registrations do not count either, not even its first one.
    let c = ContainerBuilder::new()
        .group("NeedsFoo", Conditions::new().on_bean::<Foo>(), |b| {
            b.instance(Foo).instance(Other)
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Foo>().unwrap().is_none());
    assert!(c.get_opt::<Other>().unwrap().is_none());

    // A registration before the group counts.
    let c = ContainerBuilder::new()
        .instance(Foo)
        .group("NeedsFoo", Conditions::new().on_bean::<Foo>(), |b| {
            b.instance(Other)
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Other>().unwrap().is_some());

    // A nested group sees the kept registrations of its enclosing group before it.
    let c = ContainerBuilder::new()
        .group("Outer", always(), |b| {
            b.instance(Foo)
                .group("Inner", Conditions::new().on_bean::<Foo>(), |b| {
                    b.instance(Inner)
                })
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Inner>().unwrap().is_some());
}

#[test]
fn entry_conditions_inside_group_are_anded_with_group() {
    let c = ContainerBuilder::new()
        .instance(Foo)
        .group("On", always(), |b| {
            b.instance(Other)
                .on_bean::<Foo>()
                .instance(Inner)
                .on_missing_bean::<Foo>()
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Other>().unwrap().is_some());
    assert!(c.get_opt::<Inner>().unwrap().is_none());

    // A failing group: the entry's own conditions are not evaluated (no record).
    let r = records(
        ContainerBuilder::new()
            .instance(Foo)
            .group("Off", never(), |b| b.instance(Other).on_bean::<Foo>()),
    );
    assert_eq!(r, [("Off".to_owned(), "never".to_owned(), false)]);

    // A matching group: the group's record comes first, then the entry's.
    let r = records(
        ContainerBuilder::new()
            .instance(Foo)
            .group("On", always(), |b| b.instance(Other).on_bean::<Foo>()),
    );
    assert_eq!(r.len(), 2);
    assert_eq!(r[0], ("On".to_owned(), "always".to_owned(), true));
    assert!(r[1].0.starts_with("instance ") && r[1].1 == "on_bean" && r[1].2);
}

#[test]
fn group_record_uses_group_name() {
    let (_, report) = ContainerBuilder::new()
        .group(
            "CacheAutoConfiguration",
            Conditions::new()
                .on_property("cache.enabled")
                .on_bean::<Foo>(),
            |b| b.instance(Other),
        )
        .build_with_report();
    // Short-circuit: only the first, failing condition has a record.
    assert_eq!(report.records().len(), 1);
    let r = &report.records()[0];
    assert_eq!(r.registration(), "CacheAutoConfiguration");
    assert_eq!(r.condition(), "on_property");
    assert!(!r.matched());
    assert_eq!(r.reason(), "property \"cache.enabled\" is not set");
    assert!(
        report.to_string().contains(
            "  CacheAutoConfiguration: on_property - property \"cache.enabled\" is not set"
        )
    );

    // The context of a group condition carries the group name as registration.
    let c = ContainerBuilder::new()
        .group(
            "Named",
            Conditions::new().condition("name", |cx| {
                if cx.registration() == "Named" {
                    ConditionOutcome::matched()
                } else {
                    ConditionOutcome::skipped("wrong name")
                }
            }),
            |b| b.instance(Other),
        )
        .build()
        .unwrap();
    assert!(c.get_opt::<Other>().unwrap().is_some());
}

#[test]
fn group_with_profile_skipped_first_entry_still_decided_once() {
    let evaluations = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&evaluations);
    let c = ContainerBuilder::new()
        .group(
            "Counted",
            Conditions::new()
                .condition("count", move |_| {
                    seen.fetch_add(1, Ordering::SeqCst);
                    ConditionOutcome::matched()
                })
                .on_missing_bean::<Foo>(),
            |b| {
                b.instance(Foo)
                    .profile("prod") // not active: skipped, not a visit
                    .instance(Other)
                    .instance(Inner)
            },
        )
        .build()
        .unwrap();
    assert_eq!(evaluations.load(Ordering::SeqCst), 1);
    assert!(c.get_opt::<Foo>().unwrap().is_none());
    assert!(c.get_opt::<Other>().unwrap().is_some());
    assert!(c.get_opt::<Inner>().unwrap().is_some());
    assert_eq!(c.condition_report().records().len(), 2);

    // Only profile-skipped registrations: the group is never decided, no record.
    let r = records(
        ContainerBuilder::new().group("AllProd", never(), |b| b.instance(Foo).profile("prod")),
    );
    assert!(r.is_empty());
}

#[test]
fn modifier_after_group_applies_to_last_registration_of_group() {
    let c = ContainerBuilder::new()
        .instance(Disk)
        .provides(|s: Arc<Disk>| -> Arc<dyn Store> { s })
        .group("Stores", always(), |b| {
            b.instance(Memory)
                .provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
        })
        .primary()
        .build()
        .unwrap();
    assert_eq!(c.get::<dyn Store>().unwrap().kind(), "memory");

    // A condition after the group applies to that registration only, ANDed with the group.
    let c = ContainerBuilder::new()
        .group("Two", always(), |b| b.instance(Foo).instance(Other))
        .on_bean::<Later>()
        .build()
        .unwrap();
    assert!(c.get_opt::<Foo>().unwrap().is_some());
    assert!(c.get_opt::<Other>().unwrap().is_none());

    // The registration after the group is outside it.
    let c = ContainerBuilder::new()
        .group("Off", never(), |b| b.instance(Foo))
        .instance(Other)
        .build()
        .unwrap();
    assert!(c.get_opt::<Other>().unwrap().is_some());
}

#[test]
fn conditions_methods_mirror_the_builder() {
    use oxbloom_di::Environment;
    struct Props;
    impl Environment for Props {
        fn property(&self, key: &str) -> Option<String> {
            (key == "mode").then(|| "fast".to_owned())
        }
    }
    let build = |conditions: Conditions| {
        ContainerBuilder::new()
            .environment(Props)
            .instance(Foo)
            .group("G", conditions, |b| b.instance(Other))
            .build()
            .unwrap()
            .get_opt::<Other>()
            .unwrap()
            .is_some()
    };
    assert!(build(Conditions::new()));
    assert!(build(Conditions::default()));
    assert!(build(Conditions::new().on_bean::<Foo>()));
    assert!(!build(Conditions::new().on_bean::<Later>()));
    assert!(build(Conditions::new().on_missing_bean::<Later>()));
    assert!(!build(Conditions::new().on_missing_bean::<Foo>()));
    assert!(build(Conditions::new().on_property("mode")));
    assert!(!build(Conditions::new().on_property("other")));
    assert!(build(Conditions::new().on_property_value("mode", "fast")));
    assert!(!build(Conditions::new().on_property_value("mode", "fas")));
    assert!(build(
        Conditions::new().on_property_value_or_missing("other", "x")
    ));
    assert!(!build(
        Conditions::new().on_property_value_or_missing("mode", "slow")
    ));

    let names: Vec<_> = {
        let (_, report) = ContainerBuilder::new()
            .group(
                "G",
                Conditions::new()
                    .on_missing_bean::<Foo>()
                    .on_property_value_or_missing("a", "b")
                    .on_property_value("a", "b"),
                |b| b.instance(Other),
            )
            .build_with_report();
        report
            .records()
            .iter()
            .map(|r| r.condition().to_owned())
            .collect()
    };
    assert_eq!(
        names,
        [
            "on_missing_bean",
            "on_property_value_or_missing",
            "on_property_value"
        ]
    );
}

#[test]
fn skipped_group_registrations_are_invisible() {
    // No `Duplicate` from a skipped group, and a dependant gets `Missing`.
    let c = ContainerBuilder::new()
        .instance(Foo)
        .group("Off", never(), |b| b.instance(Foo))
        .build();
    assert!(c.is_ok());
    let err = ContainerBuilder::new()
        .group("Off", never(), |b| b.instance(Foo))
        .factory(|r: &Resolver| {
            Ok(NeedsFoo {
                _foo: r.get::<Foo>()?,
            })
        })
        .build()
        .err();
    assert!(matches!(err, Some(DiError::Missing { .. })));
}

#[test]
fn many_sibling_groups_stay_linear() {
    let mut b = ContainerBuilder::new().instance(Foo);
    for i in 0..1000 {
        let conditions = if i % 2 == 0 {
            Conditions::new().on_bean::<Foo>()
        } else {
            Conditions::new().on_missing_bean::<Foo>()
        };
        b = b.group("Sibling", conditions, |b| b.instance(Other).instance(Inner));
    }
    let start = Instant::now();
    let (_, report) = b.build_with_report();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_eq!(report.records().len(), 1000);
    assert_eq!(report.records().iter().filter(|r| r.matched()).count(), 500);
}

fn nest(b: ContainerBuilder, depth: usize) -> ContainerBuilder {
    if depth == 0 {
        return b;
    }
    b.group("Level", always(), move |b| {
        nest(b.instance(Other), depth - 1)
    })
}

#[test]
fn deeply_nested_groups_stay_linear() {
    let b = nest(ContainerBuilder::new(), 200);
    let start = Instant::now();
    let (_, report) = b.build_with_report();
    assert!(start.elapsed() < Duration::from_secs(5));
    // Every level is decided exactly once.
    assert_eq!(report.records().len(), 200);

    // A failing outermost level: one record, nothing evaluated below it.
    let b = ContainerBuilder::new().group("Top", never(), |b| nest(b, 200));
    let (_, report) = b.build_with_report();
    assert_eq!(report.records().len(), 1);
}

struct A;
struct B;
struct C;
struct D;

#[test]
fn replacement_builder_inside_group_does_not_panic() {
    // `f` returns a different builder with groups of its own: the returned builder is used as
    // it is (its own groups intact), the outer group has no effect on it, and nothing panics.
    let c = ContainerBuilder::new()
        .instance(A)
        .instance(B)
        .group("G", always(), |_| {
            ContainerBuilder::new()
                .group("X", always(), |b| b.instance(C))
                .group("Y", never(), |b| b.instance(D))
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<A>().unwrap().is_none());
    assert!(c.get_opt::<C>().unwrap().is_some());
    assert!(c.get_opt::<D>().unwrap().is_none());
    let names: Vec<_> = c
        .condition_report()
        .records()
        .iter()
        .map(|r| (r.registration().to_owned(), r.matched()))
        .collect();
    assert_eq!(names, [("X".to_owned(), true), ("Y".to_owned(), false)]);

    // A replacement builder without groups works too.
    let c = ContainerBuilder::new()
        .instance(A)
        .group("G", never(), |_| ContainerBuilder::new().instance(C))
        .build()
        .unwrap();
    assert!(c.get_opt::<C>().unwrap().is_some());
}

#[test]
fn modifier_at_start_of_group_is_invalid_and_does_not_touch_the_registration_before() {
    let err = ContainerBuilder::new()
        .instance(A)
        .group("G", always(), |b| b.on_bean::<D>().instance(B))
        .build()
        .err();
    match err {
        Some(DiError::Invalid { message }) => {
            assert_eq!(message, "`on_bean` needs a registration before it")
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    for (name, f) in [
        ("primary", (|b: ContainerBuilder| b.primary()) as fn(_) -> _),
        ("lazy", |b| b.lazy()),
        ("order", |b| b.order(1)),
        ("profile", |b| b.profile("dev")),
        ("provides", |b| {
            b.provides(|m: Arc<Memory>| -> Arc<dyn Store> { m })
        }),
        ("pre_destroy", |b| b.pre_destroy(|_: &Memory| {})),
    ] {
        let err = ContainerBuilder::new()
            .instance(Memory)
            .group("G", always(), move |b| f(b).instance(B))
            .build()
            .err();
        assert!(
            matches!(&err, Some(DiError::Invalid { message }) if message.contains(name)),
            "{name}: {err:?}"
        );
    }
    // In a nested group the barrier is the nested group's start.
    let err = ContainerBuilder::new()
        .group("Outer", always(), |b| {
            b.instance(A)
                .group("Inner", always(), |b| b.lazy().instance(B))
        })
        .build()
        .err();
    assert!(matches!(err, Some(DiError::Invalid { .. })));
    // After a nested group closes, modifiers apply to the nested group's last registration.
    let c = ContainerBuilder::new()
        .group("Outer", always(), |b| {
            b.instance(A)
                .group("Inner", always(), |b| b.instance(B))
                .on_bean::<D>()
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<A>().unwrap().is_some());
    assert!(c.get_opt::<B>().unwrap().is_none());
}

#[test]
fn empty_property_key_in_group_conditions_is_invalid() {
    type Make = fn() -> Conditions;
    let cases: [(&str, Make); 3] = [
        ("on_property", || Conditions::new().on_property("")),
        ("on_property_value", || {
            Conditions::new().on_property_value("", "zq-val-5521")
        }),
        ("on_property_value_or_missing", || {
            Conditions::new().on_property_value_or_missing("", "zq-val-5521")
        }),
    ];
    for (name, conditions) in cases {
        let result = ContainerBuilder::new()
            .group("defaults", conditions(), |b| b.instance(Foo))
            .build();
        match result {
            Err(DiError::Invalid { message }) => {
                assert_eq!(message, format!("`{name}` needs a non-empty key"));
                assert!(!message.contains("zq-val-5521"));
            }
            Err(other) => panic!("{name}: unexpected error {other}"),
            Ok(_) => panic!("{name}: expected Invalid"),
        }
    }
    // A valid key is still fine.
    let c = ContainerBuilder::new()
        .group(
            "defaults",
            Conditions::new().on_property_value_or_missing("a", "b"),
            |b| b.instance(Foo),
        )
        .build()
        .unwrap();
    assert!(c.get_opt::<Foo>().unwrap().is_some());
}
