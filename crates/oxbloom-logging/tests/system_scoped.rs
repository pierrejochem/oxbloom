//! `LoggingSystem` against a scoped subscriber (`with_default`): console format and color,
//! groups at runtime, and `set_level` from inside an event. Nothing here installs the global
//! subscriber.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use oxbloom_logging::__private::scoped;
use oxbloom_logging::{Level, LoggingError, LoggingSystem};
use tracing::Dispatch;
use tracing_subscriber::fmt::MakeWriter;

/// Collects the console output. When armed, the next write calls `set_level`.
#[derive(Clone, Default)]
struct Capture {
    buf: Arc<Mutex<Vec<u8>>>,
    system: Arc<OnceLock<LoggingSystem>>,
    armed: Arc<AtomicBool>,
}

impl Capture {
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.buf.lock().unwrap())).unwrap()
    }
}

impl io::Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(data);
        if self.armed.swap(false, Ordering::SeqCst) {
            let system = self.system.get().unwrap();
            system.set_level("root", Level::Trace).unwrap();
            system.reset("root").unwrap();
            system.set_level("inside", Level::Trace).unwrap();
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

fn subscriber(toml: &str) -> (Dispatch, LoggingSystem, Capture) {
    let out = Capture::default();
    let (dispatch, system) = scoped(&Config::from_str(toml).unwrap(), out.clone()).unwrap();
    out.system.set(system.clone()).unwrap();
    (dispatch, system, out)
}

/// The output of `events` under the scoped subscriber.
fn output(dispatch: &Dispatch, out: &Capture, events: impl FnOnce()) -> String {
    tracing::dispatcher::with_default(dispatch, events);
    out.take()
}

#[test]
fn every_format_prints_the_message() {
    for format in ["full", "compact", "pretty"] {
        let (dispatch, _, out) = subscriber(&format!(
            "[logging.console]\nformat = \"{format}\"\ncolor = \"never\""
        ));
        let text = output(&dispatch, &out, || {
            tracing::info!(target: "app", "hello from {format}");
        });
        assert!(text.contains(&format!("hello from {format}")), "{text}");
        assert!(!text.contains('\x1b'), "{format}: {text}");
    }
}

#[test]
fn formats_differ() {
    let mut texts = Vec::new();
    for format in ["full", "compact", "pretty"] {
        let (dispatch, _, out) = subscriber(&format!(
            "[logging.console]\nformat = \"{format}\"\ncolor = \"never\""
        ));
        texts.push(output(&dispatch, &out, || {
            tracing::info!(target: "app", answer = 42, "same message");
        }));
    }
    // Pretty spreads an event over several lines; compact and full keep one.
    assert_eq!(texts[0].lines().count(), 1, "{}", texts[0]);
    assert_eq!(texts[1].lines().count(), 1, "{}", texts[1]);
    assert!(texts[2].lines().count() > 1, "{}", texts[2]);
    assert_ne!(texts[0], texts[1]);
}

#[test]
fn color_always_and_never() {
    let (dispatch, _, out) = subscriber("[logging.console]\ncolor = \"always\"");
    let text = output(&dispatch, &out, || tracing::warn!(target: "app", "colored"));
    assert!(
        text.contains("colored") && text.contains("\x1b["),
        "{text:?}"
    );

    let (dispatch, _, out) = subscriber("[logging.console]\ncolor = \"never\"");
    let text = output(&dispatch, &out, || tracing::warn!(target: "app", "plain"));
    assert!(text.contains("plain") && !text.contains('\x1b'), "{text:?}");
}

#[test]
fn root_level_and_target_fallback() {
    let (dispatch, system, out) = subscriber(
        "[logging.level]\nroot = \"error\"\n\"oxbloom_web::router\" = \"trace\"\n\
         [logging.console]\ncolor = \"never\"",
    );
    assert_eq!(system.level("root"), Level::Error);
    assert_eq!(system.level("ROOT"), Level::Error);
    assert_eq!(system.level("oxbloom_web"), Level::Error);
    assert_eq!(system.level("oxbloom_web::router"), Level::Trace);
    assert_eq!(system.level("oxbloom_web::router::path"), Level::Trace);

    let text = output(&dispatch, &out, || {
        tracing::trace!(target: "oxbloom_web::router::path", "deep");
        tracing::warn!(target: "oxbloom_web", "shallow");
    });
    assert!(text.contains("deep") && !text.contains("shallow"), "{text}");
}

#[test]
fn set_level_and_reset_a_target() {
    let (dispatch, system, out) =
        subscriber("[logging.level]\napp = \"warn\"\n[logging.console]\ncolor = \"never\"");
    system.set_level("app", Level::Debug).unwrap();
    system.set_level("other", Level::Trace).unwrap();
    assert_eq!(system.level("app"), Level::Debug);
    let text = output(&dispatch, &out, || {
        tracing::debug!(target: "app", "app-debug");
        tracing::trace!(target: "other", "other-trace");
    });
    assert!(
        text.contains("app-debug") && text.contains("other-trace"),
        "{text}"
    );

    // Reset goes back to the configured level, or to none (root) when there was none.
    system.reset("app").unwrap();
    system.reset("other").unwrap();
    assert_eq!(system.level("app"), Level::Warn);
    assert_eq!(system.level("other"), Level::Info);
    let text = output(&dispatch, &out, || {
        tracing::debug!(target: "app", "app-debug");
        tracing::debug!(target: "other", "other-debug");
    });
    assert_eq!(text, "");
}

#[test]
fn invalid_target_names_the_key() {
    let (_dispatch, system, _) = subscriber("");
    for bad in ["a,b", "a=trace", "", "a b"] {
        let err = system.set_level(bad, Level::Trace).unwrap_err();
        assert!(
            matches!(&err, LoggingError::Config(ConfigError::Invalid { key, .. })
                if *key == format!("logging.level.{bad}")),
            "{err:?}"
        );
        assert!(system.reset(bad).is_err());
    }
    // Nothing changed.
    assert_eq!(system.level("a"), Level::Info);
}

const GROUPS: &str = "[logging.group]\nweb = [\"app_web\", \"app_api\"]\n\
                      [logging.level]\nweb = \"debug\"\napp_api = \"trace\"\n\
                      [logging.console]\ncolor = \"never\"";

#[test]
fn level_of_a_group_and_its_members() {
    let (_dispatch, system, _) = subscriber(GROUPS);
    assert_eq!(system.level("web"), Level::Debug);
    assert_eq!(system.level("Web"), Level::Debug);
    assert_eq!(system.level("app_web"), Level::Debug);
    assert_eq!(system.level("app_web::router"), Level::Debug);
    // A member's own level beats its group.
    assert_eq!(system.level("app_api"), Level::Trace);
    assert_eq!(system.level("elsewhere"), Level::Info);

    // A group without a level reports the root level.
    let (_dispatch, system, _) = subscriber("[logging.group]\nquiet = [\"q\"]");
    assert_eq!(system.level("quiet"), Level::Info);
}

#[test]
fn set_level_on_a_group_applies_to_its_members() {
    let (dispatch, system, out) = subscriber(GROUPS);
    system.set_level("WEB", Level::Warn).unwrap();
    assert_eq!(system.level("web"), Level::Warn);
    assert_eq!(system.level("app_web"), Level::Warn);
    assert_eq!(system.level("app_api"), Level::Trace);
    let text = output(&dispatch, &out, || {
        tracing::info!(target: "app_web", "web-info");
        tracing::warn!(target: "app_web", "web-warn");
        tracing::trace!(target: "app_api", "api-trace");
    });
    assert!(!text.contains("web-info"), "{text}");
    assert!(
        text.contains("web-warn") && text.contains("api-trace"),
        "{text}"
    );

    // The group name is not a target of its own.
    let text = output(
        &dispatch,
        &out,
        || tracing::info!(target: "web", "named-web"),
    );
    assert!(text.contains("named-web"), "{text}");
}

#[test]
fn reset_on_a_group_restores_every_member() {
    let (dispatch, system, out) = subscriber(GROUPS);
    system.set_level("web", Level::Error).unwrap();
    system.set_level("app_web", Level::Off).unwrap();
    system.set_level("app_api", Level::Off).unwrap();
    system.reset("web").unwrap();
    assert_eq!(system.level("web"), Level::Debug);
    assert_eq!(system.level("app_web"), Level::Debug);
    assert_eq!(system.level("app_api"), Level::Trace);
    let text = output(&dispatch, &out, || {
        tracing::debug!(target: "app_web", "web-debug");
        tracing::trace!(target: "app_api", "api-trace");
    });
    assert!(
        text.contains("web-debug") && text.contains("api-trace"),
        "{text}"
    );
}

#[test]
fn overlapping_groups_last_by_name_wins() {
    let (_dispatch, system, _) = subscriber(
        "[logging.group]\na = [\"shared\"]\nb = [\"shared\"]\n\
         [logging.level]\na = \"debug\"\nb = \"warn\"",
    );
    assert_eq!(system.level("shared"), Level::Warn);
    system.set_level("a", Level::Trace).unwrap();
    assert_eq!(system.level("shared"), Level::Warn);
    system.set_level("b", Level::Error).unwrap();
    assert_eq!(system.level("shared"), Level::Error);
    // A member's own level beats every group.
    system.set_level("shared", Level::Trace).unwrap();
    assert_eq!(system.level("shared"), Level::Trace);
}

#[test]
fn reload_replaces_levels_and_drops_runtime_changes() {
    let (dispatch, system, out) = subscriber(GROUPS);
    system.set_level("extra", Level::Trace).unwrap();
    let next = Config::from_str("[logging.level]\nroot = \"warn\"\napp_api = \"error\"").unwrap();
    system.reload(&next).unwrap();
    assert_eq!(system.level("root"), Level::Warn);
    assert_eq!(system.level("extra"), Level::Warn);
    // `web` is no longer a group, so it is a plain target at the root level.
    assert_eq!(system.level("web"), Level::Warn);
    assert_eq!(system.level("app_web"), Level::Warn);
    assert_eq!(system.level("app_api"), Level::Error);
    let text = output(&dispatch, &out, || {
        tracing::trace!(target: "extra", "extra-trace");
        tracing::debug!(target: "app_web", "web-debug");
    });
    assert_eq!(text, "");

    // An invalid config leaves the current levels alone.
    let bad = Config::from_str("[logging.level]\nroot = \"hunter2\"").unwrap();
    let err = system.reload(&bad).unwrap_err();
    assert!(!format!("{err} {err:?}").contains("hunter2"));
    assert_eq!(system.level("root"), Level::Warn);
}

#[test]
fn set_level_inside_event_does_not_deadlock() {
    let (dispatch, system, out) = subscriber("[logging.console]\ncolor = \"never\"");
    out.armed.store(true, Ordering::SeqCst);
    let (done, finished) = mpsc::channel();
    let events = dispatch.clone();
    thread::spawn(move || {
        tracing::dispatcher::with_default(&events, || {
            tracing::info!(target: "app", "trigger");
            tracing::trace!(target: "inside", "after");
        });
        done.send(()).unwrap();
    });
    finished
        .recv_timeout(Duration::from_secs(1))
        .expect("set_level inside an event deadlocked");
    let text = out.take();
    assert!(text.contains("trigger") && text.contains("after"), "{text}");
    assert_eq!(system.level("inside"), Level::Trace);
    assert_eq!(system.level("root"), Level::Info);
}

#[test]
fn system_is_cheap_to_share() {
    fn assert_traits<T: Clone + Send + Sync + 'static>() {}
    assert_traits::<LoggingSystem>();

    let (_dispatch, system, _) = subscriber("");
    let copy = system.clone();
    copy.set_level("shared_state", Level::Debug).unwrap();
    assert_eq!(system.level("shared_state"), Level::Debug);
}

#[test]
fn set_level_after_the_subscriber_is_gone_is_a_filter_error() {
    let (dispatch, system, _) = subscriber("");
    drop(dispatch);
    assert!(matches!(
        system.set_level("app", Level::Debug),
        Err(LoggingError::Filter(_))
    ));
}

fn json_lines(toml: &str, events: impl FnOnce()) -> Vec<serde_json::Value> {
    let (dispatch, _system, out) = subscriber(toml);
    let text = output(&dispatch, &out, events);
    text.lines()
        .map(|l| serde_json::from_str(l).expect("each line is valid JSON"))
        .collect()
}

const JSON: &str = "[logging.console]\nformat = \"json\"\n";

#[test]
fn json_line_has_default_fields() {
    let lines = json_lines(JSON, || tracing::info!(target: "app", "hello"));
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(line["level"], "info");
    assert_eq!(line["target"], "app");
    assert_eq!(line["message"], "hello");
    let ts = line["timestamp"].as_str().unwrap();
    assert!(ts.ends_with('Z') && ts.contains('T'), "{ts}");
    assert!(line["thread"].is_string());
}

#[test]
fn toggle_off_removes_field() {
    let toml = format!("{JSON}[logging.json.fields]\nthread = false\ntimestamp = false\n");
    let lines = json_lines(&toml, || tracing::info!("hello"));
    let line = lines[0].as_object().unwrap();
    assert!(!line.contains_key("thread") && !line.contains_key("timestamp"));
    assert!(line.contains_key("message") && line.contains_key("level"));
}

#[test]
fn rename_changes_key() {
    let toml = format!("{JSON}[logging.json.names]\nmessage = \"msg\"\n");
    let lines = json_lines(&toml, || tracing::info!("hello"));
    assert_eq!(lines[0]["msg"], "hello");
    assert!(lines[0].get("message").is_none());
}

#[test]
fn event_fields_are_kept() {
    let lines = json_lines(JSON, || {
        tracing::info!(user = 7, ok = true, ratio = 0.5, who = "bob", "hi");
    });
    assert_eq!(lines[0]["user"], 7);
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[0]["ratio"], 0.5);
    assert_eq!(lines[0]["who"], "bob");
    assert_eq!(lines[0]["message"], "hi");
}

