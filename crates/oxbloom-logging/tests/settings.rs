//! `Settings::from_config`: levels, groups, console options and the property list.

use oxbloom_config::{Config, ConfigError};
use oxbloom_logging::__private::Settings;
use oxbloom_logging::{
    ConsoleColor, ConsoleFormat, FileFormat, FileRotation, JsonField, Level, LoggingError,
    PROPERTIES,
};
use std::error::Error as _;
use std::path::PathBuf;

fn settings(toml: &str) -> Settings {
    Settings::from_config(&Config::from_str(toml).unwrap()).unwrap()
}

fn error(toml: &str) -> LoggingError {
    match Settings::from_config(&Config::from_str(toml).unwrap()) {
        Ok(_) => panic!("expected an error"),
        Err(e) => e,
    }
}

/// A config built the way boot builds it: a file in a directory, no environment, `args`.
fn built(toml: &str, args: &[&str]) -> Config {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application.toml"), toml).unwrap();
    Config::builder()
        .dir(dir.path())
        .env_from([])
        .args(args.iter().map(|a| a.to_string()))
        .build()
        .unwrap()
}

#[test]
fn defaults_are_info_full_auto() {
    let s = settings("");
    assert_eq!(s.root(), Level::Info);
    assert_eq!(s.format(), ConsoleFormat::Full);
    assert_eq!(s.color(), ConsoleColor::Auto);
    assert_eq!(s.directives(), "info");

    // An empty `[logging]` table is the same as none.
    assert_eq!(settings("[logging]").directives(), "info");
}

#[test]
fn levels_parse_case_insensitively() {
    for (text, want) in [
        ("INFO", Level::Info),
        ("Debug", Level::Debug),
        ("trace", Level::Trace),
        ("off", Level::Off),
        ("ERROR", Level::Error),
        ("wArN", Level::Warn),
    ] {
        assert_eq!(text.parse::<Level>(), Ok(want), "{text}");
        let s = settings(&format!("[logging.level]\nroot = \"{text}\""));
        assert_eq!(s.root(), want, "{text}");
    }
    assert!("verbose".parse::<Level>().is_err());
}

#[test]
fn target_levels_become_directives() {
    let s = settings("[logging.level]\nroot = \"warn\"\noxbloom_web = \"debug\"");
    assert_eq!(s.root(), Level::Warn);
    assert_eq!(s.directives(), "warn,oxbloom_web=debug");
}

#[test]
fn root_key_matches_relaxed() {
    // Config matches keys relaxed, so `ROOT` is the root level, not a target named `ROOT`.
    let s = settings("[logging.level]\nROOT = \"error\"");
    assert_eq!(s.directives(), "error");
}

#[test]
fn colon_targets_work_from_toml_and_args() {
    let toml = settings("[logging.level]\n\"oxbloom_web::router\" = \"trace\"");
    assert_eq!(toml.directives(), "info,oxbloom_web::router=trace");

    let config = built("", &["--logging.level.oxbloom_web::router=trace"]);
    let args = Settings::from_config(&config).unwrap();
    assert_eq!(args.directives(), "info,oxbloom_web::router=trace");

    // The command line overrides the same quoted key from the file.
    let config = built(
        "[logging.level]\n\"oxbloom_web::router\" = \"error\"",
        &["--logging.level.oxbloom_web::router=trace"],
    );
    let both = Settings::from_config(&config).unwrap();
    assert_eq!(both.directives(), "info,oxbloom_web::router=trace");
}

#[test]
fn group_expands_to_targets() {
    let s = settings(
        "[logging.group]\nweb = [\"oxbloom_web\", \"oxbloom_security\"]\n\
         [logging.level]\nweb = \"debug\"",
    );
    assert_eq!(
        s.directives(),
        "info,oxbloom_security=debug,oxbloom_web=debug"
    );
    assert!(s.directives().split(',').all(|d| !d.starts_with("web=")));
}

