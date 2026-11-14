//! Reloading SSL bundles when their files change: the polling task, the warn-once rule, the
//! check against files that change during the read, and `SslBundles::reload`.
//!
//! Every test captures its own events with a scoped subscriber; nothing here installs the
//! global subscriber. Files are replaced by writing a new file and renaming it over the old
//! one, the way deployment tools and Kubernetes secret mounts do it.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{TestCa, ca, handshake, write};
use oxbloom_config::Config;
use oxbloom_ssl::{SslBundles, SslError};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

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
    /// The events on target `oxbloom::ssl` at `level`.
    fn ssl(&self, level: Level) -> Vec<Captured> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.target == "oxbloom::ssl" && e.level == level)
            .cloned()
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

/// Lets `ticks` polling intervals of 50 milliseconds pass.
fn let_ticks_pass(ticks: u32) {
    std::thread::sleep(Duration::from_millis(50) * ticks);
}

/// Replaces the file at `path` by writing a sibling file and renaming it over `path`.
fn replace(path: &Path, contents: impl AsRef<[u8]>) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).expect("write replacement");
    std::fs::rename(&tmp, path).expect("rename replacement");
}

fn toml_path(path: &Path) -> String {
    path.display().to_string()
}

fn load(toml: &str) -> SslBundles {
    SslBundles::from_config(&Config::from_str(toml).expect("valid TOML")).expect("bundles load")
}

/// A configuration with `ssl.reload-interval = "<interval>"` and a PEM bundle `web` whose
/// certificate and key live in the files `cert` and `key`.
fn web_toml(interval: &str, cert: &Path, key: &Path) -> String {
    format!(
        "[ssl]\nreload-interval = \"{interval}\"\n\
         [ssl.bundle.pem.web]\ncertificate = \"{}\"\nprivate-key = \"{}\"",
        toml_path(cert),
        toml_path(key)
    )
}

/// The leaf certificate `server` presents to a rustls client that trusts only `ca`, or
/// `None` when the handshake fails.
fn served_leaf(server: Arc<ServerConfig>, ca: &TestCa) -> Option<Vec<u8>> {
    let mut roots = RootCertStore::empty();
    roots.add(ca.cert_der.clone().into()).expect("root");
    let client = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connection = handshake(server, Arc::new(client), "localhost").ok()?;
    Some(connection.peer_certificates()?.first()?.to_vec())
}

/// A web bundle on its own files, with the authority that issued its first certificate.
struct Web {
    dir: tempfile::TempDir,
    cert: PathBuf,
    key: PathBuf,
    old_ca: TestCa,
    old_leaf: Vec<u8>,
}

