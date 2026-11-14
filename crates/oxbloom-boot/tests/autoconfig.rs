//! Auto-configurations in a run: registration after the user's wiring (back-off), ordering by
//! `after`/`before`, `exclude`, the cycle error and its analysis, and the `Config` environment.

mod common;

use std::any::type_name;
use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxbloom_boot::{
    ApplicationArguments, AutoConfiguration, AutoConfigurations, Banner, ConfigRef, Configuration,
    OxbloomApplication, OxbloomApplicationBuilder, RunListener, StartupError, StartupPhase,
    auto_configuration,
};
use oxbloom_config::Config;
use oxbloom_di::{
    Component, Container, ContainerBuilder, DiError, Environment, Resolver, component,
};
use oxbloom_test::{Captured, Log};

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

/// An application with `config` as TOML, no banner and `out` as output.
fn app(config: &str, out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str(config).unwrap())
        .banner(Banner::off())
        .output(out.clone())
}

/// Runs a closure on `context_ready`.
struct OnReady<F>(F);

impl<F: Fn(&Container) + Send + Sync + 'static> RunListener for OnReady<F> {
    fn context_ready(&self, c: &Container) {
        (self.0)(c)
    }
}

/// Logs the lifecycle phases a failing run reaches.
struct Phases(Log);

impl RunListener for Phases {
    fn context_ready(&self, _: &Container) {
        self.0.push("context_ready");
    }
    fn failed(&self, e: &StartupError) {
        self.0.push(format!("failed {}", e.phase()));
    }
}

/// Runs `builder` to success and returns what `f` read from the container on `context_ready`.
async fn inspect<T: Send + 'static>(
    builder: OxbloomApplicationBuilder,
    f: impl Fn(&Container) -> T + Send + Sync + 'static,
) -> T {
    let slot = Arc::new(Mutex::new(None));
    let s = slot.clone();
    within(
        builder
            .listener(OnReady(move |c: &Container| {
                *s.lock().unwrap() = Some(f(c));
            }))
            .run(),
    )
    .await
    .unwrap();
    let value = slot.lock().unwrap().take();
    value.expect("context_ready ran")
}

fn has<T: Send + Sync + 'static>(c: &Container) -> bool {
    c.get_opt::<T>().unwrap().is_some()
}

// ---------------------------------------------------------------------------------------------
// Hand-written auto-configurations for ordering: each registers one `Mark<Self>` that provides
// `dyn Marker`, and `get_all::<dyn Marker>()` returns them in registration order.

trait Marker: Send + Sync {
    fn name(&self) -> &'static str;
}

trait Named: 'static {
    const NAME: &'static str;
}

struct Mark<T>(PhantomData<fn() -> T>);

impl<T: Named> Marker for Mark<T> {
    fn name(&self) -> &'static str {
        T::NAME
    }
}

macro_rules! auto {
    ($name:ident, after = [$($a:ty),*], before = [$($b:ty),*]) => {
        struct $name;
        impl Named for $name {
            const NAME: &'static str = stringify!($name);
        }
        impl Component for $name {
            fn create(_: &Resolver) -> Result<Self, DiError> {
                Ok($name)
            }
        }
        impl Configuration for $name {
            fn register(builder: ContainerBuilder) -> ContainerBuilder {
                builder
                    .instance(Mark::<$name>(PhantomData))
                    .provides(|m: Arc<Mark<$name>>| -> Arc<dyn Marker> { m })
            }
        }
        impl AutoConfiguration for $name {
            fn after() -> Vec<ConfigRef> {
                vec![$(ConfigRef::of::<$a>()),*]
            }
            fn before() -> Vec<ConfigRef> {
                vec![$(ConfigRef::of::<$b>()),*]
            }
        }
    };
}

/// The marker names in registration order.
async fn order(builder: OxbloomApplicationBuilder) -> Vec<&'static str> {
    inspect(builder, |c| {
        c.get_all::<dyn Marker>()
            .unwrap()
            .iter()
            .map(|m| m.name())
            .collect()
    })
    .await
}

/// A type that is never in any list.
struct Absent;

auto!(Plain1, after = [], before = []);
auto!(Plain2, after = [], before = []);
auto!(Plain3, after = [], before = []);
auto!(Plain4, after = [], before = []);

auto!(AfterA, after = [AfterB], before = []);
auto!(AfterB, after = [], before = []);

