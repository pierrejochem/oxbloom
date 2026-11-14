mod common;

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use oxbloom_boot::async_trait;
use oxbloom_boot::{
    Banner, CommandLineRunner, FailureAnalysis, FailureAnalyzer, OxbloomApplication,
    OxbloomApplicationBuilder, RunListener, RunnerError, StartupError, StartupPhase,
};
use oxbloom_config::Config;
use oxbloom_di::DiError;
use oxbloom_test::{Captured, Log};

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

const STARS: &str = "***************************";
const BARE: &str = "startup failed in the runner phase: runner broke";

/// The rendered block for `description` and an optional `action`.
fn block(description: &str, action: Option<&str>) -> String {
    let mut s =
        format!("{STARS}\nAPPLICATION FAILED TO START\n{STARS}\n\nDescription:\n\n{description}");
    if let Some(action) = action {
        s.push_str(&format!("\n\nAction:\n\n{action}"));
    }
    s
}

/// A command-line runner that always fails.
struct Broken;

#[async_trait]
impl CommandLineRunner for Broken {
    async fn run(&self, _: &[String]) -> Result<(), RunnerError> {
        Err(RunnerError::new("runner broke"))
    }
}

/// A builder whose run fails in the runner phase, writing to `out`.
fn failing(out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str("").unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .configure(|b| {
            b.instance(Broken)
                .provides(|r: Arc<Broken>| -> Arc<dyn CommandLineRunner> { r })
        })
}

/// Logs its label and answers with `answer`.
struct Labeled {
    label: &'static str,
    log: Log,
    answer: Option<FailureAnalysis>,
}

impl Labeled {
    fn none(label: &'static str, log: &Log) -> Self {
        Self {
            label,
            log: log.clone(),
            answer: None,
        }
    }

    fn some(label: &'static str, log: &Log) -> Self {
        Self {
            label,
            log: log.clone(),
            answer: Some(FailureAnalysis::new(format!("found by {label}"), "fix it")),
        }
    }
}

impl FailureAnalyzer for Labeled {
    fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
        self.log.push(self.label);
        self.answer.clone()
    }
}

/// Always answers with a fixed analysis.
struct Fixed(&'static str, Option<&'static str>);

impl FailureAnalyzer for Fixed {
    fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
        Some(match self.1 {
            Some(action) => FailureAnalysis::new(self.0, action),
            None => FailureAnalysis::with_no_action(self.0),
        })
    }
}

struct Panicker;

impl FailureAnalyzer for Panicker {
    fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
        panic!("analyzer-secret-marker");
    }
}

// ----- rendering -----

#[test]
fn display_with_action() {
    let a = FailureAnalysis::new("It broke.", "Fix it.");
    assert_eq!(a.to_string(), block("It broke.", Some("Fix it.")));
    assert_eq!(a.description(), "It broke.");
    assert_eq!(a.action(), Some("Fix it."));
}

#[test]
fn display_without_action() {
    let a = FailureAnalysis::with_no_action("It broke.");
    assert_eq!(a.to_string(), block("It broke.", None));
    assert_eq!(a.action(), None);
    assert!(!a.to_string().ends_with('\n'));
}

#[tokio::test]
async fn generic_report_for_unanalyzed_failure() {
    let out = Captured::new();
    within(failing(&out).run_and_exit()).await;
    assert_eq!(out.contents(), format!("{}\n", block(BARE, None)));
}

// ----- find_cause -----

#[derive(Debug)]
struct Leaf(&'static str);

impl fmt::Display for Leaf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for Leaf {}

#[derive(Debug)]
struct Wrap(Leaf);

impl fmt::Display for Wrap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("wrap")
    }
}

impl Error for Wrap {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// An error whose `source()` is itself.
#[derive(Debug)]
struct Cyclic;

impl fmt::Display for Cyclic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cyclic")
    }
}

impl Error for Cyclic {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self)
    }
}

