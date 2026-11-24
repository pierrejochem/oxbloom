//! Validation of an OpenID Connect id token.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{DecodingKey, Validation};
use oxbloom_http_client::HttpClient;
use serde_json::{Map, Value};

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
    /// The header `alg` must be one of [`ProviderMetadata::id_token_algorithms`], which only
    /// ever holds `RS256`, `RS384`, `RS512`, `ES256`, `ES384` or `PS256`, so `none` and HMAC
    /// are refused whatever the header says. The key is the one with the header's `kid` in the
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
        if !metadata.id_token_algorithms.contains(&header.alg) || header.crit.is_some() {
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
        let fetcher = Arc::new(ReqwestJwksFetcher::with_client(self.client.clone()));
        let mut builder = JwksSource::builder(fetcher, jwks_uri);
        // The metadata only lets `http` through for a loopback host.
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

    let found = claims.get("nonce").and_then(Value::as_str).ok_or(INVALID)?;
    if !constant_time_eq(found, nonce) {
        return Err(INVALID);
    }
    Ok(())
}