#[test]
fn group_from_args_is_a_comma_list() {
    let config = built(
        "",
        &[
            "--logging.group.web=oxbloom_web,oxbloom_security",
            "--logging.level.web=debug",
        ],
    );
    let s = Settings::from_config(&config).unwrap();
    assert_eq!(
        s.directives(),
        "info,oxbloom_security=debug,oxbloom_web=debug"
    );
}

#[test]
fn group_beats_same_named_target() {
    let s = settings(
        "[logging.group]\nweb = [\"oxbloom_web\", \"oxbloom_security\"]\n\
         [logging.level]\nweb = \"error\"",
    );
    assert_eq!(
        s.directives(),
        "info,oxbloom_security=error,oxbloom_web=error"
    );
    assert!(s.directives().split(',').all(|d| !d.starts_with("web=")));
}

#[test]
fn group_without_level_adds_nothing() {
    let s = settings("[logging.group]\nweb = [\"oxbloom_web\"]");
    assert_eq!(s.directives(), "info");
}

#[test]
fn explicit_target_beats_its_group() {
    let s = settings(
        "[logging.group]\nweb = [\"oxbloom_web\", \"oxbloom_security\"]\n\
         [logging.level]\nweb = \"debug\"\noxbloom_web = \"trace\"",
    );
    assert_eq!(
        s.directives(),
        "info,oxbloom_security=debug,oxbloom_web=trace"
    );
}

#[test]
fn target_in_two_groups_takes_the_later_group_by_name() {
    let s = settings(
        "[logging.group]\na = [\"shared\"]\nb = [\"shared\"]\n\
         [logging.level]\na = \"error\"\nb = \"trace\"",
    );
    assert_eq!(s.directives(), "info,shared=trace");
}

#[test]
fn console_options_are_read() {
    let s = settings("[logging.console]\nformat = \"Compact\"\ncolor = \"never\"");
    assert_eq!(s.format(), ConsoleFormat::Compact);
    assert_eq!(s.color(), ConsoleColor::Never);
    let s = settings("[logging.console]\nformat = \"pretty\"\ncolor = \"ALWAYS\"");
    assert_eq!(s.format(), ConsoleFormat::Pretty);
    assert_eq!(s.color(), ConsoleColor::Always);
}

fn assert_invalid(err: &LoggingError, key: &str, secret: &str) {
    match err {
        LoggingError::Config(ConfigError::Invalid { key: k, .. }) => assert_eq!(k, key),
        other => panic!("expected Config(Invalid), got {other:?}"),
    }
    let text = format!("{err} {err:?}");
    assert!(text.contains(key), "{text}");
    assert!(!text.contains(secret), "{text}");
}

#[test]
fn invalid_level_names_key_not_value() {
    assert_invalid(
        &error("[logging.level]\nx = \"hunter2\""),
        "logging.level.x",
        "hunter2",
    );
    assert_invalid(
        &error("[logging.level]\nroot = \"hunter2\""),
        "logging.level.root",
        "hunter2",
    );
    assert_invalid(
        &error("[logging.console]\nformat = \"hunter2\""),
        "logging.console.format",
        "hunter2",
    );
    assert_invalid(
        &error("[logging.console]\ncolor = \"hunter2\""),
        "logging.console.color",
        "hunter2",
    );
}

#[test]
fn invalid_level_from_args_names_key_not_value() {
    let config = built("", &["--logging.level.x=hunter2"]);
    let err = Settings::from_config(&config).unwrap_err();
    assert_invalid(&err, "logging.level.x", "hunter2");
}

#[test]
fn target_names_outside_the_filter_syntax_are_invalid() {
    // A `,` or `=` in a target would inject extra filter directives.
    let err = error("[logging.level]\n\"a,b\" = \"debug\"");
    assert!(
        matches!(&err, LoggingError::Config(ConfigError::Invalid { key, .. }) if key == "logging.level.a,b"),
        "{err:?}"
    );
    assert!(err.to_string().contains("logging.level.a,b"), "{err}");
    let err = error("[logging.group]\nweb = [\"hunter2=trace\"]\n[logging.level]\nweb = \"debug\"");
    assert_invalid(&err, "logging.group.web", "hunter2");
}

