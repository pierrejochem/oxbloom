mod common;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use oxbloom_boot::async_trait;
use oxbloom_boot::{
    ApplicationArguments, ApplicationRunner, CommandLineRunner, ExitStatus, OxbloomApplication,
    OxbloomApplicationBuilder, RunListener, RunnerError, ShutdownHandle, StartupError,
    StartupPhase,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{Container, DiError};
use oxbloom_test::{Captured, Log};

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

/// Logs every lifecycle phase.
struct Recorder(Log);

impl RunListener for Recorder {
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
    fn stopped(&self, _: &ExitStatus) {
        self.0.push("stopped");
    }
    fn failed(&self, e: &StartupError) {
        self.0.push(format!("failed {}", e.phase()));
    }
}

/// Runs a closure on `context_ready`.
struct OnReady<F>(F);

impl<F: Fn(&Container) + Send + Sync + 'static> RunListener for OnReady<F> {
    fn context_ready(&self, c: &Container) {
        (self.0)(c)
    }
}

/// Runs a closure on `environment_prepared`.
struct OnEnv<F>(F);

impl<F: Fn(&Config) + Send + Sync + 'static> RunListener for OnEnv<F> {
    fn environment_prepared(&self, c: &Config) {
        (self.0)(c)
    }
}

struct AppRunner<const N: u8>(Log);

#[async_trait]
impl<const N: u8> ApplicationRunner for AppRunner<N> {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        self.0.push(format!("app{N}"));
        Ok(())
    }
}

struct CmdRunner<const N: u8>(Log);

#[async_trait]
impl<const N: u8> CommandLineRunner for CmdRunner<N> {
    async fn run(&self, args: &[String]) -> Result<(), RunnerError> {
        self.0.push(format!("cmd{N} {}", args.len()));
        Ok(())
    }
}

struct LabelApp(Log);

#[async_trait]
impl ApplicationRunner for LabelApp {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        self.0.push("runner:app");
        Ok(())
    }
}

struct LabelCmd(Log);

#[async_trait]
impl CommandLineRunner for LabelCmd {
    async fn run(&self, _: &[String]) -> Result<(), RunnerError> {
        self.0.push("runner:cmdline");
        Ok(())
    }
}

#[derive(Debug)]
struct UserFailure;

impl std::fmt::Display for UserFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("user failure")
    }
}

impl std::error::Error for UserFailure {}

struct FailingRunner;

#[async_trait]
impl ApplicationRunner for FailingRunner {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        Err(RunnerError::new(UserFailure))
    }
}

/// A bean whose `pre_destroy` hook logs, to observe `Container::shutdown`.
struct Probe;

fn base() -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str("").unwrap())
        .output(Captured::new())
}

fn failures(log: &Log) -> usize {
    log.entries()
        .iter()
        .filter(|e| e.starts_with("failed"))
        .count()
}

