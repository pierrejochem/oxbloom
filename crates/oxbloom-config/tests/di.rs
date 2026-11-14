#![cfg(feature = "di")]

use std::sync::Arc;

use oxbloom_config::{Config, ContainerBuilderExt};
use oxbloom_di::{ContainerBuilder, component};

#[test]
fn config_is_resolvable_as_arc() {
    let container = ContainerBuilder::new()
        .config(Config::from_str("[server]\nport = 80").unwrap())
        .build()
        .unwrap();
    let a = container.get::<Config>().unwrap();
    let b = container.get::<Config>().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
}

#[component]
struct PortReader {
    config: Arc<Config>,
}

#[derive(serde::Deserialize)]
struct Server {
    port: u16,
}

#[test]
fn component_reads_typed_setting() {
    let container = ContainerBuilder::new()
        .config(Config::from_str("[server]\nport = \"8081\"").unwrap())
        .component::<PortReader>()
        .build()
        .unwrap();
    let reader = container.get::<PortReader>().unwrap();
    assert_eq!(reader.config.get::<Server>("server").unwrap().port, 8081);
}

#[test]
fn on_property_matches_tables_and_lists() {
    struct Jwt;
    let config =
        || Config::from_str("[security.jwt]\nalgorithms = [\"HS256\"]\n[empty]\n").unwrap();
    let kept = |key: &str| {
        ContainerBuilder::new()
            .environment(config())
            .instance(Jwt)
            .on_property(key)
            .build()
            .unwrap()
            .get_opt::<Jwt>()
            .unwrap()
            .is_some()
    };
    assert!(kept("security.jwt"));
    assert!(kept("Security.JWT"));
    assert!(kept("security.jwt.algorithms"));
    assert!(kept("empty"));
    assert!(!kept("security.jwt.secret"));
    assert!(!kept("security.web"));
}
