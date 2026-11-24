//! `FakeProvider`: discovery, JWKS, the token endpoint, id tokens signed with the generated key,
//! user info and the faults that drive the negative tests.

#![cfg(feature = "oauth2")]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use http::StatusCode;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use oxbloom_http_client::{Body, HttpClient, Request, Response};
use oxbloom_test::{FakeProvider, FakeUser, Fault, ManualClock};
use serde_json::{Map, Value};

const ISSUER: &str = "https://idp.example.test";

/// Checks the claims and the header of an id token.
type Check = Box<dyn Fn(&Map<String, Value>, &Value)>;

fn token_request(code: &str) -> Request {
    Request::post(format!("{ISSUER}/token"))
        .header(
            "authorization",
            &format!("Basic {}", STANDARD.encode("app:secret")),
        )
        .body(Body::Form(vec![
            ("grant_type".into(), "authorization_code".into()),
            ("code".into(), code.into()),
            ("redirect_uri".into(), "https://app.example/cb".into()),
            ("code_verifier".into(), "verifier".into()),
        ]))
}

async fn exchange(fake: &FakeProvider, code: &str) -> Response {
    fake.client().execute(token_request(code)).await.unwrap()
}

async fn jwks(fake: &FakeProvider) -> JwkSet {
    let response = fake
        .client()
        .execute(Request::get(format!("{ISSUER}/jwks")))
        .await
        .unwrap();
    response.json().unwrap()
}

