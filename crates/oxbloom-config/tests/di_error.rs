#![cfg(feature = "di")]

use std::error::Error;

use oxbloom_config::{Config, ConfigError};
use oxbloom_di::{ContainerBuilder, DiError};

struct Pool;

#[test]
fn config_error_converts_with_downcastable_source() {
    let err = ContainerBuilder::new()
        .try_bean(|| -> Result<Pool, DiError> {
            let config = Config::from_str("")?;
            let _port: u16 = config.get("x")?;
            Ok(Pool)
        })
        .build()
        .err()
        .unwrap();
    let DiError::Creation { type_name, .. } = &err else {
        panic!("expected Creation, got {err:?}");
    };
    assert_eq!(type_name, std::any::type_name::<Pool>());
    let source = err.source().and_then(|s| s.downcast_ref::<ConfigError>());
    assert!(matches!(source, Some(ConfigError::Missing { key }) if key == "x"));
}

#[test]
fn message_is_value_free() {
    let marker = "ZZ-MARKER-VALUE-ZZ";
    let err = ContainerBuilder::new()
        .try_bean(move || -> Result<Pool, DiError> {
            let config = Config::from_str(&format!("x = \"{marker}\""))?;
            let _port: u16 = config.get("x")?;
            Ok(Pool)
        })
        .build()
        .err()
        .unwrap();
    let DiError::Creation { message, .. } = &err else {
        panic!("expected Creation, got {err:?}");
    };
    assert!(
        !message.contains(marker),
        "message leaked the value: {message}"
    );
    assert!(!err.to_string().contains(marker));
    assert!(!format!("{err:?}").contains(marker));
    assert!(
        message.contains("`x`"),
        "message should name the key: {message}"
    );
}
