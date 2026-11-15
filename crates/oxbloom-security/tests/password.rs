//! Tests for the password encoders.

use std::sync::Arc;

use oxbloom_security::{
    Argon2PasswordEncoder, DelegatingPasswordEncoder, PasswordEncoder, PasswordEncoderError,
};

fn cheap() -> Argon2PasswordEncoder {
    Argon2PasswordEncoder::new(8, 1, 1).unwrap()
}

#[test]
fn argon2_round_trip() {
    let enc = cheap();
    let hash = enc.encode("pw");
    assert!(enc.matches("pw", &hash));
    assert!(!enc.matches("other", &hash));
}

#[test]
fn argon2_hashes_differ_per_call() {
    let enc = cheap();
    let (a, b) = (enc.encode("pw"), enc.encode("pw"));
    assert_ne!(a, b);
    assert!(enc.matches("pw", &a) && enc.matches("pw", &b));
}

#[test]
fn argon2_malformed_hash_never_matches() {
    let enc = cheap();
    let hash = enc.encode("pw");
    assert!(!enc.matches("pw", ""));
    assert!(!enc.matches("pw", "garbage"));
    assert!(!enc.matches("pw", &hash[..hash.len() / 2]));
}

#[test]
fn argon2_empty_and_long_passwords() {
    let enc = cheap();
    assert!(enc.matches("", &enc.encode("")));
    let long = "a".repeat(10_000);
    assert!(enc.matches(&long, &enc.encode(&long)));
}

#[test]
fn argon2_rejects_bad_parameters() {
    for (m, t, p) in [(0, 1, 1), (8, 0, 1), (8, 1, 0)] {
        assert_eq!(
            Argon2PasswordEncoder::new(m, t, p).unwrap_err(),
            PasswordEncoderError::InvalidParameters
        );
    }
}

#[test]
fn argon2_defaults_are_owasp() {
    let enc = Argon2PasswordEncoder::default();
    assert_eq!(
        (enc.memory_kib(), enc.iterations(), enc.parallelism()),
        (19456, 2, 1)
    );
}

#[test]
fn delegating_prefixes_and_verifies() {
    let enc = DelegatingPasswordEncoder::argon2(cheap());
    let hash = enc.encode("pw");
    assert!(hash.starts_with("{argon2}"));
    assert!(enc.matches("pw", &hash));
    assert!(!enc.matches("x", &hash));
}

#[test]
fn delegating_unknown_or_missing_prefix_never_matches() {
    let enc = DelegatingPasswordEncoder::argon2(cheap());
    let bare = cheap().encode("pw");
    assert!(bare.starts_with("$argon2"));
    for stored in ["{bcrypt}x", bare.as_str(), "{argon2", ""] {
        assert!(!enc.matches("pw", stored), "{stored}");
    }
}

#[test]
fn delegating_upgrade_encoding() {
    let weak = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let strong = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(16, 2, 1).unwrap());
    let weak_hash = weak.encode("pw");
    assert!(strong.upgrade_encoding(&weak_hash));
    assert!(!strong.upgrade_encoding(&strong.encode("pw")));
    assert!(!weak.upgrade_encoding(&strong.encode("pw")));
    assert!(strong.upgrade_encoding("{bcrypt}x"));
}

#[test]
fn delegating_constructor_errors() {
    assert_eq!(
        DelegatingPasswordEncoder::new("missing", vec![]).unwrap_err(),
        PasswordEncoderError::UnknownDefaultId
    );
    let a: Arc<dyn PasswordEncoder> = Arc::new(cheap());
    assert_eq!(
        DelegatingPasswordEncoder::new(
            "argon2",
            vec![("argon2".into(), a.clone()), ("argon2".into(), a)]
        )
        .unwrap_err(),
        PasswordEncoderError::DuplicateId
    );
}

#[test]
fn debug_never_shows_a_hash() {
    let a = cheap();
    let d = DelegatingPasswordEncoder::argon2(cheap());
    let _ = (a.encode("pw"), d.encode("pw"));
    assert!(!format!("{a:?}").contains("$argon2"));
    assert!(!format!("{d:?}").contains("$argon2"));
}