auto!(BeforeA, after = [], before = []);
auto!(BeforeB, after = [], before = [BeforeA]);

auto!(BothA, after = [BothB], before = []);
auto!(BothB, after = [], before = [BothA]);

auto!(TieP, after = [], before = []);
auto!(TieQ, after = [], before = []);
auto!(TieR, after = [], before = []);
auto!(TieS, after = [], before = [TieQ]);

auto!(RefA, after = [Absent], before = [Absent]);
auto!(RefB, after = [], before = []);

auto!(SelfRef, after = [SelfRef], before = [SelfRef]);

auto!(CycA, after = [CycB], before = []);
auto!(CycB, after = [CycA], before = []);

auto!(TriA, after = [TriC], before = []);
auto!(TriB, after = [TriA], before = []);
auto!(TriC, after = [TriB], before = []);
auto!(TriD, after = [TriA], before = []);

auto!(ExA, after = [ExB], before = []);
auto!(ExB, after = [ExC], before = []);
auto!(ExC, after = [], before = []);

#[tokio::test]
async fn order_by_after() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<AfterA>()
        .auto_configuration::<AfterB>();
    assert_eq!(order(builder).await, ["AfterB", "AfterA"]);
}

#[tokio::test]
async fn order_by_before() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<BeforeA>()
        .auto_configuration::<BeforeB>();
    assert_eq!(order(builder).await, ["BeforeB", "BeforeA"]);
}

#[tokio::test]
async fn after_and_before_agree_and_conflict_free() {
    // `BothA::after() = [BothB]` and `BothB::before() = [BothA]` state the same edge.
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<BothA>()
        .auto_configuration::<BothB>();
    assert_eq!(order(builder).await, ["BothB", "BothA"]);
}

#[tokio::test]
async fn ties_keep_list_order() {
    let out = Captured::new();
    let builder = app("", &out).auto_configurations(
        AutoConfigurations::new()
            .add::<Plain3>()
            .add::<Plain1>()
            .add::<Plain2>(),
    );
    assert_eq!(order(builder).await, ["Plain3", "Plain1", "Plain2"]);

    // Among the configurations that are ready, the earliest in the list goes first.
    let builder = app("", &out).auto_configurations(
        AutoConfigurations::new()
            .add::<TieP>()
            .add::<TieQ>()
            .add::<TieR>()
            .add::<TieS>(),
    );
    assert_eq!(order(builder).await, ["TieP", "TieR", "TieS", "TieQ"]);
}

#[tokio::test]
async fn absent_reference_is_ignored() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<RefA>()
        .auto_configuration::<RefB>();
    assert_eq!(order(builder).await, ["RefA", "RefB"]);
}

#[tokio::test]
async fn self_reference_is_ignored() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<Plain1>()
        .auto_configuration::<SelfRef>()
        .auto_configuration::<Plain2>();
    assert_eq!(order(builder).await, ["Plain1", "SelfRef", "Plain2"]);
}

const CYCLE_ACTION: &str =
    "Remove one `after` or `before` entry so that the order is no longer circular.";

#[tokio::test]
async fn cycle_is_container_phase_with_fixed_text_and_report() {
    let out = Captured::new();
    let log = Log::new();
    let created = log.clone();
    let err = within(
        app("", &out)
            .failure_report(true)
            .listener(Phases(log.clone()))
            .configure(move |b| {
                b.bean(move || {
                    created.push("user bean created");
                    7_u8
                })
            })
            .auto_configuration::<CycA>()
            .auto_configuration::<CycB>()
            .run(),
    )
    .await
    .unwrap_err();

    assert_eq!(err.phase(), StartupPhase::Container);
    let path = format!(
        "{a} -> {b} -> {a}",
        a = type_name::<CycA>(),
        b = type_name::<CycB>()
    );
    let source = std::error::Error::source(&err).unwrap();
    assert_eq!(
        source.to_string(),
        format!("auto-configurations form an ordering cycle: {path}")
    );
    let analysis = err.analysis().expect("analyzed");
    let description = format!("The auto-configurations form an ordering cycle: `{path}`.");
    assert_eq!(analysis.description(), description);
    assert_eq!(analysis.action(), Some(CYCLE_ACTION));

    let report = out.contents();
    assert!(report.contains("APPLICATION FAILED TO START"), "{report}");
    assert!(
        report.contains(&format!("Description:\n\n{description}\n")),
        "{report}"
    );
    assert!(
        report.ends_with(&format!("Action:\n\n{CYCLE_ACTION}\n")),
        "{report}"
    );
    // Detected before the container is built: no bean is created, no `context_ready`.
    assert_eq!(log.entries(), ["failed container"]);
}

