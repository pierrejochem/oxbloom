//! Entry conditions: `condition`, `on_bean`, `on_missing_bean`, `on_property*`, the
//! `Environment`, the condition report and `build_with_report`.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxbloom_di::{ConditionOutcome, ContainerBuilder, DiError, Environment, Resolver};

struct Server(&'static str);
struct Client;
struct Later;

trait Mailer: Send + Sync {
    fn kind(&self) -> &'static str;
}
struct Smtp;
impl Mailer for Smtp {
    fn kind(&self) -> &'static str {
        "smtp"
    }
}
struct Console;
impl Mailer for Console {
    fn kind(&self) -> &'static str {
        "console"
    }
}

/// A fixed map of properties.
struct Props(HashMap<String, String>);

impl Props {
    fn of(pairs: &[(&str, &str)]) -> Self {
        Props(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }
}

impl Environment for Props {
    fn property(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// `(registration, condition, matched, reason)` of every record.
fn records(b: ContainerBuilder) -> Vec<(String, String, bool, String)> {
    let (_, report) = b.build_with_report();
    report
        .records()
        .iter()
        .map(|r| {
            (
                r.registration().to_owned(),
                r.condition().to_owned(),
                r.matched(),
                r.reason().to_owned(),
            )
        })
        .collect()
}

#[test]
fn user_bean_first_makes_default_skip() {
    let c = ContainerBuilder::new()
        .instance(Server("user"))
        .factory(|_: &Resolver| Ok(Server("default")))
        .on_missing_bean::<Server>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Server>().unwrap().0, "user");
}

#[test]
fn default_first_then_user_bean_is_unaffected() {
    // The condition sees only earlier registrations: the default is kept, so the user bean
    // registered after it makes the key a duplicate.
    let result = ContainerBuilder::new()
        .factory(|_: &Resolver| Ok(Server("default")))
        .on_missing_bean::<Server>()
        .instance(Server("user"))
        .build();
    assert!(matches!(result, Err(DiError::Duplicate { .. })));
}

#[test]
fn on_bean_requires_earlier_provider() {
    let c = ContainerBuilder::new()
        .instance(Server("s"))
        .instance(Client)
        .on_bean::<Server>()
        .build()
        .unwrap();
    assert!(c.get_opt::<Client>().unwrap().is_some());

    let c = ContainerBuilder::new()
        .instance(Client)
        .on_bean::<Server>()
        .build()
        .unwrap();
    assert!(c.get_opt::<Client>().unwrap().is_none());
}

#[test]
fn on_bean_for_later_registration_does_not_match_and_report_says_so() {
    let b = ContainerBuilder::new()
        .instance(Client)
        .on_bean::<Later>()
        .instance(Later);
    let recs = records(b);
    assert_eq!(recs.len(), 1);
    let (registration, condition, matched, reason) = &recs[0];
    assert!(registration.contains("Client"), "{registration}");
    assert_eq!(condition, "on_bean");
    assert!(!matched);
    assert!(
        reason.starts_with("no earlier registration provides `"),
        "{reason}"
    );
    assert!(reason.contains("Later"), "{reason}");

    let c = ContainerBuilder::new()
        .instance(Client)
        .on_bean::<Later>()
        .instance(Later)
        .build()
        .unwrap();
    assert!(c.get_opt::<Client>().unwrap().is_none());
    assert!(c.get_opt::<Later>().unwrap().is_some());
}

#[test]
fn conditions_and_together() {
    let both = ContainerBuilder::new()
        .instance(Server("s"))
        .instance(Client)
        .on_bean::<Server>()
        .on_missing_bean::<Later>()
        .build()
        .unwrap();
    assert!(both.get_opt::<Client>().unwrap().is_some());

    let one_fails = ContainerBuilder::new()
        .instance(Server("s"))
        .instance(Client)
        .on_bean::<Server>()
        .on_bean::<Later>()
        .build()
        .unwrap();
    assert!(one_fails.get_opt::<Client>().unwrap().is_none());
}

#[test]
fn short_circuit_records_only_up_to_first_failure() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let b = ContainerBuilder::new()
        .instance(Client)
        .condition("first", |_| ConditionOutcome::matched())
        .condition("second", |_| ConditionOutcome::skipped("second says no"))
        .condition("third", move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            ConditionOutcome::matched()
        });
    let recs = records(b);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let summary: Vec<_> = recs
        .iter()
        .map(|(_, c, m, r)| (c.as_str(), *m, r.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("first", true, "matched"),
            ("second", false, "second says no")
        ]
    );
}