#[test]
fn a_group_named_root_is_invalid() {
    let err = error("[logging.group]\nroot = [\"a\"]");
    assert!(
        matches!(&err, LoggingError::Config(ConfigError::Invalid { key, .. }) if key == "logging.group.root"),
        "{err:?}"
    );
}

#[test]
fn logging_that_is_not_a_table_is_invalid() {
    let err = error("logging = \"hunter2\"");
    assert_invalid(&err, "logging", "hunter2");
}

/// The group sample's level, without the `logging.group` entry.
const GROUP_LEVEL: &str = "[logging.level]\nweb = \"trace\"";

/// A sample for a key that leaves the level and console settings at their defaults.
fn same(toml: &str) -> (String, Level, &'static str, ConsoleFormat, ConsoleColor) {
    (
        toml.into(),
        Level::Info,
        "info",
        ConsoleFormat::Full,
        ConsoleColor::Auto,
    )
}

/// One config per declared key that sets it, and the exact settings it must give:
/// `(toml, root, directives, format, color)`.
fn sample(key: &str) -> (String, Level, &'static str, ConsoleFormat, ConsoleColor) {
    use ConsoleColor::{Auto, Never};
    use ConsoleFormat::{Full, Pretty};
    match key {
        "logging.level.root" => (
            "[logging.level]\nroot = \"trace\"".into(),
            Level::Trace,
            "trace",
            Full,
            Auto,
        ),
        "logging.level.<target>" => (
            "[logging.level]\nsome_target = \"trace\"".into(),
            Level::Info,
            "info,some_target=trace",
            Full,
            Auto,
        ),
        "logging.group.<name>" => (
            format!("[logging.group]\nweb = [\"some_target\"]\n{GROUP_LEVEL}"),
            Level::Info,
            "info,some_target=trace",
            Full,
            Auto,
        ),
        "logging.console.format" => (
            "[logging.console]\nformat = \"pretty\"".into(),
            Level::Info,
            "info",
            Pretty,
            Auto,
        ),
        "logging.console.color" => (
            "[logging.console]\ncolor = \"never\"".into(),
            Level::Info,
            "info",
            Full,
            Never,
        ),
        "logging.file.enabled" => same("[logging.file]\nenabled = true"),
        "logging.file.path" => same("[logging.file]\npath = \"var/log\""),
        "logging.file.name" => same("[logging.file]\nname = \"shop\""),
        "logging.file.rotation" => same("[logging.file]\nrotation = \"hourly\""),
        "logging.file.max-history" => same("[logging.file]\nmax-history = 3"),
        "logging.file.format" => same("[logging.file]\nformat = \"json\""),
        "logging.json.fields.<field>" => same("[logging.json.fields]\nthread = false"),
        "logging.json.names.<field>" => same("[logging.json.names]\nmessage = \"msg\""),
        other => panic!("no sample for declared key {other}"),
    }
}