#[tokio::test]
async fn three_node_cycle_path_message() {
    // `TriD` waits on the cycle but is not on it; the message starts at the first configuration
    // on the cycle and follows `after` edges until it closes.
    let out = Captured::new();
    let err = within(
        app("", &out)
            .auto_configurations(
                AutoConfigurations::new()
                    .add::<TriD>()
                    .add::<TriA>()
                    .add::<TriB>()
                    .add::<TriC>(),
            )
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    let path = format!(
        "{a} -> {c} -> {b} -> {a}",
        a = type_name::<TriA>(),
        b = type_name::<TriB>(),
        c = type_name::<TriC>()
    );
    assert_eq!(
        std::error::Error::source(&err).unwrap().to_string(),
        format!("auto-configurations form an ordering cycle: {path}")
    );
    assert_eq!(
        err.analysis().unwrap().description(),
        format!("The auto-configurations form an ordering cycle: `{path}`.")
    );
    assert!(!err.to_string().contains(type_name::<TriD>()), "{err}");
}

#[tokio::test]
async fn exclude_removes_configuration() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<Plain1>()
        .auto_configuration::<Plain2>()
        .exclude::<Plain1>();
    let (names, instance) = inspect(builder, |c| {
        let names: Vec<_> = c
            .get_all::<dyn Marker>()
            .unwrap()
            .iter()
            .map(|m| m.name())
            .collect();
        (names, has::<Plain1>(c))
    })
    .await;
    assert_eq!(names, ["Plain2"]);
    assert!(
        !instance,
        "the excluded configuration's instance is not registered"
    );
}

#[tokio::test]
async fn exclude_of_referenced_configuration_keeps_order_valid() {
    let out = Captured::new();
    let list = || {
        AutoConfigurations::new()
            .add::<ExA>()
            .add::<ExB>()
            .add::<ExC>()
    };
    assert_eq!(
        order(app("", &out).auto_configurations(list())).await,
        ["ExC", "ExB", "ExA"]
    );
    // `ExA` waits on `ExB`, which waits on `ExC`: without `ExB` nothing links them.
    assert_eq!(
        order(app("", &out).auto_configurations(list()).exclude::<ExB>()).await,
        ["ExA", "ExC"]
    );
    // Excluding one side of a cycle removes the cycle.
    assert_eq!(
        order(
            app("", &out)
                .auto_configuration::<CycA>()
                .auto_configuration::<CycB>()
                .exclude::<CycB>()
        )
        .await,
        ["CycA"]
    );
}

#[tokio::test]
async fn exclude_absent_is_noop() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<Plain1>()
        .exclude::<Plain2>();
    assert_eq!(order(builder).await, ["Plain1"]);
}

#[tokio::test]
async fn exclude_call_order_does_not_matter() {
    let out = Captured::new();
    let builder = app("", &out)
        .exclude::<Plain2>()
        .auto_configurations(
            AutoConfigurations::new()
                .add::<Plain1>()
                .add::<Plain2>()
                .add::<Plain3>(),
        )
        .exclude::<Plain3>()
        .auto_configuration::<Plain3>();
    assert_eq!(order(builder).await, ["Plain1"]);
}

#[tokio::test]
async fn several_lists_combine_in_call_order() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configurations(AutoConfigurations::new().add::<Plain2>().add::<Plain1>())
        .auto_configuration::<Plain4>()
        // `Plain1` is already in the list and keeps its first position.
        .auto_configurations(AutoConfigurations::new().add::<Plain1>().add::<Plain3>())
        .auto_configuration::<Plain2>();
    assert_eq!(
        order(builder).await,
        ["Plain2", "Plain1", "Plain4", "Plain3"]
    );
}

// ---------------------------------------------------------------------------------------------
// Back-off and class-level conditions, written with the macro.

pub struct Greeter(&'static str);

#[component]
pub struct GreeterAuto;

#[auto_configuration]
impl GreeterAuto {
    #[bean(on_missing_bean)]
    fn greeter(&self) -> Greeter {
        Greeter("default")
    }
}

#[tokio::test]
async fn user_bean_wins_over_auto_configured_default() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<GreeterAuto>()
        .configure(|b| b.instance(Greeter("user")));
    let greeting = inspect(builder, |c| c.get::<Greeter>().unwrap().0).await;
    assert_eq!(greeting, "user");
}

