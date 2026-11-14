//! Bean factories: `.bean`, `.try_bean`, the `Inject` shapes of their parameters, and the
//! `Configuration` trait.

use std::any::type_name;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxbloom_di::{Configuration, ContainerBuilder, DiError, ErrorSource, component};

fn name_of<T: ?Sized>() -> String {
    type_name::<T>().to_owned()
}

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

struct Config {
    url: &'static str,
}

struct Pool {
    url: String,
}

#[test]
fn bean_with_zero_one_and_twelve_params() {
    struct Zero(u8);
    struct One(u8);
    struct P1(u32);
    struct P2(u32);
    struct P3(u32);
    struct P4(u32);
    struct P5(u32);
    struct P6(u32);
    struct P7(u32);
    struct P8(u32);
    struct P9(u32);
    struct P10(u32);
    struct P11(u32);
    struct P12(u32);
    struct Twelve(u32);
    let b = ContainerBuilder::new()
        .instance(P1(1))
        .instance(P2(2))
        .instance(P3(3))
        .instance(P4(4))
        .instance(P5(5))
        .instance(P6(6))
        .instance(P7(7))
        .instance(P8(8))
        .instance(P9(9))
        .instance(P10(10))
        .instance(P11(11))
        .instance(P12(12))
        .bean(
            |a: Arc<P1>,
             b: Arc<P2>,
             c: Arc<P3>,
             d: Arc<P4>,
             e: Arc<P5>,
             f: Arc<P6>,
             g: Arc<P7>,
             h: Arc<P8>,
             i: Arc<P9>,
             j: Arc<P10>,
             k: Arc<P11>,
             l: Arc<P12>| {
                Twelve(a.0 + b.0 + c.0 + d.0 + e.0 + f.0 + g.0 + h.0 + i.0 + j.0 + k.0 + l.0)
            },
        );
    let c = b
        .bean(|| Zero(0))
        .bean(|z: Arc<Zero>| One(z.0 + 1))
        .build()
        .unwrap();
    assert_eq!(c.get::<Zero>().unwrap().0, 0);
    assert_eq!(c.get::<One>().unwrap().0, 1);
    assert_eq!(c.get::<Twelve>().unwrap().0, 78);
}

#[test]
fn bean_mixed_shapes_arc_dyn_option_vec() {
    struct Absent;
    struct Report {
        config: Arc<Config>,
        primary: Arc<dyn Notifier>,
        absent: Option<Arc<Absent>>,
        present: Option<Arc<Mailer>>,
        all: Vec<Arc<dyn Notifier>>,
    }
    let c = ContainerBuilder::new()
        .instance(Config { url: "mem://" })
        .instance(Mailer)
        .provides(|m: Arc<Mailer>| -> Arc<dyn Notifier> { m })
        .primary()
        .instance(Sms)
        .provides(|s: Arc<Sms>| -> Arc<dyn Notifier> { s })
        .bean(
            |config: Arc<Config>,
             primary: Arc<dyn Notifier>,
             absent: Option<Arc<Absent>>,
             present: Option<Arc<Mailer>>,
             all: Vec<Arc<dyn Notifier>>| Report {
                config,
                primary,
                absent,
                present,
                all,
            },
        )
        .build()
        .unwrap();
    let r = c.get::<Report>().unwrap();
    assert_eq!(r.config.url, "mem://");
    assert_eq!(r.primary.name(), "mail");
    assert!(r.absent.is_none());
    assert!(r.present.is_some());
    let names: Vec<_> = r.all.iter().map(|n| n.name()).collect();
    assert_eq!(names, ["mail", "sms"]);
}

fn make_pool(config: Arc<Config>) -> Pool {
    Pool {
        url: config.url.to_owned(),
    }
}

fn try_make_pool(config: Arc<Config>) -> Result<Pool, DiError> {
    Ok(make_pool(config))
}

