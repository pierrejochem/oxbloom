use std::time::Duration;

use oxbloom_security::{SameSite, SecureMode, SessionError, SessionSettings};

#[test]
fn settings_defaults() {
    let s = SessionSettings::default();
    assert_eq!(s.cookie_name, "OXBLOOM_SESSION");
    assert_eq!(s.path, "/");
    assert_eq!(s.secure, SecureMode::Auto);
    assert_eq!(s.same_site, SameSite::Lax);
    assert_eq!(s.idle_timeout, Duration::from_secs(1800));
    assert_eq!(s.absolute_timeout, Duration::from_secs(8 * 3600));
    assert_eq!(s.max_sessions, 100_000);
    assert_eq!(s.max_cookie_bytes, 3072);
    assert!(s.validate().is_ok());
}

#[test]
fn settings_validate_rejects_bad_values() {
    let bad = |s: SessionSettings| assert_eq!(s.validate(), Err(SessionError::InvalidSettings));
    let d = SessionSettings::default;
    bad(d().cookie_name(""));
    bad(d().cookie_name("a;b"));
    bad(d().cookie_name("a=b"));
    bad(d().cookie_name("a b"));
    bad(d().cookie_name("a\rb"));
    bad(d().cookie_name("a\nb"));
    bad(d().path("a;b"));
    bad(d().path("/a;b"));
    bad(d().path("/a b"));
    bad(d().path("/a\nb"));
    bad(d().path(""));
    bad(d().idle_timeout(Duration::ZERO));
    bad(d().absolute_timeout(Duration::ZERO));
    bad(d().idle_timeout(Duration::from_secs(9 * 3600)));
    bad(d().max_sessions(0));
    bad(d().max_cookie_bytes(255));
    assert!(d().max_cookie_bytes(256).validate().is_ok());
    assert!(d().path("/app").validate().is_ok());
    assert!(d().cookie_name("my-session_1").validate().is_ok());
}

#[test]
fn settings_validate_rejects_a_sub_second_idle_timeout() {
    // A sub-second idle timeout would make every write answer `Clear`.
    let d = SessionSettings::default;
    assert_eq!(
        d().idle_timeout(Duration::from_millis(999)).validate(),
        Err(SessionError::InvalidSettings)
    );
    assert!(d().idle_timeout(Duration::from_secs(1)).validate().is_ok());
}

#[test]
fn same_site_none_needs_secure_always() {
    let none = SessionSettings::default().same_site(SameSite::None);
    assert!(none.clone().validate().is_err());
    assert!(none.clone().secure(SecureMode::Never).validate().is_err());
    assert!(none.clone().secure(SecureMode::Auto).validate().is_err());
    assert!(none.secure(SecureMode::Always).validate().is_ok());
}

#[test]
fn from_str_round_trip_and_unknown_names_rejected() {
    for (name, v) in [
        ("lax", SameSite::Lax),
        ("strict", SameSite::Strict),
        ("none", SameSite::None),
    ] {
        assert_eq!(name.parse::<SameSite>(), Ok(v));
    }
    for (name, v) in [
        ("auto", SecureMode::Auto),
        ("always", SecureMode::Always),
        ("never", SecureMode::Never),
    ] {
        assert_eq!(name.parse::<SecureMode>(), Ok(v));
    }
    for bad in ["", "Lax", "STRICT", "None", "other", " lax"] {
        assert!(bad.parse::<SameSite>().is_err());
    }
    for bad in ["", "Auto", "ALWAYS", "Never", "maybe", "never "] {
        assert!(bad.parse::<SecureMode>().is_err());
    }
}

#[test]
fn session_error_texts_are_fixed() {
    for e in [
        SessionError::Store,
        SessionError::Codec,
        SessionError::TooLarge,
        SessionError::InvalidSettings,
        SessionError::Serialization,
    ] {
        let text = e.to_string();
        assert!(!text.is_empty());
        assert!(!text.contains('{') && !text.contains('}'));
    }
    let err = "SECRET-NAME".parse::<SameSite>().unwrap_err();
    assert!(!err.to_string().contains("SECRET"));
    assert!(!format!("{err:?}").contains("SECRET"));
}