fn web_files() -> Web {
    let dir = tempfile::tempdir().unwrap();
    let old_ca = ca();
    let issued = old_ca.issue(&["localhost"]);
    let cert = write(dir.path(), "web.crt", &issued.chain_pem);
    let key = write(dir.path(), "web.key", &issued.key_pem);
    Web {
        dir,
        cert,
        key,
        old_ca,
        old_leaf: issued.cert_der,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_certificate_file_is_served_to_the_next_handshake() {
    let web = web_files();
    let bundles = load(&web_toml("50ms", &web.cert, &web.key));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    assert_eq!(
        served_leaf(server.clone(), &web.old_ca),
        Some(web.old_leaf.clone())
    );
    let task = bundles.start_reload();
    assert!(task.is_running());

    let new_ca = ca();
    let issued = new_ca.issue(&["localhost"]);
    replace(&web.key, &issued.key_pem);
    replace(&web.cert, &issued.chain_pem);

    let new_leaf = issued.cert_der.clone();
    assert!(
        wait_for(|| served_leaf(server.clone(), &new_ca).as_ref() == Some(&new_leaf)),
        "the new certificate was not served within five seconds"
    );
    assert_eq!(served_leaf(server, &web.old_ca), None);
    task.stop();
}

#[cfg(unix)]
#[test]
fn symlink_swap_is_detected() {
    use std::os::unix::fs::symlink;

    // The layout of a Kubernetes secret mount: the visible files point through `..data`,
    // and an update swaps `..data` to a new directory in one rename.
    let dir = tempfile::tempdir().unwrap();
    let old_ca = ca();
    let old = old_ca.issue(&["localhost"]);
    let first = dir.path().join("..2026_11_14_a");
    std::fs::create_dir(&first).unwrap();
    write(&first, "tls.crt", &old.chain_pem);
    write(&first, "tls.key", &old.key_pem);
    symlink("..2026_11_14_a", dir.path().join("..data")).unwrap();
    symlink("..data/tls.crt", dir.path().join("tls.crt")).unwrap();
    symlink("..data/tls.key", dir.path().join("tls.key")).unwrap();

    let bundles = load(&web_toml(
        "50ms",
        &dir.path().join("tls.crt"),
        &dir.path().join("tls.key"),
    ));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    assert_eq!(
        served_leaf(server.clone(), &old_ca),
        Some(old.cert_der.clone())
    );
    let task = bundles.start_reload();

    let new_ca = ca();
    let new = new_ca.issue(&["localhost"]);
    let second = dir.path().join("..2026_11_14_b");
    std::fs::create_dir(&second).unwrap();
    write(&second, "tls.crt", &new.chain_pem);
    write(&second, "tls.key", &new.key_pem);
    symlink("..2026_11_14_b", dir.path().join("..data_tmp")).unwrap();
    std::fs::rename(dir.path().join("..data_tmp"), dir.path().join("..data")).unwrap();

    assert!(
        wait_for(|| served_leaf(server.clone(), &new_ca).as_ref() == Some(&new.cert_der)),
        "the swapped symlink was not followed within five seconds"
    );
    task.stop();
}

#[test]
fn bad_replacement_keeps_the_old_certificate_and_warns_once() {
    let (events, _guard) = capture();
    let web = web_files();
    let old_key_pem = std::fs::read_to_string(&web.key).unwrap();
    let bundles = load(&web_toml("50ms", &web.cert, &web.key));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    let task = bundles.start_reload();

    replace(&web.cert, "this is not a certificate");
    assert!(
        wait_for(|| events.ssl(Level::WARN).len() == 1),
        "no warning within five seconds"
    );
    let_ticks_pass(3);

    let warnings = events.ssl(Level::WARN);
    assert_eq!(warnings.len(), 1, "warned more than once: {warnings:?}");
    let text = &warnings[0].text;
    assert!(text.contains("web"), "the bundle name is missing: {text}");
    assert!(text.contains("pem"), "the error kind is missing: {text}");
    assert!(!text.contains("PRIVATE KEY"), "key text in: {text}");
    for line in old_key_pem.lines().filter(|l| !l.starts_with("-----")) {
        assert!(!text.contains(line), "key text in: {text}");
    }
    assert!(
        !text.contains(&*toml_path(&web.cert)),
        "file path in: {text}"
    );
    assert_eq!(served_leaf(server.clone(), &web.old_ca), Some(web.old_leaf));

    // A second change to another bad file warns again.
    replace(&web.cert, "still not a certificate");
    assert!(
        wait_for(|| events.ssl(Level::WARN).len() == 2),
        "the second change did not warn"
    );
    task.stop();
}

#[test]
fn mismatching_key_and_certificate_replacement_is_rejected() {
    let (events, _guard) = capture();
    let web = web_files();
    let bundles = load(&web_toml("50ms", &web.cert, &web.key));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    let task = bundles.start_reload();

    let other = ca().issue(&["localhost"]);
    replace(&web.cert, &other.chain_pem);
    assert!(
        wait_for(|| events.ssl(Level::WARN).len() == 1),
        "no warning within five seconds"
    );
    let text = &events.ssl(Level::WARN)[0].text;
    assert!(text.contains("key-mismatch"), "unexpected warning: {text}");
    assert_eq!(served_leaf(server, &web.old_ca), Some(web.old_leaf));
    task.stop();
}

#[test]
fn file_that_changes_during_the_read_is_retried_on_the_next_tick() {
    let (events, _guard) = capture();
    // One file holds the certificate chain and the key, so each change is a single rename.
    let dir = tempfile::tempdir().unwrap();
    let old_ca = ca();
    let old = old_ca.issue(&["localhost"]);
    let both = write(
        dir.path(),
        "web.pem",
        format!("{}{}", old.chain_pem, old.key_pem),
    );
    let bundles = load(&web_toml("50ms", &both, &both));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    assert_eq!(served_leaf(server.clone(), &old_ca), Some(old.cert_der));

    // The first read after the change is overtaken by another write: the hook replaces the
    // file between the read and the second metadata check.
    let latest_ca = ca();
    let latest = latest_ca.issue(&["localhost"]);
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let calls = Arc::clone(&calls);
        let path = both.clone();
        let contents = format!("{}{}", latest.chain_pem, latest.key_pem);
        oxbloom_ssl::__private::set_read_hook(&bundles, move |bundle| {
            assert_eq!(bundle, "web");
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                replace(&path, &contents);
            }
        });
    }
    let task = bundles.start_reload();

    let middle = ca().issue(&["localhost"]);
    replace(&both, format!("{}{}", middle.chain_pem, middle.key_pem));

    assert!(
        wait_for(|| served_leaf(server.clone(), &latest_ca).as_ref() == Some(&latest.cert_der)),
        "the latest certificate was not served within five seconds"
    );
    assert!(calls.load(Ordering::SeqCst) >= 2);
    let retries = events.ssl(Level::DEBUG);
    assert!(
        retries
            .iter()
            .any(|e| e.text.contains("changed during the read")),
        "no retry event: {retries:?}"
    );
    assert!(events.ssl(Level::WARN).is_empty());
    task.stop();
}