#[test]
fn bean_from_fn_item_and_closure() {
    let c = ContainerBuilder::new()
        .instance(Config { url: "pg://db" })
        .bean(make_pool)
        .build()
        .unwrap();
    assert_eq!(c.get::<Pool>().unwrap().url, "pg://db");

    let c = ContainerBuilder::new()
        .instance(Config { url: "pg://db" })
        .try_bean(try_make_pool)
        .build()
        .unwrap();
    assert_eq!(c.get::<Pool>().unwrap().url, "pg://db");

    let suffix = String::from("/app");
    let c = ContainerBuilder::new()
        .instance(Config { url: "pg://db" })
        .bean(move |config: Arc<Config>| Pool {
            url: format!("{}{suffix}", config.url),
        })
        .build()
        .unwrap();
    assert_eq!(c.get::<Pool>().unwrap().url, "pg://db/app");
}

#[test]
fn try_bean_error_is_creation_naming_the_type() {
    struct Other;
    let err = ContainerBuilder::new()
        .try_bean(|| -> Result<Pool, DiError> { Err(DiError::creation::<Other>("no route")) })
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Creation {
            type_name: name_of::<Pool>(),
            message: "no route".to_owned(),
            source: ErrorSource::none(),
        })
    );

    // A non-`Creation` error returned by the function is wrapped, with fixed text only.
    let err = ContainerBuilder::new()
        .try_bean(|| -> Result<Pool, DiError> {
            Err(DiError::Invalid {
                message: "bad".to_owned(),
            })
        })
        .build()
        .err();
    let returned = DiError::Invalid {
        message: "bad".to_owned(),
    };
    assert_eq!(
        err,
        Some(DiError::Creation {
            type_name: name_of::<Pool>(),
            message: format!("bean failed: {returned}"),
            source: ErrorSource::none(),
        })
    );
}

#[test]
fn try_bean_failure_is_cached_and_not_run_again() {
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&runs);
    let c = ContainerBuilder::new()
        .try_bean(move || -> Result<Pool, DiError> {
            counter.fetch_add(1, Ordering::SeqCst);
            Err(DiError::creation::<Pool>("down"))
        })
        .lazy()
        .build()
        .unwrap();
    let first = c.get::<Pool>().err();
    assert!(matches!(first, Some(DiError::Creation { .. })));
    assert_eq!(c.get::<Pool>().err(), first);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn try_bean_dependency_errors_are_not_wrapped() {
    let err = ContainerBuilder::new()
        .try_bean(|c: Arc<Config>| -> Result<Pool, DiError> { Ok(make_pool(c)) })
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Missing {
            requested: name_of::<Config>(),
            required_by: vec![name_of::<Pool>()],
        })
    );
}

// --- error paths match `#[component]` ------------------------------------------------------------

struct Leaf;

#[component]
struct CompMid {
    _leaf: Arc<Leaf>,
}

#[component]
struct CompTop {
    _mid: Arc<CompMid>,
}

struct BeanMid;
struct BeanTop;

#[test]
fn missing_through_bean_params_reports_the_same_chain_as_component() {
    let from_component = ContainerBuilder::new()
        .component::<CompTop>()
        .component::<CompMid>()
        .build()
        .err();
    assert_eq!(
        from_component,
        Some(DiError::Missing {
            requested: name_of::<Leaf>(),
            required_by: vec![name_of::<CompTop>(), name_of::<CompMid>()],
        })
    );
    let from_bean = ContainerBuilder::new()
        .bean(|_: Arc<BeanMid>| BeanTop)
        .bean(|_: Arc<Leaf>| BeanMid)
        .build()
        .err();
    assert_eq!(
        from_bean,
        Some(DiError::Missing {
            requested: name_of::<Leaf>(),
            required_by: vec![name_of::<BeanTop>(), name_of::<BeanMid>()],
        })
    );
    // Optional and collection parameters never report a missing provider.
    let c = ContainerBuilder::new()
        .bean(|_: Option<Arc<Leaf>>, _: Vec<Arc<Leaf>>| BeanTop)
        .build();
    assert!(c.is_ok());
}

#[test]
fn cycle_through_bean_params_names_concrete_types() {
    struct A;
    struct B;
    let err = ContainerBuilder::new()
        .bean(|_: Arc<B>| A)
        .bean(|_: Arc<dyn Notifier>| B)
        .bean(|_: Arc<A>| Mailer)
        .provides(|m: Arc<Mailer>| -> Arc<dyn Notifier> { m })
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Cycle {
            path: vec![
                name_of::<A>(),
                name_of::<B>(),
                name_of::<Mailer>(),
                name_of::<A>()
            ],
        })
    );
}