fn payload(token: &str) -> Map<String, Value> {
    let part = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

fn header(token: &str) -> Value {
    let part = token.split('.').next().unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

async fn id_token(fake: &FakeProvider, user: FakeUser) -> String {
    let code = fake.issue_code(user);
    let body: Value = exchange(fake, &code).await.json().unwrap();
    body["id_token"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn discovery_lists_the_endpoints_of_the_issuer() {
    let fake = FakeProvider::new(ISSUER);
    let response = fake
        .client()
        .execute(Request::get(format!(
            "{ISSUER}/.well-known/openid-configuration"
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let doc: Value = response.json().unwrap();
    assert_eq!(doc["issuer"], ISSUER);
    assert_eq!(doc["authorization_endpoint"], format!("{ISSUER}/authorize"));
    assert_eq!(doc["token_endpoint"], format!("{ISSUER}/token"));
    assert_eq!(doc["userinfo_endpoint"], format!("{ISSUER}/userinfo"));
    assert_eq!(doc["jwks_uri"], format!("{ISSUER}/jwks"));
    assert_eq!(doc["id_token_signing_alg_values_supported"][0], "ES256");
}

#[tokio::test]
async fn the_id_token_verifies_with_the_published_key() {
    let fake = FakeProvider::new(ISSUER);
    fake.expect_nonce("n-1");
    let token = id_token(&fake, FakeUser::new("alice").email("alice@example.test")).await;
    let set = jwks(&fake).await;
    assert_eq!(set.keys.len(), 1);
    let jwk = &set.keys[0];
    assert_eq!(header(&token)["kid"], jwk.common.key_id.clone().unwrap());

    let mut validation = Validation::new(Algorithm::ES256);
    validation.set_audience(&["app"]);
    validation.set_issuer(&[ISSUER]);
    let data = jsonwebtoken::decode::<Map<String, Value>>(
        &token,
        &DecodingKey::from_jwk(jwk).unwrap(),
        &validation,
    )
    .unwrap();
    assert_eq!(data.claims["sub"], "alice");
    assert_eq!(data.claims["email"], "alice@example.test");
    assert_eq!(data.claims["nonce"], "n-1");
    // The audience is the client id the token request authenticated with.
    assert_eq!(data.claims["aud"], "app");
}

#[tokio::test]
async fn each_instance_has_its_own_key_and_codes() {
    let a = FakeProvider::new(ISSUER);
    let b = FakeProvider::new(ISSUER);
    assert_ne!(
        jwks(&a).await.keys[0].common.key_id,
        jwks(&b).await.keys[0].common.key_id
    );
    let code_a = a.issue_code(FakeUser::new("alice"));
    let code_b = a.issue_code(FakeUser::new("alice"));
    assert_ne!(code_a, code_b);
    // A code of one instance is unknown to the other.
    assert_eq!(
        exchange(&b, &code_a).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_code_works_once() {
    let fake = FakeProvider::new(ISSUER);
    let code = fake.issue_code(FakeUser::new("alice"));
    let first = exchange(&fake, &code).await;
    assert_eq!(first.status(), StatusCode::OK);
    let body: Value = first.json().unwrap();
    assert_eq!(body["token_type"], "Bearer");
    assert!(body["access_token"].as_str().is_some_and(|t| !t.is_empty()));
    assert!(body["refresh_token"].is_string());
    assert_eq!(body["expires_in"], 3600);

    let second = exchange(&fake, &code).await;
    assert_eq!(second.status(), StatusCode::BAD_REQUEST);
    let error: Value = second.json().unwrap();
    assert_eq!(error["error"], "invalid_grant");
}

#[tokio::test]
async fn token_requests_are_recorded_with_their_fields_and_authorization() {
    let fake = FakeProvider::new(ISSUER);
    let code = fake.issue_code(FakeUser::new("alice"));
    exchange(&fake, &code).await;
    let seen = fake.token_requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].field("grant_type"), Some("authorization_code"));
    assert_eq!(seen[0].field("code"), Some(code.as_str()));
    assert_eq!(seen[0].field("code_verifier"), Some("verifier"));
    assert_eq!(seen[0].field("client_secret"), None);
    assert_eq!(
        seen[0].authorization(),
        Some(format!("Basic {}", STANDARD.encode("app:secret")).as_str())
    );
    assert_eq!(seen[0].fields().len(), 4);
}

#[tokio::test]
async fn the_client_id_of_the_form_is_the_audience_without_basic() {
    let fake = FakeProvider::new(ISSUER);
    let code = fake.issue_code(FakeUser::new("alice"));
    let request = Request::post(format!("{ISSUER}/token")).body(Body::Form(vec![
        ("grant_type".into(), "authorization_code".into()),
        ("code".into(), code),
        ("client_id".into(), "public-app".into()),
    ]));
    let body: Value = fake
        .client()
        .execute(request)
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        payload(body["id_token"].as_str().unwrap())["aud"],
        "public-app"
    );
}

#[tokio::test]
async fn userinfo_answers_the_attributes_of_the_access_token_user() {
    let fake = FakeProvider::new(ISSUER);
    let code = fake.issue_code(
        FakeUser::new("alice")
            .name("Alice")
            .attr("login", "alice-gh"),
    );
    let body: Value = exchange(&fake, &code).await.json().unwrap();
    let token = body["access_token"].as_str().unwrap();
    let info: Value = fake
        .client()
        .execute(
            Request::get(format!("{ISSUER}/userinfo"))
                .header("authorization", &format!("Bearer {token}")),
        )
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(info["sub"], "alice");
    assert_eq!(info["name"], "Alice");
    assert_eq!(info["login"], "alice-gh");

    let unknown = fake
        .client()
        .execute(Request::get(format!("{ISSUER}/userinfo")).header("authorization", "Bearer nope"))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_granted_scope_is_in_the_token_response() {
    let fake = FakeProvider::new(ISSUER);
    let code = fake.issue_code(FakeUser::new("alice").scope("openid email"));
    let body: Value = exchange(&fake, &code).await.json().unwrap();
    assert_eq!(body["scope"], "openid email");
}

#[tokio::test]
async fn the_times_come_from_the_given_clock() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let fake = FakeProvider::new(ISSUER).clock(Arc::new(ManualClock::at(at)));
    let claims = payload(&id_token(&fake, FakeUser::new("alice")).await);
    assert_eq!(claims["iat"], 1_800_000_000u64);
    assert_eq!(claims["exp"], 1_800_000_000u64 + 3600);
}

#[tokio::test]
async fn claim_faults_change_the_id_token() {
    let cases: Vec<(Fault, Check)> = vec![
        (
            Fault::WrongIssuer,
            Box::new(|c, _| assert_ne!(c["iss"], ISSUER)),
        ),
        (
            Fault::WrongAudience,
            Box::new(|c, _| assert_ne!(c["aud"], "app")),
        ),
        (
            Fault::WrongAzp,
            Box::new(|c, _| {
                assert_eq!(c["aud"].as_array().unwrap().len(), 2);
                assert_ne!(c["azp"], "app");
            }),
        ),
        (
            Fault::Expired,
            Box::new(|c, _| {
                let now = jsonwebtoken::get_current_timestamp();
                assert!(c["exp"].as_u64().unwrap() + 60 < now);
            }),
        ),
        (
            Fault::NotYetValid,
            Box::new(|c, _| {
                let now = jsonwebtoken::get_current_timestamp();
                assert!(c["iat"].as_u64().unwrap() > now + 60);
                assert!(c["nbf"].as_u64().unwrap() > now + 60);
            }),
        ),
        (
            Fault::WrongNonce,
            Box::new(|c, _| assert_ne!(c["nonce"], "n-1")),
        ),
        (
            Fault::MissingSub,
            Box::new(|c, _| assert!(!c.contains_key("sub"))),
        ),
        (
            Fault::AlgNone,
            Box::new(|_, h| assert_eq!(h["alg"], "none")),
        ),
        (
            Fault::Hs256WithPublicKey,
            Box::new(|_, h| assert_eq!(h["alg"], "HS256")),
        ),
    ];
    for (fault, check) in cases {
        let fake = FakeProvider::new(ISSUER);
        fake.expect_nonce("n-1");
        fake.misbehave(fault);
        let token = id_token(&fake, FakeUser::new("alice")).await;
        check(&payload(&token), &header(&token));
    }
}

#[tokio::test]
async fn an_alg_none_token_has_an_empty_signature() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::AlgNone);
    let token = id_token(&fake, FakeUser::new("alice")).await;
    assert!(token.ends_with('.'));
}

#[tokio::test]
async fn token_endpoint_faults_change_the_answer() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::TokenEndpointError("invalid_grant"));
    let code = fake.issue_code(FakeUser::new("alice"));
    let response = exchange(&fake, &code).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().unwrap();
    assert_eq!(body["error"], "invalid_grant");
    assert!(body["error_description"].is_string());

    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::TokenEndpointStatus(503));
    let code = fake.issue_code(FakeUser::new("alice"));
    assert_eq!(
        exchange(&fake, &code).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::OversizedBody);
    let code = fake.issue_code(FakeUser::new("alice"));
    let response = exchange(&fake, &code).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.bytes().len() > 1024 * 1024);
}

