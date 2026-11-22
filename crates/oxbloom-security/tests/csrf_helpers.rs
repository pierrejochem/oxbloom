//! The public surface of the CSRF settings and the token type.

use oxbloom_security::{CsrfConfig, SecurityBuildError};

const PATHS: [&str; 2] = ["/login", "/logout"];

#[test]
fn defaults() {
    let d = CsrfConfig::default();
    assert!(d.enabled);
    assert_eq!(d.field_name, "_csrf");
    assert_eq!(d.header_name, "X-CSRF-TOKEN");
    assert_eq!(d.token_endpoint, None);
    assert_eq!(d.validate(&PATHS), Ok(()));
}

#[test]
fn setters() {
    let c = CsrfConfig::default()
        .enabled(false)
        .field_name("token")
        .header_name("X-Token")
        .token_endpoint("/csrf");
    assert!(!c.enabled);
    assert_eq!(c.field_name, "token");
    assert_eq!(c.header_name, "X-Token");
    assert_eq!(c.token_endpoint.as_deref(), Some("/csrf"));
    assert_eq!(c.clone(), c);
}

#[test]
fn validate_rejects_bad_names_and_endpoints() {
    let bad = SecurityBuildError::InvalidCsrfConfig;
    for c in [
        CsrfConfig::default().field_name(""),
        CsrfConfig::default().field_name("a b"),
        CsrfConfig::default().field_name("a=b"),
        CsrfConfig::default().header_name(""),
        CsrfConfig::default().header_name("X Y"),
        CsrfConfig::default().header_name("X:Y"),
        CsrfConfig::default().token_endpoint("https://evil.example"),
        CsrfConfig::default().token_endpoint("//evil.example"),
        CsrfConfig::default().token_endpoint("csrf"),
        CsrfConfig::default().token_endpoint("/login"),
        CsrfConfig::default().token_endpoint("/logout?x=1"),
    ] {
        assert_eq!(c.validate(&PATHS), Err(bad), "{c:?}");
    }
    assert_eq!(
        CsrfConfig::default()
            .token_endpoint("/csrf")
            .validate(&PATHS),
        Ok(())
    );
    assert_eq!(
        CsrfConfig::default()
            .token_endpoint("/login?x")
            .validate(&["/other"]),
        Ok(())
    );
}

#[test]
fn build_error_texts_are_fixed() {
    assert_eq!(
        SecurityBuildError::CsrfWithoutFormLogin.to_string(),
        "csrf protection configured without form login"
    );
    assert_eq!(
        SecurityBuildError::InvalidCsrfConfig.to_string(),
        "invalid csrf field name, header name or token endpoint"
    );
}