#[tokio::test]
async fn event_order_without_services() {
    let log = Log::new();
    let (l1, l2) = (log.clone(), log.clone());
    let status = within(
        base()
            .listener(Recorder(log.clone()))
            .configure(move |b| {
                b.instance(LabelCmd(l1))
                    .provides(|r: Arc<LabelCmd>| -> Arc<dyn CommandLineRunner> { r })
                    .instance(LabelApp(l2))
                    .provides(|r: Arc<LabelApp>| -> Arc<dyn ApplicationRunner> { r })
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(
        log.entries(),
        [
            "starting",
            "environment_prepared",
            "context_ready",
            "runner:app",
            "runner:cmdline",
            "started",
            "ready",
            "stopping",
            "stopped",
        ]
    );
}

#[tokio::test]
async fn runners_follow_di_order() {
    let log = Log::new();
    let (l, l3) = (log.clone(), log.clone());
    within(
        base()
            .args(["--a=1", "x"])
            .configure(move |b| {
                b.instance(AppRunner::<2>(l.clone()))
                    .provides(|r: Arc<AppRunner<2>>| -> Arc<dyn ApplicationRunner> { r })
                    .order(2)
                    .instance(AppRunner::<1>(l.clone()))
                    .provides(|r: Arc<AppRunner<1>>| -> Arc<dyn ApplicationRunner> { r })
                    .order(1)
                    .instance(CmdRunner::<1>(l.clone()))
                    .provides(|r: Arc<CmdRunner<1>>| -> Arc<dyn CommandLineRunner> { r })
                    .instance(CmdRunner::<2>(l.clone()))
                    .provides(|r: Arc<CmdRunner<2>>| -> Arc<dyn CommandLineRunner> { r })
            })
            // A second `configure` applies after the first.
            .configure(move |b| {
                b.instance(AppRunner::<3>(l3))
                    .provides(|r: Arc<AppRunner<3>>| -> Arc<dyn ApplicationRunner> { r })
                    .order(3)
            })
            .run(),
    )
    .await
    .unwrap();
    // Command-line runners receive the raw arguments (2 here).
    assert_eq!(log.entries(), ["app1", "app2", "app3", "cmd1 2", "cmd2 2"]);
}

#[tokio::test]
async fn config_error_is_config_phase() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application.toml"), "this is = = not toml").unwrap();
    let path = dir.path().to_owned();
    let log = Log::new();
    let err = within(
        OxbloomApplication::builder()
            .output(Captured::new())
            .listener(Recorder(log.clone()))
            .config(move |b| b.dir(path).env_from([]))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    let source = std::error::Error::source(&err).unwrap();
    assert!(source.downcast_ref::<ConfigError>().is_some(), "{source}");
    assert_eq!(log.entries(), ["starting", "failed config"]);
}

#[tokio::test]
async fn duplicate_boot_instance_is_container_phase() {
    let log = Log::new();
    let err = within(
        base()
            .listener(Recorder(log.clone()))
            .configure(|b| b.instance(Config::from_str("").unwrap()))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    let source = std::error::Error::source(&err).unwrap();
    assert!(
        matches!(
            source.downcast_ref::<DiError>(),
            Some(DiError::Duplicate { .. })
        ),
        "{source}"
    );
    assert_eq!(
        log.entries(),
        ["starting", "environment_prepared", "failed container"]
    );
}

#[tokio::test]
async fn runner_error_is_runner_phase_and_container_shut_down() {
    let log = Log::new();
    let (l1, l2) = (log.clone(), log.clone());
    let err = within(
        base()
            .listener(Recorder(log.clone()))
            .configure(move |b| {
                b.instance(Probe)
                    .pre_destroy(move |_: &Probe| l1.push("pre_destroy"))
                    .instance(FailingRunner)
                    .provides(|r: Arc<FailingRunner>| -> Arc<dyn ApplicationRunner> { r })
                    .instance(LabelCmd(l2))
                    .provides(|r: Arc<LabelCmd>| -> Arc<dyn CommandLineRunner> { r })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Runner);
    let runner = std::error::Error::source(&err)
        .unwrap()
        .downcast_ref::<RunnerError>()
        .expect("source is the RunnerError");
    let user = std::error::Error::source(runner).unwrap();
    assert!(user.downcast_ref::<UserFailure>().is_some());
    assert_eq!(
        err.to_string(),
        "startup failed in the runner phase: user failure"
    );
    assert_eq!(
        log.entries(),
        [
            "starting",
            "environment_prepared",
            "context_ready",
            "failed runner",
            "pre_destroy",
        ]
    );
}

#[tokio::test]
async fn failed_fires_once_per_failure() {
    // Container phase.
    let log = Log::new();
    let bean_log = Log::new();
    let bl = bean_log.clone();
    within(
        base()
            .listener(Recorder(log.clone()))
            .configure(|b| b.instance(ShutdownHandle::new()))
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(failures(&log), 1);

    // Runner phase, with a bean listener as well: each listener sees `failed` once.
    let log = Log::new();
    within(
        base()
            .listener(Recorder(log.clone()))
            .configure(move |b| {
                b.instance(Recorder(bl))
                    .provides(|r: Arc<Recorder>| -> Arc<dyn RunListener> { r })
                    .instance(FailingRunner)
                    .provides(|r: Arc<FailingRunner>| -> Arc<dyn ApplicationRunner> { r })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(failures(&log), 1);
    assert_eq!(failures(&bean_log), 1);
    assert!(!log.entries().iter().any(|e| e == "stopped"));
}

struct Boom;
struct NeedsBoom;

impl RunListener for NeedsBoom {}

#[tokio::test]
async fn listener_bean_failing_lazy_dependency_is_container_phase() {
    let log = Log::new();
    let l = log.clone();
    let err = within(
        base()
            .listener(Recorder(log.clone()))
            .configure(move |b| {
                b.instance(Probe)
                    .pre_destroy(move |_: &Probe| l.push("pre_destroy"))
                    .try_bean(|| -> Result<Boom, DiError> {
                        Err(DiError::creation::<Boom>("boom"))
                    })
                    .lazy()
                    .bean(|_: Arc<Boom>| NeedsBoom)
                    .lazy()
                    .provides(|n: Arc<NeedsBoom>| -> Arc<dyn RunListener> { n })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    assert!(
        std::error::Error::source(&err)
            .unwrap()
            .downcast_ref::<DiError>()
            .is_some()
    );
    assert_eq!(
        log.entries(),
        [
            "starting",
            "environment_prepared",
            "failed container",
            "pre_destroy"
        ]
    );
}

#[tokio::test]
async fn builder_listener_gets_early_phases_bean_listener_does_not() {
    let early = Log::new();
    let bean = Log::new();
    let b2 = bean.clone();
    within(
        base()
            .listener(Recorder(early.clone()))
            .configure(move |b| {
                b.instance(Recorder(b2))
                    .provides(|r: Arc<Recorder>| -> Arc<dyn RunListener> { r })
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(
        early.entries(),
        [
            "starting",
            "environment_prepared",
            "context_ready",
            "started",
            "ready",
            "stopping",
            "stopped"
        ]
    );
    assert_eq!(
        bean.entries(),
        ["context_ready", "started", "ready", "stopping", "stopped"]
    );
}

struct Mode(&'static str);

#[tokio::test]
async fn active_profiles_flow_to_container() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application-dev.toml"), "").unwrap();
    let path = dir.path().to_owned();
    let log = Log::new();
    let l = log.clone();
    within(
        OxbloomApplication::builder()
            .output(Captured::new())
            .args(["--oxbloom.profiles.active=dev"])
            .config(move |b| b.dir(path).env_from([]))
            .configure(|b| {
                b.instance(Mode("dev"))
                    .profile("dev")
                    .instance(Mode("default"))
                    .profile_not("dev")
            })
            .listener(OnReady(move |c: &Container| {
                l.push(c.get::<Mode>().unwrap().0);
            }))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(log.entries(), ["dev"]);
}

struct Seen(String);

#[tokio::test]
async fn arguments_and_config_are_injectable() {
    let log = Log::new();
    let l = log.clone();
    within(
        OxbloomApplication::builder()
            .output(Captured::new())
            .with_config(Config::from_str("app.name = \"demo\"").unwrap())
            .args(["--who=me", "rest"])
            .configure(|b| {
                b.bean(
                    |a: Arc<ApplicationArguments>, c: Arc<Config>, _: Arc<ShutdownHandle>| {
                        Seen(format!(
                            "{} {} {}",
                            a.option("who").unwrap(),
                            a.non_option_args().len(),
                            c.get::<String>("app.name").unwrap()
                        ))
                    },
                )
            })
            .listener(OnReady(move |c: &Container| {
                l.push(&c.get::<Seen>().unwrap().0);
            }))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(log.entries(), ["me 1 demo"]);
}

#[tokio::test]
async fn with_config_uses_ready_config() {
    let log = Log::new();
    let (l1, l2) = (log.clone(), log.clone());
    // `with_config` after `config`: the ready config wins.
    within(
        OxbloomApplication::builder()
            .output(Captured::new())
            .config(|b| b.dir("/nonexistent-oxbloom-dir").env_from([]))
            .with_config(Config::from_str("greeting = \"hi\"").unwrap())
            .listener(OnEnv(move |c: &Config| {
                l1.push(c.get::<String>("greeting").unwrap());
            }))
            .run(),
    )
    .await
    .unwrap();
    // `config` after `with_config`: the builder wins.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application.toml"), "greeting = \"file\"").unwrap();
    let path = dir.path().to_owned();
    within(
        OxbloomApplication::builder()
            .output(Captured::new())
            .with_config(Config::from_str("greeting = \"hi\"").unwrap())
            .config(move |b| b.dir(path).env_from([]))
            .listener(OnEnv(move |c: &Config| {
                l2.push(c.get::<String>("greeting").unwrap());
            }))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(log.entries(), ["hi", "file"]);
}

#[tokio::test]
async fn shutdown_handle_triggered_before_run_is_remembered() {
    let log = Log::new();
    let l = log.clone();
    let builder = base();
    let handle = builder.shutdown_handle();
    handle.trigger();
    let status = within(
        builder
            .listener(OnReady(move |c: &Container| {
                let h = c.get::<ShutdownHandle>().unwrap();
                l.push(format!("triggered {}", h.is_triggered()));
            }))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 0);
    assert!(handle.is_triggered());
    assert_eq!(log.entries(), ["triggered true"]);
}

#[tokio::test]
async fn run_future_is_send() {
    fn assert_send<T: Send>(t: T) -> T {
        t
    }
    let status = within(tokio::spawn(assert_send(base().run())))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status, ExitStatus::new(0));
}

#[tokio::test]
async fn config_closure_cannot_drop_boot_arguments() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application-dev.toml"), "").unwrap();
    let path = dir.path().to_owned();
    let log = Log::new();
    let l = log.clone();
    within(
        OxbloomApplication::builder()
            .output(Captured::new())
            .args(["--oxbloom.profiles.active=dev"])
            .config(move |b| b.dir(path).env_from([]).args(["--other=1"]))
            .configure(|b| {
                b.instance(Mode("dev"))
                    .profile("dev")
                    .instance(Mode("default"))
                    .profile_not("dev")
            })
            .listener(OnReady(move |c: &Container| {
                l.push(c.get::<Mode>().unwrap().0);
            }))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(log.entries(), ["dev"]);
}