#[test]
fn reload_returns_the_result_per_bundle() {
    let web = web_files();
    let trust = write(web.dir.path(), "ca.pem", &web.old_ca.cert_pem);
    let bundles = load(&format!(
        "{}\n[ssl.bundle.pem.api]\ntrust = \"{}\"",
        web_toml("0s", &web.cert, &web.key),
        toml_path(&trust)
    ));

    replace(&trust, "not a certificate");
    let new_ca = ca();
    let issued = new_ca.issue(&["localhost"]);
    replace(&web.key, &issued.key_pem);
    replace(&web.cert, &issued.chain_pem);

    let results = bundles.reload();
    let names: Vec<&str> = results.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["api", "web"]);
    assert!(matches!(
        &results[0].1,
        Err(SslError::Pem { bundle, key })
            if bundle == "api" && key == "ssl.bundle.pem.api.trust"
    ));
    assert!(results[1].1.is_ok());

    // The failed bundle keeps its material; the other one serves its new certificate.
    assert!(bundles.get("api").unwrap().has_trust());
    let server = bundles.get("web").unwrap().server_config().unwrap();
    assert_eq!(served_leaf(server, &new_ca), Some(issued.cert_der));
}

#[test]
fn reload_retries_a_file_that_changes_during_the_read() {
    let web = web_files();
    let bundles = load(&web_toml("0s", &web.cert, &web.key));
    let latest_ca = ca();
    let latest = latest_ca.issue(&["localhost"]);
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let calls = Arc::clone(&calls);
        let (cert, key) = (web.cert.clone(), web.key.clone());
        let (cert_pem, key_pem) = (latest.chain_pem.clone(), latest.key_pem.clone());
        oxbloom_ssl::__private::set_read_hook(&bundles, move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                replace(&key, &key_pem);
                replace(&cert, &cert_pem);
            }
        });
    }

    let results = bundles.reload();
    assert!(results[0].1.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let server = bundles.get("web").unwrap().server_config().unwrap();
    assert_eq!(served_leaf(server, &latest_ca), Some(latest.cert_der));
}

