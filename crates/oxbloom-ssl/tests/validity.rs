//! Validity warnings: expired, not yet valid and soon expiring certificates warn at load, after
//! a successful reload and again every 24 hours by the clock, and never stop the startup.
//!
//! Every test captures its own events with a scoped subscriber; nothing here installs the
//! global subscriber. The certificates are generated with explicit dates around the time the
//! test starts, and the tests set a clock of their own with `SslBundles::with_clock`.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use common::{ca, ca_between, write};
use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_ssl::{CertValidity, SslBundles};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const HOUR: Duration = Duration::from_secs(60 * 60);

/// A clock that always reads the same time.
struct FixedClock(SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// A clock that reads the time it was last set to; tests move it forward by hand.
struct ManualClock(Mutex<SystemTime>);

impl ManualClock {
    fn new(start: SystemTime) -> Arc<ManualClock> {
        Arc::new(ManualClock(Mutex::new(start)))
    }

    fn set(&self, time: SystemTime) {
        *self.0.lock().unwrap() = time;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

/// One captured event: its level, its target and its message followed by every field.
#[derive(Clone, Debug)]
struct Captured {
    level: Level,
    target: String,
    text: String,
}

/// A layer that records every event it sees.
#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<Captured>>>);

struct Text(String);

impl Visit for Text {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={:?}", field.name(), value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push_str(&format!(" {}={}", field.name(), value));
    }
}

impl<S: Subscriber> Layer<S> for Events {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut text = Text(String::new());
        event.record(&mut text);
        self.0.lock().unwrap().push(Captured {
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            text: text.0,
        });
    }
}

impl Events {
    /// The text of every warning on target `oxbloom::ssl`.
    fn warnings(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.target == "oxbloom::ssl" && e.level == Level::WARN)
            .map(|e| e.text.clone())
            .collect()
    }

    /// The text of every event of any level and target.
    fn all(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.text.clone())
            .collect()
    }
}

/// Installs a scoped subscriber that records events for the rest of the calling scope. The
/// polling task started in that scope reports to the same subscriber.
fn capture() -> (Events, tracing::subscriber::DefaultGuard) {
    let events = Events::default();
    let subscriber = tracing_subscriber::registry().with(events.clone());
    let guard = tracing::subscriber::set_default(subscriber);
    (events, guard)
}

/// Waits up to five seconds for `condition` and reports whether it became true.
fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    condition()
}

/// The current time cut to whole seconds, as certificates store their dates.
fn now_seconds() -> SystemTime {
    let since = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    SystemTime::UNIX_EPOCH + Duration::from_secs(since.as_secs())
}

/// The TOML of a PEM bundle `web` whose certificate and key are in the files `cert` and
/// `key`, after the `[ssl]` lines `ssl`.
fn web_toml(ssl: &str, cert: &Path, key: &Path) -> String {
    format!(
        "[ssl]\n{ssl}\n[ssl.bundle.pem.web]\ncertificate = \"{}\"\nprivate-key = \"{}\"",
        cert.display(),
        key.display()
    )
}

/// A bundle `web` with a leaf for `web.test` valid from `not_before` to `not_after`, loaded
/// with the `[ssl]` lines `ssl`. Returns the bundles and the key PEM of the leaf.
fn web_bundle(
    dir: &Path,
    ssl: &str,
    not_before: SystemTime,
    not_after: SystemTime,
) -> (SslBundles, String) {
    let issued = ca().issue_between(&["web.test"], not_before, not_after);
    let cert = write(dir, "web.crt", &issued.chain_pem);
    let key = write(dir, "web.key", &issued.key_pem);
    let config = Config::from_str(&web_toml(ssl, &cert, &key)).unwrap();
    let bundles = SslBundles::from_config(&config).expect("a bad date never stops the load");
    (bundles, issued.key_pem)
}

#[test]
fn expired_certificate_warns_at_load_and_does_not_fail() {
    let (events, _guard) = capture();
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    let (bundles, key_pem) = web_bundle(dir.path(), "", now - 30 * DAY, now - DAY);
    let bundles = bundles.with_clock(Arc::new(FixedClock(now)));

    let web = bundles.get("web").unwrap();
    assert!(web.has_identity());
    assert!(web.server_config().is_ok());

    let warnings = events.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("has expired"), "{warnings:?}");
    assert!(warnings[0].contains("bundle=web"), "{warnings:?}");
    assert!(warnings[0].contains("subject=web.test"), "{warnings:?}");

    // No event carries key material.
    let key_body: String = key_pem.lines().nth(1).unwrap().to_string();
    for text in events.all() {
        assert!(!text.contains("PRIVATE KEY"), "{text}");
        assert!(!text.contains(&key_body), "{text}");
    }
}

#[test]
fn not_yet_valid_certificate_warns() {
    let (events, _guard) = capture();
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    // Valid by the system time, but the clock reads a time before the certificate starts.
    let (bundles, _) = web_bundle(dir.path(), "", now - DAY, now + 365 * DAY);
    assert!(events.warnings().is_empty(), "{:?}", events.warnings());

    let _bundles = bundles.with_clock(Arc::new(FixedClock(now - 30 * DAY)));
    let warnings = events.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("is not valid yet"), "{warnings:?}");
    assert!(warnings[0].contains("bundle=web"), "{warnings:?}");
    assert!(warnings[0].contains("subject=web.test"), "{warnings:?}");
}

