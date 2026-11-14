//! The file layer: names, rotation, retention, flush on drop and errors. Every test builds a
//! scoped subscriber; nothing installs the global one.

use std::fs;
use std::path::Path;

use oxbloom_config::{Config, ConfigError};
use oxbloom_logging::__private::scoped;
use oxbloom_logging::{Level, LoggingError, LoggingSystem};
use tracing::Dispatch;

fn build(dir: &Path, extra: &str, console: &str) -> (Dispatch, LoggingSystem) {
    let toml = format!(
        "[logging.file]\nenabled = true\npath = {:?}\nname = \"app\"\n{extra}\n{console}",
        dir.to_str().unwrap()
    );
    scoped(&Config::from_str(&toml).unwrap(), std::io::sink).unwrap()
}

fn log(dispatch: &Dispatch, message: &str) {
    tracing::dispatcher::with_default(dispatch, || tracing::info!("{}", message));
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

/// Drops the subscriber and every clone of the system, which flushes the file.
fn finish(dispatch: Dispatch, system: LoggingSystem) {
    drop(dispatch);
    drop(system);
}

fn read_all(dir: &Path) -> String {
    names(dir)
        .iter()
        .map(|n| fs::read_to_string(dir.join(n)).unwrap())
        .collect()
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

#[test]
fn writes_to_file_with_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let (d, s) = build(dir.path(), "", "");
    log(&d, "hello file");
    finish(d, s);
    let n = names(dir.path());
    assert_eq!(n.len(), 1);
    assert!(n[0].starts_with("app"));
    assert!(read_all(dir.path()).contains("hello file"));
}

#[test]
fn no_ansi_in_file() {
    let dir = tempfile::tempdir().unwrap();
    let (d, s) = build(dir.path(), "", "[logging.console]\ncolor = \"always\"");
    log(&d, "plain");
    finish(d, s);
    let text = read_all(dir.path());
    assert!(text.contains("plain"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn daily_and_hourly_and_never_names() {
    let cases = [("daily", Some(10)), ("hourly", Some(13)), ("never", None)];
    for (rotation, suffix) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (d, s) = build(dir.path(), &format!("rotation = \"{rotation}\""), "");
        log(&d, "x");
        finish(d, s);
        let n = names(dir.path());
        assert_eq!(n.len(), 1, "{rotation}");
        match suffix {
            None => assert_eq!(n[0], "app"),
            Some(len) => {
                let rest = n[0].strip_prefix("app.").unwrap();
                assert_eq!(rest.len(), len, "{rotation}");
                assert!(is_date(&rest[..10]));
                if len == 13 {
                    assert!(
                        rest.as_bytes()[10] == b'-'
                            && rest[11..].bytes().all(|c| c.is_ascii_digit())
                    );
                }
            }
        }
    }
}

#[test]
fn max_history_removes_old_files() {
    let dir = tempfile::tempdir().unwrap();
    for day in 1..=6 {
        fs::write(dir.path().join(format!("app.2020-01-0{day}")), "old").unwrap();
    }
    let (d, s) = build(dir.path(), "max-history = 3", "");
    log(&d, "new");
    finish(d, s);
    let n = names(dir.path());
    assert!(n.len() <= 3, "{n:?}");
    assert!(read_all(dir.path()).contains("new"));
}

#[test]
fn flush_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let (d, s) = build(dir.path(), "", "");
    let clone = s.clone();
    log(&d, "last words");
    drop(s);
    drop(d);
    drop(clone);
    assert!(read_all(dir.path()).contains("last words"));
}

#[test]
fn json_format_in_file() {
    let dir = tempfile::tempdir().unwrap();
    let (d, s) = build(dir.path(), "format = \"json\"", "");
    log(&d, "as \"json\"\nline");
    finish(d, s);
    let text = read_all(dir.path());
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 1);
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["message"], "as \"json\"\nline");
    assert_eq!(v["level"], "info");
}

#[test]
fn creates_missing_directory() {
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("a/b/logs");
    let (d, s) = build(&dir, "", "");
    log(&d, "deep");
    finish(d, s);
    assert!(read_all(&dir).contains("deep"));
}

#[test]
fn path_is_a_file_gives_io_error_with_path() {
    let base = tempfile::tempdir().unwrap();
    let file = base.path().join("not-a-dir");
    fs::write(&file, "x").unwrap();
    let toml = format!(
        "[logging.file]\nenabled = true\npath = {:?}\n",
        file.to_str().unwrap()
    );
    let err = scoped(&Config::from_str(&toml).unwrap(), std::io::sink).unwrap_err();
    match err {
        LoggingError::Io { path, .. } => assert_eq!(path, file),
        other => panic!("{other:?}"),
    }
}

