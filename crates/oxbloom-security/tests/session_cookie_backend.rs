//! The public surface of the cookie session backend: construction, secret checks and `Debug`.
//!
//! Sealing and opening are crate-private until the session filter uses them, so the round
//! trip, tampering, rotation, size, timeout and garbage-input tests live next to the code in
//! `src/session/cookie_backend.rs`.

use std::sync::Arc;

use oxbloom_config::Secret;
use oxbloom_core::SystemClock;
use oxbloom_security::{CookieSessionBackend, SessionError, SessionSettings};

const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn secret(s: &str) -> Secret<String> {
    Secret::new(s.to_owned())
}

fn build(current: &str, old: &[&str]) -> Result<CookieSessionBackend, SessionError> {
    let old: Vec<_> = old.iter().map(|s| secret(s)).collect();
    CookieSessionBackend::new(
        &secret(current),
        &old,
        &SessionSettings::default(),
        Arc::new(SystemClock),
    )
}

#[test]
fn short_secret_is_rejected() {
    let short = &SECRET[..31];
    assert_eq!(
        build(short, &[]).unwrap_err(),
        SessionError::InvalidSettings
    );
    assert_eq!(
        build(SECRET, &[short]).unwrap_err(),
        SessionError::InvalidSettings
    );
    // The error text names no secret.
    let text = build(short, &[]).unwrap_err().to_string();
    assert!(!text.contains(short));
}

#[test]
fn exactly_32_bytes_is_accepted() {
    assert_eq!(SECRET.len(), 32);
    assert!(build(SECRET, &[SECRET]).is_ok());
}

#[test]
fn debug_shows_no_key_or_value() {
    let backend = build(SECRET, &[]).unwrap();
    let text = format!("{backend:?}");
    assert_eq!(text, "CookieSessionBackend { .. }");
    assert!(!text.contains(SECRET));
}