#[test]
fn trait_provision_counts() {
    let c = ContainerBuilder::new()
        .instance(Smtp)
        .provides(|m: Arc<Smtp>| -> Arc<dyn Mailer> { m })
        .instance(Console)
        .provides(|m: Arc<Console>| -> Arc<dyn Mailer> { m })
        .on_missing_bean::<dyn Mailer>()
        .build()
        .unwrap();
    assert_eq!(c.get::<dyn Mailer>().unwrap().kind(), "smtp");
    assert!(c.get_opt::<Console>().unwrap().is_none());

    let c = ContainerBuilder::new()
        .instance(Smtp)
        .provides(|m: Arc<Smtp>| -> Arc<dyn Mailer> { m })
        .instance(Client)
        .on_bean::<dyn Mailer>()
        .build()
        .unwrap();
    assert!(c.get_opt::<Client>().unwrap().is_some());
}

#[test]
fn skipped_entry_is_invisible() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    // No `Duplicate`: the second `Server` is skipped, and its provider never runs.
    let c = ContainerBuilder::new()
        .instance(Server("user"))
        .factory(move |_: &Resolver| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Server("default"))
        })
        .on_missing_bean::<Server>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Server>().unwrap().0, "user");
    assert_eq!(c.get_all::<Server>().unwrap().len(), 1);
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    // A dependant of a skipped registration gets `Missing`.
    struct Cache;
    struct Service {
        _cache: Arc<Cache>,
    }
    let result = ContainerBuilder::new()
        .factory(|_: &Resolver| Ok(Cache))
        .on_bean::<Later>()
        .factory(|r: &Resolver| {
            Ok(Service {
                _cache: r.get::<Cache>()?,
            })
        })
        .build();
    assert!(matches!(result, Err(DiError::Missing { .. })));
}

#[test]
fn profile_skipped_entry_does_not_count_for_on_bean() {
    let c = ContainerBuilder::new()
        .instance(Server("prod"))
        .profile("prod")
        .instance(Client)
        .on_bean::<Server>()
        .build()
        .unwrap();
    assert!(c.get_opt::<Client>().unwrap().is_none());

    let c = ContainerBuilder::new()
        .instance(Server("prod"))
        .profile("prod")
        .instance(Server("default"))
        .on_missing_bean::<Server>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Server>().unwrap().0, "default");

    // A profile-skipped registration has no condition record.
    let recs = records(
        ContainerBuilder::new()
            .instance(Server("prod"))
            .profile("prod")
            .on_missing_bean::<Later>(),
    );
    assert!(recs.is_empty());
}

#[test]
fn condition_before_registration_is_invalid() {
    type Modifier = fn(ContainerBuilder) -> ContainerBuilder;
    let cases: [(&str, Modifier); 6] = [
        ("condition", |b| {
            b.condition("c", |_| ConditionOutcome::matched())
        }),
        ("on_bean", |b| b.on_bean::<Server>()),
        ("on_missing_bean", |b| b.on_missing_bean::<Server>()),
        ("on_property", |b| b.on_property("a.b")),
        ("on_property_value", |b| b.on_property_value("a.b", "x")),
        ("on_property_value_or_missing", |b| {
            b.on_property_value_or_missing("a.b", "x")
        }),
    ];
    for (name, modifier) in cases {
        let (result, report) = modifier(ContainerBuilder::new())
            .instance(Client)
            .build_with_report();
        match result {
            Err(DiError::Invalid { message }) => {
                assert_eq!(message, format!("`{name}` needs a registration before it"))
            }
            Err(other) => panic!("{name}: unexpected error {other}"),
            Ok(_) => panic!("{name}: expected Invalid"),
        }
        assert!(report.records().is_empty());
    }

    // The first misuse wins.
    let err = ContainerBuilder::new()
        .on_missing_bean::<Server>()
        .primary()
        .build()
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "invalid registration: `on_missing_bean` needs a registration before it"
    );
}

#[test]
fn on_property_present_and_absent() {
    let kept = |props: &[(&str, &str)]| {
        ContainerBuilder::new()
            .environment(Props::of(props))
            .instance(Client)
            .on_property("feature.enabled")
            .build()
            .unwrap()
            .get_opt::<Client>()
            .unwrap()
            .is_some()
    };
    assert!(kept(&[("feature.enabled", "false")])); // presence only
    assert!(!kept(&[("feature.other", "true")]));

    let recs = records(
        ContainerBuilder::new()
            .environment(Props::of(&[]))
            .instance(Client)
            .on_property("feature.enabled"),
    );
    assert_eq!(recs[0].1, "on_property");
    assert_eq!(recs[0].3, "property \"feature.enabled\" is not set");
}

