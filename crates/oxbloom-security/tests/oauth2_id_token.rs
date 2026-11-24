//! Id token validation against `FakeProvider`: one valid token and one test per rejection.

#![cfg(feature = "oauth2")]

use std::time::{Duration, SystemTime};

use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_security::__test_support::{
    IdTokenValidator, OAuth2Error, ProviderMetadata, TokenEndpoint,
};
use oxbloom_security::{Algorithm, ClientAuthentication, ClientRegistration, ProviderConfig};
use oxbloom_test::{FakeProvider, FakeUser, Fault, ManualClock};

const ISSUER: &str = "https://idp.example.test";
const NONCE: &str = "n-0123456789";

fn registration() -> ClientRegistration {
    ClientRegistration {
        id: "idp".into(),
        client_id: "app".into(),
        client_secret: Some(Secret::new("app-secret".into())),
        scopes: vec!["openid".into()],
        redirect_uri: "https://app.example.test/cb".into(),
        client_authentication: ClientAuthentication::Basic,
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

/// Logs `alice` in at `fake` and returns the id token of the token response.
async fn id_token(fake: &FakeProvider) -> String {
    fake.expect_nonce(NONCE);
    let code = fake.issue_code(FakeUser::new("alice").email("alice@example.test"));
    let tokens = TokenEndpoint::new(fake.client())
        .exchange_code(
            &registration(),
            &metadata(fake).await,
            &code,
            "https://app.example.test/cb",
            "verifier",
        )
        .await
        .unwrap();
    tokens.id_token.unwrap().expose().clone()
}

async fn validate_at(fake: &FakeProvider, token: &str, now: SystemTime) -> Result<(), OAuth2Error> {
    let validator = IdTokenValidator::new(fake.client());
    let result = validator
        .validate(token, &registration(), &metadata(fake).await, NONCE, now)
        .await
        .map(|_| ());
    // Every rejection must come from the token, not from an unanswered request.
    assert!(fake.client().unmatched_requests().is_empty());
    result
}

async fn rejected_with(fault: Fault) {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(fault);
    let token = id_token(&fake).await;
    assert_eq!(
        validate_at(&fake, &token, SystemTime::now()).await,
        Err(OAuth2Error::InvalidIdToken),
        "{fault:?}"
    );
}

#[tokio::test]
async fn a_valid_token_validates() {
    let fake = FakeProvider::new(ISSUER);
    let token = id_token(&fake).await;
    let claims = IdTokenValidator::new(fake.client())
        .validate(
            &token,
            &registration(),
            &metadata(&fake).await,
            NONCE,
            SystemTime::now(),
        )
        .await
        .unwrap();
    assert_eq!(claims.subject(), "alice");
    assert_eq!(claims.claims()["email"], "alice@example.test");
    assert_eq!(claims.claims()["nonce"], NONCE);
    assert!(!format!("{claims:?}").contains(NONCE));
}

#[tokio::test]
async fn the_key_set_is_fetched_once_for_several_tokens() {
    let fake = FakeProvider::new(ISSUER);
    let validator = IdTokenValidator::new(fake.client());
    let metadata = metadata(&fake).await;
    for _ in 0..3 {
        let token = id_token(&fake).await;
        validator
            .validate(&token, &registration(), &metadata, NONCE, SystemTime::now())
            .await
            .unwrap();
    }
    let jwks_fetches = fake
        .client()
        .requests()
        .iter()
        .filter(|r| r.url().ends_with("/jwks"))
        .count();
    assert_eq!(jwks_fetches, 1);
}

#[tokio::test]
async fn wrong_issuer() {
    rejected_with(Fault::WrongIssuer).await;
}

#[tokio::test]
async fn wrong_audience() {
    rejected_with(Fault::WrongAudience).await;
}

#[tokio::test]
async fn wrong_azp_with_several_audiences() {
    rejected_with(Fault::WrongAzp).await;
}

#[tokio::test]
async fn expired() {
    rejected_with(Fault::Expired).await;

    // A valid token expires one hour after it is issued, with a leeway of 60 seconds.
    let fake = FakeProvider::new(ISSUER);
    let token = id_token(&fake).await;
    let clock = ManualClock::at(SystemTime::now());
    clock.advance(Duration::from_secs(3600 + 55));
    assert_eq!(validate_at(&fake, &token, clock.now()).await, Ok(()));
    clock.advance(Duration::from_secs(10));
    assert_eq!(
        validate_at(&fake, &token, clock.now()).await,
        Err(OAuth2Error::InvalidIdToken)
    );
}

#[tokio::test]
async fn not_yet_valid() {
    rejected_with(Fault::NotYetValid).await;

    // A token issued in the future is accepted within the leeway only.
    let fake = FakeProvider::new(ISSUER);
    let token = id_token(&fake).await;
    let now = SystemTime::now();
    assert_eq!(
        validate_at(&fake, &token, now - Duration::from_secs(55)).await,
        Ok(())
    );
    assert_eq!(
        validate_at(&fake, &token, now - Duration::from_secs(65)).await,
        Err(OAuth2Error::InvalidIdToken)
    );
}

#[tokio::test]
async fn wrong_nonce() {
    rejected_with(Fault::WrongNonce).await;

    // A token without a nonce is refused too.
    let fake = FakeProvider::new(ISSUER);
    let code = fake.issue_code(FakeUser::new("alice"));
    let tokens = TokenEndpoint::new(fake.client())
        .exchange_code(
            &registration(),
            &metadata(&fake).await,
            &code,
            "https://app.example.test/cb",
            "verifier",
        )
        .await
        .unwrap();
    let token = tokens.id_token.unwrap().expose().clone();
    assert_eq!(
        validate_at(&fake, &token, SystemTime::now()).await,
        Err(OAuth2Error::InvalidIdToken)
    );
}

#[tokio::test]
async fn alg_none() {
    rejected_with(Fault::AlgNone).await;
}

#[tokio::test]
async fn hs256_signed_with_the_public_key() {
    rejected_with(Fault::Hs256WithPublicKey).await;
}

#[tokio::test]
async fn unknown_kid() {
    // A token from another provider instance with the same issuer has another key id.
    let fake = FakeProvider::new(ISSUER);
    let impostor = FakeProvider::new(ISSUER);
    let token = id_token(&impostor).await;
    assert_eq!(
        validate_at(&fake, &token, SystemTime::now()).await,
        Err(OAuth2Error::InvalidIdToken)
    );
}

#[tokio::test]
async fn missing_sub() {
    rejected_with(Fault::MissingSub).await;
}

#[tokio::test]
async fn an_algorithm_outside_the_provider_list_is_refused() {
    let fake = FakeProvider::new(ISSUER);
    let token = id_token(&fake).await;
    let mut metadata = metadata(&fake).await;
    metadata.id_token_algorithms = vec![Algorithm::RS256];
    let err = IdTokenValidator::new(fake.client())
        .validate(&token, &registration(), &metadata, NONCE, SystemTime::now())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::InvalidIdToken);
}

#[tokio::test]
async fn a_provider_without_a_key_set_or_a_malformed_token_is_refused() {
    let fake = FakeProvider::new(ISSUER);
    let token = id_token(&fake).await;
    let mut metadata = metadata(&fake).await;
    metadata.jwks_uri = None;
    let validator = IdTokenValidator::new(fake.client());
    let err = validator
        .validate(&token, &registration(), &metadata, NONCE, SystemTime::now())
        .await
        .unwrap_err();
    assert_eq!(err, OAuth2Error::InvalidIdToken);

    let metadata = self::metadata(&fake).await;
    for garbage in ["", "a.b", "a.b.c", "not a token at all"] {
        let err = validator
            .validate(
                garbage,
                &registration(),
                &metadata,
                NONCE,
                SystemTime::now(),
            )
            .await
            .unwrap_err();
        assert_eq!(err, OAuth2Error::InvalidIdToken);
    }
}

#[tokio::test]
async fn the_error_text_holds_no_token_or_nonce() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::WrongAudience);
    let token = id_token(&fake).await;
    let err = validate_at(&fake, &token, SystemTime::now())
        .await
        .unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains(&token));
    assert!(!text.contains(NONCE));
    let validator = IdTokenValidator::new(fake.client());
    assert!(!format!("{validator:?}").contains(ISSUER));
}