#[tokio::test]
async fn auto_configured_default_used_when_user_gave_none() {
    let out = Captured::new();
    let builder = app("", &out).auto_configuration::<GreeterAuto>();
    let (greeting, instance) = inspect(builder, |c| {
        (c.get::<Greeter>().unwrap().0, has::<GreeterAuto>(c))
    })
    .await;
    assert_eq!(greeting, "default");
    assert!(instance);
}

pub trait Store: Send + Sync {}
pub struct DiskStore;
impl Store for DiskStore {}
pub struct MemStore;

#[component]
pub struct StoreAuto;

#[auto_configuration]
impl StoreAuto {
    #[bean(on_missing_bean(dyn Store))]
    fn mem(&self) -> MemStore {
        MemStore
    }
}

#[tokio::test]
async fn back_off_with_trait_provision() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<StoreAuto>()
        .configure(|b| {
            b.instance(DiskStore)
                .provides(|s: Arc<DiskStore>| -> Arc<dyn Store> { s })
        });
    assert!(!inspect(builder, has::<MemStore>).await);

    let builder = app("", &out).auto_configuration::<StoreAuto>();
    assert!(inspect(builder, has::<MemStore>).await);
}

pub struct Greeting(String);

#[component]
pub struct GreetingAuto {
    args: Arc<ApplicationArguments>,
}

#[auto_configuration]
impl GreetingAuto {
    #[bean]
    fn greeting(&self) -> Greeting {
        Greeting(format!(
            "hello {}",
            self.args.option("name").unwrap_or("world")
        ))
    }
}

#[tokio::test]
async fn auto_configuration_instance_available_to_beans() {
    let out = Captured::new();
    let builder = app("", &out)
        .args(["--name=oxbloom"])
        .auto_configuration::<GreetingAuto>();
    let (greeting, instance) = inspect(builder, |c| {
        (
            c.get::<Greeting>().unwrap().0.clone(),
            has::<GreetingAuto>(c),
        )
    })
    .await;
    assert_eq!(greeting, "hello oxbloom");
    assert!(instance);
}

static SKIPPED_PROVIDER_RUNS: AtomicUsize = AtomicUsize::new(0);

pub struct Skipped;
pub struct SkippedSibling;
pub struct Kept;

#[component]
pub struct SkippedAuto;

#[auto_configuration(on_property("feature.absent"))]
impl SkippedAuto {
    #[bean]
    fn skipped(&self) -> Skipped {
        SKIPPED_PROVIDER_RUNS.fetch_add(1, Ordering::SeqCst);
        Skipped
    }
    #[bean]
    fn sibling(&self) -> SkippedSibling {
        SKIPPED_PROVIDER_RUNS.fetch_add(1, Ordering::SeqCst);
        SkippedSibling
    }
}

#[component]
pub struct KeptAuto;

#[auto_configuration]
impl KeptAuto {
    #[bean]
    fn kept(&self) -> Kept {
        Kept
    }
}

#[tokio::test]
async fn class_condition_failing_skips_group_only() {
    let out = Captured::new();
    let builder = app("", &out)
        .auto_configuration::<SkippedAuto>()
        .auto_configuration::<KeptAuto>();
    let seen = inspect(builder, |c| {
        let group = c
            .condition_report()
            .records()
            .iter()
            .find(|r| r.registration() == type_name::<SkippedAuto>())
            .map(|r| (r.condition().to_owned(), r.matched()));
        (
            has::<Skipped>(c),
            has::<SkippedSibling>(c),
            has::<SkippedAuto>(c),
            has::<Kept>(c),
            has::<KeptAuto>(c),
            group,
        )
    })
    .await;
    assert_eq!(
        seen,
        (
            false,
            false,
            false,
            true,
            true,
            Some(("on_property".to_owned(), false))
        )
    );
    assert_eq!(SKIPPED_PROVIDER_RUNS.load(Ordering::SeqCst), 0);
}

pub struct Enabled;
pub struct Fast;

#[component]
pub struct EnabledAuto;

#[auto_configuration(on_property("feature.enabled"))]
impl EnabledAuto {
    #[bean]
    fn enabled(&self) -> Enabled {
        Enabled
    }
}

#[component]
pub struct FastAuto;

