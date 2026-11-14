//! Value hygiene: a marker passed as a configuration value never shows up in any error,
//! warning, `Debug`, `Display` or `explain` output. Only the public API is used.

use std::fmt::{Debug, Display};
use std::fs;
use std::path::Path;
use std::time::Duration;

use oxbloom_config::{Config, ConfigBuilder, ConfigError, DataSize};
use serde::Deserialize;

const MARKER: &str = "MARKER_8f3a91";

fn clean(label: &str, text: &str) {
    assert!(!text.contains(MARKER), "value leaked in {label}: {text}");
}

fn clean_error(label: &str, err: &ConfigError) {
    clean(&format!("{label} (Display)"), &err.to_string());
    clean(&format!("{label} (Debug)"), &format!("{err:?}"));
    let alt = format!("{err:#?}");
    clean(&format!("{label} (Debug alt)"), &alt);
}

fn clean_both<T: Debug + Display>(label: &str, v: &T) {
    clean(&format!("{label} (Display)"), &v.to_string());
    clean(&format!("{label} (Debug)"), &format!("{v:?}"));
}

fn clean_config(label: &str, config: &Config) {
    clean(&format!("{label} Config Debug"), &format!("{config:?}"));
    clean(
        &format!("{label} Config Debug alt"),
        &format!("{config:#?}"),
    );
    for w in config.warnings() {
        clean_both(&format!("{label} warning"), w);
    }
    for e in config.explain() {
        clean(&format!("{label} Entry Debug"), &format!("{e:?}"));
        clean(&format!("{label} Origin Debug"), &format!("{:?}", e.origin));
        clean(&format!("{label} Origin Display"), &e.origin.to_string());
        assert_eq!(config.origin(&e.key), Some(&e.origin));
    }
    for k in config.keys("") {
        clean(&format!("{label} keys"), &k);
    }
}

fn dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    for (name, text) in files {
        fs::write(d.path().join(name), text).unwrap();
    }
    d
}

