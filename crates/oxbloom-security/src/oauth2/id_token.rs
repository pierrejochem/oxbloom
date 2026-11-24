//! Validation of an OpenID Connect id token.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{DecodingKey, Validation};
use oxbloom_http_client::HttpClient;
use serde_json::{Map, Value};

use super::metadata::ASYMMETRIC;
use super::settings::is_acceptable_endpoint;
use super::{ClientRegistration, OAuth2Error, ProviderMetadata, constant_time_eq};
use crate::{JwksSource, ReqwestJwksFetcher};

/// The tolerance for `exp`, `iat` and `nbf`, in seconds.
const LEEWAY_SECS: u64 = 60;

/// The claims of a validated id token.
///
/// `Debug` shows the subject and the claim names, never the values.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{ClientAuthentication, ClientRegistration, ProviderConfig};
/// use oxbloom_security::__test_support::{IdTokenValidator, ProviderMetadata, TokenEndpoint};
/// use oxbloom_test::{FakeProvider, FakeUser};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// # let fake = FakeProvider::new("https://idp.example.test");
/// # let provider = ProviderConfig { issuer_uri: Some("https://idp.example.test".into()), ..ProviderConfig::default() };
/// # let metadata = ProviderMetadata::resolve(&provider, fake.client().as_ref(), SystemTime::now()).await?;
/// # let registration = ClientRegistration {
/// #     id: "idp".into(), client_id: "app".into(), client_secret: Some(Secret::new("s".into())),
/// #     scopes: vec!["openid".into()], redirect_uri: "https://app.example/cb".into(),
/// #     client_authentication: ClientAuthentication::Basic, provider: "idp".into(),
/// # };
/// # fake.expect_nonce("n-1");
/// # let code = fake.issue_code(FakeUser::new("alice").email("alice@example.test"));
/// # let tokens = TokenEndpoint::new(fake.client())
/// #     .exchange_code(&registration, &metadata, &code, "https://app.example/cb", "v").await?;
/// # let id_token = tokens.id_token.unwrap();
/// let claims = IdTokenValidator::new(fake.client())
///     .validate(id_token.expose(), &registration, &metadata, "n-1", SystemTime::now())
///     .await?;
/// assert_eq!(claims.subject(), "alice");
/// assert_eq!(claims.claims()["email"], "alice@example.test");
/// # Ok::<(), oxbloom_security::__test_support::OAuth2Error>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct IdTokenClaims {
    subject: String,
    claims: Map<String, Value>,
}

impl IdTokenClaims {
    /// The `sub` claim.
    ///
    /// # Examples
    ///
    /// See [`IdTokenClaims`].
    ///
    /// ```
    /// # fn show(claims: &oxbloom_security::__test_support::IdTokenClaims) {
    /// println!("signed in: {}", claims.subject());
    /// # }
    /// ```
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Every claim of the token.
    ///
    /// # Examples
    ///
    /// See [`IdTokenClaims`].
    ///
    /// ```
    /// # fn show(claims: &oxbloom_security::__test_support::IdTokenClaims) {
    /// let email = claims.claims().get("email").and_then(|v| v.as_str());
    /// # let _ = email;
    /// # }
    /// ```
    #[must_use]
    pub fn claims(&self) -> &Map<String, Value> {
        &self.claims
    }

    /// The claims, consumed.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn take(claims: oxbloom_security::__test_support::IdTokenClaims) {
    /// let attributes: serde_json::Map<String, serde_json::Value> = claims.into_claims();
    /// # let _ = attributes;
    /// # }
    /// ```
    #[must_use]
    pub fn into_claims(self) -> Map<String, Value> {
        self.claims
    }
}

