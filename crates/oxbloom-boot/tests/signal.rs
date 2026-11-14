//! Real OS signals against a running application.
//!
//! A signal is process-wide, so this file is its own test binary and every
//! scenario runs one after the other inside a single `#[test]`.
#![cfg(unix)]

use std::future::Future;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oxbloom_boot::async_trait;
use oxbloom_boot::{
    ExitStatus, OxbloomApplication, RunListener, Service, ServiceError, StartupError,
};
use oxbloom_config::Config;
use oxbloom_test::{Captured, Log};
use tokio::signal::unix::{SignalKind, signal};

/// Logs `stopped {code}`.
struct Stopped(Log);

impl RunListener for Stopped {
    fn stopped(&self, status: &ExitStatus) {
        self.0.push(format!("stopped {}", status.code()));
    }
}

/// Logs `dropped` when the future holding it is dropped.
struct DropGuard(Log);

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.push("dropped");
    }
}

/// Logs `start`, then `stop began`; with `slow`, the stop then takes 10 s.
/// With `slow_start`, `start` logs `start began` and takes 500 ms first.
struct Svc {
    log: Log,
    slow: bool,
    slow_start: bool,
}

#[async_trait]
impl Service for Svc {
    async fn start(&self) -> Result<(), ServiceError> {
        if self.slow_start {
            self.log.push("start began");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.log.push("start");
        Ok(())
    }

    async fn stop(&self) {
        self.log.push("stop began");
        if self.slow {
            let _guard = DropGuard(self.log.clone());
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        self.log.push("stop");
    }
}

struct Probe;

fn send(flag: &str) {
    let status = Command::new("kill")
        .args([flag, &std::process::id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill {flag} failed");
}

async fn until(log: &Log, entry: &str, wait: Duration) -> bool {
    let begin = Instant::now();
    while begin.elapsed() < wait {
        if log.entries().iter().any(|e| e == entry) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Sends `flag` until `entry` is logged. The application installs its
/// listeners just before the services start; a signal that arrives before
/// that is ignored, so it is resent.
async fn signal_until(flag: &str, log: &Log, entry: &str) {
    for _ in 0..5 {
        send(flag);
        if until(log, entry, Duration::from_secs(1)).await {
            return;
        }
    }
    panic!("`{entry}` not logged after {flag}: {:?}", log.entries());
}

async fn within<F: Future>(wait: Duration, f: F) -> F::Output {
    tokio::time::timeout(wait, f)
        .await
        .expect("scenario timed out")
}

fn spawn_app(log: &Log, slow: bool) -> tokio::task::JoinHandle<Result<ExitStatus, StartupError>> {
    spawn_app_with(log, slow, false)
}

fn spawn_app_with(
    log: &Log,
    slow: bool,
    slow_start: bool,
) -> tokio::task::JoinHandle<Result<ExitStatus, StartupError>> {
    let l = log.clone();
    let app = OxbloomApplication::builder()
        .with_config(Config::from_str("").unwrap())
        .output(Captured::new())
        .shutdown_timeout(Duration::from_secs(30))
        .listener(Stopped(log.clone()))
        .configure(move |b| {
            b.instance(Svc {
                log: l.clone(),
                slow,
                slow_start,
            })
            .provides(|s: Arc<Svc>| -> Arc<dyn Service> { s })
            .instance(Probe)
            .pre_destroy(move |_: &Probe| l.push("pre_destroy"))
        });
    tokio::spawn(app.run())
}

/// One signal: the services stop gracefully and the container shuts down.
async fn graceful(flag: &str) {
    let log = Log::new();
    let run = spawn_app(&log, false);
    assert!(until(&log, "start", Duration::from_secs(2)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    signal_until(flag, &log, "stop").await;
    let status = within(Duration::from_secs(2), run).await.unwrap().unwrap();
    assert_eq!(status.code(), 0, "{flag}");
    assert_eq!(
        log.entries(),
        ["start", "stop began", "stop", "pre_destroy", "stopped 0"],
        "{flag}"
    );
}

/// A second signal during a slow stop returns at once with `code`, skipping
/// the rest of the stop and the container shutdown.
async fn forced(flag: &str, code: i32) {
    let log = Log::new();
    let run = spawn_app(&log, true);
    assert!(until(&log, "start", Duration::from_secs(2)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    signal_until(flag, &log, "stop began").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let begin = Instant::now();
    send(flag);
    let status = within(Duration::from_secs(2), run).await.unwrap().unwrap();
    assert!(begin.elapsed() < Duration::from_secs(2));
    assert_eq!(status.code(), code, "{flag}");
    // The abandoned stop task is aborted, not left running.
    assert!(
        until(&log, "dropped", Duration::from_secs(1)).await,
        "{:?}",
        log.entries()
    );
    let entries = log.entries();
    assert!(!entries.iter().any(|e| e == "pre_destroy"), "{entries:?}");
    assert!(!entries.iter().any(|e| e == "stop"), "{entries:?}");
    assert!(
        entries.iter().any(|e| *e == format!("stopped {code}")),
        "{entries:?}"
    );
}

/// A signal while a service is still starting is kept: the start finishes,
/// the wait ends at once and the services stop gracefully.
async fn sigterm_during_slow_start_stops_after_start() {
    let log = Log::new();
    let begin = Instant::now();
    let run = spawn_app_with(&log, false, true);
    assert!(until(&log, "start began", Duration::from_secs(2)).await);
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Sent once: boot listens from before the services start.
    send("-TERM");
    let status = within(Duration::from_secs(3), run).await.unwrap().unwrap();
    assert!(begin.elapsed() < Duration::from_secs(3));
    assert_eq!(status.code(), 0);
    assert_eq!(
        log.entries(),
        [
            "start began",
            "start",
            "stop began",
            "stop",
            "pre_destroy",
            "stopped 0"
        ]
    );
}

async fn sigterm_triggers_graceful_shutdown() {
    graceful("-TERM").await;
}

async fn second_sigterm_during_slow_stop_returns_143() {
    forced("-TERM", 143).await;
}

async fn sigint_triggers_graceful_shutdown() {
    graceful("-INT").await;
}

async fn second_sigint_during_slow_stop_returns_130() {
    forced("-INT", 130).await;
}

#[test]
fn signals() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // Installs Tokio's process-wide handlers before the first `kill`, so a
        // signal that arrives before the application listens cannot kill the
        // test process. The application's own listeners are separate.
        let _term = signal(SignalKind::terminate()).unwrap();
        let _int = signal(SignalKind::interrupt()).unwrap();
        within(Duration::from_secs(60), async {
            sigterm_triggers_graceful_shutdown().await;
            second_sigterm_during_slow_stop_returns_143().await;
            sigint_triggers_graceful_shutdown().await;
            second_sigint_during_slow_stop_returns_130().await;
            sigterm_during_slow_start_stops_after_start().await;
        })
        .await;
    });
}