fn builder(d: &Path) -> ConfigBuilder {
    Config::builder().dir(d).env_from([])
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Builds with `b`, which must fail, and checks the error.
fn fails(label: &str, b: ConfigBuilder) -> ConfigError {
    let err = b.build().expect_err(label);
    clean_error(label, &err);
    err
}

#[test]
fn marker_value_never_appears_in_any_error_warning_origin_or_explain() {
    // The marker travels through every source type.
    let d = dir(&[
        (
            "application.toml",
            &format!(
                "oxbloom.config.import = [\"extra.toml\"]\nbase = \"{MARKER}\"\nph = \"${{undefined.key:{MARKER}}}\"\n"
            ),
        ),
        ("extra.toml", &format!("imported = \"{MARKER}\"\n")),
        (
            "application-dev.toml",
            &format!("profiled = \"{MARKER}\"\n"),
        ),
    ]);
    let config = builder(d.path())
        .profile("dev")
        .env_from(env(&[("APP_FROM_ENV", MARKER)]))
        .args([format!("--from.cli={MARKER}")])
        .build()
        .unwrap();
    for key in ["base", "imported", "profiled", "from-env", "from.cli", "ph"] {
        assert_eq!(config.get::<String>(key).unwrap(), MARKER, "{key}");
    }
    clean_config("success", &config);
    assert_eq!(config.explain().len(), 6);

    // wrong type on bind
    let d = dir(&[("application.toml", &format!("port = \"{MARKER}\"\n"))]);
    let config = builder(d.path()).build().unwrap();
    let err = config.get::<u16>("port").unwrap_err();
    clean_error("wrong type", &err);
    let err = config.get::<bool>("port").unwrap_err();
    clean_error("wrong type bool", &err);
    let err = config.get::<Vec<u8>>("port").unwrap_err();
    clean_error("wrong type list", &err);
    clean_config("wrong type", &config);

    // wrong type / unparsable number from the environment and the command line
    let config = builder(d.path())
        .env_from(env(&[("APP_PORT", MARKER)]))
        .build()
        .unwrap();
    clean_error("env not a number", &config.get::<u16>("port").unwrap_err());
    let config = builder(d.path())
        .args([format!("--port={MARKER}")])
        .build()
        .unwrap();
    clean_error("cli not a number", &config.get::<u16>("port").unwrap_err());

    // out-of-range number
    let d = dir(&[(
        "application.toml",
        &format!("port = 70000\nneighbour = \"{MARKER}\"\n"),
    )]);
    let config = builder(d.path()).build().unwrap();
    clean_error("out of range", &config.get::<u16>("port").unwrap_err());
    clean_error("negative", &config.get::<u8>("port").unwrap_err());

    // unknown field with deny_unknown_fields
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Strict {
        known: u8,
    }
    let d = dir(&[(
        "application.toml",
        &format!("[s]\nknown = 1\nextra = \"{MARKER}\"\n"),
    )]);
    let config = builder(d.path()).build().unwrap();
    clean_error("unknown field", &config.get::<Strict>("s").unwrap_err());

    // duplicate key spellings
    let d = dir(&[(
        "application.toml",
        &format!("max-conn = \"{MARKER}\"\nmax_conn = \"{MARKER}\"\n"),
    )]);
    let err = fails("duplicate key", builder(d.path()));
    assert!(matches!(err, ConfigError::DuplicateKey { .. }));

    // unresolved placeholder
    let d = dir(&[(
        "application.toml",
        &format!("a = \"{MARKER} ${{NOPE_X}} {MARKER}\"\n"),
    )]);
    let err = fails("unresolved placeholder", builder(d.path()));
    assert!(matches!(err, ConfigError::UnresolvedPlaceholder { .. }));

    // placeholder cycle
    let d = dir(&[(
        "application.toml",
        &format!("a = \"{MARKER} ${{b}}\"\nb = \"{MARKER} ${{a}}\"\n"),
    )]);
    let err = fails("placeholder cycle", builder(d.path()));
    assert!(matches!(err, ConfigError::PlaceholderCycle { .. }));

    // placeholder too large
    let big = MARKER.repeat(20_000);
    let d = dir(&[(
        "application.toml",
        &format!("a = \"{big}\"\nb = \"${{a}}${{a}}${{a}}${{a}}${{a}}\"\n"),
    )]);
    let err = fails("placeholder too large", builder(d.path()));
    assert!(matches!(err, ConfigError::PlaceholderTooLarge { .. }));

    // bad imports: missing, invalid path, cycle
    let d = dir(&[(
        "application.toml",
        &format!("oxbloom.config.import = [\"gone.toml\"]\nv = \"{MARKER}\"\n"),
    )]);
    let err = fails("import missing", builder(d.path()));
    assert!(matches!(err, ConfigError::ImportMissing { .. }));
    let d = dir(&[(
        "application.toml",
        &format!("oxbloom.config.import = [\"../x.toml\"]\nv = \"{MARKER}\"\n"),
    )]);
    let err = fails("import invalid", builder(d.path()));
    assert!(matches!(err, ConfigError::ImportInvalid { .. }));
    let d = dir(&[
        (
            "application.toml",
            &format!("oxbloom.config.import = [\"a.toml\"]\nv = \"{MARKER}\"\n"),
        ),
        (
            "a.toml",
            &format!("oxbloom.config.import = [\"a.toml\"]\nw = \"{MARKER}\"\n"),
        ),
    ]);
    let err = fails("import cycle", builder(d.path()));
    assert!(matches!(err, ConfigError::ImportCycle { .. }));

    // deprecated key
    let d = dir(&[("application.toml", &format!("[old]\nkey = \"{MARKER}\"\n"))]);
    let err = fails(
        "deprecated",
        builder(d.path())
            .renamed("old.key", "new.key")
            .deny_deprecated(true),
    );
    assert!(matches!(err, ConfigError::Deprecated { .. }));
    let config = builder(d.path())
        .renamed("old.key", "new.key")
        .build()
        .unwrap();
    assert_eq!(config.warnings().len(), 1);
    clean_config("renamed", &config);
    let config = builder(d.path())
        .removed("old.key", "gone")
        .build()
        .unwrap();
    assert_eq!(config.warnings().len(), 1);
    clean_config("removed", &config);

    // bad duration / data size
    let d = dir(&[(
        "application.toml",
        &format!(
            "t = \"{MARKER}s\"\nu = \"5 {MARKER}\"\nsz = \"{MARKER}\"\nsz2 = \"1 {MARKER}\"\n"
        ),
    )]);
    let config = builder(d.path()).build().unwrap();
    clean_error("bad duration", &config.get::<Duration>("t").unwrap_err());
    clean_error(
        "bad duration unit",
        &config.get::<Duration>("u").unwrap_err(),
    );
    clean_error("bad data size", &config.get::<DataSize>("sz").unwrap_err());
    clean_error(
        "bad data size unit",
        &config.get::<DataSize>("sz2").unwrap_err(),
    );

    // bad list index (gap)
    let err = fails(
        "list gap",
        builder(d.path()).env_from(env(&[("APP_L__0", MARKER), ("APP_L__2", MARKER)])),
    );
    assert!(matches!(err, ConfigError::Invalid { .. }));
    let err = fails(
        "cli list gap",
        builder(d.path()).args([format!("--l[0]={MARKER}"), format!("--l[3]={MARKER}")]),
    );
    assert!(matches!(err, ConfigError::Invalid { .. }));

    // command-line secret key rejection
    let err = fails(
        "cli secret",
        builder(d.path())
            .secret_key("security.token")
            .args([format!("--security.token={MARKER}")]),
    );
    assert!(matches!(err, ConfigError::Invalid { .. }));

    // a rename onto a secret key from the command line
    let err = fails(
        "rename onto secret",
        builder(d.path())
            .secret_key("security.token")
            .renamed("old.tok", "security.token")
            .args([format!("--old.tok={MARKER}")]),
    );
    assert!(matches!(err, ConfigError::Invalid { .. }));

    // parse error in a file whose content contains the marker
    let d = dir(&[(
        "application.toml",
        &format!("a = \"{MARKER}\nb = = {MARKER}\n"),
    )]);
    let err = fails("parse error", builder(d.path()));
    assert!(matches!(err, ConfigError::Parse { .. }));
    let d = dir(&[
        ("application.toml", "a = 1\n"),
        ("application-dev.toml", &format!("x = [ {MARKER} \n")),
    ]);
    fails("profile parse error", builder(d.path()).profile("dev"));
    let err = Config::from_str(&format!("a = {MARKER}")).unwrap_err();
    clean_error("from_str parse error", &err);

    // a missing base file is not attributed to application.toml
    let d = dir(&[]);
    let config = builder(d.path())
        .env_from(env(&[("APP_A", "1")]))
        .args(["--b=2"])
        .build()
        .unwrap();
    let entries = config.explain();
    assert_eq!(entries.len(), 2);
    for e in &entries {
        assert!(
            !e.origin.to_string().contains("application.toml"),
            "{:?}",
            e.origin
        );
    }
    let empty = builder(d.path()).build().unwrap();
    assert!(empty.explain().is_empty());
}

#[test]
fn marker_inside_placeholder_syntax_in_env_and_cli_values_is_literal() {
    let env_value = format!("ab${{{MARKER}}}cd");
    let cli_value = format!("x${{{MARKER}:{MARKER}}}y");
    let d = dir(&[("application.toml", "[db]\nhost = \"h\"\n")]);
    let config = Config::builder()
        .dir(d.path())
        .env_from(env(&[("APP_DB__PASSWORD", env_value.as_str())]))
        .args([format!("--db.token={cli_value}")])
        .build()
        .unwrap();
    assert_eq!(config.get::<String>("db.password").unwrap(), env_value);
    assert_eq!(config.get::<String>("db.token").unwrap(), cli_value);
    clean_config("placeholder syntax in env/cli", &config);
    // Binding errors for these leaves name the key only.
    let err = config.get::<u16>("db.password").unwrap_err();
    clean_error("env placeholder syntax bind", &err);
    let err = config.get::<u16>("db.token").unwrap_err();
    clean_error("cli placeholder syntax bind", &err);
}
