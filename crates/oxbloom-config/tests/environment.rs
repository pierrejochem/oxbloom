#![cfg(feature = "di")]

use oxbloom_config::{Config, ContainerBuilderExt, Secret};
use oxbloom_di::{ContainerBuilder, Environment};

fn cfg(text: &str) -> Config {
    Config::from_str(text).unwrap()
}

#[test]
fn string_integer_bool_float_scalars() {
    let c = cfg(
        "name = \"api\"\nport = 8080\nneg = -3\nratio = 1.5\non = true\noff = false\nd = 2026-10-25\n",
    );
    assert_eq!(c.property("name").as_deref(), Some("api"));
    assert_eq!(c.property("port").as_deref(), Some("8080"));
    assert_eq!(c.property("neg").as_deref(), Some("-3"));
    assert_eq!(c.property("ratio").as_deref(), Some("1.5"));
    assert_eq!(c.property("on").as_deref(), Some("true"));
    assert_eq!(c.property("off").as_deref(), Some("false"));
    assert_eq!(c.property("d").as_deref(), Some("2026-10-25"));
}

#[test]
fn absent_key_is_none() {
    let c = cfg("[a]\nb = 1");
    assert_eq!(c.property("nope"), None);
    assert_eq!(c.property("a.nope"), None);
    assert_eq!(c.property("a.b.c"), None);
}

#[test]
fn table_and_list_are_none() {
    let c = cfg("hosts = [\"x\", \"y\"]\n[server]\nport = 1");
    assert_eq!(c.property("server"), None);
    assert_eq!(c.property("hosts"), None);
    // Indexed elements are not paths, as for `Config::get`.
    assert_eq!(c.property("hosts[0]"), None);
}

#[test]
fn relaxed_key_forms() {
    let c = cfg("[server]\nmax_conns = 7");
    for key in [
        "server.max_conns",
        "SERVER.maxConns",
        "server.max-conns",
        "Server.MAXCONNS",
    ] {
        assert_eq!(c.property(key).as_deref(), Some("7"), "{key}");
    }
}

#[test]
fn empty_key_is_none() {
    let c = cfg("a = 1");
    // `get("")` addresses the whole (table) root; as a property it is not a scalar.
    assert!(c.get::<std::collections::BTreeMap<String, u8>>("").is_ok());
    assert_eq!(c.property(""), None);
}

struct Flagged;

#[test]
fn works_with_on_property_value_in_a_build() {
    let c = cfg("[metrics]\nenabled = true\nkind = \"redis-cluster\"");
    let container = ContainerBuilder::new()
        .environment(c)
        .instance(1u8)
        .on_property_value("metrics.enabled", "true")
        .instance(2u16)
        .on_property_value("metrics.enabled", "false")
        .instance(3u32)
        .on_property_value("metrics.kind", "redis")
        .instance(Flagged)
        .on_property_value("METRICS.Enabled", "true")
        .build()
        .unwrap();
    assert!(container.get_opt::<u8>().unwrap().is_some());
    assert!(container.get_opt::<u16>().unwrap().is_none());
    assert!(container.get_opt::<u32>().unwrap().is_none());
    assert!(container.get_opt::<Flagged>().unwrap().is_some());
}

#[derive(serde::Deserialize)]
struct Creds {
    #[allow(dead_code)]
    token: Secret<String>,
}

#[test]
fn secret_value_compares_plain_and_is_not_in_report() {
    let marker = "s3cr3t-MARKER-value";
    let c = cfg(&format!("[creds]\ntoken = \"{marker}\""));
    assert!(c.get::<Creds>("creds").is_ok());
    assert_eq!(c.property("creds.token").as_deref(), Some(marker));
    let container = ContainerBuilder::new()
        .environment(c)
        .instance(1u8)
        .on_property_value("creds.token", marker)
        .instance(2u16)
        .on_property_value("creds.token", "other")
        .build()
        .unwrap();
    assert!(container.get_opt::<u8>().unwrap().is_some());
    assert!(container.get_opt::<u16>().unwrap().is_none());
    let report = container.condition_report().to_string();
    assert!(report.contains("creds.token"));
    assert!(!report.contains(marker), "{report}");
    assert!(!report.contains("other"), "{report}");
}

#[test]
fn file_origin_values_work_too() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("application.toml"),
        "[server]\nport = 80\nmode = \"base\"\n[cache]\nkind = \"memory\"",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("application-dev.toml"),
        "[server]\nmode = \"dev\"",
    )
    .unwrap();
    let c = Config::builder()
        .dir(dir.path())
        .profile("dev")
        .env_from([("APP_CACHE__KIND".to_string(), "redis".to_string())])
        .build()
        .unwrap();
    assert_eq!(c.property("server.port").as_deref(), Some("80"));
    assert_eq!(c.property("server.mode").as_deref(), Some("dev"));
    assert_eq!(c.property("cache.kind").as_deref(), Some("redis"));
    let container = ContainerBuilder::new()
        .config(c.clone())
        .environment(c)
        .instance(1u8)
        .on_property_value("server.mode", "dev")
        .build()
        .unwrap();
    assert!(container.get_opt::<u8>().unwrap().is_some());
}

#[test]
fn file_integer_and_env_or_cli_string_compare_as_the_same_text() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("application.toml"),
        "[server]\nport = 8080\n",
    )
    .unwrap();
    let from_file = Config::builder()
        .dir(dir.path())
        .env_from([])
        .build()
        .unwrap();
    let from_env = Config::builder()
        .dir(dir.path())
        .env_from([("APP_SERVER__PORT".to_string(), "8080".to_string())])
        .build()
        .unwrap();
    let from_cli = Config::builder()
        .dir(dir.path())
        .env_from([])
        .args(["--server.port=8080"])
        .build()
        .unwrap();
    // The file holds an integer, the environment and the command line a string: the same text.
    for c in [from_file, from_env, from_cli] {
        assert_eq!(c.property("server.port").as_deref(), Some("8080"));
        let container = ContainerBuilder::new()
            .environment(c)
            .instance(1u8)
            .on_property_value("server.port", "8080")
            .build()
            .unwrap();
        assert!(container.get_opt::<u8>().unwrap().is_some());
    }
}