#[test]
fn ambiguous_bean_param_lists_closure_labels() {
    struct User;
    let (l1, l2) = (line!() + 2, line!() + 4);
    let err = ContainerBuilder::new()
        .bean(|| Mailer)
        .provides(|m: Arc<Mailer>| -> Arc<dyn Notifier> { m })
        .bean(|| Sms)
        .provides(|s: Arc<Sms>| -> Arc<dyn Notifier> { s })
        .bean(|_: Arc<dyn Notifier>| User)
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Ambiguous {
            requested: name_of::<dyn Notifier>(),
            candidates: vec![
                format!("closure at {}:{l1}", file!()),
                format!("closure at {}:{l2}", file!()),
            ],
        })
    );
}

#[test]
fn bean_modifiers_primary_lazy_provides() {
    let created = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&created);
    let c = ContainerBuilder::new()
        .bean(|| Mailer)
        .provides(|m: Arc<Mailer>| -> Arc<dyn Notifier> { m })
        .bean(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Sms
        })
        .provides(|s: Arc<Sms>| -> Arc<dyn Notifier> { s })
        .primary()
        .lazy()
        .order(-1)
        .build()
        .unwrap();
    assert_eq!(created.load(Ordering::SeqCst), 0, "lazy bean not created");
    assert_eq!(c.get::<dyn Notifier>().unwrap().name(), "sms");
    let all: Vec<_> = c
        .get_all::<dyn Notifier>()
        .unwrap()
        .iter()
        .map(|n| n.name())
        .collect();
    assert_eq!(all, ["sms", "mail"]);
    assert_eq!(created.load(Ordering::SeqCst), 1);

    // Lifecycle and profile modifiers apply to beans too.
    let destroyed = Arc::new(AtomicUsize::new(0));
    let d = Arc::clone(&destroyed);
    let c = ContainerBuilder::new()
        .active_profiles(["dev"])
        .bean(|| Config { url: "dev" })
        .profile("dev")
        .pre_destroy(move |_: &Config| {
            d.fetch_add(1, Ordering::SeqCst);
        })
        .bean(|| Config { url: "prod" })
        .profile("prod")
        .build()
        .unwrap();
    assert_eq!(c.get::<Config>().unwrap().url, "dev");
    assert_eq!(c.shutdown().destroyed, 1);
    assert_eq!(destroyed.load(Ordering::SeqCst), 1);
}

#[test]
fn duplicate_bean_names_both_closure_labels() {
    let (first, second) = (line!() + 2, line!() + 3);
    let err = ContainerBuilder::new()
        .bean(|| Config { url: "a" })
        .try_bean(|| Ok(Config { url: "b" }))
        .build()
        .err();
    assert_eq!(
        err,
        Some(DiError::Duplicate {
            type_name: name_of::<Config>(),
            sources: vec![
                format!("closure at {}:{first}", file!()),
                format!("closure at {}:{second}", file!()),
            ],
        })
    );
}

struct AppConfig;

impl Configuration for AppConfig {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .bean(|| Config { url: "mem://" })
            .bean(make_pool)
            .bean(|| Mailer)
            .provides(|m: Arc<Mailer>| -> Arc<dyn Notifier> { m })
    }
}

#[test]
fn configuration_trait_register_adds_entries() {
    let c = ContainerBuilder::new()
        .configuration::<AppConfig>()
        .build()
        .unwrap();
    assert_eq!(c.get::<Pool>().unwrap().url, "mem://");
    assert_eq!(c.get::<dyn Notifier>().unwrap().name(), "mail");

    // Registering a configuration twice registers its beans twice.
    let err = ContainerBuilder::new()
        .configuration::<AppConfig>()
        .configuration::<AppConfig>()
        .build()
        .err();
    assert!(matches!(err, Some(DiError::Duplicate { .. })), "{err:?}");
}

#[test]
fn bean_after_shutdown_is_shut_down() {
    let c = ContainerBuilder::new()
        .bean(|| Mailer)
        .lazy()
        .build()
        .unwrap();
    c.shutdown();
    assert_eq!(c.get::<Mailer>().err(), Some(DiError::ShutDown));
}

#[test]
fn non_injectable_parameter_type_fails_to_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/beans/*.rs");
}