#[test]
fn on_property_value_exact_match_and_substring_does_not_match() {
    let kept = |value: &str| {
        ContainerBuilder::new()
            .environment(Props::of(&[("cache.kind", value)]))
            .instance(Client)
            .on_property_value("cache.kind", "redis")
            .build()
            .unwrap()
            .get_opt::<Client>()
            .unwrap()
            .is_some()
    };
    assert!(kept("redis"));
    assert!(!kept("redis-cluster"));
    assert!(!kept("Redis"));
    assert!(!kept("redi"));

    let absent = ContainerBuilder::new()
        .instance(Client)
        .on_property_value("cache.kind", "redis")
        .build()
        .unwrap();
    assert!(absent.get_opt::<Client>().unwrap().is_none());

    let recs = records(
        ContainerBuilder::new()
            .environment(Props::of(&[("cache.kind", "redis-cluster")]))
            .instance(Client)
            .on_property_value("cache.kind", "redis"),
    );
    assert_eq!(recs[0].1, "on_property_value");
    assert_eq!(
        recs[0].3,
        "property \"cache.kind\" does not have the expected value"
    );
}

#[test]
fn on_property_value_or_missing_matches_absent() {
    let kept = |props: &[(&str, &str)]| {
        ContainerBuilder::new()
            .environment(Props::of(props))
            .instance(Client)
            .on_property_value_or_missing("cache.kind", "memory")
            .build()
            .unwrap()
            .get_opt::<Client>()
            .unwrap()
            .is_some()
    };
    assert!(kept(&[]));
    assert!(kept(&[("cache.kind", "memory")]));
    assert!(!kept(&[("cache.kind", "redis")]));
}

#[test]
fn without_environment_every_property_is_absent() {
    let seen = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&seen);
    let c = ContainerBuilder::new()
        .instance(Server("s"))
        .on_property_value_or_missing("x", "y")
        .instance(Client)
        .on_property("x")
        .instance(Later)
        .condition("look", move |cx| {
            *slot.lock().unwrap() = Some(cx.property("x"));
            ConditionOutcome::matched()
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Server>().unwrap().is_some());
    assert!(c.get_opt::<Client>().unwrap().is_none());
    assert_eq!(*seen.lock().unwrap(), Some(None));
}

#[test]
fn custom_condition_gets_context() {
    type Seen = (Vec<String>, String, bool, bool, Option<String>);
    let seen: Arc<Mutex<Option<Seen>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&seen);
    let c = ContainerBuilder::new()
        .active_profiles(["dev", "local"])
        .environment(Props::of(&[("app.name", "demo")]))
        .instance(Server("s"))
        .instance(Client)
        .condition("inspect", move |cx| {
            *slot.lock().unwrap() = Some((
                cx.active_profiles().to_vec(),
                cx.registration().to_owned(),
                cx.has_bean::<Server>(),
                cx.has_bean::<Later>(),
                cx.property("app.name"),
            ));
            ConditionOutcome::matched()
        })
        .build()
        .unwrap();
    assert!(c.get_opt::<Client>().unwrap().is_some());
    let (profiles, registration, server, later, name) = seen.lock().unwrap().take().unwrap();
    assert_eq!(profiles, ["dev", "local"]);
    assert_eq!(
        registration,
        format!("instance {}", std::any::type_name::<Client>())
    );
    assert!(server);
    assert!(!later);
    assert_eq!(name.as_deref(), Some("demo"));
}

#[test]
fn panicking_condition_aborts_build() {
    let result = std::panic::catch_unwind(|| {
        ContainerBuilder::new()
            .instance(Client)
            .condition("explodes", |_| panic!("condition failed"))
            .build()
            .map(drop)
    });
    assert!(result.is_err());
}

