//! `LoggingSystem::init` against the global subscriber. The subscriber is per process and
//! can be installed once, so every scenario is a step of the one test below.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use oxbloom_config::Config;
use oxbloom_logging::{Level, LoggingSystem};
use tracing_subscriber::fmt::MakeWriter;

/// Collects the console output. When armed, the next write calls `set_level` from inside
/// the event, as a user layer or writer might.
#[derive(Clone, Default)]
struct Capture {
    buf: Arc<Mutex<Vec<u8>>>,
    system: Arc<OnceLock<LoggingSystem>>,
    armed: Arc<AtomicBool>,
}

impl Capture {
    /// The output so far, cleared.
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.buf.lock().unwrap())).unwrap()
    }
}

impl io::Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(data);
        if self.armed.swap(false, Ordering::SeqCst) {
            let system = self.system.get().unwrap();
            system.set_level("inside", Level::Debug).unwrap();
            assert_eq!(system.level("inside"), Level::Debug);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

// One callsite each, so the steps also prove the interest cache is rebuilt.
fn info(message: &str) {
    tracing::info!(target: "app", "{message}");
}

fn warn(message: &str) {
    tracing::warn!(target: "app", "{message}");
}

fn web_debug(message: &str) {
    tracing::debug!(target: "oxbloom_web", "{message}");
}

fn log_info(message: &str) {
    log::info!(target: "app", "{message}");
}

#[test]
fn init_installs_and_levels_change_at_runtime() {
    let config = Config::from_str(
        "[logging.level]\nroot = \"warn\"\noxbloom_web = \"debug\"\n\
         [logging.console]\ncolor = \"never\"",
    )
    .unwrap();
    let out = Capture::default();
    let system = LoggingSystem::init_with_writer(&config, out.clone()).unwrap();
    out.system.set(system.clone()).unwrap();

    // Root warn: info is filtered, warn passes.
    info("one-info");
    warn("one-warn");
    let text = out.take();
    assert!(!text.contains("one-info"), "{text}");
    assert!(text.contains("one-warn"), "{text}");
    assert_eq!(system.level("root"), Level::Warn);

    // Raising root lets the same info callsite through.
    system.set_level("root", Level::Info).unwrap();
    info("two-info");
    assert!(out.take().contains("two-info"));
    assert_eq!(system.level("root"), Level::Info);
    assert_eq!(system.level("app"), Level::Info);

    // Reset returns to the configured warn.
    system.reset("root").unwrap();
    info("three-info");
    assert!(!out.take().contains("three-info"));
    assert_eq!(system.level("root"), Level::Warn);

    // Root off silences everything; info brings logging back without a reinstall.
    system.set_level("root", Level::Off).unwrap();
    system.set_level("oxbloom_web", Level::Off).unwrap();
    warn("four-warn");
    web_debug("four-web");
    log_info("four-log");
    assert_eq!(out.take(), "");
    system.set_level("root", Level::Info).unwrap();
    info("five-info");
    warn("five-warn");
    let text = out.take();
    assert!(
        text.contains("five-info") && text.contains("five-warn"),
        "{text}"
    );
    system.reset("oxbloom_web").unwrap();

    // The configured target directive applies; reload without it removes it.
    web_debug("six-web");
    assert!(out.take().contains("six-web"));
    assert_eq!(system.level("oxbloom_web::router"), Level::Debug);
    let reloaded = Config::from_str("[logging.level]\nroot = \"warn\"").unwrap();
    system.reload(&reloaded).unwrap();
    web_debug("seven-web");
    info("seven-info");
    assert_eq!(out.take(), "");
    assert_eq!(system.level("oxbloom_web"), Level::Warn);

    // `log` facade events reach the subscriber and honor the levels.
    log_info("eight-log-hidden");
    assert!(!out.take().contains("eight-log-hidden"));
    system.set_level("app", Level::Info).unwrap();
    log_info("eight-log-shown");
    assert!(out.take().contains("eight-log-shown"));

    // `set_level` from inside an event (the writer here) completes.
    out.armed.store(true, Ordering::SeqCst);
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        warn("nine-warn");
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(Duration::from_secs(1))
        .expect("set_level inside an event deadlocked");
    assert!(out.take().contains("nine-warn"));
    assert_eq!(system.level("inside"), Level::Debug);
    tracing::debug!(target: "inside", "ten-inside");
    assert!(out.take().contains("ten-inside"));
}
