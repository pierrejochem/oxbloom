mod common;

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxbloom_boot::async_trait;
use oxbloom_boot::{
    ApplicationArguments, ApplicationRunner, ExitStatus, OxbloomApplication,
    OxbloomApplicationBuilder, RunListener, RunnerError, Service, ServiceError, ShutdownHandle,
    StartupError, StartupPhase,
};
use oxbloom_config::Config;
use oxbloom_di::Container;
use oxbloom_test::{Captured, Log};

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(2), f)
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

#[derive(Clone, Copy)]
enum Kind {
    Ok,
    FailStart,
    PanicStop,
}

/// A service that logs `start:{name}` and `stop:{name}`.
struct Svc<const N: u8> {
    name: &'static str,
    phase: i32,
    kind: Kind,
    log: Log,
}

impl<const N: u8> Svc<N> {
    fn new(name: &'static str, phase: i32, log: &Log) -> Self {
        Self {
            name,
            phase,
            kind: Kind::Ok,
            log: log.clone(),
        }
    }

    fn kind(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }
}

#[derive(Debug)]
struct PortInUse;

impl std::fmt::Display for PortInUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("port in use")
    }
}

impl std::error::Error for PortInUse {}

#[async_trait]
impl<const N: u8> Service for Svc<N> {
    async fn start(&self) -> Result<(), ServiceError> {
        if let Kind::FailStart = self.kind {
            self.log.push(format!("fail:{}", self.name));
            return Err(ServiceError::new(PortInUse));
        }
        self.log.push(format!("start:{}", self.name));
        Ok(())
    }

    async fn stop(&self) {
        if let Kind::PanicStop = self.kind {
            self.log.push(format!("panic:{}", self.name));
            panic!("stop failed");
        }
        self.log.push(format!("stop:{}", self.name));
    }

    fn phase(&self) -> i32 {
        self.phase
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

/// A service whose `stop` takes 10 seconds; uses the default `name`.
struct Slow(Log);

#[async_trait]
impl Service for Slow {
    async fn start(&self) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn stop(&self) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        self.0.push("slow stopped");
    }
}

/// Logs `dropped` when dropped, to observe that an abandoned stop is aborted.
struct DropGuard(Log);

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.push("dropped");
    }
}

/// Like [`Slow`], but holds a [`DropGuard`] while it sleeps.
struct Guarded(Log);

#[async_trait]
impl Service for Guarded {
    async fn start(&self) -> Result<(), ServiceError> {
        Ok(())
    }

    async fn stop(&self) {
        let _guard = DropGuard(self.0.clone());
        tokio::time::sleep(Duration::from_secs(10)).await;
        self.0.push("guarded stopped");
    }
}

/// Triggers the injected `ShutdownHandle`.
struct Trigger(Arc<ShutdownHandle>, Log);

#[async_trait]
impl ApplicationRunner for Trigger {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        self.0.trigger();
        self.1.push("runner");
        Ok(())
    }
}

struct FailingRunner;

#[async_trait]
impl ApplicationRunner for FailingRunner {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        Err(RunnerError::new("runner failure"))
    }
}

/// A bean whose `pre_destroy` hook logs, to observe `Container::shutdown`.
struct Probe;

fn base() -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str("").unwrap())
        .output(Captured::new())
}

/// A builder whose shutdown handle is already triggered, so `run` stops the
/// services right after starting them.
fn triggered() -> OxbloomApplicationBuilder {
    let b = base();
    b.shutdown_handle().trigger();
    b
}

macro_rules! service {
    ($b:expr, $svc:expr, $ty:ty) => {
        $b.instance($svc)
            .provides(|s: Arc<$ty>| -> Arc<dyn Service> { s })
    };
}