#[test]
fn awkward_message_is_one_valid_line() {
    let nasty = "q\"uote \\ new\nline \u{1} tab\t caf\u{e9} \u{1f600}";
    let (dispatch, _system, out) = subscriber(JSON);
    let text = output(&dispatch, &out, || tracing::info!("{nasty}"));
    assert_eq!(text.lines().count(), 1);
    assert!(text.ends_with('\n'));
    let value: serde_json::Value = serde_json::from_str(text.trim_end()).unwrap();
    assert_eq!(value["message"], nasty);
}

#[test]
fn span_fields_when_enabled() {
    let lines = json_lines(JSON, || {
        let span = tracing::info_span!("request", id = 42, path = "/x");
        let _guard = span.enter();
        tracing::info!("inside");
    });
    let span = &lines[0]["span"];
    assert_eq!(span["name"], "request");
    assert_eq!(span["id"], 42);
    assert_eq!(span["path"], "/x");
}

#[test]
fn span_off_and_outside_span_have_no_span_key() {
    let toml = format!("{JSON}[logging.json.fields]\nspan = false\n");
    let lines = json_lines(&toml, || {
        let span = tracing::info_span!("request");
        let _guard = span.enter();
        tracing::info!("inside");
    });
    assert!(lines[0].get("span").is_none());
    let lines = json_lines(JSON, || tracing::info!("outside"));
    assert!(lines[0].get("span").is_none());
}

#[test]
fn level_filter_still_applies() {
    let (dispatch, system, out) = subscriber(JSON);
    let hidden = output(&dispatch, &out, || tracing::debug!(target: "app", "no"));
    assert!(hidden.is_empty());
    system.set_level("app", Level::Debug).unwrap();
    let shown = output(&dispatch, &out, || tracing::debug!(target: "app", "yes"));
    assert!(shown.contains("\"yes\""));
}

#[test]
fn levels_lists_root_targets_and_group_members() {
    let (_dispatch, system, _) = subscriber(GROUPS);
    let levels = system.levels();
    assert_eq!(levels["root"], Level::Info);
    assert_eq!(levels["app_web"], Level::Debug);
    assert_eq!(levels["app_api"], Level::Trace);

    system.set_level("extra", Level::Warn).unwrap();
    system.set_level("root", Level::Error).unwrap();
    let levels = system.levels();
    assert_eq!(levels["extra"], Level::Warn);
    assert_eq!(levels["root"], Level::Error);
}
