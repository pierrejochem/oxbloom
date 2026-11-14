//! A reload task started before any subscriber exists reports to the global subscriber that
//! is installed later. This binary installs the global subscriber, so it holds this one test
//! only.

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{ca, write};
use oxbloom_config::Config;
use oxbloom_ssl::SslBundles;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

/// The warnings on target `oxbloom::ssl`, each as its message followed by every field.
#[derive(Clone, Default)]
struct Warnings(Arc<Mutex<Vec<String>>>);

struct Text(String);

impl Visit for Text {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={:?}", field.name(), value));
    }
}

impl<S: Subscriber> Layer<S> for Warnings {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        if meta.target() == "oxbloom::ssl" && *meta.level() == Level::WARN {
            let mut text = Text(String::new());
            event.record(&mut text);
            self.0.lock().unwrap().push(text.0);
        }
    }
}

fn replace(path: &Path, contents: impl AsRef<[u8]>) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).expect("write replacement");
    std::fs::rename(&tmp, path).expect("rename replacement");
}

#[test]
fn a_task_started_before_logging_reports_to_the_global_subscriber_installed_later() {
    let dir = tempfile::tempdir().unwrap();
    let issued = ca().issue(&["localhost"]);
    let cert = write(dir.path(), "web.crt", &issued.chain_pem);
    let key = write(dir.path(), "web.key", &issued.key_pem);
    let bundles = SslBundles::from_config(
        &Config::from_str(&format!(
            "[ssl]\nreload-interval = \"50ms\"\n\
             [ssl.bundle.pem.web]\ncertificate = \"{}\"\nprivate-key = \"{}\"",
            cert.display(),
            key.display()
        ))
        .unwrap(),
    )
    .unwrap();

    // No subscriber exists yet when the task starts.
    let task = bundles.start_reload();
    let warnings = Warnings::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(warnings.clone()))
        .expect("the only global subscriber of this binary");

    replace(&cert, "this is not a certificate");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && warnings.0.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let seen = warnings.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "expected one warning: {seen:?}");
    assert!(
        seen[0].contains("web") && seen[0].contains("pem"),
        "{seen:?}"
    );
    task.stop();
}
