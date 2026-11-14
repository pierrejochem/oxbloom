//! The `oxbloom.main.conditions-report` setting: `off` (the default) prints nothing, `console`
//! prints the conditions evaluation report to the output target right after the container is
//! built, also when the build fails.

mod common;

use std::any::type_name;
use std::future::Future;
use std::process::ExitCode;
use std::time::Duration;

use oxbloom_boot::{
    Banner, OxbloomApplication, OxbloomApplicationBuilder, RunListener, StartupError, StartupPhase,
    auto_configuration,
};
use oxbloom_config::Config;
use oxbloom_di::{Container, ContainerBuilder, DiError, Resolver, component};
use oxbloom_test::{Captured, Log};

const KEY: &str = "oxbloom.main.conditions-report";
const EMPTY: &str = "CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n  (none)";

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

/// A run with `toml` as its configuration and the banner off, writing to `out`.
fn app(toml: &str, out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str(toml).unwrap())
        .banner(Banner::off())
        .output(out.clone())
}

fn console(out: &Captured) -> OxbloomApplicationBuilder {
    app(&format!("{KEY} = \"console\""), out)
}

/// The report of [`conditional`], as printed.
const CONDITIONAL: &str = "CONDITIONS EVALUATION REPORT\n\nMatched:\n  instance u8: on_missing_bean - \
                           no earlier registration provides `u8`\n\nSkipped:\n  (none)\n";

/// A user registration with one condition, so the report is not empty.
fn conditional(b: ContainerBuilder) -> ContainerBuilder {
    b.instance(7u8).on_missing_bean::<u8>()
}

/// Logs what the output target holds when `context_ready` and `failed` fire.
struct Snapshot {
    log: Log,
    out: Captured,
}

impl RunListener for Snapshot {
    fn context_ready(&self, _: &Container) {
        self.log
            .push(format!("context_ready saw {:?}", self.out.contents()));
    }

    fn failed(&self, _: &StartupError) {
        self.log
            .push(format!("failed saw {:?}", self.out.contents()));
    }
}