#[test]
fn report_records_in_order_with_value_free_reasons() {
    const MARKER: &str = "marker-7f3a9c";
    const EXPECTED: &str = "expected-51b2d4";
    let b = ContainerBuilder::new()
        .environment(Props::of(&[("app.mode", MARKER)]))
        .instance(Server("s"))
        .on_property("app.mode")
        .on_property_value("app.mode", MARKER)
        .instance(Client)
        .on_bean::<Server>()
        .on_property_value("app.mode", EXPECTED)
        .instance(Later)
        .on_property_value_or_missing("app.mode", EXPECTED)
        .instance(Smtp)
        .on_missing_bean::<Server>()
        .instance(Console)
        .on_missing_bean::<Later>()
        .condition("custom", |_| ConditionOutcome::skipped("custom reason"));
    let (result, report) = b.build_with_report();
    assert!(result.is_ok());
    let server = format!("instance {}", std::any::type_name::<Server>());
    let client = format!("instance {}", std::any::type_name::<Client>());
    let later = format!("instance {}", std::any::type_name::<Later>());
    let smtp = format!("instance {}", std::any::type_name::<Smtp>());
    let console = format!("instance {}", std::any::type_name::<Console>());
    let s = std::any::type_name::<Server>();
    let l = std::any::type_name::<Later>();
    let got: Vec<_> = report
        .records()
        .iter()
        .map(|r| {
            (
                r.registration().to_owned(),
                r.condition(),
                r.matched(),
                r.reason().to_owned(),
            )
        })
        .collect();
    let want = vec![
        (
            server.clone(),
            "on_property",
            true,
            "property \"app.mode\" is set".to_owned(),
        ),
        (
            server,
            "on_property_value",
            true,
            "property \"app.mode\" has the expected value".to_owned(),
        ),
        (
            client.clone(),
            "on_bean",
            true,
            format!("an earlier registration provides `{s}`"),
        ),
        (
            client,
            "on_property_value",
            false,
            "property \"app.mode\" does not have the expected value".to_owned(),
        ),
        (
            later,
            "on_property_value_or_missing",
            false,
            "property \"app.mode\" does not have the expected value".to_owned(),
        ),
        (
            smtp,
            "on_missing_bean",
            false,
            format!("an earlier registration already provides `{s}`"),
        ),
        (
            console.clone(),
            "on_missing_bean",
            true,
            format!("no earlier registration provides `{l}`"),
        ),
        (console, "custom", false, "custom reason".to_owned()),
    ];
    assert_eq!(got, want);

    let text = report.to_string();
    for r in report.records() {
        assert!(!r.reason().contains(MARKER) && !r.reason().contains(EXPECTED));
    }
    assert!(!text.contains(MARKER), "{text}");
    assert!(!text.contains(EXPECTED), "{text}");
    assert!(text.starts_with("CONDITIONS EVALUATION REPORT\n\nMatched:\n"));
}

#[test]
fn build_with_report_returns_report_on_failure() {
    struct Service {
        _later: Arc<Later>,
    }
    let (result, report) = ContainerBuilder::new()
        .instance(Client)
        .on_missing_bean::<Server>()
        .factory(|r: &Resolver| {
            Ok(Service {
                _later: r.get::<Later>()?,
            })
        })
        .build_with_report();
    assert!(matches!(result, Err(DiError::Missing { .. })));
    assert_eq!(report.records().len(), 1);
    assert!(report.records()[0].matched());
    assert_eq!(report.records()[0].condition(), "on_missing_bean");
}

#[test]
fn container_condition_report_after_success() {
    let c = ContainerBuilder::new()
        .instance(Server("s"))
        .instance(Client)
        .on_bean::<Server>()
        .build()
        .unwrap();
    let records = c.condition_report().records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].condition(), "on_bean");
    assert!(records[0].matched());

    let empty = ContainerBuilder::new().instance(Client).build().unwrap();
    assert!(empty.condition_report().records().is_empty());
}

/// Type-level binary tree: `Depth::register::<T>` adds `2^depth` registrations of distinct
/// types, each with `on_missing_bean` of another distinct type.
trait Depth {
    fn register<T: Send + Sync + 'static>(b: ContainerBuilder) -> ContainerBuilder;
}
struct Zero;
struct Succ<P>(PhantomData<P>);
struct Left<T>(PhantomData<T>);
struct Right<T>(PhantomData<T>);
struct Leaf<T>(PhantomData<T>);
struct Absent<T>(PhantomData<T>);

impl Depth for Zero {
    fn register<T: Send + Sync + 'static>(b: ContainerBuilder) -> ContainerBuilder {
        b.instance(Leaf::<T>(PhantomData))
            .on_missing_bean::<Absent<T>>()
    }
}

impl<P: Depth> Depth for Succ<P> {
    fn register<T: Send + Sync + 'static>(b: ContainerBuilder) -> ContainerBuilder {
        P::register::<Right<T>>(P::register::<Left<T>>(b))
    }
}

type D10 = Succ<Succ<Succ<Succ<Succ<Succ<Succ<Succ<Succ<Succ<Zero>>>>>>>>>>;