#[test]
fn empty_path_or_name_is_a_config_error() {
    for (toml, key) in [
        (
            "[logging.file]\nenabled = true\npath = \"\"",
            "logging.file.path",
        ),
        (
            "[logging.file]\nenabled = true\nname = \"\"",
            "logging.file.name",
        ),
    ] {
        let err = scoped(&Config::from_str(toml).unwrap(), std::io::sink).unwrap_err();
        match err {
            LoggingError::Config(ConfigError::Invalid { key: k, .. }) => assert_eq!(k, key),
            other => panic!("{other:?}"),
        }
    }
    // Disabled file output ignores them.
    let off = "[logging.file]\npath = \"\"\nname = \"\"";
    assert!(scoped(&Config::from_str(off).unwrap(), std::io::sink).is_ok());
}

#[test]
fn file_and_console_both_receive_events() {
    let dir = tempfile::tempdir().unwrap();
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = out.clone();
    let toml = format!(
        "[logging.file]\nenabled = true\npath = {:?}\nname = \"app\"",
        dir.path().to_str().unwrap()
    );
    let (d, s) = scoped(&Config::from_str(&toml).unwrap(), move || {
        Shared(sink.clone())
    })
    .unwrap();
    log(&d, "to both");
    finish(d, s);
    assert!(read_all(dir.path()).contains("to both"));
    assert!(
        String::from_utf8(out.lock().unwrap().clone())
            .unwrap()
            .contains("to both")
    );
}

struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Shared {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn set_level_applies_to_file() {
    let dir = tempfile::tempdir().unwrap();
    let (d, s) = build(dir.path(), "", "");
    tracing::dispatcher::with_default(&d, || tracing::debug!(target: "app", "hidden"));
    s.set_level("app", Level::Debug).unwrap();
    tracing::dispatcher::with_default(&d, || tracing::debug!(target: "app", "shown"));
    finish(d, s);
    let text = read_all(dir.path());
    assert!(text.contains("shown"));
    assert!(!text.contains("hidden"));
}

/// Builds file + captured console output from `extra` (file keys) and `console` (console keys),
/// runs a span `req{id=42}` with `x` recorded later and one event inside it, and returns the file
/// text and the console text.
fn span_run(extra: &str, console: &str) -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let sink = out.clone();
    let toml = format!(
        "[logging.file]\nenabled = true\npath = {:?}\nname = \"app\"\n{extra}\n{console}",
        dir.path().to_str().unwrap()
    );
    let (d, s) = scoped(&Config::from_str(&toml).unwrap(), move || {
        Shared(sink.clone())
    })
    .unwrap();
    tracing::dispatcher::with_default(&d, || {
        let span = tracing::info_span!("req", id = 42, x = tracing::field::Empty);
        span.record("x", 5);
        let _g = span.enter();
        tracing::info!("inside");
    });
    finish(d, s);
    let console = String::from_utf8(out.lock().unwrap().clone()).unwrap();
    (read_all(dir.path()), console)
}

#[test]
fn colored_console_span_fields_stay_plain_in_file() {
    for format in ["full", "compact"] {
        for console in ["full", "compact", "pretty"] {
            let (file, out) = span_run(
                &format!("format = \"{format}\""),
                &format!("[logging.console]\ncolor = \"always\"\nformat = \"{console}\""),
            );
            assert!(file.contains("inside"), "{format}/{console}: {file}");
            assert!(file.contains("42"), "{format}/{console}: {file}");
            assert!(!file.contains('\u{1b}'), "{format}/{console}: {file:?}");
            assert!(out.contains('\u{1b}'), "{format}/{console}: {out:?}");
        }
    }
}

#[test]
fn recorded_span_field_appears_once_in_each_output() {
    for format in ["full", "compact"] {
        for console in ["full", "compact"] {
            let (file, out) = span_run(
                &format!("format = \"{format}\""),
                &format!("[logging.console]\ncolor = \"never\"\nformat = \"{console}\""),
            );
            assert_eq!(file.matches("x=5").count(), 1, "{format}/{console}: {file}");
            assert_eq!(
                file.matches("id=42").count(),
                1,
                "{format}/{console}: {file}"
            );
            assert_eq!(out.matches("x=5").count(), 1, "{format}/{console}: {out}");
            assert_eq!(out.matches("id=42").count(), 1, "{format}/{console}: {out}");
        }
    }
}
