//! The user info endpoint and the mapping of attributes to an `Authentication`.

#![cfg(feature = "oauth2")]

use std::time::SystemTime;

use http::Method;
use oxbloom_config::Secret;
use oxbloom_security::__test_support::{
    OAuth2Error, ProviderMetadata, TokenEndpoint, UserInfo, authentication_from,
};
use oxbloom_security::{ClientAuthentication, ClientRegistration, ProviderConfig};
use oxbloom_test::{FakeProvider, FakeUser, Fault, MockHttpClient, MockResponse};
use serde_json::{Map, Value, json};

const ISSUER: &str = "https://idp.example.test";
const USERINFO: &str = "https://idp.example.test/userinfo";

fn registration() -> ClientRegistration {
    ClientRegistration {
        id: "idp".into(),
        client_id: "app".into(),
        client_secret: Some(Secret::new("app-secret".into())),
        scopes: vec!["read:user".into()],
        redirect_uri: "https://app.example.test/cb".into(),
        client_authentication: ClientAuthentication::Post,
        provider: "idp".into(),
    }
}

fn plain(user_name_attribute: Option<&str>) -> ProviderConfig {
    ProviderConfig {
        authorization_uri: Some(format!("{ISSUER}/authorize")),
        token_uri: Some(format!("{ISSUER}/token")),
        user_info_uri: Some(USERINFO.into()),
        user_name_attribute: user_name_attribute.map(str::to_owned),
        ..ProviderConfig::default()
    }
}

fn attributes(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => panic!("an object"),
    }
}

/// Logs `user` in at `fake` as a plain OAuth2 client and returns the access token.
async fn access_token(fake: &FakeProvider, user: FakeUser) -> Secret<String> {
    let metadata = ProviderMetadata::resolve(
        &plain(Some("login")),
        fake.client().as_ref(),
        SystemTime::now(),
    )
    .await
    .unwrap();
    let code = fake.issue_code(user);
    TokenEndpoint::new(fake.client())
        .exchange_code(
            &registration(),
            &metadata,
            &code,
            "https://app.example.test/cb",
            "v",
        )
        .await
        .unwrap()
        .access_token
}

#[tokio::test]
async fn userinfo_attributes_become_the_authentication_details() {
    let fake = FakeProvider::new(ISSUER);
    let token = access_token(
        &fake,
        FakeUser::new("1234")
            .attr("login", "alice-gh")
            .email("alice@example.test"),
    )
    .await;
    let attributes = UserInfo
        .fetch(USERINFO, &token, fake.client().as_ref())
        .await
        .unwrap();
    let sent = fake.client().requests();
    let request = sent.iter().find(|r| r.url() == USERINFO).unwrap();
    assert_eq!(request.method(), Method::GET);
    assert_eq!(
        request.headers()["authorization"],
        format!("Bearer {}", token.expose()).as_str()
    );
    assert_eq!(request.headers()["accept"], "application/json");

    let auth = authentication_from(
        &registration(),
        &plain(Some("login")),
        false,
        attributes,
        &["read:user".to_owned()],
    )
    .unwrap();
    assert_eq!(auth.principal(), "alice-gh");
    assert_eq!(auth.details()["login"], "alice-gh");
    assert_eq!(auth.details()["email"], "alice@example.test");
    assert_eq!(auth.details()["sub"], "1234");
    assert!(auth.has_authority("OAUTH2_USER"));
    assert!(auth.has_authority("SCOPE_read:user"));
    assert!(!auth.has_authority("OIDC_USER"));
}

#[tokio::test]
async fn userinfo_failures_are_errors_with_fixed_text() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::UserinfoError);
    let token = access_token(&fake, FakeUser::new("alice")).await;
    let err = UserInfo
        .fetch(USERINFO, &token, fake.client().as_ref())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::UserInfo);
    assert!(!format!("{err} {err:?}").contains(token.expose().as_str()));

    for answer in [
        MockResponse::status(200).json(&json!(["not", "an", "object"])),
        MockResponse::status(200).text("not json"),
        MockResponse::status(200).json(&json!({"padding": "x".repeat(1024 * 1024)})),
        MockResponse::status(401),
    ] {
        let mock = MockHttpClient::new();
        mock.on(Method::GET, USERINFO, answer);
        let err = UserInfo
            .fetch(USERINFO, &Secret::new("at".into()), &mock)
            .await
            .unwrap_err();
        assert_eq!(err, OAuth2Error::UserInfo);
        assert!(mock.unmatched_requests().is_empty());
    }
    let err = UserInfo
        .fetch(USERINFO, &Secret::new("at".into()), &MockHttpClient::new())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::Http);
}