#[test]
fn static_key_list_is_complete() {
    let keys: Vec<&str> = PROPERTIES.iter().map(|p| p.0).collect();
    assert_eq!(
        keys,
        [
            "logging.level.root",
            "logging.level.<target>",
            "logging.group.<name>",
            "logging.console.format",
            "logging.console.color",
            "logging.file.enabled",
            "logging.file.path",
            "logging.file.name",
            "logging.file.rotation",
            "logging.file.max-history",
            "logging.file.format",
            "logging.json.fields.<field>",
            "logging.json.names.<field>",
        ]
    );
    let defaults = settings("");
    assert_eq!(
        (
            defaults.root(),
            defaults.directives().as_str(),
            defaults.format(),
            defaults.color()
        ),
        (Level::Info, "info", ConsoleFormat::Full, ConsoleColor::Auto)
    );
    for &(key, type_name, default, description) in PROPERTIES {
        assert!(!type_name.is_empty() && !description.is_empty(), "{key}");
        let (toml, root, directives, format, color) = sample(key);
        let set = settings(&toml);
        assert_eq!(
            (
                set.root(),
                set.directives().as_str(),
                set.format(),
                set.color()
            ),
            (root, directives, format, color),
            "{key}"
        );
        // A declared default is the value an empty config gives.
        match key {
            "logging.level.root" => assert_eq!(default, Some("info")),
            "logging.console.format" => assert_eq!(default, Some("full")),
            "logging.console.color" => assert_eq!(default, Some("auto")),
            "logging.file.enabled" => assert_eq!(default, Some("false")),
            "logging.file.path" => assert_eq!(default, Some("logs")),
            "logging.file.rotation" => assert_eq!(default, Some("daily")),
            "logging.file.format" => assert_eq!(default, Some("full")),
            "logging.json.fields.<field>" => assert_eq!(default, Some("true")),
            _ => assert_eq!(default, None, "{key}"),
        }
    }
    assert_eq!(PROPERTIES.len(), 13);
    let format_row = PROPERTIES
        .iter()
        .find(|p| p.0 == "logging.console.format")
        .unwrap();
    assert!(format_row.3.contains("json"));
    // Without the `logging.group` entry, the group sample's level is a plain target: the
    // group key is what turns `web` into `some_target`.
    assert_eq!(settings(GROUP_LEVEL).directives(), "info,web=trace");
}

// ---- C5b: file, JSON ----

fn file_of(toml: &str) -> (PathBuf, String, FileRotation, Option<usize>, FileFormat) {
    let s = settings(toml);
    let f = s.file().expect("file output enabled");
    (
        f.dir.clone(),
        f.name.clone(),
        f.rotation,
        f.max_history,
        f.format,
    )
}

#[test]
fn file_defaults_when_enabled() {
    assert_eq!(
        file_of("[logging.file]\nenabled = true"),
        (
            PathBuf::from("logs"),
            "oxbloom".to_string(),
            FileRotation::Daily,
            None,
            FileFormat::Full
        )
    );
}

#[test]
fn file_absent_when_not_enabled() {
    assert!(settings("").file().is_none());
    assert!(
        settings("[logging.file]\nenabled = false\npath = \"x\"")
            .file()
            .is_none()
    );
}

#[test]
fn file_name_defaults_to_application_name() {
    let (_, name, ..) = file_of("[application]\nname = \"shop\"\n[logging.file]\nenabled = true");
    assert_eq!(name, "shop");
    let (_, name, ..) =
        file_of("[application]\nname = \"shop\"\n[logging.file]\nenabled = true\nname = \"audit\"");
    assert_eq!(name, "audit");
}

#[test]
fn each_file_key_parses() {
    assert_eq!(
        file_of(
            "[logging.file]\nenabled = true\npath = \"var/log\"\nname = \"app\"\n\
             rotation = \"hourly\"\nmax-history = 7\nformat = \"json\""
        ),
        (
            PathBuf::from("var/log"),
            "app".to_string(),
            FileRotation::Hourly,
            Some(7),
            FileFormat::Json
        )
    );
    let (_, _, rotation, _, format) =
        file_of("[logging.file]\nenabled = true\nrotation = \"never\"\nformat = \"compact\"");
    assert_eq!(
        (rotation, format),
        (FileRotation::Never, FileFormat::Compact)
    );
}

#[test]
fn max_history_zero_is_invalid() {
    let err = error("[logging.file]\nenabled = true\nmax-history = 0");
    assert_invalid(&err, "logging.file.max-history", "0");
}

#[test]
fn bad_rotation_names_key_not_value() {
    let err = error("[logging.file]\nenabled = true\nrotation = \"hunter2\"");
    assert_invalid(&err, "logging.file.rotation", "hunter2");
}