#[tokio::test]
async fn start_order_by_phase_then_registration_stop_reversed() {
    let log = Log::new();
    let l = log.clone();
    within(
        triggered()
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("a", 10, &l), Svc<1>);
                let b = service!(b, Svc::<2>::new("b", -5, &l), Svc<2>);
                let b = service!(b, Svc::<3>::new("c", 10, &l), Svc<3>);
                service!(b, Svc::<4>::new("d", 0, &l), Svc<4>)
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(
        log.entries(),
        [
            "start:b", "start:d", "start:a", "start:c", "stop:c", "stop:a", "stop:d", "stop:b",
        ]
    );
}

#[tokio::test]
async fn equal_phase_and_order_keep_registration_order() {
    let log = Log::new();
    let l = log.clone();
    within(
        triggered()
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("first", 0, &l), Svc<1>).order(3);
                let b = service!(b, Svc::<2>::new("second", 0, &l), Svc<2>).order(3);
                service!(b, Svc::<3>::new("third", 0, &l), Svc<3>).order(3)
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(
        log.entries(),
        [
            "start:first",
            "start:second",
            "start:third",
            "stop:third",
            "stop:second",
            "stop:first",
        ]
    );
}

#[tokio::test]
async fn start_error_stops_earlier_services_and_is_service_phase() {
    let log = Log::new();
    let l = log.clone();
    let err = within(
        base()
            .listener(Recorder(log.clone()))
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("a", 0, &l), Svc<1>);
                let b = service!(b, Svc::<2>::new("b", 1, &l), Svc<2>);
                let b = service!(b, Svc::<3>::new("bad", 2, &l).kind(Kind::FailStart), Svc<3>);
                let b = service!(b, Svc::<4>::new("never", 3, &l), Svc<4>);
                b.instance(Probe)
                    .pre_destroy(move |_: &Probe| l.push("pre_destroy"))
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Service);
    assert_eq!(
        err.to_string(),
        "startup failed in the service phase: port in use"
    );
    let service = std::error::Error::source(&err)
        .unwrap()
        .downcast_ref::<ServiceError>()
        .expect("source is the ServiceError");
    assert!(
        std::error::Error::source(service)
            .unwrap()
            .downcast_ref::<PortInUse>()
            .is_some()
    );
    assert_eq!(
        log.entries(),
        [
            "starting",
            "environment_prepared",
            "context_ready",
            "start:a",
            "start:b",
            "fail:bad",
            "failed service",
            "stop:b",
            "stop:a",
            "pre_destroy",
        ]
    );
}

#[tokio::test]
async fn runner_error_starts_no_service() {
    let log = Log::new();
    let l = log.clone();
    let err = within(
        base()
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("a", 0, &l), Svc<1>);
                b.instance(FailingRunner)
                    .provides(|r: Arc<FailingRunner>| -> Arc<dyn ApplicationRunner> { r })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Runner);
    assert!(log.entries().is_empty(), "{:?}", log.entries());
}

#[tokio::test]
async fn application_with_no_service_returns_without_waiting() {
    let log = Log::new();
    // The handle is never triggered: a wait would hang until the watchdog fires.
    let status = within(base().listener(Recorder(log.clone())).run())
        .await
        .unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(
        log.entries(),
        [
            "starting",
            "environment_prepared",
            "context_ready",
            "started",
            "ready",
            "stopping",
            "stopped",
        ]
    );
}

#[tokio::test]
async fn trigger_from_runner_stops_services_once_in_reverse_order() {
    let log = Log::new();
    let l = log.clone();
    let status = within(
        base()
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("a", 0, &l), Svc<1>);
                let b = service!(b, Svc::<2>::new("b", 1, &l), Svc<2>);
                let l = l.clone();
                b.bean(move |h: Arc<ShutdownHandle>| Trigger(h, l.clone()))
                    .provides(|r: Arc<Trigger>| -> Arc<dyn ApplicationRunner> { r })
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(
        log.entries(),
        ["runner", "start:a", "start:b", "stop:b", "stop:a"]
    );
}