#[test]
fn zero_interval_starts_no_task() {
    let web = web_files();
    let bundles = load(&web_toml("0s", &web.cert, &web.key));
    assert_eq!(bundles.reload_interval(), Duration::ZERO);
    let server = bundles.get("web").unwrap().server_config().unwrap();
    let task = bundles.start_reload();
    assert!(!task.is_running());

    let issued = ca().issue(&["localhost"]);
    replace(&web.key, &issued.key_pem);
    replace(&web.cert, &issued.chain_pem);
    let_ticks_pass(4);
    assert_eq!(served_leaf(server, &web.old_ca), Some(web.old_leaf));
    task.stop();
}

#[test]
fn stop_ends_the_task_and_drop_is_safe() {
    let web = web_files();
    let bundles = load(&web_toml("50ms", &web.cert, &web.key));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    let task = bundles.start_reload();
    assert!(task.is_running());
    task.stop();

    let issued = ca().issue(&["localhost"]);
    replace(&web.key, &issued.key_pem);
    replace(&web.cert, &issued.chain_pem);
    let_ticks_pass(4);
    assert_eq!(
        served_leaf(server.clone(), &web.old_ca),
        Some(web.old_leaf.clone())
    );

    // Dropping a task stops it as well, inside and outside a runtime.
    drop(bundles.start_reload());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async { drop(bundles.start_reload()) });
    drop(runtime);
    let_ticks_pass(4);
    assert_eq!(served_leaf(server, &web.old_ca), Some(web.old_leaf));
}

#[cfg(feature = "di")]
#[test]
fn the_container_starts_and_stops_the_task() {
    use oxbloom_di::ContainerBuilder;

    let web = web_files();
    let bundles = load(&web_toml("50ms", &web.cert, &web.key));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    let container = ContainerBuilder::new()
        .instance(bundles)
        .lifecycle::<SslBundles>()
        .build()
        .unwrap();

    let new_ca = ca();
    let issued = new_ca.issue(&["localhost"]);
    replace(&web.key, &issued.key_pem);
    replace(&web.cert, &issued.chain_pem);
    assert!(
        wait_for(|| served_leaf(server.clone(), &new_ca).as_ref() == Some(&issued.cert_der)),
        "the container did not start the reload task"
    );

    container.shutdown();
    let last = ca().issue(&["localhost"]);
    replace(&web.key, &last.key_pem);
    replace(&web.cert, &last.chain_pem);
    let_ticks_pass(4);
    assert_eq!(served_leaf(server, &new_ca), Some(issued.cert_der));
}

#[test]
fn a_panicking_pass_is_logged_and_the_next_tick_tries_again() {
    let (events, _guard) = capture();
    let web = web_files();
    let bundles = load(&web_toml("50ms", &web.cert, &web.key));
    let server = bundles.get("web").unwrap().server_config().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    {
        let calls = Arc::clone(&calls);
        oxbloom_ssl::__private::set_read_hook(&bundles, move |_| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("the test hook panics once");
            }
        });
    }
    let task = bundles.start_reload();

    let new_ca = ca();
    let issued = new_ca.issue(&["localhost"]);
    replace(&web.key, &issued.key_pem);
    replace(&web.cert, &issued.chain_pem);

    assert!(
        wait_for(|| served_leaf(server.clone(), &new_ca).as_ref() == Some(&issued.cert_der)),
        "the next tick did not reload after the panic"
    );
    let warnings = events.ssl(Level::WARN);
    assert!(
        warnings.iter().any(|e| e.text.contains("panicked")),
        "no panic warning: {warnings:?}"
    );
    task.stop();
}
