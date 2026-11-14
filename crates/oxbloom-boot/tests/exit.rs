mod common;

use std::future::Future;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use oxbloom_boot::async_trait;
use oxbloom_boot::{
    ExitCodeGenerator, ExitStatus, OxbloomApplication, OxbloomApplicationBuilder, RunListener,
    Service, ServiceError, StartupPhase,
};
use oxbloom_config::Config;
use oxbloom_di::DiError;
use oxbloom_test::{Captured, Log};

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

fn base() -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str("").unwrap())
        .output(std::io::sink())
}

struct Code<const N: i32>;

impl<const N: i32> ExitCodeGenerator for Code<N> {
    fn exit_code(&self) -> i32 {
        N
    }
}

macro_rules! with_code {
    ($b:expr, $n:literal) => {
        $b.instance(Code::<$n>)
            .provides(|c: Arc<Code<$n>>| -> Arc<dyn ExitCodeGenerator> { c })
    };
}

#[tokio::test]
async fn max_of_generators() {
    let status = within(
        base()
            .configure(|b| with_code!(with_code!(with_code!(b, 2), 7), 4))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 7);
}

#[tokio::test]
async fn no_generator_is_zero() {
    let status = within(base().run()).await.unwrap();
    assert_eq!(status.code(), 0);
}

#[tokio::test]
async fn generator_order_does_not_matter() {
    let a = within(base().configure(|b| with_code!(with_code!(b, 1), 9)).run())
        .await
        .unwrap();
    let b = within(base().configure(|b| with_code!(with_code!(b, 9), 1)).run())
        .await
        .unwrap();
    assert_eq!(a.code(), 9);
    assert_eq!(b.code(), 9);
}

async fn code_of(b: OxbloomApplicationBuilder) -> i32 {
    within(b.run()).await.unwrap().code()
}

fn exit_of(code: u8) -> String {
    format!("{:?}", ExitCode::from(code))
}

#[tokio::test]
async fn lone_negative_code_maps_to_255() {
    assert_eq!(code_of(base().configure(|b| with_code!(b, -1))).await, 255);
    let code = within(base().configure(|b| with_code!(b, -1)).run_and_exit()).await;
    assert_eq!(format!("{code:?}"), exit_of(255));
}

#[tokio::test]
async fn negative_code_wins_over_zero() {
    // Mapped before the maximum: -1 becomes 255, not a loser to 0.
    let b = base().configure(|b| with_code!(with_code!(b, -1), 0));
    assert_eq!(code_of(b).await, 255);
    let b = base().configure(|b| with_code!(with_code!(b, 0), -7));
    let code = within(b.run_and_exit()).await;
    assert_eq!(format!("{code:?}"), exit_of(255));
}

#[tokio::test]
async fn code_above_255_maps_to_255() {
    assert_eq!(code_of(base().configure(|b| with_code!(b, 300))).await, 255);
    let code = within(base().configure(|b| with_code!(b, 1000)).run_and_exit()).await;
    assert_eq!(format!("{code:?}"), exit_of(255));
    let code = within(base().configure(|b| with_code!(b, 42)).run_and_exit()).await;
    assert_eq!(format!("{code:?}"), exit_of(42));
}

#[tokio::test]
async fn maximum_is_taken_over_mapped_codes() {
    assert_eq!(
        code_of(base().configure(|b| with_code!(with_code!(b, -5), 200))).await,
        255
    );
    assert_eq!(
        code_of(base().configure(|b| with_code!(with_code!(b, 255), 300))).await,
        255
    );
    assert_eq!(
        code_of(base().configure(|b| with_code!(with_code!(b, 254), 12))).await,
        254
    );
}

struct Logged(Log);

#[async_trait]
impl Service for Logged {
    async fn start(&self) -> Result<(), ServiceError> {
        Ok(())
    }
    async fn stop(&self) {
        self.0.push("stop");
    }
}

impl ExitCodeGenerator for Logged {
    fn exit_code(&self) -> i32 {
        self.0.push("exit_code");
        5
    }
}