#[auto_configuration(on_property("feature.mode", "fast"))]
impl FastAuto {
    #[bean]
    fn fast(&self) -> Fast {
        Fast
    }
}

/// Runs with `application.toml` set to `toml` in a temporary directory and reports which of
/// `Enabled` and `Fast` exist.
async fn from_file(toml: &str) -> (bool, bool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application.toml"), toml).unwrap();
    let path = dir.path().to_owned();
    let builder = OxbloomApplication::builder()
        .config(move |b| b.dir(path).env_from([]))
        .banner(Banner::off())
        .output(std::io::sink())
        .auto_configuration::<EnabledAuto>()
        .auto_configuration::<FastAuto>();
    inspect(builder, |c| (has::<Enabled>(c), has::<Fast>(c))).await
}

#[tokio::test]
async fn on_property_class_condition_reads_config() {
    assert_eq!(
        from_file("[feature]\nenabled = true\nmode = \"fast\"\n").await,
        (true, true)
    );
    // An exact comparison: a value that contains the expected text does not match.
    assert_eq!(
        from_file("[feature]\nenabled = true\nmode = \"fast-ish\"\n").await,
        (true, false)
    );
    assert_eq!(from_file("").await, (false, false));
}

/// A fixed property source.
struct Props(HashMap<&'static str, &'static str>);

impl Environment for Props {
    fn property(&self, key: &str) -> Option<String> {
        self.0.get(key).map(|v| (*v).to_owned())
    }
}

#[tokio::test]
async fn user_environment_replaces_config_environment() {
    let out = Captured::new();
    let config = "[feature]\nenabled = true\nmode = \"fast\"\n";
    let builder = app(config, &out)
        .auto_configuration::<EnabledAuto>()
        .auto_configuration::<FastAuto>();
    assert_eq!(
        inspect(builder, |c| (has::<Enabled>(c), has::<Fast>(c))).await,
        (true, true)
    );

    let builder = app(config, &out)
        .configure(|b| b.environment(Props([("feature.mode", "fast")].into())))
        .auto_configuration::<EnabledAuto>()
        .auto_configuration::<FastAuto>();
    assert_eq!(
        inspect(builder, |c| (has::<Enabled>(c), has::<Fast>(c))).await,
        (false, true)
    );
}

#[tokio::test]
async fn no_auto_configurations_changes_nothing() {
    let out = Captured::new();
    let builder = app("", &out)
        .exclude::<Plain1>()
        .configure(|b| b.instance(Greeter("user")));
    let (markers, greeting, records, config) = inspect(builder, |c| {
        (
            c.get_all::<dyn Marker>().unwrap().len(),
            c.get::<Greeter>().unwrap().0,
            c.condition_report().records().len(),
            has::<Config>(c),
        )
    })
    .await;
    assert_eq!((markers, greeting, records, config), (0, "user", 0, true));
    assert_eq!(out.contents(), "");
}

#[tokio::test]
async fn marker_value_never_in_cycle_error_or_report() {
    const MARKER: &str = "MARKER-7731-secret";
    let config = format!("secret = \"{MARKER}\"\n[feature]\nmode = \"{MARKER}\"\n");

    let out = Captured::new();
    let err = within(
        app(&config, &out)
            .args([format!("--password={MARKER}")])
            .failure_report(true)
            .auto_configuration::<FastAuto>()
            .auto_configuration::<CycA>()
            .auto_configuration::<CycB>()
            .run(),
    )
    .await
    .unwrap_err();
    let texts = [
        err.to_string(),
        format!("{err:?}"),
        std::error::Error::source(&err).unwrap().to_string(),
        format!("{:?}", std::error::Error::source(&err).unwrap()),
        out.contents(),
    ];
    for text in &texts {
        assert!(!text.contains(MARKER), "{text}");
    }
    assert!(out.contents().contains("ordering cycle"));

    // Without the cycle the group is skipped, and its record does not name the value either.
    let out = Captured::new();
    let records = inspect(app(&config, &out).auto_configuration::<FastAuto>(), |c| {
        c.condition_report()
            .records()
            .iter()
            .map(|r| format!("{}: {} - {}", r.registration(), r.condition(), r.reason()))
            .collect::<Vec<_>>()
    })
    .await;
    assert_eq!(records.len(), 1, "{records:?}");
    for record in &records {
        assert!(!record.contains(MARKER), "{record}");
    }
}