#[test]
fn json_fields_default_on_with_own_names() {
    let s = settings("");
    for (field, name) in [
        (JsonField::Timestamp, "timestamp"),
        (JsonField::Level, "level"),
        (JsonField::Target, "target"),
        (JsonField::Message, "message"),
        (JsonField::Span, "span"),
        (JsonField::Thread, "thread"),
    ] {
        assert!(s.json().is_on(field), "{name}");
        assert_eq!(s.json().name(field), name);
    }
}

#[test]
fn json_toggle_and_rename() {
    let s =
        settings("[logging.json.fields]\nthread = false\n[logging.json.names]\nmessage = \"msg\"");
    assert!(!s.json().is_on(JsonField::Thread));
    assert!(s.json().is_on(JsonField::Message));
    assert_eq!(s.json().name(JsonField::Message), "msg");
    assert_eq!(s.json().name(JsonField::Level), "level");
}

#[test]
fn json_unknown_field_is_invalid() {
    let err = error("[logging.json.fields]\nbogus = false");
    assert_invalid(&err, "logging.json.fields.bogus", "bogus = false");
    let err = error("[logging.json.names]\nbogus = \"hunter2\"");
    assert_invalid(&err, "logging.json.names.bogus", "hunter2");
}

#[test]
fn json_rename_collision_is_invalid() {
    let err = error("[logging.json.names]\nlevel = \"x\"\ntarget = \"x\"");
    let LoggingError::Config(ConfigError::Invalid { key, .. }) = &err else {
        panic!("{err}")
    };
    // Both renamed: the later field in declaration order is blamed.
    assert_eq!(key, "logging.json.names.target");
    // A rename onto another field's default name collides too, and blames the renamed key,
    // whichever of the two comes first in declaration order.
    for (toml, blamed) in [
        ("level = \"target\"", "logging.json.names.level"),
        ("thread = \"level\"", "logging.json.names.thread"),
        ("timestamp = \"thread\"", "logging.json.names.timestamp"),
    ] {
        let err = error(&format!("[logging.json.names]\n{toml}"));
        let LoggingError::Config(ConfigError::Invalid { key, .. }) = &err else {
            panic!("{err}")
        };
        assert_eq!(key, blamed, "{toml}");
        assert!(!err.to_string().contains("target\""));
    }
}

#[test]
fn json_rename_empty_is_invalid() {
    let err = error("[logging.json.names]\nspan = \"\"");
    assert_invalid(&err, "logging.json.names.span", "\"\"");
}

#[test]
fn console_format_json_parses() {
    assert_eq!(
        settings("[logging.console]\nformat = \"json\"").format(),
        ConsoleFormat::Json
    );
}

#[test]
fn key_spellings_match() {
    let (.., history, _) = file_of("[logging.file]\nenabled = true\nmax_history = 4");
    assert_eq!(history, Some(4));
    let (.., history, _) = file_of("[logging.file]\nenabled = true\nMax-History = 5");
    assert_eq!(history, Some(5));
}

#[test]
fn io_error_names_path_and_has_source() {
    // Constructed by the file layer in a later task; the variant is public now.
    let err = LoggingError::Io {
        path: PathBuf::from("/no/such/dir"),
        source: std::io::Error::other("denied"),
    };
    assert!(err.to_string().contains("/no/such/dir"));
    assert!(err.source().is_some());
}

#[test]
fn wrong_typed_values_never_echo_the_value() {
    for toml in [
        "[logging.json.fields]\nmessage = \"hunter2\"",
        "[logging.json.names]\nmessage = 7",
        "[logging.json.names]\nmessage = [\"hunter2\"]",
        "[logging.file]\nmax-history = \"hunter2\"",
        "[logging.file]\nenabled = true\nmax-history = -3",
        "[logging.file]\nenabled = \"hunter2\"",
        "[logging.file]\npath = 7",
    ] {
        let err = error(toml);
        let text = format!("{err} {err:?}");
        assert!(!text.contains("hunter2"), "{toml}: {text}");
        assert!(text.contains("logging."), "{toml}: {text}");
    }
}