impl fmt::Debug for IdTokenClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdTokenClaims")
            .field("subject", &self.subject)
            .field("claims", &self.claims.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Validates id tokens, with the signing keys loaded from the provider's JWKS through an
/// [`HttpClient`] and cached per key set URI (see [`JwksSource`]).
///
/// # Examples
///
/// ```
/// use oxbloom_security::__test_support::IdTokenValidator;
/// use oxbloom_test::FakeProvider;
///
/// let fake = FakeProvider::new("https://idp.example.test");
/// let validator = IdTokenValidator::new(fake.client());
/// # let _ = validator;
/// ```
pub struct IdTokenValidator {
    client: Arc<dyn HttpClient>,
    sources: Mutex<HashMap<String, Arc<JwksSource>>>,
}

impl IdTokenValidator {
    /// A validator that loads key sets through `client`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::__test_support::IdTokenValidator;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let validator = IdTokenValidator::new(Arc::new(MockHttpClient::new()));
    /// # let _ = validator;
    /// ```
    #[must_use]
    pub fn new(client: Arc<dyn HttpClient>) -> IdTokenValidator {
        IdTokenValidator {
            client,
            sources: Mutex::new(HashMap::new()),
        }
    }

    /// Validates `id_token` for `registration` at the time `now` and returns its claims.
    ///
    /// The header `alg` must be one of [`ProviderMetadata::id_token_algorithms`] and one of
    /// `RS256`, `RS384`, `RS512`, `ES256` and `ES384` (checked again here, so a hand-built
    /// metadata cannot widen the list), so `none` and HMAC are refused whatever the header
    /// says. The key set URI is checked again as well: `https`, or `http` for a loopback host. The key is the one with the header's `kid` in the
    /// provider's key set, and the signature must verify with it. Then: `iss` equals the
    /// issuer exactly (for a provider without an issuer it must be present); `aud` contains
    /// the client id; `azp`, when present and always when `aud` has several entries, equals
    /// the client id; `exp` is not more than 60 seconds in the past; `iat`, and `nbf` when
    /// present, are not more than 60 seconds in the future; `nonce` equals `nonce`, compared
    /// in constant time; `sub` is a non-empty string.
    ///
    /// # Errors
    ///
    /// [`OAuth2Error::InvalidIdToken`] for every failure, including a provider without a key
    /// set and a key set that cannot be loaded.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{ClientAuthentication, ClientRegistration, ProviderConfig};
    /// use oxbloom_security::__test_support::{
    ///     IdTokenValidator, OAuth2Error, ProviderMetadata, TokenEndpoint,
    /// };
    /// use oxbloom_test::{FakeProvider, FakeUser};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// let provider = ProviderConfig {
    ///     issuer_uri: Some("https://idp.example.test".into()),
    ///     ..ProviderConfig::default()
    /// };
    /// let metadata = ProviderMetadata::resolve(&provider, fake.client().as_ref(), SystemTime::now()).await?;
    /// let registration = ClientRegistration {
    ///     id: "idp".into(),
    ///     client_id: "app".into(),
    ///     client_secret: Some(Secret::new("app-secret".into())),
    ///     scopes: vec!["openid".into()],
    ///     redirect_uri: "https://app.example/cb".into(),
    ///     client_authentication: ClientAuthentication::Basic,
    ///     provider: "idp".into(),
    /// };
    /// fake.expect_nonce("the-nonce-of-the-login");
    /// let code = fake.issue_code(FakeUser::new("alice"));
    /// let tokens = TokenEndpoint::new(fake.client())
    ///     .exchange_code(&registration, &metadata, &code, "https://app.example/cb", "v")
    ///     .await?;
    /// let id_token = tokens.id_token.ok_or(OAuth2Error::Token)?;
    ///
    /// let validator = IdTokenValidator::new(fake.client());
    /// let now = SystemTime::now();
    /// // Another nonce than the one of the login is refused.
    /// let err = validator
    ///     .validate(id_token.expose(), &registration, &metadata, "another-nonce", now)
    ///     .await
    ///     .unwrap_err();
    /// assert_eq!(err, OAuth2Error::InvalidIdToken);
    /// let claims = validator
    ///     .validate(id_token.expose(), &registration, &metadata, "the-nonce-of-the-login", now)
    ///     .await?;
    /// assert_eq!(claims.subject(), "alice");
    /// # Ok::<(), OAuth2Error>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn validate(
        &self,
        id_token: &str,
        registration: &ClientRegistration,
        metadata: &ProviderMetadata,
        nonce: &str,
        now: SystemTime,
    ) -> Result<IdTokenClaims, OAuth2Error> {
        const INVALID: OAuth2Error = OAuth2Error::InvalidIdToken;
        let header = jsonwebtoken::decode_header(id_token).map_err(|_| INVALID)?;
        if !metadata.id_token_algorithms.contains(&header.alg)
            || !ASYMMETRIC.contains(&header.alg)
            || header.crit.is_some()
        {
            return Err(INVALID);
        }
        let kid = header.kid.as_deref().ok_or(INVALID)?;
        let jwks_uri = metadata.jwks_uri.as_deref().ok_or(INVALID)?;
        let source = self.source(jwks_uri)?;
        let jwk = source
            .key_for(kid, &[header.alg])
            .await
            .map_err(|_| INVALID)?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| INVALID)?;
        // `jsonwebtoken` checks the signature only; the claims are checked below.
        let mut validation = Validation::new(header.alg);
        validation.required_spec_claims = HashSet::new();
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        let claims = jsonwebtoken::decode::<Map<String, Value>>(id_token, &key, &validation)
            .map_err(|_| INVALID)?
            .claims;
        check_claims(&claims, registration, metadata, nonce, now)?;
        let subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(INVALID)?
            .to_owned();
        Ok(IdTokenClaims { subject, claims })
    }

    /// The cached key source for `jwks_uri`, built on first use.
    fn source(&self, jwks_uri: &str) -> Result<Arc<JwksSource>, OAuth2Error> {
        let mut sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(source) = sources.get(jwks_uri) {
            return Ok(source.clone());
        }
        // The metadata checks this too; a hand-built `ProviderMetadata` must not skip it.
        if !is_acceptable_endpoint(jwks_uri, true) {
            return Err(OAuth2Error::InvalidIdToken);
        }
        let fetcher = Arc::new(ReqwestJwksFetcher::with_client(self.client.clone()));
        let mut builder = JwksSource::builder(fetcher, jwks_uri);
        // `is_acceptable_endpoint` only lets `http` through for a loopback host.
        if jwks_uri
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("http://"))
        {
            builder = builder.allow_insecure_http();
        }
        let source = builder.build().map_err(|_| OAuth2Error::InvalidIdToken)?;
        sources.insert(jwks_uri.to_owned(), source.clone());
        Ok(source)
    }
}

