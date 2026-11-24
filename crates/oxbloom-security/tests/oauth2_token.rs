//! The authorization code exchange at the token endpoint, against `FakeProvider`.

#![cfg(feature = "oauth2")]

use std::sync::Arc;
use std::time::SystemTime;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::Method;
use oxbloom_config::Secret;
use oxbloom_security::__test_support::{OAuth2Error, ProviderMetadata, TokenEndpoint};
use oxbloom_security::{ClientAuthentication, ClientRegistration, ProviderConfig};
use oxbloom_test::{FakeProvider, FakeUser, Fault, MockHttpClient, MockResponse};
use serde_json::json;

const ISSUER: &str = "https://idp.example.test";
const REDIRECT: &str = "https://app.example.test/login/oauth2/code/idp";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const SECRET: &str = "s3cr3t:with space";

fn registration(client_id: &str, auth: ClientAuthentication) -> ClientRegistration {
    ClientRegistration {
        id: "idp".into(),
        client_id: client_id.into(),
        client_secret: (auth != ClientAuthentication::None).then(|| Secret::new(SECRET.into())),
        scopes: vec!["openid".into()],
        redirect_uri: REDIRECT.into(),
        client_authentication: auth,
        provider: "idp".into(),
    }
}

async fn metadata(fake: &FakeProvider) -> ProviderMetadata {
    let provider = ProviderConfig {
        issuer_uri: Some(ISSUER.into()),
        ..ProviderConfig::default()
    };
    ProviderMetadata::resolve(&provider, fake.client().as_ref(), SystemTime::now())
        .await
        .unwrap()
}

async fn exchange(
    fake: &FakeProvider,
    registration: &ClientRegistration,
) -> Result<oxbloom_security::__test_support::TokenResponse, OAuth2Error> {
    let metadata = metadata(fake).await;
    let code = fake.issue_code(FakeUser::new("alice"));
    TokenEndpoint::new(fake.client())
        .exchange_code(registration, &metadata, &code, REDIRECT, VERIFIER)
        .await
}

#[tokio::test]
async fn basic_authentication_urlencodes_id_and_secret() {
    let fake = FakeProvider::new(ISSUER);
    exchange(
        &fake,
        &registration("my client:1", ClientAuthentication::Basic),
    )
    .await
    .unwrap();
    let seen = fake.token_requests();
    assert_eq!(seen.len(), 1);
    // RFC 6749 section 2.3.1: both parts form-URL-encoded, then joined with a colon.
    let expected = format!(
        "Basic {}",
        STANDARD.encode("my+client%3A1:s3cr3t%3Awith+space")
    );
    assert_eq!(seen[0].authorization(), Some(expected.as_str()));
    assert_eq!(seen[0].field("client_id"), None);
    assert_eq!(seen[0].field("client_secret"), None);
    let sent = fake.client().requests();
    let token_request = sent.iter().find(|r| r.method() == Method::POST).unwrap();
    assert_eq!(token_request.headers()["accept"], "application/json");
    assert_eq!(token_request.url(), format!("{ISSUER}/token"));
}

#[tokio::test]
async fn post_authentication_puts_credentials_in_the_body() {
    let fake = FakeProvider::new(ISSUER);
    exchange(&fake, &registration("app", ClientAuthentication::Post))
        .await
        .unwrap();
    let seen = &fake.token_requests()[0];
    assert_eq!(seen.authorization(), None);
    assert_eq!(seen.field("client_id"), Some("app"));
    assert_eq!(seen.field("client_secret"), Some(SECRET));
}

#[tokio::test]
async fn none_sends_only_the_client_id() {
    let fake = FakeProvider::new(ISSUER);
    exchange(
        &fake,
        &registration("public-app", ClientAuthentication::None),
    )
    .await
    .unwrap();
    let seen = &fake.token_requests()[0];
    assert_eq!(seen.authorization(), None);
    assert_eq!(seen.field("client_id"), Some("public-app"));
    assert_eq!(seen.field("client_secret"), None);
}

#[tokio::test]
async fn the_code_verifier_and_redirect_uri_are_sent() {
    let fake = FakeProvider::new(ISSUER);
    let metadata = metadata(&fake).await;
    let code = fake.issue_code(FakeUser::new("alice").scope("openid email"));
    let tokens = TokenEndpoint::new(fake.client())
        .exchange_code(
            &registration("app", ClientAuthentication::Basic),
            &metadata,
            &code,
            REDIRECT,
            VERIFIER,
        )
        .await
        .unwrap();
    let seen = &fake.token_requests()[0];
    assert_eq!(seen.field("grant_type"), Some("authorization_code"));
    assert_eq!(seen.field("code"), Some(code.as_str()));
    assert_eq!(seen.field("redirect_uri"), Some(REDIRECT));
    assert_eq!(seen.field("code_verifier"), Some(VERIFIER));
    assert_eq!(seen.fields().len(), 4);

    assert!(!tokens.access_token.expose().is_empty());
    assert_eq!(tokens.token_type, "Bearer");
    assert_eq!(tokens.expires_in, Some(3600));
    assert!(tokens.refresh_token.is_some());
    assert!(tokens.id_token.is_some());
    assert_eq!(tokens.scope.as_deref(), Some("openid email"));
}

fn scripted(answer: MockResponse) -> (Arc<MockHttpClient>, ProviderMetadata) {
    let mock = Arc::new(MockHttpClient::new());
    mock.on(Method::POST, &format!("{ISSUER}/token"), answer);
    let metadata = ProviderMetadata {
        issuer: Some(ISSUER.into()),
        authorization_endpoint: format!("{ISSUER}/authorize"),
        token_endpoint: format!("{ISSUER}/token"),
        userinfo_endpoint: None,
        jwks_uri: None,
        id_token_algorithms: Vec::new(),
        resolved_at: SystemTime::UNIX_EPOCH,
    };
    (mock, metadata)
}

