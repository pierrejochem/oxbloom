//! The public surface of the stored authentication: what it serializes and what it prints.

use oxbloom_security::{Authentication, GrantedAuthority};

fn sample() -> Authentication {
    let mut details = serde_json::Map::new();
    details.insert("claim".into(), serde_json::json!("hidden-claim-value"));
    Authentication::new("alice", [GrantedAuthority::new("ROLE_USER")]).with_details(details)
}

#[test]
fn authentication_serializes_and_deserializes() {
    let json = serde_json::to_value(sample()).unwrap();
    let back: Authentication = serde_json::from_value(json).unwrap();
    assert_eq!(back.principal(), "alice");
    assert!(back.has_role("USER"));
    assert_eq!(back.details()["claim"], "hidden-claim-value");
}

#[test]
fn serialized_form_has_no_password_field() {
    let json = serde_json::to_value(sample()).unwrap();
    let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["authorities", "details", "principal"]);
    assert!(!json.to_string().to_lowercase().contains("password"));
}

#[test]
fn debug_of_authentication_still_hides_claims() {
    let debug = format!("{:?}", sample());
    assert!(debug.contains("alice"));
    assert!(!debug.contains("hidden-claim-value"));
    assert!(!debug.contains("claim"));
}