#[test]
fn certificate_expiring_within_the_window_warns() {
    let (events, _guard) = capture();
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    let (bundles, _) = web_bundle(dir.path(), "", now - DAY, now + 3 * DAY);
    let _bundles = bundles.with_clock(Arc::new(FixedClock(now)));

    let warnings = events.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("expires soon"), "{warnings:?}");
    assert!(warnings[0].contains("bundle=web"), "{warnings:?}");
    assert!(warnings[0].contains("subject=web.test"), "{warnings:?}");
}

#[test]
fn repeat_warning_comes_after_24_hours_not_before() {
    let (events, _guard) = capture();
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    let (bundles, _) = web_bundle(
        dir.path(),
        "reload-interval = \"20ms\"",
        now - 30 * DAY,
        now - DAY,
    );
    let clock = ManualClock::new(now);
    let bundles = bundles.with_clock(clock.clone());
    assert_eq!(events.warnings().len(), 1, "{:?}", events.warnings());

    let task = bundles.start_reload();
    clock.set(now + 23 * HOUR);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(events.warnings().len(), 1, "{:?}", events.warnings());

    clock.set(now + 24 * HOUR + Duration::from_secs(1));
    assert!(
        wait_for(|| events.warnings().len() == 2),
        "no repeat warning after 24 hours: {:?}",
        events.warnings()
    );
    let warnings = events.warnings();
    assert!(warnings[1].contains("has expired"), "{warnings:?}");

    // The next repeat waits for another 24 hours.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(events.warnings().len(), 2, "{:?}", events.warnings());
    task.stop();
}

#[test]
fn valid_certificate_is_silent() {
    let (events, _guard) = capture();
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    let (bundles, _) = web_bundle(
        dir.path(),
        "reload-interval = \"20ms\"",
        now - DAY,
        now + 365 * DAY,
    );
    let bundles = bundles.with_clock(Arc::new(FixedClock(now)));
    let task = bundles.start_reload();
    std::thread::sleep(Duration::from_millis(100));
    task.stop();
    assert!(events.warnings().is_empty(), "{:?}", events.warnings());
}

#[test]
fn validity_lists_subject_and_dates_for_every_certificate_of_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    let root = ca();
    let intermediate = root.intermediate();
    let issued = intermediate.issue_between(&["web.test"], now - DAY, now + 90 * DAY);
    let cert = write(dir.path(), "web.crt", &issued.chain_pem);
    let key = write(dir.path(), "web.key", &issued.key_pem);
    let ca_file = write(dir.path(), "ca.pem", &root.cert_pem);
    let config = Config::from_str(&format!(
        "{}\ntrust = \"{}\"",
        web_toml("", &cert, &key),
        ca_file.display()
    ))
    .unwrap();
    let web = SslBundles::from_config(&config)
        .unwrap()
        .get("web")
        .unwrap();

    let validity = web.validity();
    let subjects: Vec<&str> = validity.iter().map(|v| v.subject.as_str()).collect();
    // The root is in the chain and in the trust store, and it is listed once.
    assert_eq!(
        subjects,
        ["web.test", "Oxbloom Test Intermediate", "Oxbloom Test Root"]
    );
    assert_eq!(
        validity[0],
        CertValidity {
            subject: "web.test".to_string(),
            not_before: now - DAY,
            not_after: now + 90 * DAY,
        }
    );
    for entry in &validity[1..] {
        assert!(entry.not_before < now && now < entry.not_after, "{entry:?}");
    }
}

#[test]
fn expiry_warning_key_changes_the_window() {
    let (events, _guard) = capture();
    let now = now_seconds();
    let clock = Arc::new(FixedClock(now));

    let dir = tempfile::tempdir().unwrap();
    let (bundles, _) = web_bundle(
        dir.path(),
        "expiry-warning = \"2d\"",
        now - DAY,
        now + 3 * DAY,
    );
    let _bundles = bundles.with_clock(clock.clone());
    assert!(events.warnings().is_empty(), "{:?}", events.warnings());

    let dir = tempfile::tempdir().unwrap();
    let (bundles, _) = web_bundle(
        dir.path(),
        "expiry-warning = \"60d\"",
        now - DAY,
        now + 30 * DAY,
    );
    let _bundles = bundles.with_clock(clock);
    let warnings = events.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("expires soon"), "{warnings:?}");
}

#[test]
fn expired_trust_certificate_of_a_trust_only_bundle_warns() {
    let (events, _guard) = capture();
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    let old = ca_between("Old Partner Root", now - 400 * DAY, now - 5 * DAY);
    let ca_file = write(dir.path(), "ca.pem", &old.cert_pem);
    let config = Config::from_str(&format!(
        "[ssl.bundle.pem.partner]\ntrust = \"{}\"",
        ca_file.display()
    ))
    .unwrap();
    let bundles = SslBundles::from_config(&config).unwrap();
    let _bundles = bundles.with_clock(Arc::new(FixedClock(now)));

    let warnings = events.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("has expired"), "{warnings:?}");
    assert!(warnings[0].contains("bundle=partner"), "{warnings:?}");
    assert!(
        warnings[0].contains("subject=Old Partner Root"),
        "{warnings:?}"
    );
}

#[test]
fn successful_reload_warns_again_at_once() {
    let (events, _guard) = capture();
    let dir = tempfile::tempdir().unwrap();
    let now = now_seconds();
    let (bundles, _) = web_bundle(dir.path(), "", now - 30 * DAY, now - DAY);
    let bundles = bundles.with_clock(Arc::new(FixedClock(now)));
    assert_eq!(events.warnings().len(), 1, "{:?}", events.warnings());

    let results = bundles.reload();
    assert!(results[0].1.is_ok());
    let warnings = events.warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[1].contains("has expired"), "{warnings:?}");
}
