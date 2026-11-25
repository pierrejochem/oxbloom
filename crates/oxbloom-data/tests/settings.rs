//! `DataSettings`: the `data.*` keys, their defaults, their errors and the secrecy of the URL.

use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use oxbloom_data::{DataSettings, Driver, PROPERTIES, SourceSettings, SslMode};

const PG: &str = "postgres://u:s3cret%40pw@[::1]:5432/db?sslmode=require";

fn settings(toml: &str) -> Result<DataSettings, ConfigError> {
    DataSettings::from_config(&Config::from_str(toml).unwrap())
}

fn invalid_key(toml: &str) -> String {
    match settings(toml) {
        Err(ConfigError::Invalid { key, message, .. }) => {
            assert!(!message.contains("s3cret"), "message leaks: {message}");
            key
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn source(settings: &DataSettings, name: &str) -> SourceSettings {
    settings
        .sources
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no source {name}"))
        .clone()
}

#[test]
fn default_source_from_unnamed_keys() {
    let s = settings("[data]\nurl = \"postgres://db/app\"").unwrap();
    assert!(s.connect_on_startup);
    assert_eq!(s.sources.len(), 1);
    let default = &s.sources[0];
    assert_eq!(default.name, "default");
    assert_eq!(default.url.expose(), "postgres://db/app");
    assert_eq!(default.driver, Driver::Postgres);
}

#[test]
fn named_sources_from_the_sources_table() {
    let s = settings(
        "[data]\nurl = \"postgres://db/app\"\n\
         [data.sources.reporting]\nurl = \"mysql://db/rep\"\nmax-connections = 3\n\
         [data.sources.cache-1]\nurl = \"sqlite://file.db\"",
    )
    .unwrap();
    let names: Vec<&str> = s.sources.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["default", "cache-1", "reporting"]);
    assert_eq!(source(&s, "reporting").driver, Driver::Mysql);
    assert_eq!(source(&s, "reporting").max_connections, 3);
    assert_eq!(source(&s, "cache-1").driver, Driver::Sqlite);

    let only_named = settings("[data.sources.a]\nurl = \"postgres://db/a\"").unwrap();
    assert_eq!(only_named.sources.len(), 1);
    assert_eq!(only_named.sources[0].name, "a");
}

#[test]
fn defaults_are_applied() {
    let s = settings("[data]\nurl = \"postgres://db/app\"").unwrap();
    let d = &s.sources[0];
    assert_eq!(d.max_connections, 10);
    assert_eq!(d.min_connections, 0);
    assert_eq!(d.acquire_timeout, Duration::from_secs(30));
    assert_eq!(d.idle_timeout, None);
    assert_eq!(d.max_lifetime, None);
    assert!(d.test_before_acquire);
    assert_eq!(d.ssl_mode, None);
    assert_eq!(d.ssl_bundle, None);
}

#[test]
fn every_duration_and_bool_key_is_read() {
    let s = settings(
        "[data]\nurl = \"postgres://db/app\"\nconnect-on-startup = false\n\
         max-connections = 20\nmin-connections = 2\nacquire-timeout = \"5s\"\n\
         idle-timeout = \"10m\"\nmax-lifetime = \"1h\"\ntest-before-acquire = false\n\
         ssl-mode = \"verify-full\"\nssl-bundle = \"db\"",
    )
    .unwrap();
    assert!(!s.connect_on_startup);
    let d = &s.sources[0];
    assert_eq!(d.max_connections, 20);
    assert_eq!(d.min_connections, 2);
    assert_eq!(d.acquire_timeout, Duration::from_secs(5));
    assert_eq!(d.idle_timeout, Some(Duration::from_secs(600)));
    assert_eq!(d.max_lifetime, Some(Duration::from_secs(3600)));
    assert!(!d.test_before_acquire);
    assert_eq!(d.ssl_mode, Some(SslMode::VerifyFull));
    assert_eq!(d.ssl_bundle.as_deref(), Some("db"));
}

#[test]
fn driver_comes_from_the_scheme() {
    let cases = [
        ("postgres://h/d", Driver::Postgres, "postgres"),
        ("postgresql://h/d", Driver::Postgres, "postgres"),
        ("mysql://h/d", Driver::Mysql, "mysql"),
        ("mariadb://h/d", Driver::Mysql, "mysql"),
        ("sqlite:app.db", Driver::Sqlite, "sqlite"),
    ];
    for (url, driver, shown) in cases {
        let s = settings(&format!("[data]\nurl = \"{url}\"")).unwrap();
        assert_eq!(s.sources[0].driver, driver, "{url}");
        assert_eq!(driver.to_string(), shown);
    }
}

#[test]
fn an_unknown_scheme_names_the_url_key_and_not_the_value() {
    let err = settings("[data]\nurl = \"oracle://u:s3cret@host/db\"").unwrap_err();
    let text = err.to_string();
    assert!(text.contains("data.url"), "{text}");
    for part in ["oracle", "s3cret", "host"] {
        assert!(!text.contains(part), "{text}");
    }
    assert_eq!(
        invalid_key("[data.sources.x]\nurl = \"nope\""),
        "data.sources.x.url"
    );
}

#[test]
fn min_above_max_is_refused() {
    assert_eq!(
        invalid_key("[data]\nurl = \"postgres://h/d\"\nmax-connections = 2\nmin-connections = 3"),
        "data.min-connections"
    );
    assert_eq!(
        invalid_key(
            "[data.sources.a]\nurl = \"postgres://h/d\"\nmax-connections = 2\nmin-connections = 3"
        ),
        "data.sources.a.min-connections"
    );
    // The default maximum is 10.
    assert_eq!(
        invalid_key("[data]\nurl = \"postgres://h/d\"\nmin-connections = 11"),
        "data.min-connections"
    );
}

#[test]
fn zero_max_connections_and_zero_durations_are_refused() {
    let base = "[data]\nurl = \"postgres://h/d\"\n";
    assert_eq!(
        invalid_key(&format!("{base}max-connections = 0")),
        "data.max-connections"
    );
    for key in ["acquire-timeout", "idle-timeout", "max-lifetime"] {
        assert_eq!(
            invalid_key(&format!("{base}{key} = \"0s\"")),
            format!("data.{key}")
        );
    }
}

#[test]
fn ssl_on_a_sqlite_source_is_refused_with_the_key() {
    assert_eq!(
        invalid_key("[data]\nurl = \"sqlite:app.db\"\nssl-mode = \"require\""),
        "data.ssl-mode"
    );
    assert_eq!(
        invalid_key("[data.sources.l]\nurl = \"sqlite:app.db\"\nssl-bundle = \"b\""),
        "data.sources.l.ssl-bundle"
    );
    assert_eq!(
        invalid_key("[data]\nurl = \"postgres://h/d\"\nssl-mode = \"sometimes\""),
        "data.ssl-mode"
    );
}

#[test]
fn an_ssl_bundle_with_ssl_mode_disable_is_refused() {
    assert_eq!(
        invalid_key("[data]\nurl = \"postgres://h/d\"\nssl-mode = \"disable\"\nssl-bundle = \"b\""),
        "data.ssl-bundle"
    );
    assert_eq!(
        invalid_key(
            "[data.sources.rep]\nurl = \"mysql://h/d\"\nssl-mode = \"disable\"\nssl-bundle = \"b\""
        ),
        "data.sources.rep.ssl-bundle"
    );
    match settings(
        "[data]\nurl = \"postgres://h/d\"\nssl-mode = \"disable\"\nssl-bundle = \"s3cret\"",
    ) {
        Err(ConfigError::Invalid { message, .. }) => {
            assert!(message.contains("no effect"), "{message}");
            assert!(!message.contains("s3cret"), "{message}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    // Every other mode is allowed with a bundle.
    for mode in ["prefer", "require", "verify-ca", "verify-full"] {
        let toml =
            format!("[data]\nurl = \"postgres://h/d\"\nssl-mode = \"{mode}\"\nssl-bundle = \"b\"");
        assert!(settings(&toml).is_ok(), "{mode}");
    }
}

#[test]
fn a_bad_source_name_and_the_name_default_are_refused() {
    for name in [
        "default",
        "Has_Underscore",
        "Reporting",
        "with.dot",
        "with space",
        "",
    ] {
        let toml = format!("[data.sources.\"{name}\"]\nurl = \"postgres://h/d\"");
        assert_eq!(
            invalid_key(&toml),
            format!("data.sources.{name}"),
            "{name:?}"
        );
    }
}

#[test]
fn schemes_are_case_sensitive_and_network_schemes_need_slashes() {
    for url in [
        "SQLITE::memory:",
        "Sqlite:app.db",
        "POSTGRES://h/db",
        "MySQL://h/db",
        "postgres:db",
        "postgresql:db",
        "mysql:x",
        "mariadb:x",
    ] {
        let toml = format!("[data]\nurl = \"{url}\"");
        let err = settings(&toml).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { .. }), "{url}: {err:?}");
        let text = format!("{err} {err:?}");
        assert!(text.contains("data.url"), "{url}: {text}");
        // The fixed text names the schemes only, never a part of the value.
        for part in ["app.db", "memory", "h/db"] {
            assert!(!text.contains(part), "{url}: {text}");
        }
        assert_eq!(
            invalid_key(&format!("[data.sources.x]\nurl = \"{url}\"")),
            "data.sources.x.url"
        );
    }
}

#[test]
fn an_empty_ssl_bundle_is_refused() {
    for value in ["", "  "] {
        assert_eq!(
            invalid_key(&format!(
                "[data]\nurl = \"postgres://h/d\"\nssl-bundle = \"{value}\""
            )),
            "data.ssl-bundle"
        );
    }
}

#[test]
fn the_config_prefix_names_the_source_key() {
    let s =
        settings("[data]\nurl = \"postgres://h/d\"\n[data.sources.rep-1]\nurl = \"mysql://h/d\"")
            .unwrap();
    assert_eq!(source(&s, "default").config_prefix(), "data");
    assert_eq!(source(&s, "rep-1").config_prefix(), "data.sources.rep-1");
}

#[test]
fn min_above_the_implicit_in_memory_maximum_names_min_connections() {
    let key = invalid_key("[data]\nurl = \"sqlite::memory:\"\nmin-connections = 2");
    assert_eq!(key, "data.min-connections");
}

#[test]
fn a_missing_url_is_refused() {
    assert_eq!(invalid_key("[data]\nmax-connections = 3"), "data.url");
    assert_eq!(
        invalid_key("[data.sources.a]\nmax-connections = 3"),
        "data.sources.a.url"
    );
}

#[test]
fn an_unknown_key_is_refused_naming_it() {
    for toml in [
        "[data]\nurl = \"postgres://h/d\"\nmax-conections = 3",
        "[data.sources.a]\nurl = \"postgres://h/d\"\nmax-conections = 3",
    ] {
        let text = settings(toml).unwrap_err().to_string();
        assert!(text.contains("max-conections"), "{text}");
    }
}

#[test]
fn in_memory_sqlite_defaults_to_one_connection() {
    for url in [
        "sqlite::memory:",
        "sqlite://:memory:",
        "sqlite://file.db?mode=memory",
    ] {
        let s = settings(&format!("[data]\nurl = \"{url}\"")).unwrap();
        assert_eq!(s.sources[0].max_connections, 1, "{url}");
    }
    let file = settings("[data]\nurl = \"sqlite:app.db\"").unwrap();
    assert_eq!(file.sources[0].max_connections, 10);
}

#[test]
fn in_memory_sqlite_with_several_connections_is_refused_unless_shared_cache() {
    assert_eq!(
        invalid_key("[data]\nurl = \"sqlite::memory:\"\nmax-connections = 4"),
        "data.max-connections"
    );
    assert_eq!(
        invalid_key("[data.sources.m]\nurl = \"sqlite://x?mode=memory\"\nmax-connections = 2"),
        "data.sources.m.max-connections"
    );
    let shared =
        settings("[data]\nurl = \"sqlite://x?mode=memory&cache=shared\"\nmax-connections = 4")
            .unwrap();
    assert_eq!(shared.sources[0].max_connections, 4);
    let one = settings("[data]\nurl = \"sqlite::memory:\"\nmax-connections = 1").unwrap();
    assert_eq!(one.sources[0].max_connections, 1);
}

#[test]
fn the_url_never_appears_in_debug_or_errors() {
    let secrets = ["s3cret", "%40", "::1", "5432", "u:", "sslmode"];
    let ok = settings(&format!("[data]\nurl = \"{PG}\"")).unwrap();
    assert_eq!(ok.sources[0].url.expose(), PG);
    let debug = format!("{ok:?} {:?}", ok.sources[0]);
    assert!(debug.contains("REDACTED"), "{debug}");
    for part in secrets {
        assert!(!debug.contains(part), "{debug}");
    }

    let cases = [
        format!("[data]\nurl = \"{PG}\"\nmax-connections = 0"),
        format!("[data]\nurl = \"{PG}\"\nmin-connections = 99"),
        format!("[data]\nurl = \"{PG}\"\nacquire-timeout = \"0s\""),
        format!("[data]\nurl = \"{PG}\"\nssl-mode = \"bad\""),
        format!("[data]\nurl = \"{PG}\"\nbogus = 1"),
        format!("[data]\nurl = \"{PG}\"\nmax-connections = \"many\""),
        format!("[data.sources.Bad]\nurl = \"{PG}\""),
        format!("[data.sources.default]\nurl = \"{PG}\""),
        "[data]\nurl = \"mongodb://u:s3cret%40pw@[::1]:5432/db?sslmode=require\"".to_string(),
        "[data]\nurl = \"sqlite://u:s3cret%40pw@[::1]:5432/db\"\nssl-mode = \"require\""
            .to_string(),
        "[data]\nurl = 42".to_string(),
        "[data]\nurl = { v = \"postgres://u:s3cret%40pw@[::1]:5432/db\" }".to_string(),
        format!("[data.sources.a]\nurl = \"{PG}\"\nbogus = 1"),
        format!("[data.sources.a]\nurl = \"{PG}\"\nidle-timeout = \"0s\""),
        "[data]\nurl = \"sqlite://u:s3cret%40pw@[::1]:5432/db\"\nssl-bundle = \"b\"".to_string(),
        "[data]\nurl = \"sqlite://u:s3cret%40pw@[::1]:5432/db?mode=memory\"\nmax-connections = 3"
            .to_string(),
    ];
    for toml in cases {
        let text = match settings(&toml) {
            Err(e) => format!("{e} {e:?}"),
            Ok(_) => panic!("expected an error for {toml}"),
        };
        for part in secrets {
            assert!(!text.contains(part), "{toml}: {text}");
        }
    }
}

#[test]
fn percent_encoded_passwords_and_ipv6_hosts_parse() {
    let s = settings(&format!("[data]\nurl = \"{PG}\"")).unwrap();
    assert_eq!(s.sources[0].driver, Driver::Postgres);
    assert_eq!(s.sources[0].url.expose(), PG);
    let s = settings("[data]\nurl = \"mysql://u:p%3Ass@[2001:db8::1]:3306/db\"").unwrap();
    assert_eq!(s.sources[0].driver, Driver::Mysql);
}

#[test]
fn no_data_section_gives_no_sources() {
    let s = settings("[other]\nkey = 1").unwrap();
    assert!(s.connect_on_startup);
    assert!(s.sources.is_empty());
    let off = settings("[data]\nconnect-on-startup = false").unwrap();
    assert!(!off.connect_on_startup);
    assert!(off.sources.is_empty());
}

#[test]
fn properties_list_every_key() {
    let keys: Vec<&str> = PROPERTIES.iter().map(|p| p.0).collect();
    for expected in [
        "data.connect-on-startup",
        "data.url",
        "data.max-connections",
        "data.min-connections",
        "data.acquire-timeout",
        "data.idle-timeout",
        "data.max-lifetime",
        "data.test-before-acquire",
        "data.ssl-mode",
        "data.ssl-bundle",
        "data.sources.<name>.url",
        "data.sources.<name>.max-connections",
        "data.sources.<name>.min-connections",
        "data.sources.<name>.acquire-timeout",
        "data.sources.<name>.idle-timeout",
        "data.sources.<name>.max-lifetime",
        "data.sources.<name>.test-before-acquire",
        "data.sources.<name>.ssl-mode",
        "data.sources.<name>.ssl-bundle",
    ] {
        assert!(keys.contains(&expected), "missing {expected}");
    }
    assert_eq!(keys.len(), 31);
    let max = PROPERTIES
        .iter()
        .find(|p| p.0 == "data.max-connections")
        .unwrap();
    assert_eq!((max.1, max.2), ("u32", Some("10")));
}

#[test]
fn driver_from_url_follows_the_scheme_rule_of_the_settings() {
    assert_eq!(Driver::from_url("postgres://h/db"), Some(Driver::Postgres));
    assert_eq!(
        Driver::from_url("postgresql://h/db"),
        Some(Driver::Postgres)
    );
    assert_eq!(Driver::from_url("mysql://h/db"), Some(Driver::Mysql));
    assert_eq!(Driver::from_url("mariadb://h/db"), Some(Driver::Mysql));
    assert_eq!(Driver::from_url("sqlite::memory:"), Some(Driver::Sqlite));
    assert_eq!(Driver::from_url("sqlite:app.db"), Some(Driver::Sqlite));
    // The network schemes need `://`, and every scheme is matched case-sensitively.
    assert_eq!(Driver::from_url("postgres:h"), None);
    assert_eq!(Driver::from_url("POSTGRES://h/db"), None);
    assert_eq!(Driver::from_url("SQLITE::memory:"), None);
    assert_eq!(Driver::from_url("redis://h"), None);
    assert_eq!(Driver::from_url("no-scheme"), None);
}

// Migration settings.

const SQLITE: &str = "[data]\nurl = \"sqlite::memory:\"\n";

#[test]
fn migrations_are_off_by_default() {
    let s = settings(SQLITE).unwrap();
    assert!(s.sources[0].migrations.is_none());
}

#[test]
fn enabled_with_defaults() {
    let s = settings(&format!("{SQLITE}[data.migrations]\nenabled = true")).unwrap();
    let m = s.sources[0].migrations.as_ref().unwrap();
    assert_eq!(m.table, "_sqlx_migrations");
    assert!(!m.ignore_missing);
    assert_eq!(m.timeout, Duration::from_secs(300));
    assert!(m.locking);
    assert!(m.path.is_none());
}

#[test]
fn every_key_is_read_for_the_default_and_a_named_source() {
    let body = "enabled = true\npath = \"db/migrations\"\ntable = \"history_1\"\n\
                ignore-missing = true\ntimeout = \"45s\"\nlocking = false\n";
    let s = settings(&format!(
        "{SQLITE}[data.migrations]\n{body}\
         [data.sources.rep]\nurl = \"sqlite::memory:\"\n[data.sources.rep.migrations]\n{body}"
    ))
    .unwrap();
    for name in ["default", "rep"] {
        let src = source(&s, name);
        let m = src.migrations.as_ref().unwrap();
        assert_eq!(
            m.path.as_deref(),
            Some(std::path::Path::new("db/migrations"))
        );
        assert_eq!(m.table, "history_1");
        assert!(m.ignore_missing);
        assert_eq!(m.timeout, Duration::from_secs(45));
        assert!(!m.locking);
    }
}

#[test]
fn other_keys_are_validated_but_dropped_when_not_enabled() {
    let s = settings(&format!(
        "{SQLITE}[data.migrations]\nenabled = false\ntable = \"ok_name\""
    ))
    .unwrap();
    assert!(s.sources[0].migrations.is_none());
    let key = invalid_key(&format!(
        "{SQLITE}[data.migrations]\nenabled = false\ntable = \"a b\""
    ));
    assert_eq!(key, "data.migrations.table");
}

#[test]
fn a_bad_table_name_names_the_key_and_no_value() {
    let long = "a".repeat(64);
    for bad in [
        "x; drop table users",
        "a b",
        "\\\"quoted\\\"",
        "1abc",
        "",
        &long,
    ] {
        let toml = format!("{SQLITE}[data.migrations]\nenabled = true\ntable = \"{bad}\"");
        match settings(&toml) {
            Err(ConfigError::Invalid { key, message, .. }) => {
                assert_eq!(key, "data.migrations.table");
                assert!(!message.contains("drop"), "message leaks: {message}");
                assert!(!message.contains("quoted"), "message leaks: {message}");
                assert!(!message.contains("1abc"), "message leaks: {message}");
                assert!(!message.contains("aaaa"), "message leaks: {message}");
                assert!(!message.contains("a b"), "message leaks: {message}");
            }
            other => panic!("expected Invalid for {bad:?}, got {other:?}"),
        }
    }
}

#[test]
fn a_63_character_table_name_is_accepted() {
    let name = format!("_{}", "Z9".repeat(31));
    assert_eq!(name.len(), 63);
    let s = settings(&format!(
        "{SQLITE}[data.migrations]\nenabled = true\ntable = \"{name}\""
    ))
    .unwrap();
    assert_eq!(s.sources[0].migrations.as_ref().unwrap().table, name);
}

#[test]
fn an_unknown_migrations_key_fails_with_the_full_key() {
    let err = settings(&format!(
        "{SQLITE}[data.migrations]\nenabled = true\nvalidate-checksums = false"
    ))
    .unwrap_err();
    assert!(err.to_string().contains("validate-checksums"), "{err}");
    assert!(err.to_string().contains("data.migrations"), "{err}");
}

#[test]
fn a_zero_timeout_is_rejected_naming_the_key() {
    let key = invalid_key(&format!(
        "{SQLITE}[data.migrations]\nenabled = true\ntimeout = \"0s\""
    ));
    assert_eq!(key, "data.migrations.timeout");
}

#[test]
fn the_property_metadata_lists_every_migrations_key() {
    for key in [
        "enabled",
        "path",
        "table",
        "ignore-missing",
        "timeout",
        "locking",
    ] {
        for prefix in ["data.migrations", "data.sources.<name>.migrations"] {
            let full = format!("{prefix}.{key}");
            assert!(PROPERTIES.iter().any(|p| p.0 == full), "missing {full}");
        }
    }
    assert!(
        !PROPERTIES
            .iter()
            .any(|p| p.0.contains("validate-checksums"))
    );
}

#[test]
fn a_named_source_prefix_is_in_the_error() {
    let key = invalid_key(
        "[data.sources.rep]\nurl = \"sqlite::memory:\"\n\
         [data.sources.rep.migrations]\nenabled = true\ntable = \"a b\"",
    );
    assert_eq!(key, "data.sources.rep.migrations.table");
}