#[tokio::test]
async fn discovery_and_userinfo_faults() {
    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::DiscoveryIssuerMismatch);
    let doc: Value = fake
        .client()
        .execute(Request::get(format!(
            "{ISSUER}/.well-known/openid-configuration"
        )))
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_ne!(doc["issuer"], ISSUER);

    let fake = FakeProvider::new(ISSUER);
    fake.misbehave(Fault::UserinfoError);
    let code = fake.issue_code(FakeUser::new("alice"));
    let body: Value = exchange(&fake, &code).await.json().unwrap();
    let token = body["access_token"].as_str().unwrap();
    let response = fake
        .client()
        .execute(
            Request::get(format!("{ISSUER}/userinfo"))
                .header("authorization", &format!("Bearer {token}")),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn the_authorization_endpoint_is_never_fetched_and_other_urls_are_unmatched() {
    let fake = FakeProvider::new(ISSUER);
    let client = fake.client();
    assert!(
        client
            .execute(Request::get(format!("{ISSUER}/authorize")))
            .await
            .is_err()
    );
    assert_eq!(client.unmatched_requests().len(), 1);
}

#[test]
fn debug_shows_no_key_material_or_codes() {
    let fake = FakeProvider::new(ISSUER);
    let code = fake.issue_code(FakeUser::new("alice"));
    let text = format!("{fake:?}");
    assert!(!text.contains(&code));
    assert!(text.contains(ISSUER));
}