#[tokio::test]
async fn trigger_while_waiting_stops_services() {
    let log = Log::new();
    let l = log.clone();
    let builder = base();
    let handle = builder.shutdown_handle();
    let run = tokio::spawn(
        builder
            .configure(move |b| service!(b, Svc::<1>::new("a", 0, &l), Svc<1>))
            .run(),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!run.is_finished(), "run must wait for the shutdown trigger");
    assert_eq!(log.entries(), ["start:a"]);
    handle.trigger();
    let status = within(run).await.unwrap().unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(log.entries(), ["start:a", "stop:a"]);
}

#[tokio::test]
async fn stop_panic_does_not_skip_later_services_or_container_shutdown() {
    let log = Log::new();
    let l = log.clone();
    let out = Captured::new();
    let status = within(
        triggered()
            .output(out.clone())
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("a", 0, &l), Svc<1>);
                let b = service!(
                    b,
                    Svc::<2>::new("boom", 1, &l).kind(Kind::PanicStop),
                    Svc<2>
                );
                let b = service!(b, Svc::<3>::new("c", 2, &l), Svc<3>);
                b.instance(Probe)
                    .pre_destroy(move |_: &Probe| l.push("pre_destroy"))
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(
        log.entries(),
        [
            "start:a",
            "start:boom",
            "start:c",
            "stop:c",
            "panic:boom",
            "stop:a",
            "pre_destroy",
        ]
    );
    assert!(
        out.contents()
            .contains("service boom panicked while stopping"),
        "{}",
        out.contents()
    );
}

#[tokio::test]
async fn slow_stop_is_abandoned_at_deadline() {
    let log = Log::new();
    let l = log.clone();
    let out = Captured::new();
    let begin = Instant::now();
    let status = within(
        triggered()
            .output(out.clone())
            .shutdown_timeout(Duration::from_millis(100))
            .configure(move |b| {
                // Both are phase 0: `Slow`, registered later, stops first and
                // uses up the shared deadline, so the stop of `a` must not run.
                let b = service!(b, Svc::<1>::new("a", 0, &l), Svc<1>);
                service!(b, Slow(l.clone()), Slow)
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 0);
    assert!(begin.elapsed() < Duration::from_secs(2));
    assert!(begin.elapsed() >= Duration::from_millis(100));
    assert_eq!(log.entries(), ["start:a"]);
    let text = out.contents();
    assert!(
        text.contains(&format!(
            "service {} did not stop in time",
            std::any::type_name::<Slow>()
        )),
        "{text}"
    );
    // The deadline is shared: the service after the slow one gets no time left.
    assert!(text.contains("service a did not stop in time"), "{text}");
}

#[tokio::test]
async fn zero_timeout_abandons_a_slow_stop_immediately() {
    let log = Log::new();
    let l = log.clone();
    let out = Captured::new();
    let begin = Instant::now();
    within(
        triggered()
            .output(out.clone())
            .shutdown_timeout(Duration::ZERO)
            .configure(move |b| service!(b, Slow(l), Slow))
            .run(),
    )
    .await
    .unwrap();
    assert!(begin.elapsed() < Duration::from_millis(500));
    assert!(log.entries().is_empty());
    assert!(
        out.contents().contains("did not stop in time"),
        "{}",
        out.contents()
    );
}

#[tokio::test]
async fn full_event_log_with_services() {
    let log = Log::new();
    let l = log.clone();
    let status = within(
        base()
            .listener(Recorder(log.clone()))
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("svc", 0, &l), Svc<1>);
                let l2 = l.clone();
                b.bean(move |h: Arc<ShutdownHandle>| Trigger(h, l2.clone()))
                    .provides(|r: Arc<Trigger>| -> Arc<dyn ApplicationRunner> { r })
                    .instance(Probe)
                    .pre_destroy(move |_: &Probe| l.push("pre_destroy"))
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
            "runner",
            "start:svc",
            "started",
            "ready",
            "stopping",
            "stop:svc",
            "pre_destroy",
            "stopped",
        ]
    );
}