#[tokio::test]
async fn generators_read_after_services_stop() {
    let log = Log::new();
    let l = log.clone();
    let builder = base().configure(move |b| {
        b.instance(Logged(l))
            .provides(|s: Arc<Logged>| -> Arc<dyn Service> { s })
            .provides(|s: Arc<Logged>| -> Arc<dyn ExitCodeGenerator> { s })
    });
    builder.shutdown_handle().trigger();
    let status = within(builder.run()).await.unwrap();
    assert_eq!(status.code(), 5);
    assert_eq!(log.entries(), ["stop", "exit_code"]);
}

#[tokio::test]
async fn startup_error_gives_exit_one() {
    let out = Captured::new();
    let code = within(
        OxbloomApplication::builder()
            .with_config(Config::from_str("").unwrap())
            .output(out.clone())
            .banner(oxbloom_boot::Banner::off())
            .configure(|b| b.instance(Config::from_str("").unwrap()))
            .run_and_exit(),
    )
    .await;
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(1)));
    // The failure report replaces the bare error line by default.
    let text = out.contents();
    assert!(
        text.starts_with("***************************\nAPPLICATION FAILED TO START\n"),
        "{text}"
    );
    assert!(
        text.ends_with("\n\nAction:\n\nRegister the type only once.\n"),
        "{text}"
    );
}

#[tokio::test]
async fn startup_error_without_report_prints_one_line() {
    let out = Captured::new();
    let code = within(
        OxbloomApplication::builder()
            .with_config(Config::from_str("").unwrap())
            .output(out.clone())
            .banner(oxbloom_boot::Banner::off())
            .failure_report(false)
            .configure(|b| b.instance(Config::from_str("").unwrap()))
            .run_and_exit(),
    )
    .await;
    assert_eq!(format!("{code:?}"), format!("{:?}", ExitCode::from(1)));
    let text = out.contents();
    assert!(
        text.starts_with("startup failed in the container phase: "),
        "{text}"
    );
    assert_eq!(text.lines().count(), 1);
}

struct Stopped(Log);

impl RunListener for Stopped {
    fn stopped(&self, status: &ExitStatus) {
        self.0.push(format!("stopped {}", status.code()));
    }
}

#[tokio::test]
async fn stopped_listener_receives_computed_status() {
    let log = Log::new();
    let status = within(
        base()
            .listener(Stopped(log.clone()))
            .configure(|b| with_code!(with_code!(b, 3), 6))
            .run(),
    )
    .await
    .unwrap();
    assert_eq!(status.code(), 6);
    assert_eq!(log.entries(), ["stopped 6"]);
}

struct Boom;
struct NeedsBoom;

impl ExitCodeGenerator for NeedsBoom {
    fn exit_code(&self) -> i32 {
        1
    }
}

struct CountFailed(Log);

impl RunListener for CountFailed {
    fn failed(&self, _: &oxbloom_boot::StartupError) {
        self.0.push("failed");
    }
}

/// A service whose start would be logged.
struct Svc(Log);

#[async_trait]
impl Service for Svc {
    async fn start(&self) -> Result<(), ServiceError> {
        self.0.push("service started");
        Ok(())
    }

    async fn stop(&self) {}
}

struct Probe;

#[tokio::test]
async fn generator_failing_get_all_is_container_phase() {
    let log = Log::new();
    let (l1, l2) = (log.clone(), log.clone());
    let err = within(
        base()
            .listener(CountFailed(log.clone()))
            .configure(move |b| {
                b.try_bean(|| -> Result<Boom, DiError> { Err(DiError::creation::<Boom>("boom")) })
                    .lazy()
                    .bean(|_: Arc<Boom>| NeedsBoom)
                    .lazy()
                    .provides(|n: Arc<NeedsBoom>| -> Arc<dyn ExitCodeGenerator> { n })
                    .instance(Svc(l1))
                    .provides(|s: Arc<Svc>| -> Arc<dyn Service> { s })
                    .instance(Probe)
                    .pre_destroy(move |_: &Probe| l2.push("pre_destroy"))
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    // `failed` once, no service started, the container was shut down.
    assert_eq!(log.entries(), ["failed", "pre_destroy"]);
}
