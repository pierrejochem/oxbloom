#![cfg(feature = "di")]

use std::error::Error;

use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_security::{HttpSecurity, SecurityBuildError, SecurityFilterChain};

#[test]
fn security_build_error_converts() {
    let err = ContainerBuilder::new()
        .try_bean(|| -> Result<SecurityFilterChain, DiError> { Ok(HttpSecurity::new().build()?) })
        .build()
        .err()
        .unwrap();
    let DiError::Creation {
        type_name, message, ..
    } = &err
    else {
        panic!("expected Creation, got {err:?}");
    };
    assert_eq!(type_name, std::any::type_name::<SecurityFilterChain>());
    assert_eq!(message, &SecurityBuildError::NoAuthorizeSection.to_string());
    let source = err
        .source()
        .and_then(|s| s.downcast_ref::<SecurityBuildError>());
    assert_eq!(source, Some(&SecurityBuildError::NoAuthorizeSection));
}