#[test]
fn find_cause_returns_first_match() {
    let e = StartupError::new(StartupPhase::Service, Wrap(Leaf("inner")));
    assert_eq!(e.find_cause::<Wrap>().map(|w| w.0.0), Some("inner"));
    assert_eq!(e.find_cause::<Leaf>().map(|l| l.0), Some("inner"));
    // Through a forwarding error too.
    let e = StartupError::new(StartupPhase::Runner, RunnerError::new(Leaf("deep")));
    assert_eq!(e.find_cause::<Leaf>().map(|l| l.0), Some("deep"));
    assert!(e.find_cause::<RunnerError>().is_some());
}

#[test]
fn find_cause_none_for_absent_type() {
    let e = StartupError::new(StartupPhase::Config, Leaf("x"));
    assert!(e.find_cause::<Wrap>().is_none());
    assert!(e.find_cause::<DiError>().is_none());
    assert!(e.analysis().is_none());
}

#[test]
fn find_cause_stops_on_cyclic_chain() {
    let e = StartupError::new(StartupPhase::Config, Cyclic);
    assert!(e.find_cause::<Leaf>().is_none());
    assert!(e.find_cause::<Cyclic>().is_some());
}

// ----- matching -----

#[tokio::test]
async fn builder_analyzers_run_in_call_order() {
    let log = Log::new();
    let out = Captured::new();
    let err = within(
        failing(&out)
            .failure_analyzer(Labeled::none("a", &log))
            .failure_analyzer(Labeled::some("b", &log))
            .failure_analyzer(Labeled::some("c", &log))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(log.entries(), ["a", "b"]);
    assert_eq!(err.analysis().unwrap().description(), "found by b");
}

#[tokio::test]
async fn bean_analyzers_follow_builder_analyzers() {
    let log = Log::new();
    let out = Captured::new();
    let l1 = log.clone();
    let err = within(
        failing(&out)
            .failure_analyzer(Labeled::none("builder", &log))
            .configure(move |b| {
                b.instance(Labeled::none("bean1", &l1))
                    .provides(|a: Arc<Labeled>| -> Arc<dyn FailureAnalyzer> { a })
                    .instance(Fixed("found by bean2", None))
                    .provides(|a: Arc<Fixed>| -> Arc<dyn FailureAnalyzer> { a })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(log.entries(), ["builder", "bean1"]);
    assert_eq!(err.analysis().unwrap().description(), "found by bean2");
}

#[tokio::test]
async fn bean_analyzer_ignored_without_container() {
    let log = Log::new();
    let out = Captured::new();
    let l = log.clone();
    // An invalid banner mode fails the config phase, before any container.
    let err = within(
        OxbloomApplication::builder()
            .with_config(Config::from_str("oxbloom.main.banner-mode = \"loud\"").unwrap())
            .output(out.clone())
            .failure_analyzer(Labeled::some("builder", &log))
            .configure(move |b| {
                b.instance(Labeled::some("bean", &l))
                    .provides(|a: Arc<Labeled>| -> Arc<dyn FailureAnalyzer> { a })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    assert_eq!(log.entries(), ["builder"]);
    assert_eq!(err.analysis().unwrap().description(), "found by builder");
}

#[derive(Debug)]
struct Boom;

struct NeedsBoom;

impl FailureAnalyzer for NeedsBoom {
    fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
        Some(FailureAnalysis::with_no_action("unreachable"))
    }
}

#[tokio::test]
async fn failing_get_all_for_analyzer_beans_is_ignored() {
    let log = Log::new();
    let out = Captured::new();
    let err = within(
        failing(&out)
            .failure_report(true)
            .failure_analyzer(Labeled::none("builder", &log))
            .configure(|b| {
                b.try_bean(|| -> Result<Boom, DiError> { Err(DiError::creation::<Boom>("boom")) })
                    .lazy()
                    .bean(|_: Arc<Boom>| NeedsBoom)
                    .lazy()
                    .provides(|n: Arc<NeedsBoom>| -> Arc<dyn FailureAnalyzer> { n })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Runner);
    assert_eq!(err.to_string(), BARE);
    assert_eq!(log.entries(), ["builder"]);
    assert!(err.analysis().is_none());
    assert_eq!(out.contents(), format!("{}\n", block(BARE, None)));
}

#[tokio::test]
async fn panicking_analyzer_is_skipped_and_named() {
    let out = Captured::new();
    let err = within(
        failing(&out)
            .failure_analyzer(Panicker)
            .failure_analyzer(Fixed("after the panic", None))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.analysis().unwrap().description(), "after the panic");
    let text = out.contents();
    assert_eq!(
        text,
        format!("analyzer {} panicked\n", std::any::type_name::<Panicker>())
    );
    assert!(!text.contains("analyzer-secret-marker"));
}

struct SeesAnalysis(Log);

impl RunListener for SeesAnalysis {
    fn failed(&self, e: &StartupError) {
        let seen = e.analysis().map_or("none", |a| a.description());
        self.0.push(format!("failed: {seen}"));
    }
}

#[tokio::test]
async fn failed_listener_sees_analysis() {
    let log = Log::new();
    let out = Captured::new();
    within(
        failing(&out)
            .listener(SeesAnalysis(log.clone()))
            .failure_analyzer(Fixed("seen", None))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(log.entries(), ["failed: seen"]);
}

// ----- printing -----

#[tokio::test]
async fn run_is_silent_by_default() {
    let out = Captured::new();
    within(failing(&out).failure_analyzer(Fixed("quiet", None)).run())
        .await
        .unwrap_err();
    assert_eq!(out.contents(), "");
}

#[tokio::test]
async fn run_prints_report_once_when_flag_true() {
    let out = Captured::new();
    let err = within(
        failing(&out)
            .failure_report(true)
            .failure_analyzer(Fixed("loud", Some("act")))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(out.contents(), format!("{}\n", block("loud", Some("act"))));
    assert_eq!(err.to_string(), BARE);
}

#[tokio::test]
async fn run_and_exit_prints_report_not_bare_line() {
    let out = Captured::new();
    within(
        failing(&out)
            .failure_analyzer(Fixed("analyzed", Some("act")))
            .run_and_exit(),
    )
    .await;
    assert_eq!(
        out.contents(),
        format!("{}\n", block("analyzed", Some("act")))
    );
    assert!(!out.contents().contains(BARE));
}

#[tokio::test]
async fn run_and_exit_with_flag_false_prints_bare_line() {
    let out = Captured::new();
    within(
        failing(&out)
            .failure_report(false)
            .failure_analyzer(Fixed("analyzed", Some("act")))
            .run_and_exit(),
    )
    .await;
    assert_eq!(out.contents(), format!("{BARE}\n"));
}

#[tokio::test]
async fn no_double_print() {
    let out = Captured::new();
    within(
        failing(&out)
            .failure_report(true)
            .failure_analyzer(Fixed("once", None))
            .run_and_exit(),
    )
    .await;
    let text = out.contents();
    assert_eq!(text.matches("APPLICATION FAILED TO START").count(), 1);
    assert_eq!(text.lines().filter(|l| *l == STARS).count(), 2);
    assert!(!text.contains(BARE));
    assert_eq!(text, format!("{}\n", block("once", None)));
}

#[tokio::test]
async fn newline_in_description_keeps_block_structure() {
    let out = Captured::new();
    within(
        failing(&out)
            .failure_analyzer(Fixed("line one\nline two", Some("step one\nstep two")))
            .run_and_exit(),
    )
    .await;
    let text = out.contents();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            STARS,
            "APPLICATION FAILED TO START",
            STARS,
            "",
            "Description:",
            "",
            "line one",
            "line two",
            "",
            "Action:",
            "",
            "step one",
            "step two",
        ]
    );
    assert_eq!(text.matches("APPLICATION FAILED TO START").count(), 1);
    assert!(text.ends_with("step two\n"));
}

#[tokio::test]
async fn report_does_not_change_error_or_exit_code() {
    let plain = within(failing(&Captured::new()).run()).await.unwrap_err();
    let analyzed = within(
        failing(&Captured::new())
            .failure_report(true)
            .failure_analyzer(Fixed("x", Some("y")))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(plain.phase(), analyzed.phase());
    assert_eq!(plain.to_string(), analyzed.to_string());
    assert_eq!(format!("{plain:?}"), format!("{analyzed:?}"));
    assert!(analyzed.find_cause::<RunnerError>().is_some());

    let code = within(
        failing(&Captured::new())
            .failure_analyzer(Fixed("x", Some("y")))
            .run_and_exit(),
    )
    .await;
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(1)));
}

struct PanicsOnCreate;

impl FailureAnalyzer for PanicsOnCreate {
    fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
        Some(FailureAnalysis::with_no_action("unreachable"))
    }
}

struct CountFailed(Log);

impl RunListener for CountFailed {
    fn failed(&self, _: &StartupError) {
        self.0.push("failed");
    }
}

struct Probe;

#[tokio::test]
async fn panicking_analyzer_bean_collection_is_ignored() {
    // The process-wide panic hook is left alone: swapping it would race with
    // the other tests of this binary. The panic message goes to stderr only.
    let log = Log::new();
    let out = Captured::new();
    let l = log.clone();
    let err = within(
        failing(&out)
            .failure_report(true)
            .listener(CountFailed(log.clone()))
            .configure(move |b| {
                b.instance(Probe)
                    .pre_destroy(move |_: &Probe| l.push("pre_destroy"))
                    .bean(|| -> PanicsOnCreate { panic!("factory-secret-marker") })
                    .lazy()
                    .provides(|p: Arc<PanicsOnCreate>| -> Arc<dyn FailureAnalyzer> { p })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Runner);
    assert!(err.analysis().is_none());
    assert_eq!(log.entries(), ["failed", "pre_destroy"]);
    assert_eq!(
        out.contents(),
        format!("failure analyzer beans panicked\n{}\n", block(BARE, None))
    );
}

/// Records whether the report is already in the output when `failed` fires.
struct ReportNotYet(Captured, Log);

impl RunListener for ReportNotYet {
    fn failed(&self, _: &StartupError) {
        let printed = self.0.contents().contains("APPLICATION FAILED TO START");
        self.1.push(format!("report printed: {printed}"));
    }
}

#[tokio::test]
async fn report_is_printed_last() {
    let log = Log::new();
    let out = Captured::new();
    let o = out.clone();
    within(
        failing(&out)
            .listener(ReportNotYet(out.clone(), log.clone()))
            .failure_analyzer(Fixed("last", None))
            .configure(move |b| {
                b.instance(Probe).pre_destroy(move |_: &Probe| {
                    let mut w = o.clone();
                    std::io::Write::write_all(&mut w, b"pre_destroy ran\n").unwrap();
                })
            })
            .run_and_exit(),
    )
    .await;
    assert_eq!(log.entries(), ["report printed: false"]);
    assert_eq!(
        out.contents(),
        format!("pre_destroy ran\n{}\n", block("last", None))
    );
}

struct BadName;

impl FailureAnalyzer for BadName {
    fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
        panic!("analyze");
    }
    fn name(&self) -> &'static str {
        panic!("name");
    }
}

#[tokio::test]
async fn panicking_analyzer_name_is_unknown() {
    let out = Captured::new();
    let err = within(
        failing(&out)
            .failure_analyzer(BadName)
            .failure_analyzer(Fixed("after", None))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.analysis().unwrap().description(), "after");
    assert_eq!(out.contents(), "analyzer <unknown> panicked\n");
}