async fn exchange_with(answer: MockResponse) -> Result<(), OAuth2Error> {
    let (mock, metadata) = scripted(answer);
    let result = TokenEndpoint::new(mock.clone())
        .exchange_code(
            &registration("app", ClientAuthentication::Basic),
            &metadata,
            "code",
            REDIRECT,
            VERIFIER,
        )
        .await
        .map(|_| ());
    assert!(mock.unmatched_requests().is_empty());
    result
}

#[tokio::test]
async fn token_type_must_be_bearer() {
    for token_type in ["bearer", "BEARER", "Bearer"] {
        exchange_with(
            MockResponse::status(200)
                .json(&json!({"access_token": "at", "token_type": token_type})),
        )
        .await
        .unwrap();
    }
    for body in [
        json!({"access_token": "at", "token_type": "mac"}),
        json!({"access_token": "at"}),
        json!({"token_type": "Bearer"}),
        json!({"access_token": "", "token_type": "Bearer"}),
    ] {
        assert_eq!(
            exchange_with(MockResponse::status(200).json(&body)).await,
            Err(OAuth2Error::Token)
        );
    }
}

#[tokio::test]
async fn an_error_code_from_the_allow_list_is_surfaced_and_other_codes_become_unknown() {
    for code in [
        "invalid_request",
        "invalid_client",
        "invalid_grant",
        "unauthorized_client",
        "unsupported_grant_type",
        "invalid_scope",
    ] {
        let fake = FakeProvider::new(ISSUER);
        fake.misbehave(Fault::TokenEndpointError(code));
        let err = exchange(&fake, &registration("app", ClientAuthentication::Basic))
            .await
            .unwrap_err();
        assert_eq!(err, OAuth2Error::Provider(code));
        assert!(err.to_string().contains(code));
    }
    for code in ["access_denied", "temporarily_unavailable", "<script>"] {
        let fake = FakeProvider::new(ISSUER);
        fake.misbehave(Fault::TokenEndpointError(code));
        let err = exchange(&fake, &registration("app", ClientAuthentication::Basic))
            .await
            .unwrap_err();
        assert_eq!(err, OAuth2Error::Provider("unknown"));
        assert!(!err.to_string().contains(code));
    }
}

#[tokio::test]
async fn error_description_never_appears() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::TokenEndpointError("invalid_grant"));
    let err = exchange(&fake, &registration("app", ClientAuthentication::Basic))
        .await
        .unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains("DESCRIPTION"), "{text}");
    assert!(!text.to_lowercase().contains("description"), "{text}");
}

#[tokio::test]
async fn an_oversized_token_response_is_a_failure() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::OversizedBody);
    let err = exchange(&fake, &registration("app", ClientAuthentication::Basic))
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::Token);
}

#[tokio::test]
async fn a_5xx_is_a_failure_with_fixed_text() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::TokenEndpointStatus(502));
    let err = exchange(&fake, &registration("app", ClientAuthentication::Basic))
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::Token);
    assert_eq!(err.to_string(), "the token response is invalid");
    assert!(!err.to_string().contains("502"));

    // A transport failure is an HTTP failure, also with fixed text.
    let mock = Arc::new(MockHttpClient::new());
    let (_, metadata) = scripted(MockResponse::status(200));
    let err = TokenEndpoint::new(mock)
        .exchange_code(
            &registration("app", ClientAuthentication::Basic),
            &metadata,
            "code",
            REDIRECT,
            VERIFIER,
        )
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::Http);
    assert_eq!(err.to_string(), "the request to the OAuth2 provider failed");
}

#[tokio::test]
async fn secrets_never_appear_in_debug_or_error_text() {
    let fake = FakeProvider::new(ISSUER);
    let registration = registration("app", ClientAuthentication::Post);
    let metadata = metadata(&fake).await;
    let code = fake.issue_code(FakeUser::new("alice"));
    let endpoint = TokenEndpoint::new(fake.client());
    let tokens = endpoint
        .exchange_code(&registration, &metadata, &code, REDIRECT, VERIFIER)
        .await
        .unwrap();
    let debug = format!("{tokens:?} {endpoint:?} {registration:?}");
    for secret in [
        tokens.access_token.expose().as_str(),
        tokens.refresh_token.as_ref().unwrap().expose().as_str(),
        tokens.id_token.as_ref().unwrap().expose().as_str(),
        SECRET,
        code.as_str(),
        VERIFIER,
    ] {
        assert!(!debug.contains(secret), "{debug}");
    }
    // The second use of the code fails; the error names neither the code nor the secret.
    let err = endpoint
        .exchange_code(&registration, &metadata, &code, REDIRECT, VERIFIER)
        .await
        .unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains(&code));
    assert!(!text.contains(SECRET));
    assert!(!text.contains(ISSUER));
}

#[test]
fn every_error_has_fixed_text() {
    let all = [
        OAuth2Error::Config,
        OAuth2Error::Discovery,
        OAuth2Error::IssuerMismatch,
        OAuth2Error::Http,
        OAuth2Error::Provider("invalid_grant"),
        OAuth2Error::Token,
        OAuth2Error::InvalidIdToken,
        OAuth2Error::UserInfo,
        OAuth2Error::Pending,
    ];
    for err in all {
        let text = err.to_string();
        assert!(!text.is_empty());
        assert!(!text.contains("http"));
    }
}
