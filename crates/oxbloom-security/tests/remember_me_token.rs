//! The public surface of remember-me settings: the marker on `Authentication`, the
//! `fully_authenticated()` rule, the build rejections of `HttpSecurity::remember_me` and the
//! redacting `Debug` of `RememberMe`. The token codec is `pub(crate)` and is tested in
//! `src/remember_me/token.rs`.

use std::sync::Arc;
use std::time::Duration;

use oxbloom_config::Secret;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthorizationRule, DaoAuthenticationProvider,
    GrantedAuthority, HttpSecurity, InMemoryUserDetailsService, RememberMe, SecurityBuildError,
    UserDetailsService,
};
use oxbloom_web::{HttpMethod, Request};

const SECRET: &str = "a-remember-me-secret-of-32-bytes";
const SHORT: &str = "a-remember-me-secret-of-31-byte";

fn users() -> Arc<dyn UserDetailsService> {
    Arc::new(InMemoryUserDetailsService::new(vec![]).unwrap())
}

fn secret(s: &str) -> Secret<String> {
    Secret::new(s.to_owned())
}

/// A chain with form login and remember-me shaped by `f`, built.
fn build(f: impl FnOnce(RememberMe) -> RememberMe) -> Result<(), SecurityBuildError> {
    HttpSecurity::new()
        .password_provider(Arc::new(DaoAuthenticationProvider::new(
            users(),
            Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap()),
        )))
        .form_login(|f| f)
        .remember_me(f)
        .authorize(|a| a.any_request().authenticated())
        .build()
        .map(|_| ())
}

/// One way to shape the settings, for the table of rejections.
type Shape = Box<dyn FnOnce(RememberMe) -> RememberMe>;

fn valid(r: RememberMe) -> RememberMe {
    r.secret(secret(SECRET)).user_details(users())
}

#[test]
fn the_marker_round_trips_and_a_plain_authentication_is_not_remembered() {
    let plain = Authentication::new("alice", [GrantedAuthority::new("ROLE_USER")]);
    assert!(!plain.is_remembered());
    let remembered = plain.clone().remembered();
    assert!(remembered.is_remembered());
    assert_eq!(remembered.principal(), "alice");
    assert!(remembered.has_role("USER"));
    // The marker survives serialization, as in the stored session value.
    let json = serde_json::to_value(&remembered).unwrap();
    let back: Authentication = serde_json::from_value(json).unwrap();
    assert!(back.is_remembered());
    // A details entry that is not the boolean true is no marker.
    let mut details = serde_json::Map::new();
    details.insert("oxbloom.remembered".into(), serde_json::json!("true"));
    assert!(
        !Authentication::new("a", [])
            .with_details(details)
            .is_remembered()
    );
}

#[test]
fn fully_authenticated_grants_only_an_authentication_that_is_not_remembered() {
    let req = Request::new(HttpMethod::Get, "/");
    let plain = Authentication::new("alice", []);
    let remembered = plain.clone().remembered();
    let rule = AuthorizationRule::FullyAuthenticated;
    assert!(!rule.grants(None, &req));
    assert!(rule.grants(Some(&plain), &req));
    assert!(!rule.grants(Some(&remembered), &req));
    // `authenticated()` still accepts a remembered user.
    assert!(AuthorizationRule::Authenticated.grants(Some(&remembered), &req));
    assert_eq!(format!("{rule:?}"), "FullyAuthenticated");
}

#[test]
fn the_rule_builder_offers_fully_authenticated() {
    let chain = HttpSecurity::new()
        .authorize(|a| {
            a.path("/settings/**")
                .fully_authenticated()
                .any_request()
                .authenticated()
        })
        .build();
    assert!(chain.is_ok());
}

#[test]
fn a_valid_remember_me_builds() {
    assert_eq!(build(valid), Ok(()));
    assert_eq!(
        build(|r| valid(r)
            .old_secrets(vec![secret("an-older-remember-me-secret-32-b!")])
            .lifetime(Duration::from_secs(60))
            .field_name("stay_signed-in")
            .cookie_name("REMEMBER")
            .clock(Arc::new(SystemClock))),
        Ok(())
    );
    assert_eq!(
        build(|r| valid(r).lifetime(Duration::from_secs(365 * 24 * 3600))),
        Ok(())
    );
}

#[test]
fn remember_me_without_form_login_is_rejected() {
    let err = HttpSecurity::new()
        .remember_me(valid)
        .authorize(|a| a.any_request().authenticated())
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::RememberMeWithoutFormLogin));
    assert_eq!(
        SecurityBuildError::RememberMeWithoutFormLogin.to_string(),
        "remember-me configured without form login"
    );
}

#[test]
fn every_bad_setting_is_rejected_with_a_fixed_text() {
    let cases: Vec<(&str, Shape)> = vec![
        ("no secret", Box::new(|r| r.user_details(users()))),
        ("short secret", Box::new(|r| valid(r).secret(secret(SHORT)))),
        (
            "short old secret",
            Box::new(|r| valid(r).old_secrets(vec![secret(SECRET), secret(SHORT)])),
        ),
        ("no user service", Box::new(|r| r.secret(secret(SECRET)))),
        (
            "59 s",
            Box::new(|r| valid(r).lifetime(Duration::from_secs(59))),
        ),
        (
            "366 days",
            Box::new(|r| valid(r).lifetime(Duration::from_secs(366 * 24 * 3600))),
        ),
        ("empty cookie", Box::new(|r| valid(r).cookie_name(""))),
        ("bad cookie", Box::new(|r| valid(r).cookie_name("a;b"))),
        ("cookie space", Box::new(|r| valid(r).cookie_name("a b"))),
        ("empty field", Box::new(|r| valid(r).field_name(""))),
        (
            "bad field",
            Box::new(|r| valid(r).field_name("remember.me")),
        ),
    ];
    for (name, f) in cases {
        assert_eq!(
            build(f),
            Err(SecurityBuildError::InvalidRememberMe),
            "{name}"
        );
    }
    let text = SecurityBuildError::InvalidRememberMe.to_string();
    assert_eq!(
        text,
        "invalid remember-me secret, user service, lifetime, cookie name or field name"
    );
    assert!(!text.contains(SHORT) && !text.contains("a;b"));
}

#[test]
fn the_debug_output_of_remember_me_shows_no_secret() {
    let mut seen = String::new();
    let _ = HttpSecurity::new().remember_me(|r| {
        let r = r
            .secret(secret(SECRET))
            .old_secrets(vec![secret("an-older-remember-me-secret-32-b!")])
            .user_details(users());
        seen = format!("{r:?}");
        r
    });
    assert!(seen.starts_with("RememberMe"), "{seen}");
    assert!(!seen.contains(SECRET), "{seen}");
    assert!(!seen.contains("an-older"), "{seen}");
    assert!(
        seen.contains("OXBLOOM_REMEMBER_ME") && seen.contains("remember-me"),
        "{seen}"
    );
}