#[test]
fn a_missing_user_name_attribute_is_an_error() {
    let attrs = attributes(json!({"sub": "1234", "id": 42}));
    // Plain OAuth2 needs a configured user-name-attribute.
    assert_eq!(
        authentication_from(&registration(), &plain(None), false, attrs.clone(), &[]).unwrap_err(),
        OAuth2Error::Config
    );
    // The configured attribute must be present ...
    assert_eq!(
        authentication_from(
            &registration(),
            &plain(Some("login")),
            false,
            attrs.clone(),
            &[]
        )
        .unwrap_err(),
        OAuth2Error::UserInfo
    );
    // ... and a non-empty string or an integer.
    let _ = attrs;
    for value in [
        json!(4.5),
        json!(true),
        json!(null),
        json!(["a"]),
        json!({"a": 1}),
    ] {
        let odd = attributes(json!({ "id": value }));
        assert_eq!(
            authentication_from(&registration(), &plain(Some("id")), false, odd, &[]).unwrap_err(),
            OAuth2Error::UserInfo,
            "{value}"
        );
    }
    let empty = attributes(json!({"login": ""}));
    assert_eq!(
        authentication_from(&registration(), &plain(Some("login")), false, empty, &[]).unwrap_err(),
        OAuth2Error::UserInfo
    );
    // OpenID Connect defaults to `sub`, which must be there too.
    let oidc = ProviderConfig {
        issuer_uri: Some(ISSUER.into()),
        ..ProviderConfig::default()
    };
    assert_eq!(
        authentication_from(
            &registration(),
            &oidc,
            true,
            attributes(json!({"email": "a"})),
            &[]
        )
        .unwrap_err(),
        OAuth2Error::UserInfo
    );
}

#[test]
fn authorities_are_oidc_user_plus_scopes() {
    let oidc = ProviderConfig {
        issuer_uri: Some(ISSUER.into()),
        ..ProviderConfig::default()
    };
    let auth = authentication_from(
        &registration(),
        &oidc,
        true,
        attributes(json!({"sub": "alice", "email": "alice@example.test"})),
        &["openid".to_owned(), "email".to_owned()],
    )
    .unwrap();
    assert_eq!(auth.principal(), "alice");
    let names: Vec<&str> = auth.authorities().iter().map(|a| a.as_str()).collect();
    assert_eq!(names, ["OIDC_USER", "SCOPE_email", "SCOPE_openid"]);

    // A configured attribute replaces `sub` for OpenID Connect too.
    let by_email = ProviderConfig {
        user_name_attribute: Some("email".into()),
        ..oidc
    };
    let auth = authentication_from(
        &registration(),
        &by_email,
        true,
        attributes(json!({"sub": "alice", "email": "alice@example.test"})),
        &[],
    )
    .unwrap();
    assert_eq!(auth.principal(), "alice@example.test");
    assert_eq!(auth.authorities().len(), 1);
}

#[test]
fn the_oauth2_marker_is_in_the_details() {
    // Attributes that try to set the marker keys are overwritten.
    let attrs = attributes(json!({
        "login": "alice-gh",
        "oauth2.registration": "forged",
        "oauth2.principal": false,
    }));
    let auth =
        authentication_from(&registration(), &plain(Some("login")), false, attrs, &[]).unwrap();
    assert_eq!(auth.details()["oauth2.registration"], "idp");
    assert_eq!(auth.details()["oauth2.principal"], true);
    assert!(!auth.is_remembered());
}

#[test]
fn an_integer_attribute_is_the_principal_as_a_decimal_string() {
    // GitHub names its users with a numeric `id`.
    for (value, expected) in [
        (json!(583231), "583231"),
        (json!(-7), "-7"),
        (json!(u64::MAX), "18446744073709551615"),
    ] {
        let attrs = attributes(json!({ "id": value, "login": "octocat" }));
        let auth =
            authentication_from(&registration(), &plain(Some("id")), false, attrs, &[]).unwrap();
        assert_eq!(auth.principal(), expected);
        // The attribute itself keeps its JSON type in the details.
        assert_eq!(auth.details()["id"], value);
    }
}

#[test]
fn provider_attributes_in_the_reserved_namespaces_are_dropped() {
    let attrs = attributes(json!({
        "login": "alice",
        "oxbloom.remembered": true,
        "oxbloom.security.anything": "x",
        "oauth2.registration": "another",
        "oauth2.principal": false,
        "oauth2.extra": 1,
        "oxbloomish": "kept",
        "oauth2x": "kept",
    }));
    let auth = authentication_from(&registration(), &plain(Some("login")), false, attrs, &[])
        .unwrap();
    // A provider can never mark the user as remembered, or set any framework detail.
    assert!(!auth.is_remembered());
    let details = auth.details();
    assert!(
        details
            .keys()
            .all(|k| !k.starts_with("oxbloom.")),
        "{details:?}"
    );
    assert!(!details.contains_key("oauth2.extra"), "{details:?}");
    assert_eq!(details["oauth2.registration"], "idp");
    assert_eq!(details["oauth2.principal"], true);
    assert_eq!(details["oxbloomish"], "kept");
    assert_eq!(details["oauth2x"], "kept");
    assert_eq!(details["login"], "alice");
}