#[test]
fn evaluation_is_linear() {
    let builder = D10::register::<()>(ContainerBuilder::new());
    let start = Instant::now();
    let (result, report) = builder.build_with_report();
    let elapsed = start.elapsed();
    assert!(result.is_ok());
    assert_eq!(report.records().len(), 1024);
    assert!(report.records().iter().all(|r| r.matched()));
    assert!(report.to_string().len() > 1024);
    // Generous for a loaded debug build; a quadratic scan would not change much at this size,
    // the index design is what keeps it linear.
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

#[test]
fn registration_never_sees_itself() {
    // `on_missing_bean::<Server>` on the only `Server`: it does not see itself and is kept.
    let c = ContainerBuilder::new()
        .instance(Server("only"))
        .on_missing_bean::<Server>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Server>().unwrap().0, "only");
    assert!(c.condition_report().records()[0].matched());
}

struct CycleA;
struct CycleB;

fn invalid_builder() -> ContainerBuilder {
    ContainerBuilder::new().on_bean::<Server>()
}

fn missing_builder() -> ContainerBuilder {
    ContainerBuilder::new().factory(|r: &Resolver| {
        r.get::<Later>()?;
        Ok(Client)
    })
}

fn duplicate_builder() -> ContainerBuilder {
    ContainerBuilder::new()
        .instance(Server("a"))
        .instance(Server("b"))
}

fn cycle_builder() -> ContainerBuilder {
    ContainerBuilder::new()
        .factory(|r: &Resolver| {
            r.get::<CycleB>()?;
            Ok(CycleA)
        })
        .factory(|r: &Resolver| {
            r.get::<CycleA>()?;
            Ok(CycleB)
        })
}

#[test]
fn build_and_build_with_report_fail_with_the_same_error() {
    type Case = (&'static str, fn() -> ContainerBuilder, fn(&DiError) -> bool);
    let cases: [Case; 4] = [
        ("invalid", invalid_builder, |e| {
            matches!(e, DiError::Invalid { .. })
        }),
        ("missing", missing_builder, |e| {
            matches!(e, DiError::Missing { .. })
        }),
        ("duplicate", duplicate_builder, |e| {
            matches!(e, DiError::Duplicate { .. })
        }),
        ("cycle", cycle_builder, |e| {
            matches!(e, DiError::Cycle { .. })
        }),
    ];
    for (name, make, is_kind) in cases {
        let plain = make().build().err();
        let reported = make().build_with_report().0.err();
        assert!(plain.as_ref().is_some_and(is_kind), "{name}: {plain:?}");
        assert_eq!(
            plain.map(|e| e.to_string()),
            reported.map(|e| e.to_string()),
            "{name}"
        );
    }
}

#[test]
fn empty_property_key_is_invalid() {
    type Modifier = fn(ContainerBuilder) -> ContainerBuilder;
    let cases: [(&str, Modifier); 3] = [
        ("on_property", |b| b.on_property("")),
        ("on_property_value", |b| {
            b.on_property_value("", "zq-val-5521")
        }),
        ("on_property_value_or_missing", |b| {
            b.on_property_value_or_missing("", "zq-val-5521")
        }),
    ];
    for (name, modifier) in cases {
        let (result, report) =
            modifier(ContainerBuilder::new().instance(Client)).build_with_report();
        match result {
            Err(DiError::Invalid { message }) => {
                assert_eq!(message, format!("`{name}` needs a non-empty key"));
                assert!(!message.contains("zq-val-5521"));
            }
            Err(other) => panic!("{name}: unexpected error {other}"),
            Ok(_) => panic!("{name}: expected Invalid"),
        }
        assert!(report.records().is_empty());
    }
}

/// A section `db` that holds no scalar of its own, like a TOML table.
struct Sections;

impl Environment for Sections {
    fn property(&self, key: &str) -> Option<String> {
        (key == "db.url").then(|| "mem".to_owned())
    }
    fn contains(&self, key: &str) -> bool {
        key == "db" || self.property(key).is_some()
    }
}

#[test]
fn on_property_matches_a_section_through_contains() {
    let kept = |b: ContainerBuilder| b.build().unwrap().get_opt::<Client>().unwrap().is_some();
    let env = || {
        ContainerBuilder::new()
            .environment(Sections)
            .instance(Client)
    };
    assert!(kept(env().on_property("db")));
    assert!(kept(env().on_property("db.url")));
    assert!(!kept(env().on_property("cache")));
    // A value condition still needs a scalar.
    assert!(!kept(env().on_property_value("db", "")));
    // The default `contains` is `property(..).is_some()`.
    assert!(Props::of(&[("a", "1")]).contains("a"));
    assert!(!Props::of(&[("a", "1")]).contains("b"));
}