impl fmt::Debug for IdTokenValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self
            .sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        f.debug_struct("IdTokenValidator")
            .field("key_sets", &count)
            .finish_non_exhaustive()
    }
}

/// Checks `iss`, `aud`, `azp`, the times and the nonce.
fn check_claims(
    claims: &Map<String, Value>,
    registration: &ClientRegistration,
    metadata: &ProviderMetadata,
    nonce: &str,
    now: SystemTime,
) -> Result<(), OAuth2Error> {
    const INVALID: OAuth2Error = OAuth2Error::InvalidIdToken;
    let client_id = registration.client_id.as_str();

    let iss = claims.get("iss").and_then(Value::as_str).ok_or(INVALID)?;
    match &metadata.issuer {
        Some(issuer) if iss != issuer => return Err(INVALID),
        None if iss.is_empty() => return Err(INVALID),
        _ => {}
    }

    let audiences: Vec<&str> = match claims.get("aud") {
        Some(Value::String(one)) => vec![one.as_str()],
        Some(Value::Array(many)) => many
            .iter()
            .map(|v| v.as_str().ok_or(INVALID))
            .collect::<Result<_, _>>()?,
        _ => return Err(INVALID),
    };
    if !audiences.contains(&client_id) {
        return Err(INVALID);
    }
    match claims.get("azp") {
        Some(Value::String(azp)) if azp == client_id => {}
        Some(_) => return Err(INVALID),
        None if audiences.len() > 1 => return Err(INVALID),
        None => {}
    }

    let now = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| INVALID)?
        .as_secs();
    let seconds = |name: &str| claims.get(name).map(|v| v.as_u64().ok_or(INVALID));
    let exp = seconds("exp").ok_or(INVALID)??;
    if exp < now.saturating_sub(LEEWAY_SECS) {
        return Err(INVALID);
    }
    let iat = seconds("iat").ok_or(INVALID)??;
    if iat > now.saturating_add(LEEWAY_SECS) {
        return Err(INVALID);
    }
    if let Some(nbf) = seconds("nbf").transpose()?
        && nbf > now.saturating_add(LEEWAY_SECS)
    {
        return Err(INVALID);
    }

    if !claims
        .get("sub")
        .and_then(Value::as_str)
        .is_some_and(|sub| !sub.is_empty())
    {
        return Err(INVALID);
    }

    let found = claims.get("nonce").and_then(Value::as_str).ok_or(INVALID)?;
    if !constant_time_eq(found, nonce) {
        return Err(INVALID);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use jsonwebtoken::Algorithm;
    use oxbloom_config::Secret;
    use oxbloom_http_client::{HttpClientError, Request, Response};
    use serde_json::json;

    use super::*;
    use crate::ClientAuthentication;

    const NOW: u64 = 1_800_000_000;

    /// Counts the requests and answers none, so a test sees whether the key set was fetched.
    #[derive(Default)]
    struct Counting(AtomicUsize);

    #[async_trait]
    impl HttpClient for Counting {
        async fn execute(&self, _request: Request) -> Result<Response, HttpClientError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(HttpClientError::Network)
        }
    }

    fn registration() -> ClientRegistration {
        ClientRegistration {
            id: "idp".into(),
            client_id: "app".into(),
            client_secret: Some(Secret::new("s".into())),
            scopes: vec!["openid".into()],
            redirect_uri: "https://app.example/cb".into(),
            client_authentication: ClientAuthentication::Basic,
            provider: "idp".into(),
        }
    }

    fn metadata(issuer: Option<&str>) -> ProviderMetadata {
        ProviderMetadata {
            issuer: issuer.map(str::to_owned),
            authorization_endpoint: "https://idp.example.test/authorize".into(),
            token_endpoint: "https://idp.example.test/token".into(),
            userinfo_endpoint: None,
            jwks_uri: Some("https://idp.example.test/jwks".into()),
            id_token_algorithms: vec![Algorithm::RS256, Algorithm::ES256],
            resolved_at: UNIX_EPOCH,
        }
    }

    fn claims(value: Value) -> Map<String, Value> {
        let Value::Object(map) = value else {
            panic!("an object")
        };
        map
    }

    fn valid() -> Map<String, Value> {
        claims(json!({
            "iss": "https://idp.example.test",
            "sub": "alice",
            "aud": "app",
            "exp": NOW + 300,
            "iat": NOW,
            "nonce": "n-1",
        }))
    }

    fn check(claims: &Map<String, Value>, issuer: Option<&str>) -> Result<(), OAuth2Error> {
        check_claims(
            claims,
            &registration(),
            &metadata(issuer),
            "n-1",
            UNIX_EPOCH + Duration::from_secs(NOW),
        )
    }

    fn without(name: &str) -> Map<String, Value> {
        let mut c = valid();
        c.remove(name);
        c
    }

    fn with(name: &str, value: Value) -> Map<String, Value> {
        let mut c = valid();
        c.insert(name.into(), value);
        c
    }

    const ISSUER: Option<&str> = Some("https://idp.example.test");
    const INVALID: Result<(), OAuth2Error> = Err(OAuth2Error::InvalidIdToken);

    #[test]
    fn hand_built_valid_claims_pass() {
        assert_eq!(check(&valid(), ISSUER), Ok(()));
        // One audience with a matching `azp`, and several audiences with it.
        assert_eq!(check(&with("azp", json!("app")), ISSUER), Ok(()));
        let mut several = with("aud", json!(["app", "other"]));
        several.insert("azp".into(), json!("app"));
        assert_eq!(check(&several, ISSUER), Ok(()));
    }

    #[test]
    fn without_a_configured_issuer_any_non_empty_iss_passes() {
        assert_eq!(
            check(&with("iss", json!("https://anyone.example")), None),
            Ok(())
        );
        assert_eq!(check(&with("iss", json!("")), None), INVALID);
        assert_eq!(check(&without("iss"), None), INVALID);
        assert_eq!(check(&with("iss", json!(42)), None), INVALID);
        // With a configured issuer, only that one.
        assert_eq!(
            check(&with("iss", json!("https://anyone.example")), ISSUER),
            INVALID
        );
        assert_eq!(
            check(&with("iss", json!("https://idp.example.test/")), ISSUER),
            INVALID
        );
    }

    #[test]
    fn exp_and_iat_are_required_whole_numbers() {
        assert_eq!(check(&without("exp"), ISSUER), INVALID);
        assert_eq!(check(&without("iat"), ISSUER), INVALID);
        assert_eq!(check(&with("exp", json!("soon")), ISSUER), INVALID);
        assert_eq!(check(&with("iat", json!(-1)), ISSUER), INVALID);
        assert_eq!(check(&with("nbf", json!("x")), ISSUER), INVALID);
    }

    #[test]
    fn the_time_checks_have_a_60_second_leeway() {
        assert_eq!(check(&with("exp", json!(NOW - 60)), ISSUER), Ok(()));
        assert_eq!(check(&with("exp", json!(NOW - 61)), ISSUER), INVALID);
        assert_eq!(check(&with("iat", json!(NOW + 60)), ISSUER), Ok(()));
        assert_eq!(check(&with("iat", json!(NOW + 61)), ISSUER), INVALID);
        assert_eq!(check(&with("nbf", json!(NOW + 60)), ISSUER), Ok(()));
        assert_eq!(check(&with("nbf", json!(NOW + 61)), ISSUER), INVALID);
    }

    #[test]
    fn sub_must_be_a_non_empty_string() {
        assert_eq!(check(&without("sub"), ISSUER), INVALID);
        assert_eq!(check(&with("sub", json!("")), ISSUER), INVALID);
        assert_eq!(check(&with("sub", json!(7)), ISSUER), INVALID);
    }

    #[test]
    fn audience_and_azp_rules() {
        // `azp` present with one audience must still be the client id.
        assert_eq!(check(&with("azp", json!("other")), ISSUER), INVALID);
        assert_eq!(check(&with("azp", json!(1)), ISSUER), INVALID);
        // Several audiences need `azp`.
        assert_eq!(
            check(&with("aud", json!(["app", "other"])), ISSUER),
            INVALID
        );
        assert_eq!(check(&with("aud", json!("other")), ISSUER), INVALID);
        assert_eq!(check(&with("aud", json!(["other", 1])), ISSUER), INVALID);
        assert_eq!(check(&without("aud"), ISSUER), INVALID);
    }

    #[test]
    fn the_nonce_must_match() {
        assert_eq!(check(&without("nonce"), ISSUER), INVALID);
        assert_eq!(check(&with("nonce", json!("n-2")), ISSUER), INVALID);
        assert_eq!(check(&with("nonce", json!("n-1 ")), ISSUER), INVALID);
    }

    /// A token with `header` and a dummy payload and signature; the header is all that is read
    /// before the key lookup.
    fn token_with_header(header: Value) -> String {
        format!(
            "{}.{}.c2ln",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(Value::Object(valid()).to_string())
        )
    }

    async fn validate_header(
        header: Value,
        metadata: &ProviderMetadata,
    ) -> (Result<(), OAuth2Error>, usize) {
        let client = Arc::new(Counting::default());
        let validator = IdTokenValidator::new(client.clone());
        let result = validator
            .validate(
                &token_with_header(header),
                &registration(),
                metadata,
                "n-1",
                UNIX_EPOCH + Duration::from_secs(NOW),
            )
            .await
            .map(|_| ());
        (result, client.0.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn a_token_without_kid_or_with_crit_is_refused_before_any_fetch() {
        let meta = metadata(ISSUER);
        assert_eq!(
            validate_header(json!({"alg": "ES256"}), &meta).await,
            (INVALID, 0)
        );
        assert_eq!(
            validate_header(json!({"alg": "ES256", "kid": "k", "crit": ["exp"]}), &meta).await,
            (INVALID, 0)
        );
        assert_eq!(
            validate_header(json!({"alg": "HS256", "kid": "k"}), &meta).await,
            (INVALID, 0)
        );
        // A hand-built metadata cannot widen the list to HMAC.
        let mut widened = metadata(ISSUER);
        widened.id_token_algorithms.push(Algorithm::HS256);
        assert_eq!(
            validate_header(json!({"alg": "HS256", "kid": "k"}), &widened).await,
            (INVALID, 0)
        );
        // A well-formed header goes on to the key lookup, which fails here.
        assert_eq!(
            validate_header(json!({"alg": "ES256", "kid": "k"}), &meta).await,
            (INVALID, 1)
        );
    }

    #[tokio::test]
    async fn a_hand_built_metadata_with_an_unacceptable_key_set_uri_is_refused() {
        for uri in [
            "http://idp.example.test/jwks",
            "ftp://idp.example.test/jwks",
            "https://idp.example.test/jwks#frag",
            "not a uri",
        ] {
            let mut meta = metadata(ISSUER);
            meta.jwks_uri = Some(uri.into());
            assert_eq!(
                validate_header(json!({"alg": "ES256", "kid": "k"}), &meta).await,
                (INVALID, 0),
                "{uri}"
            );
        }
        // A loopback `http` key set is allowed.
        let mut meta = metadata(ISSUER);
        meta.jwks_uri = Some("http://127.0.0.1:9/jwks".into());
        assert_eq!(
            validate_header(json!({"alg": "ES256", "kid": "k"}), &meta).await,
            (INVALID, 1)
        );
    }
}