#[tokio::test]
async fn max_timeout_with_fast_service_returns() {
    let log = Log::new();
    let l = log.clone();
    let status = within(
        triggered()
            .shutdown_timeout(Duration::MAX)
            .configure(move |b| service!(b, Svc::<1>::new("a", 0, &l), Svc<1>))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(log.entries(), ["start:a", "stop:a"]);
}

#[tokio::test]
async fn abandoned_stop_task_is_aborted() {
    let log = Log::new();
    let l = log.clone();
    within(
        triggered()
            .shutdown_timeout(Duration::from_millis(50))
            .configure(move |b| service!(b, Guarded(l), Guarded))
            .run(),
    )
    .await
    .unwrap();
    // An aborted task is dropped the next time the runtime polls it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(log.entries(), ["dropped"]);
}

#[tokio::test]
async fn zero_timeout_runs_no_stop() {
    let log = Log::new();
    let l = log.clone();
    let out = Captured::new();
    within(
        triggered()
            .output(out.clone())
            .shutdown_timeout(Duration::ZERO)
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("a", 0, &l), Svc<1>);
                service!(b, Svc::<2>::new("b", 1, &l), Svc<2>)
            })
            .run(),
    )
    .await
    .unwrap();
    // No grace at all: even a fast stop is not started.
    assert_eq!(log.entries(), ["start:a", "start:b"]);
    let text = out.contents();
    assert!(text.contains("service b did not stop in time"), "{text}");
    assert!(text.contains("service a did not stop in time"), "{text}");
}

#[tokio::test]
async fn lower_order_starts_first_within_a_phase() {
    let log = Log::new();
    let l = log.clone();
    within(
        triggered()
            .configure(move |b| {
                let b = service!(b, Svc::<1>::new("five", 0, &l), Svc<1>).order(5);
                let b = service!(b, Svc::<2>::new("one", 0, &l), Svc<2>).order(1);
                let b = service!(b, Svc::<3>::new("three", 0, &l), Svc<3>).order(3);
                // A higher phase still starts after every lower-phase service.
                service!(b, Svc::<4>::new("late", 1, &l), Svc<4>).order(-10)
            })
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(
        log.entries(),
        [
            "start:one",
            "start:three",
            "start:five",
            "start:late",
            "stop:late",
            "stop:five",
            "stop:three",
            "stop:one",
        ]
    );
}

/// A service that holds a resource between start and stop and does no work of its own.
struct Resource(Log);

#[async_trait]
impl Service for Resource {
    async fn start(&self) -> Result<(), ServiceError> {
        self.0.push("start:resource");
        Ok(())
    }

    async fn stop(&self) {
        self.0.push("stop:resource");
    }

    fn keeps_running(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn services_that_do_not_keep_running_are_started_and_stopped_without_a_wait() {
    let log = Log::new();
    let l = log.clone();
    // The handle is never triggered: a wait would hang until the watchdog fires.
    let status = within(
        base()
            .configure(move |b| service!(b, Resource(l.clone()), Resource))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(log.entries(), ["start:resource", "stop:resource"]);
}

#[tokio::test]
async fn one_service_that_keeps_running_still_makes_the_run_wait() {
    let log = Log::new();
    let l = log.clone();
    let builder = base();
    let handle = builder.shutdown_handle();
    let run = tokio::spawn(
        builder
            .configure(move |b| {
                let b = service!(b, Resource(l.clone()), Resource);
                service!(b, Svc::<1>::new("a", 0, &l), Svc<1>)
            })
            .run(),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!run.is_finished(), "run must wait for the shutdown trigger");
    handle.trigger();
    let status = within(run).await.unwrap().unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(
        log.entries(),
        ["start:resource", "start:a", "stop:a", "stop:resource"]
    );
}