#[tokio::test]
async fn off_prints_nothing() {
    let out = Captured::new();
    within(
        app(&format!("{KEY} = \"off\""), &out)
            .configure(conditional)
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(out.contents(), "");
}

#[tokio::test]
async fn missing_key_means_off() {
    let out = Captured::new();
    within(app("", &out).configure(conditional).run())
        .await
        .unwrap();
    assert_eq!(out.contents(), "");
}

struct Probe;

#[tokio::test]
async fn console_prints_report_after_build_before_listeners() {
    let out = Captured::new();
    let log = Log::new();
    let (l, o) = (log.clone(), out.clone());
    within(
        console(&out)
            .listener(Snapshot {
                log: log.clone(),
                out: out.clone(),
            })
            .configure(conditional)
            .configure(move |b| {
                b.factory(move |_| {
                    l.push(format!("constructor saw {:?}", o.contents()));
                    Ok(Probe)
                })
            })
            .run(),
    )
    .await
    .unwrap();
    let report = CONDITIONAL;
    assert_eq!(
        log.entries(),
        [
            // Providers run during the build, before the report exists.
            "constructor saw \"\"".to_owned(),
            format!("context_ready saw {report:?}"),
        ]
    );
    assert_eq!(out.contents(), report);
}

struct Absent;

#[tokio::test]
async fn console_prints_report_on_failed_build_before_failure_report() {
    let out = Captured::new();
    let log = Log::new();
    let err = within(
        console(&out)
            .failure_report(true)
            .listener(Snapshot {
                log: log.clone(),
                out: out.clone(),
            })
            .configure(conditional)
            .configure(|b| b.factory(|r: &Resolver| r.get::<Absent>().map(|_| 1u16)))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    let source = std::error::Error::source(&err).and_then(|s| s.downcast_ref::<DiError>());
    assert!(matches!(source, Some(DiError::Missing { .. })), "{err:?}");

    let report = CONDITIONAL;
    assert_eq!(log.entries(), [format!("failed saw {report:?}")]);
    let text = out.contents();
    let rest = text.strip_prefix(report).expect(&text);
    assert!(
        rest.starts_with("***************************\nAPPLICATION FAILED TO START\n"),
        "{text}"
    );
    assert_eq!(text.matches("CONDITIONS EVALUATION REPORT").count(), 1);
}

pub struct Greeter(#[allow(dead_code)] &'static str);
pub struct Clock;

#[component]
pub struct GreeterAutoConfig;

#[auto_configuration(on_property("greeter.enabled"))]
impl GreeterAutoConfig {
    #[bean(on_missing_bean)]
    fn greeter(&self) -> Greeter {
        Greeter("default")
    }

    #[bean(on_missing_bean)]
    fn clock(&self) -> Clock {
        Clock
    }
}

#[tokio::test]
async fn report_lists_matched_and_skipped_with_reasons() {
    let out = Captured::new();
    within(
        app(
            &format!("{KEY} = \"console\"\ngreeter.enabled = true"),
            &out,
        )
        .auto_configuration::<GreeterAutoConfig>()
        .configure(|b| b.instance(Greeter("mine")))
        .run(),
    )
    .await
    .unwrap();
    let group = type_name::<GreeterAutoConfig>();
    let (greeter, clock) = (type_name::<Greeter>(), type_name::<Clock>());
    assert_eq!(
        out.contents(),
        format!(
            "CONDITIONS EVALUATION REPORT\n\
             \n\
             Matched:\n\
             \x20 {group}: on_property - property \"greeter.enabled\" is set\n\
             \x20 {group}::clock: on_missing_bean - no earlier registration provides `{clock}`\n\
             \n\
             Skipped:\n\
             \x20 {group}::greeter: on_missing_bean - an earlier registration already provides \
             `{greeter}`\n"
        )
    );
}

#[tokio::test]
async fn invalid_mode_is_config_phase_without_the_value() {
    for value in ["zq-secret-7731", "Console", "OFF", ""] {
        let out = Captured::new();
        let err = within(app(&format!("{KEY} = \"{value}\""), &out).run())
            .await
            .unwrap_err();
        assert_eq!(err.phase(), StartupPhase::Config);
        assert_eq!(
            err.to_string(),
            format!(
                "startup failed in the config phase: unsupported value for {KEY}; expected console or off"
            )
        );
        if !value.is_empty() {
            let shown = format!("{err} {err:?}");
            assert!(!shown.contains(value), "{shown}");
        }
        assert_eq!(out.contents(), "");
    }
    // Not a string at all.
    let out = Captured::new();
    let err = within(app(&format!("{KEY} = [1, 2]"), &out).run())
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    // A list-valued mode names the key in its text, never the list's items.
    let out = Captured::new();
    let err = within(app(&format!("{KEY} = [\"zq-list-4410\", 9917]"), &out).run())
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    let shown = format!("{err} {err:?}");
    assert!(shown.contains(KEY), "{shown}");
    assert!(!shown.contains("zq-list-4410"), "{shown}");
    assert!(!shown.contains("9917"), "{shown}");
    assert_eq!(out.contents(), "");
}

pub struct Fast;

#[component]
pub struct FastAutoConfig;

#[auto_configuration(on_property("feature.mode", "fast"))]
impl FastAutoConfig {
    #[bean]
    fn fast(&self) -> Fast {
        Fast
    }
}

#[tokio::test]
async fn report_text_has_no_config_values() {
    const MARKER: &str = "MARKER-5517-secret";
    // A value that contains the expected text still does not match, and is never shown.
    let toml =
        format!("{KEY} = \"console\"\nsecret = \"{MARKER}\"\n[feature]\nmode = \"fast-{MARKER}\"");
    let out = Captured::new();
    within(
        app(&toml, &out)
            .args([format!("--password={MARKER}")])
            .auto_configuration::<FastAutoConfig>()
            .run(),
    )
    .await
    .unwrap();
    let text = out.contents();
    assert!(!text.contains(MARKER), "{text}");
    assert!(!text.contains("fast-"), "{text}");
    assert_eq!(
        text,
        format!(
            "CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n  {}: \
             on_property_value - property \"feature.mode\" does not have the expected value\n",
            type_name::<FastAutoConfig>()
        )
    );
}

/// The exit code and the listener events of a run, with or without the report.
async fn outcome(mode: Option<&str>, fail: bool) -> (Option<StartupPhase>, Vec<String>) {
    let toml = mode.map_or(String::new(), |m| format!("{KEY} = \"{m}\""));
    let out = Captured::new();
    let log = Log::new();
    let mut builder = app(&toml, &out)
        .listener(Events(log.clone()))
        .configure(conditional);
    if fail {
        builder = builder.configure(|b| b.factory(|r: &Resolver| r.get::<Absent>().map(|_| 1u16)));
    }
    let phase = within(builder.run()).await.err().map(|e| e.phase());
    (phase, log.entries())
}

struct Events(Log);

impl RunListener for Events {
    fn starting(&self) {
        self.0.push("starting");
    }
    fn environment_prepared(&self, _: &Config) {
        self.0.push("environment_prepared");
    }
    fn context_ready(&self, _: &Container) {
        self.0.push("context_ready");
    }
    fn started(&self, _: &Container) {
        self.0.push("started");
    }
    fn ready(&self, _: &Container) {
        self.0.push("ready");
    }
    fn stopping(&self) {
        self.0.push("stopping");
    }
    fn stopped(&self, status: &oxbloom_boot::ExitStatus) {
        self.0.push(format!("stopped {}", status.code()));
    }
    fn failed(&self, error: &StartupError) {
        self.0.push(format!("failed {:?}", error.phase()));
    }
}

#[tokio::test]
async fn exit_code_and_events_unchanged_by_report() {
    for fail in [false, true] {
        let off = outcome(None, fail).await;
        assert_eq!(outcome(Some("off"), fail).await, off);
        assert_eq!(outcome(Some("console"), fail).await, off);
        if fail {
            assert_eq!(off.0, Some(StartupPhase::Container));
        } else {
            assert_eq!(off.0, None);
            assert!(off.1.contains(&"stopped 0".to_owned()), "{:?}", off.1);
        }
    }
    // The process exit code of `run_and_exit` does not change either.
    for mode in ["off", "console"] {
        let out = Captured::new();
        let code = within(app(&format!("{KEY} = \"{mode}\""), &out).run_and_exit()).await;
        assert_eq!(code, ExitCode::SUCCESS);
        let out = Captured::new();
        let code = within(
            app(&format!("{KEY} = \"{mode}\""), &out)
                .configure(|b| b.factory(|r: &Resolver| r.get::<Absent>().map(|_| 1u16)))
                .run_and_exit(),
        )
        .await;
        assert_eq!(code, ExitCode::from(1));
    }
}

#[tokio::test]
async fn report_with_zero_conditions_prints_empty_sections() {
    let out = Captured::new();
    within(console(&out).configure(|b| b.instance(1u8)).run())
        .await
        .unwrap();
    assert_eq!(out.contents(), format!("{EMPTY}\n"));
}

#[tokio::test]
async fn console_mode_via_cli_args() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().to_owned();
    let out = Captured::new();
    within(
        OxbloomApplication::builder()
            .args([format!("--{KEY}=console")])
            .config(move |b| b.dir(dir).env_from([]))
            .banner(Banner::off())
            .output(out.clone())
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(out.contents(), format!("{EMPTY}\n"));
}

// --- a 256-deep chain built through boot on a small stack ----------------------------------------

/// One link of the chain: `Link<N>` depends on `Link<N + 1>` while `N + 1 < len`.
struct Link<const N: usize>;

fn link<const N: usize, const NEXT: usize>(b: ContainerBuilder, len: usize) -> ContainerBuilder {
    if N >= len {
        return b;
    }
    b.factory(move |r: &Resolver| {
        if NEXT < len {
            r.get::<Link<NEXT>>()?;
        }
        Ok(Link::<N>)
    })
}

/// Registers `Link<0>` .. `Link<len - 1>` (at most 300) in ascending order, so building
/// `Link<0>` first walks the whole chain depth-first.
macro_rules! links {
    ($b:ident, $len:expr; $($h:literal)*) => {
        $( links!(@tens $b, $len, $h; 0 1 2 3 4 5 6 7 8 9); )*
    };
    (@tens $b:ident, $len:expr, $h:literal; $($t:literal)*) => {
        $( links!(@units $b, $len, $h, $t; 0 1 2 3 4 5 6 7 8 9); )*
    };
    (@units $b:ident, $len:expr, $h:literal, $t:literal; $($u:literal)*) => {
        $( $b = link::<{ $h * 100 + $t * 10 + $u }, { $h * 100 + $t * 10 + $u + 1 }>($b, $len); )*
    };
}

fn chain(mut b: ContainerBuilder, len: usize) -> ContainerBuilder {
    assert!(len <= 300, "chain() generates at most 300 links");
    links!(b, len; 0 1 2);
    b
}

/// Runs a whole application with a 256-deep chain and the report on, on a thread with a
/// `kb` KB stack and a current-thread runtime, and returns the exit code and the output.
fn deep_run(kb: usize) -> (Result<i32, String>, String) {
    std::thread::Builder::new()
        .stack_size(kb * 1024)
        .spawn(|| {
            let out = Captured::new();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("runtime");
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().to_owned();
            let result = runtime.block_on(
                OxbloomApplication::builder()
                    .args([format!("--{KEY}=console")])
                    .config(move |b| b.dir(path).env_from([]))
                    .banner(Banner::off())
                    .output(out.clone())
                    .configure(|b| chain(b, 256).instance(0u8).on_bean::<Link<0>>())
                    .run(),
            );
            (
                result.map(|s| s.code()).map_err(|e| format!("{e:?}")),
                out.contents(),
            )
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked")
}

#[test]
fn deep_chain_builds_on_a_small_stack() {
    let (status, text) = deep_run(512);
    assert_eq!(status, Ok(0));
    assert!(
        text.starts_with("CONDITIONS EVALUATION REPORT\n\nMatched:\n  instance u8: on_bean"),
        "{text}"
    );
}

/// Manual probe for the smallest stack that survives a 256-deep build through boot with the
/// report on: `OXBLOOM_BOOT_STACK_KB=<kb> cargo test -p oxbloom-boot --test conditions_report
/// -- --ignored probe`. A stack that is too small aborts the test process with a stack overflow.
#[test]
#[ignore = "manual stack-size probe; set OXBLOOM_BOOT_STACK_KB"]
fn probe_boot_stack_for_256_deep_build() {
    let kb: usize = std::env::var("OXBLOOM_BOOT_STACK_KB")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    assert_eq!(deep_run(kb).0, Ok(0));
}
