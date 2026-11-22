//! JWT decoding, the JWT authentication provider and claim-to-authority mapping.
//!
//! All signature work is done by `jsonwebtoken`. A decoder is built for one fixed algorithm
//! and one key type; the token's `alg` header is only checked against that allow-list and never
//! selects the key, so algorithm confusion (`none`, or an RSA public key used as an HMAC secret)
//! is rejected.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk};
use jsonwebtoken::{Algorithm, AlgorithmFamily, DecodingKey, Header, Validation};
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use serde_json::{Map, Value};

use crate::{
    Authentication, AuthenticationError, AuthenticationProvider, AuthenticationRequest,
    GrantedAuthority, JwksSource, KeyLookupError,
};

/// A verified JWT: its header, its claims and the raw token.
///
/// `Debug` prints the algorithm and the claim names only, never the token or claim values.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Secret;
/// use oxbloom_security::{Algorithm, Jwt, JwtHeader};
///
/// let mut claims = serde_json::Map::new();
/// claims.insert("sub".into(), serde_json::json!("alice"));
/// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), claims, Secret::new("t".to_string()));
/// assert_eq!(jwt.subject(), Some("alice"));
/// assert!(!format!("{jwt:?}").contains("alice"));
/// ```
#[derive(Clone)]
pub struct Jwt {
    headers: Header,
    claims: Map<String, Value>,
    token: Secret<String>,
}

impl Jwt {
    /// Assembles a JWT from parts; used by [`JwtDecoder`] implementations after verification.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, Jwt, JwtHeader};
    ///
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::RS256), Default::default(), Secret::new("t".into()));
    /// assert!(jwt.claims().is_empty());
    /// ```
    pub fn new(headers: Header, claims: Map<String, Value>, token: Secret<String>) -> Self {
        Self {
            headers,
            claims,
            token,
        }
    }

    /// The JOSE header (`alg`, `kid`, ...).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, Jwt, JwtHeader};
    ///
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::ES256), Default::default(), Secret::new("t".into()));
    /// assert_eq!(jwt.headers().alg, Algorithm::ES256);
    /// ```
    pub fn headers(&self) -> &Header {
        &self.headers
    }

    /// All claims of the token.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, Jwt, JwtHeader};
    ///
    /// let mut claims = serde_json::Map::new();
    /// claims.insert("scope".into(), serde_json::json!("read"));
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), claims, Secret::new("t".into()));
    /// assert_eq!(jwt.claims().len(), 1);
    /// ```
    pub fn claims(&self) -> &Map<String, Value> {
        &self.claims
    }

    /// The raw token, redacted in `Debug` and `Display`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, Jwt, JwtHeader};
    ///
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), Default::default(), Secret::new("raw".into()));
    /// assert_eq!(jwt.token().expose(), "raw");
    /// assert_eq!(jwt.token().to_string(), "[REDACTED]");
    /// ```
    pub fn token(&self) -> &Secret<String> {
        &self.token
    }

    /// One claim by name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, Jwt, JwtHeader};
    ///
    /// let mut claims = serde_json::Map::new();
    /// claims.insert("tenant".into(), serde_json::json!("acme"));
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), claims, Secret::new("t".into()));
    /// assert_eq!(jwt.claim("tenant"), Some(&serde_json::json!("acme")));
    /// assert_eq!(jwt.claim("other"), None);
    /// ```
    pub fn claim(&self, name: &str) -> Option<&Value> {
        self.claims.get(name)
    }

    /// The `sub` claim, if it is a string.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, Jwt, JwtHeader};
    ///
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), Default::default(), Secret::new("t".into()));
    /// assert_eq!(jwt.subject(), None);
    /// ```
    pub fn subject(&self) -> Option<&str> {
        self.claims.get("sub").and_then(Value::as_str)
    }
}

impl fmt::Debug for Jwt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Jwt")
            .field("alg", &self.headers.alg)
            .field("claims", &self.claims.keys().collect::<Vec<_>>())
            .field("token", &self.token)
            .finish()
    }
}

/// Verifies a raw token and returns its [`Jwt`].
///
/// Return [`AuthenticationError::Expired`] for an expired token,
/// [`AuthenticationError::ProviderUnavailable`] when keys cannot be obtained and
/// [`AuthenticationError::InvalidToken`] for everything else.
///
/// # Examples
///
/// ```
/// use async_trait::async_trait;
/// use oxbloom_security::{AuthenticationError, Jwt, JwtDecoder};
///
/// struct RejectAll;
///
/// #[async_trait]
/// impl JwtDecoder for RejectAll {
///     async fn decode(&self, _token: &str) -> Result<Jwt, AuthenticationError> {
///         Err(AuthenticationError::InvalidToken)
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// assert!(RejectAll.decode("x.y.z").await.is_err());
/// # }
/// ```
#[async_trait]
pub trait JwtDecoder: Send + Sync {
    /// Verifies `token` and returns its header and claims.
    async fn decode(&self, token: &str) -> Result<Jwt, AuthenticationError>;
}

/// Why a token was rejected, for logging through [`JwtDecoderBuilder::on_failure`].
///
/// It carries no token text; `Display` is fixed text.
///
/// # Examples
///
/// ```
/// use oxbloom_security::FailureReason;
///
/// assert_eq!(FailureReason::Expired.to_string(), "token expired");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailureReason {
    /// `exp` is in the past (beyond the clock skew).
    Expired,
    /// `nbf` is in the future (beyond the clock skew).
    NotYetValid,
    /// The signature does not verify.
    BadSignature,
    /// `aud` does not contain a configured audience.
    AudienceMismatch,
    /// `iss` is not the configured issuer.
    IssuerMismatch,
    /// No key matches the token's `kid`.
    UnknownKid,
    /// The token is not a well-formed JWT or a required claim is missing or malformed.
    Malformed,
    /// The token's `alg` is not on the decoder's allow-list.
    AlgorithmNotAllowed,
}

impl FailureReason {
    fn to_error(self) -> AuthenticationError {
        match self {
            Self::Expired => AuthenticationError::Expired,
            _ => AuthenticationError::InvalidToken,
        }
    }

    fn from_kind(kind: &ErrorKind) -> Self {
        match kind {
            ErrorKind::ExpiredSignature => Self::Expired,
            ErrorKind::ImmatureSignature => Self::NotYetValid,
            ErrorKind::InvalidSignature
            | ErrorKind::InvalidEcdsaKey
            | ErrorKind::InvalidRsaKey(_)
            | ErrorKind::InvalidKeyFormat => Self::BadSignature,
            ErrorKind::InvalidAudience => Self::AudienceMismatch,
            ErrorKind::InvalidIssuer => Self::IssuerMismatch,
            ErrorKind::InvalidAlgorithm
            | ErrorKind::MissingAlgorithm
            | ErrorKind::UnsupportedAlgorithm
            | ErrorKind::InvalidAlgorithmName => Self::AlgorithmNotAllowed,
            _ => Self::Malformed,
        }
    }
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Expired => "token expired",
            Self::NotYetValid => "token not yet valid",
            Self::BadSignature => "bad signature",
            Self::AudienceMismatch => "audience mismatch",
            Self::IssuerMismatch => "issuer mismatch",
            Self::UnknownKid => "unknown key id",
            Self::Malformed => "malformed token",
            Self::AlgorithmNotAllowed => "algorithm not allowed",
        })
    }
}

/// Why [`JwtDecoderBuilder`] refused a configuration. Fixed text, never key material.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Algorithm, JwtConfigError, JwtDecoderBuilder};
///
/// let err = JwtDecoderBuilder::with_public_key_pem(Algorithm::HS256, "...").unwrap_err();
/// assert_eq!(err, JwtConfigError::AlgorithmKeyMismatch);
/// assert_eq!(err.to_string(), "algorithm does not match the key type");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JwtConfigError {
    /// A symmetric algorithm with a public key, or an asymmetric one with a secret.
    AlgorithmKeyMismatch,
    /// The algorithm is not supported for this key type (e.g. `PS256`, `EdDSA`).
    UnsupportedAlgorithm,
    /// The key could not be parsed, is a private key, or does not fit the algorithm.
    InvalidKey,
    /// The HMAC secret is shorter than the hash output (32, 48 or 64 bytes).
    SecretTooShort,
    /// `audience` was given an empty list.
    EmptyAudience,
    /// [`JwtDecoderBuilder::with_jwks`] was given an empty algorithm list.
    NoAlgorithms,
    /// [`JwtDecoderBuilder::with_jwks`] was given RSA and EC algorithms together.
    MixedAlgorithmFamilies,
    /// [`JwtDecoderBuilder::clock_skew`] is longer than one hour.
    ClockSkewTooLarge,
}

impl fmt::Display for JwtConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AlgorithmKeyMismatch => "algorithm does not match the key type",
            Self::UnsupportedAlgorithm => "unsupported algorithm",
            Self::InvalidKey => "invalid key",
            Self::SecretTooShort => "secret is shorter than the algorithm's hash output",
            Self::EmptyAudience => "empty audience list",
            Self::NoAlgorithms => "empty algorithm list",
            Self::MixedAlgorithmFamilies => "algorithms mix RSA and EC key families",
            Self::ClockSkewTooLarge => "clock skew exceeds one hour",
        })
    }
}

impl std::error::Error for JwtConfigError {}

/// Why a token could not be verified: rejected (with a reason for the hook) or keys unavailable.
enum Failure {
    Rejected(FailureReason),
    Unavailable,
}

impl From<FailureReason> for Failure {
    fn from(reason: FailureReason) -> Self {
        Self::Rejected(reason)
    }
}

/// Where a decoder gets its verification key. Internal so that more sources can be added
/// without changing the public API.
#[derive(Clone)]
enum KeySource {
    /// An HMAC secret (HS256/384/512).
    Secret(DecodingKey),
    /// A static RSA or EC public key.
    PublicKey(DecodingKey),
    /// Keys looked up by `kid` in a cached JWKS.
    Jwks(Arc<JwksSource>),
}

impl KeySource {
    /// The key for a token whose `alg` is already known to be on the allow-list.
    async fn key(&self, header: &Header) -> Result<Cow<'_, DecodingKey>, Failure> {
        match self {
            Self::Secret(key) | Self::PublicKey(key) => Ok(Cow::Borrowed(key)),
            Self::Jwks(source) => {
                let kid = header.kid.as_deref().ok_or(FailureReason::UnknownKid)?;
                // Ask for a key that fits this token's algorithm exactly, so the key's type,
                // curve and `alg` all agree with what the signature check will use.
                let jwk = source
                    .key_for(kid, &[header.alg])
                    .await
                    .map_err(|e| match e {
                        KeyLookupError::UnknownKid => Failure::Rejected(FailureReason::UnknownKid),
                        KeyLookupError::Unavailable => Failure::Unavailable,
                    })?;
                let key = DecodingKey::from_jwk(&jwk).map_err(|_| FailureReason::UnknownKid)?;
                Ok(Cow::Owned(key))
            }
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Secret(_) => "secret",
            Self::PublicKey(_) => "public key",
            Self::Jwks(_) => "jwks",
        }
    }
}

/// Larger skews make little sense and could overflow the timestamp arithmetic.
const MAX_CLOCK_SKEW: Duration = Duration::from_secs(60 * 60);

type FailureHook = Arc<dyn Fn(FailureReason) + Send + Sync>;

/// Configures a [`DefaultJwtDecoder`] for one algorithm and one key.
///
/// Defaults: `exp` required, `nbf` checked when present, clock skew 60 s, no issuer or
/// audience check. When an issuer or audience is configured the claim becomes required.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Secret;
/// use oxbloom_security::{Algorithm, JwtDecoderBuilder};
///
/// let decoder = JwtDecoderBuilder::with_secret(
///     Algorithm::HS256,
///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
/// )
/// .issuer("https://issuer.example")
/// .audience(["my-api"])
/// .build()?;
/// # let _ = decoder;
/// # Ok::<(), oxbloom_security::JwtConfigError>(())
/// ```
pub struct JwtDecoderBuilder {
    algorithms: Vec<Algorithm>,
    key: Result<KeySource, JwtConfigError>,
    issuer: Option<String>,
    audience: Option<Vec<String>>,
    clock_skew: Duration,
    require_exp: bool,
    on_failure: Option<FailureHook>,
    clock: Option<Arc<dyn Clock>>,
}

impl JwtDecoderBuilder {
    fn new(algorithms: Vec<Algorithm>, key: Result<KeySource, JwtConfigError>) -> Self {
        Self {
            algorithms,
            key,
            issuer: None,
            audience: None,
            clock_skew: Duration::from_secs(60),
            require_exp: true,
            on_failure: None,
            clock: None,
        }
    }

    /// Starts a decoder for an HMAC algorithm (HS256, HS384, HS512) with a shared secret.
    ///
    /// Problems are reported by [`build`](Self::build): another algorithm is
    /// [`JwtConfigError::AlgorithmKeyMismatch`], a secret shorter than the hash output is
    /// [`JwtConfigError::SecretTooShort`], and a PEM key passed as a secret is
    /// [`JwtConfigError::InvalidKey`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, JwtConfigError, JwtDecoderBuilder};
    ///
    /// let secret = Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec());
    /// assert!(JwtDecoderBuilder::with_secret(Algorithm::HS256, secret.clone()).build().is_ok());
    /// let err = JwtDecoderBuilder::with_secret(Algorithm::RS256, secret).build().unwrap_err();
    /// assert_eq!(err, JwtConfigError::AlgorithmKeyMismatch);
    /// ```
    pub fn with_secret(algorithm: Algorithm, secret: Secret<Vec<u8>>) -> Self {
        Self::new(vec![algorithm], secret_key(algorithm, secret.expose()))
    }

    /// Starts a decoder for RS256/384/512 or ES256/384 with a PEM-encoded public key
    /// (`-----BEGIN PUBLIC KEY-----`).
    ///
    /// # Errors
    ///
    /// [`JwtConfigError::AlgorithmKeyMismatch`] for an HMAC algorithm,
    /// [`JwtConfigError::UnsupportedAlgorithm`] for PS256/384/512 and EdDSA, and
    /// [`JwtConfigError::InvalidKey`] for a private key, an unparsable PEM, or a key that does
    /// not fit the algorithm (e.g. an RSA key for ES256, a P-256 key for ES384).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Algorithm, JwtConfigError, JwtDecoderBuilder};
    ///
    /// let err = JwtDecoderBuilder::with_public_key_pem(Algorithm::RS256, "not a pem").unwrap_err();
    /// assert_eq!(err, JwtConfigError::InvalidKey);
    /// ```
    pub fn with_public_key_pem(algorithm: Algorithm, pem: &str) -> Result<Self, JwtConfigError> {
        let key = public_key(algorithm, pem)?;
        Ok(Self::new(vec![algorithm], Ok(KeySource::PublicKey(key))))
    }

    /// Starts a decoder that verifies RS256/384/512 or ES256/384 tokens with keys from a
    /// [`JwksSource`], selected by the token's `kid`.
    ///
    /// `algorithms` is the allow-list; it must be one key family (all RSA or all EC). A token
    /// without `kid`, or whose `kid` names no fitting key, is
    /// [`AuthenticationError::InvalidToken`] ([`FailureReason::UnknownKid`]); when no keys can
    /// be loaded the decoder returns [`AuthenticationError::ProviderUnavailable`] (503).
    ///
    /// Problems are reported by [`build`](Self::build): an empty list is
    /// [`JwtConfigError::NoAlgorithms`], an HMAC algorithm is
    /// [`JwtConfigError::AlgorithmKeyMismatch`], PS256/384/512 and EdDSA are
    /// [`JwtConfigError::UnsupportedAlgorithm`], and RSA mixed with EC is
    /// [`JwtConfigError::MixedAlgorithmFamilies`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// # use async_trait::async_trait;
    /// # use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::{Algorithm, JwksSource, JwtConfigError, JwtDecoderBuilder};
    /// # struct Issuer;
    /// # #[async_trait]
    /// # impl oxbloom_security::JwksFetcher for Issuer {
    /// #     async fn fetch(&self, _: &str) -> Result<JwkSet, oxbloom_security::FetchError> {
    /// #         Ok(JwkSet { keys: Vec::new() })
    /// #     }
    /// # }
    ///
    /// let source = JwksSource::builder(Arc::new(Issuer), "https://issuer.example/jwks").build()?;
    /// let decoder = JwtDecoderBuilder::with_jwks(source.clone(), vec![Algorithm::RS256, Algorithm::RS512])
    ///     .issuer("https://issuer.example")
    ///     .build();
    /// assert!(decoder.is_ok());
    ///
    /// let err = JwtDecoderBuilder::with_jwks(source, vec![Algorithm::RS256, Algorithm::ES256])
    ///     .build()
    ///     .unwrap_err();
    /// assert_eq!(err, JwtConfigError::MixedAlgorithmFamilies);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_jwks(source: Arc<JwksSource>, algorithms: Vec<Algorithm>) -> Self {
        let key = jwks_algorithms(&algorithms).map(|()| KeySource::Jwks(source));
        Self::new(algorithms, key)
    }

    /// Requires `iss` to equal `issuer`. Tokens without `iss` are then rejected.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, JwtDecoderBuilder};
    ///
    /// let decoder = JwtDecoderBuilder::with_secret(
    ///     Algorithm::HS256,
    ///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
    /// )
    /// .issuer("https://issuer.example")
    /// .build();
    /// assert!(decoder.is_ok());
    /// ```
    pub fn issuer(mut self, issuer: &str) -> Self {
        self.issuer = Some(issuer.to_owned());
        self
    }

    /// Requires `aud` to contain at least one of `audience`. Tokens without `aud` are then
    /// rejected. An empty list is a [`JwtConfigError::EmptyAudience`] at build time.
    ///
    /// Without this call `aud` is not checked.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, JwtDecoderBuilder};
    ///
    /// let decoder = JwtDecoderBuilder::with_secret(
    ///     Algorithm::HS256,
    ///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
    /// )
    /// .audience(["my-api", "my-other-api"])
    /// .build();
    /// assert!(decoder.is_ok());
    /// ```
    pub fn audience(mut self, audience: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.audience = Some(audience.into_iter().map(Into::into).collect());
        self
    }

    /// Tolerance for `exp` and `nbf`, in whole seconds (default 60 s). A token is expired when
    /// `exp < now - skew` and not yet valid when `nbf > now + skew`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, JwtDecoderBuilder};
    ///
    /// let decoder = JwtDecoderBuilder::with_secret(
    ///     Algorithm::HS256,
    ///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
    /// )
    /// .clock_skew(Duration::from_secs(5))
    /// .build();
    /// assert!(decoder.is_ok());
    /// ```
    pub fn clock_skew(mut self, skew: Duration) -> Self {
        self.clock_skew = skew;
        self
    }

    /// Whether `exp` must be present (default `true`). A present `exp` is always checked.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, JwtDecoderBuilder};
    ///
    /// let decoder = JwtDecoderBuilder::with_secret(
    ///     Algorithm::HS256,
    ///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
    /// )
    /// .require_exp(false)
    /// .build();
    /// assert!(decoder.is_ok());
    /// ```
    pub fn require_exp(mut self, require: bool) -> Self {
        self.require_exp = require;
        self
    }

    /// Calls `hook` with the reason for every rejected token, e.g. for logging. The reason never
    /// contains token text.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, JwtDecoder, JwtDecoderBuilder};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let decoder = JwtDecoderBuilder::with_secret(
    ///     Algorithm::HS256,
    ///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
    /// )
    /// .on_failure(Arc::new(|reason| eprintln!("jwt rejected: {reason}")))
    /// .build()?;
    /// assert!(decoder.decode("not-a-jwt").await.is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub fn on_failure(mut self, hook: Arc<dyn Fn(FailureReason) + Send + Sync>) -> Self {
        self.on_failure = Some(hook);
        self
    }

    /// Checks `exp` and `nbf` against `clock` instead of the system time.
    ///
    /// Without this call `jsonwebtoken` checks both claims against the system time. With a
    /// clock the decoder checks them itself with the same arithmetic and the same failure
    /// reasons: a token is [`FailureReason::Expired`] when `exp < now - skew`,
    /// [`FailureReason::NotYetValid`] when `nbf > now + skew`, and
    /// [`FailureReason::Malformed`] when either claim is not a non-negative number. A float
    /// claim is rounded to the nearest second, as `jsonwebtoken` does. A clock earlier than
    /// the Unix epoch makes every token expired.
    ///
    /// One difference is accepted: with a clock the time checks run after the issuer and
    /// audience checks, so a token that fails several checks may report another reason.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::{Duration, SystemTime};
    /// use jsonwebtoken::{EncodingKey, Header, encode};
    /// use oxbloom_config::Secret;
    /// use oxbloom_core::Clock;
    /// use oxbloom_security::{Algorithm, AuthenticationError, JwtDecoder, JwtDecoderBuilder};
    ///
    /// struct InTwoHours;
    ///
    /// impl Clock for InTwoHours {
    ///     fn now(&self) -> SystemTime {
    ///         SystemTime::now() + Duration::from_secs(2 * 60 * 60)
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let secret = b"an-example-secret-of-32-bytes!!!";
    /// let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec()))
    ///     .clock(Arc::new(InTwoHours))
    ///     .build()?;
    /// // The token is valid for one more hour of real time, but the clock is two hours ahead.
    /// let exp = jsonwebtoken::get_current_timestamp() + 3600;
    /// let claims = serde_json::json!({ "sub": "alice", "exp": exp });
    /// let token = encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret)?)?;
    /// assert_eq!(decoder.decode(&token).await.unwrap_err(), AuthenticationError::Expired);
    /// # Ok(())
    /// # }
    /// ```
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Builds the decoder.
    ///
    /// # Errors
    ///
    /// Any [`JwtConfigError`] recorded by [`with_secret`](Self::with_secret) or
    /// [`with_jwks`](Self::with_jwks), [`JwtConfigError::EmptyAudience`], and
    /// [`JwtConfigError::ClockSkewTooLarge`] for a clock skew above one hour.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, JwtConfigError, JwtDecoderBuilder};
    ///
    /// let err = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(b"short".to_vec()))
    ///     .build()
    ///     .unwrap_err();
    /// assert_eq!(err, JwtConfigError::SecretTooShort);
    /// ```
    pub fn build(self) -> Result<DefaultJwtDecoder, JwtConfigError> {
        let keys = self.key?;
        if self.clock_skew > MAX_CLOCK_SKEW {
            return Err(JwtConfigError::ClockSkewTooLarge);
        }
        let mut validation = Validation::new(self.algorithms[0]);
        validation.algorithms = self.algorithms;
        validation.leeway = self.clock_skew.as_secs();
        // With a clock the decoder checks `exp` and `nbf` itself in `check_time`.
        validation.validate_exp = self.clock.is_none();
        validation.validate_nbf = self.clock.is_none();
        let mut required = Vec::new();
        if self.require_exp {
            required.push("exp");
        }
        if let Some(issuer) = &self.issuer {
            validation.set_issuer(&[issuer]);
            required.push("iss");
        }
        match &self.audience {
            Some(audience) if audience.is_empty() => return Err(JwtConfigError::EmptyAudience),
            Some(audience) => {
                validation.set_audience(audience);
                validation.validate_aud = true;
                required.push("aud");
            }
            None => validation.validate_aud = false,
        }
        validation.set_required_spec_claims(&required);
        Ok(DefaultJwtDecoder {
            keys,
            validation,
            on_failure: self.on_failure,
            clock: self.clock,
        })
    }
}

impl fmt::Debug for JwtDecoderBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtDecoderBuilder")
            .field("algorithms", &self.algorithms)
            .field("key", &self.key.as_ref().map(KeySource::kind))
            .field("clock_skew", &self.clock_skew)
            .field("require_exp", &self.require_exp)
            .finish_non_exhaustive()
    }
}

/// Checks a JWKS allow-list: non-empty, RSA or EC only, one family.
fn jwks_algorithms(algorithms: &[Algorithm]) -> Result<(), JwtConfigError> {
    let mut family = None;
    for alg in algorithms {
        let this = match alg {
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 => AlgorithmFamily::Rsa,
            Algorithm::ES256 | Algorithm::ES384 => AlgorithmFamily::Ec,
            Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
                return Err(JwtConfigError::AlgorithmKeyMismatch);
            }
            _ => return Err(JwtConfigError::UnsupportedAlgorithm),
        };
        if family.is_some_and(|f| f != this) {
            return Err(JwtConfigError::MixedAlgorithmFamilies);
        }
        family = Some(this);
    }
    match family {
        Some(_) => Ok(()),
        None => Err(JwtConfigError::NoAlgorithms),
    }
}

fn secret_key(algorithm: Algorithm, secret: &[u8]) -> Result<KeySource, JwtConfigError> {
    let min_len = match algorithm {
        Algorithm::HS256 => 32,
        Algorithm::HS384 => 48,
        Algorithm::HS512 => 64,
        _ => return Err(JwtConfigError::AlgorithmKeyMismatch),
    };
    if secret.windows(10).any(|w| w == b"-----BEGIN") {
        return Err(JwtConfigError::InvalidKey);
    }
    if secret.len() < min_len {
        return Err(JwtConfigError::SecretTooShort);
    }
    Ok(KeySource::Secret(
        DecodingKey::from_secret(secret).map_err(|_| JwtConfigError::InvalidKey)?,
    ))
}

fn public_key(algorithm: Algorithm, pem: &str) -> Result<DecodingKey, JwtConfigError> {
    let family = match algorithm {
        Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 => AlgorithmFamily::Rsa,
        Algorithm::ES256 | Algorithm::ES384 => AlgorithmFamily::Ec,
        Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
            return Err(JwtConfigError::AlgorithmKeyMismatch);
        }
        _ => return Err(JwtConfigError::UnsupportedAlgorithm),
    };
    if pem.contains("PRIVATE KEY") {
        return Err(JwtConfigError::InvalidKey);
    }
    let key = match family {
        AlgorithmFamily::Rsa => DecodingKey::from_rsa_pem(pem.as_bytes()),
        _ => DecodingKey::from_ec_pem(pem.as_bytes()),
    }
    .map_err(|_| JwtConfigError::InvalidKey)?;
    // Parse the key now so a bad key fails at startup, and check the EC curve fits the
    // algorithm.
    let jwk =
        Jwk::from_decoding_key(&key, Some(algorithm)).map_err(|_| JwtConfigError::InvalidKey)?;
    let fits = match (&jwk.algorithm, algorithm) {
        (AlgorithmParameters::RSA(_), _) => family == AlgorithmFamily::Rsa,
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES256) => {
            ec.curve == EllipticCurve::P256
        }
        (AlgorithmParameters::EllipticCurve(ec), Algorithm::ES384) => {
            ec.curve == EllipticCurve::P384
        }
        _ => false,
    };
    if fits {
        Ok(key)
    } else {
        Err(JwtConfigError::InvalidKey)
    }
}

/// The standard [`JwtDecoder`], built by [`JwtDecoderBuilder`].
///
/// # Examples
///
/// ```
/// use jsonwebtoken::{EncodingKey, Header, encode};
/// use oxbloom_config::Secret;
/// use oxbloom_security::{Algorithm, JwtDecoder, JwtDecoderBuilder};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let secret = b"an-example-secret-of-32-bytes!!!";
/// let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec()))
///     .build()?;
/// let exp = jsonwebtoken::get_current_timestamp() + 300;
/// let claims = serde_json::json!({ "sub": "alice", "exp": exp });
/// let token = encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret)?)?;
/// let jwt = decoder.decode(&token).await?;
/// assert_eq!(jwt.subject(), Some("alice"));
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct DefaultJwtDecoder {
    keys: KeySource,
    validation: Validation,
    on_failure: Option<FailureHook>,
    /// When set, `exp` and `nbf` are checked against this clock instead of by `jsonwebtoken`.
    clock: Option<Arc<dyn Clock>>,
}

impl DefaultJwtDecoder {
    async fn verify(&self, token: &str) -> Result<Jwt, Failure> {
        let header =
            jsonwebtoken::decode_header(token).map_err(|e| FailureReason::from_kind(e.kind()))?;
        if !self.validation.algorithms.contains(&header.alg) {
            return Err(FailureReason::AlgorithmNotAllowed.into());
        }
        // No JOSE header extensions are supported, so any `crit` entry must be rejected.
        if header.crit.is_some() {
            return Err(FailureReason::Malformed.into());
        }
        let key = self.keys.key(&header).await?;
        let data = jsonwebtoken::decode::<Map<String, Value>>(token, &key, &self.validation)
            .map_err(|e| FailureReason::from_kind(e.kind()))?;
        if let Some(clock) = &self.clock {
            check_time(clock.now(), self.validation.leeway, &data.claims)?;
        }
        Ok(Jwt::new(
            data.header,
            data.claims,
            Secret::new(token.to_owned()),
        ))
    }
}

#[async_trait]
impl JwtDecoder for DefaultJwtDecoder {
    async fn decode(&self, token: &str) -> Result<Jwt, AuthenticationError> {
        self.verify(token).await.map_err(|failure| match failure {
            Failure::Rejected(reason) => {
                if let Some(hook) = &self.on_failure {
                    hook(reason);
                }
                reason.to_error()
            }
            Failure::Unavailable => AuthenticationError::ProviderUnavailable,
        })
    }
}

/// A time claim (`exp` or `nbf`) read the way `jsonwebtoken` reads it.
enum TimeClaim {
    Absent,
    Malformed,
    At(u64),
}

impl TimeClaim {
    /// Accepts a non-negative integer, or a finite non-negative float below `u64::MAX` rounded
    /// to the nearest second. Everything else, including a negative integer, is malformed.
    fn read(claims: &Map<String, Value>, name: &str) -> Self {
        let Some(value) = claims.get(name) else {
            return Self::Absent;
        };
        let Value::Number(number) = value else {
            return Self::Malformed;
        };
        if let Some(secs) = number.as_u64() {
            return Self::At(secs);
        }
        if number.is_f64() {
            let secs = number.as_f64().unwrap_or(f64::NAN);
            if secs.is_finite() && secs >= 0.0 && secs < u64::MAX as f64 {
                // The range check above makes the cast exact up to rounding.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                return Self::At(secs.round() as u64);
            }
        }
        Self::Malformed
    }
}

/// Checks `exp` and `nbf` against `now` with `leeway` seconds of tolerance, mirroring
/// `jsonwebtoken`: expired when `exp < now - leeway`, not yet valid when `nbf > now + leeway`.
/// The arithmetic saturates, so no claim value or clock reading can overflow.
fn check_time(
    now: SystemTime,
    leeway: u64,
    claims: &Map<String, Value>,
) -> Result<(), FailureReason> {
    // Parity with `jsonwebtoken` relies on `reject_tokens_expiring_in_less_than` being 0, its
    // default: `build()` never sets it, so this check needs no such margin.
    let exp = TimeClaim::read(claims, "exp");
    let nbf = TimeClaim::read(claims, "nbf");
    if matches!(exp, TimeClaim::Malformed) || matches!(nbf, TimeClaim::Malformed) {
        return Err(FailureReason::Malformed);
    }
    // A clock before the Unix epoch cannot be compared with a claim; every token is expired.
    let Ok(since_epoch) = now.duration_since(SystemTime::UNIX_EPOCH) else {
        return Err(FailureReason::Expired);
    };
    let now = since_epoch.as_secs();
    if let TimeClaim::At(exp) = exp
        && exp < now.saturating_sub(leeway)
    {
        return Err(FailureReason::Expired);
    }
    if let TimeClaim::At(nbf) = nbf
        && nbf > now.saturating_add(leeway)
    {
        return Err(FailureReason::NotYetValid);
    }
    Ok(())
}

impl fmt::Debug for DefaultJwtDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultJwtDecoder")
            .field("algorithms", &self.validation.algorithms)
            .field("key", &self.keys.kind())
            .field("clock_skew_secs", &self.validation.leeway)
            .finish_non_exhaustive()
    }
}

/// Maps a verified [`Jwt`] to an [`Authentication`].
///
/// The principal is `sub` (a non-empty string; otherwise the token is invalid). By default
/// `scope` (space-delimited string) and `scp` (array) become `SCOPE_<x>` authorities. All claims
/// are attached as [`Authentication::details`].
///
/// # Examples
///
/// ```
/// use oxbloom_config::Secret;
/// use oxbloom_security::{Algorithm, Jwt, JwtAuthenticationConverter, JwtHeader};
///
/// let mut claims = serde_json::Map::new();
/// claims.insert("sub".into(), serde_json::json!("alice"));
/// claims.insert("scope".into(), serde_json::json!("read write"));
/// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), claims, Secret::new("t".into()));
/// let auth = JwtAuthenticationConverter::default().convert(&jwt)?;
/// assert_eq!(auth.principal(), "alice");
/// assert!(auth.has_authority("SCOPE_read"));
/// # Ok::<(), oxbloom_security::AuthenticationError>(())
/// ```
#[derive(Clone, Debug)]
pub struct JwtAuthenticationConverter {
    mappings: Vec<(String, String)>,
}

impl Default for JwtAuthenticationConverter {
    fn default() -> Self {
        Self {
            mappings: vec![
                ("scope".to_owned(), "SCOPE_".to_owned()),
                ("scp".to_owned(), "SCOPE_".to_owned()),
            ],
        }
    }
}

impl JwtAuthenticationConverter {
    /// Also maps `claim` (a space-delimited string or an array of strings) to authorities named
    /// `<prefix><value>`. The default `scope`/`scp` mapping is kept; call it again for more
    /// claims.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, Jwt, JwtAuthenticationConverter, JwtHeader};
    ///
    /// let mut claims = serde_json::Map::new();
    /// claims.insert("sub".into(), serde_json::json!("alice"));
    /// claims.insert("roles".into(), serde_json::json!(["ADMIN"]));
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), claims, Secret::new("t".into()));
    /// let converter = JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_");
    /// assert!(converter.convert(&jwt)?.has_role("ADMIN"));
    /// # Ok::<(), oxbloom_security::AuthenticationError>(())
    /// ```
    pub fn authorities_claim(mut self, claim: &str, prefix: &str) -> Self {
        self.mappings.push((claim.to_owned(), prefix.to_owned()));
        self
    }

    /// Converts a verified token.
    ///
    /// # Errors
    ///
    /// [`AuthenticationError::InvalidToken`] when `sub` is missing, empty or not a string.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{
    ///     Algorithm, AuthenticationError, Jwt, JwtAuthenticationConverter, JwtHeader,
    /// };
    ///
    /// let jwt = Jwt::new(JwtHeader::new(Algorithm::HS256), Default::default(), Secret::new("t".into()));
    /// let err = JwtAuthenticationConverter::default().convert(&jwt).unwrap_err();
    /// assert_eq!(err, AuthenticationError::InvalidToken);
    /// ```
    pub fn convert(&self, jwt: &Jwt) -> Result<Authentication, AuthenticationError> {
        let principal = jwt
            .subject()
            .filter(|s| !s.is_empty())
            .ok_or(AuthenticationError::InvalidToken)?;
        let mut authorities = Vec::new();
        for (claim, prefix) in &self.mappings {
            let values: Vec<&str> = match jwt.claim(claim) {
                Some(Value::String(s)) => s.split_whitespace().collect(),
                Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            authorities.extend(
                values
                    .into_iter()
                    .filter(|v| !v.is_empty())
                    .map(|v| GrantedAuthority::new(format!("{prefix}{v}"))),
            );
        }
        Ok(Authentication::new(principal, authorities).with_details(jwt.claims().clone()))
    }
}

/// Authenticates bearer tokens with a [`JwtDecoder`] and a [`JwtAuthenticationConverter`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{
///     Algorithm, AuthenticationProvider, AuthenticationRequest, JwtAuthenticationConverter,
///     JwtAuthenticationProvider, JwtDecoderBuilder,
/// };
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let decoder = JwtDecoderBuilder::with_secret(
///     Algorithm::HS256,
///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
/// )
/// .build()?;
/// let provider =
///     JwtAuthenticationProvider::new(Arc::new(decoder), JwtAuthenticationConverter::default());
/// let request = AuthenticationRequest::BearerToken(Secret::new("not-a-jwt".to_string()));
/// assert!(provider.supports(&request));
/// assert!(provider.authenticate(&request).await.is_err());
/// # Ok(())
/// # }
/// ```
pub struct JwtAuthenticationProvider {
    decoder: Arc<dyn JwtDecoder>,
    converter: JwtAuthenticationConverter,
}

impl JwtAuthenticationProvider {
    /// Creates a provider from a decoder and a converter.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{
    ///     Algorithm, JwtAuthenticationConverter, JwtAuthenticationProvider, JwtDecoderBuilder,
    /// };
    ///
    /// let decoder = JwtDecoderBuilder::with_secret(
    ///     Algorithm::HS256,
    ///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
    /// )
    /// .build()?;
    /// let _provider =
    ///     JwtAuthenticationProvider::new(Arc::new(decoder), JwtAuthenticationConverter::default());
    /// # Ok::<(), oxbloom_security::JwtConfigError>(())
    /// ```
    pub fn new(decoder: Arc<dyn JwtDecoder>, converter: JwtAuthenticationConverter) -> Self {
        Self { decoder, converter }
    }
}

#[async_trait]
impl AuthenticationProvider for JwtAuthenticationProvider {
    fn supports(&self, req: &AuthenticationRequest) -> bool {
        matches!(req, AuthenticationRequest::BearerToken(_))
    }

    async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        let AuthenticationRequest::BearerToken(token) = req else {
            return Err(AuthenticationError::BadCredentials);
        };
        let jwt = self.decoder.decode(token.expose()).await?;
        self.converter.convert(&jwt)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use http::{HeaderValue, header};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use oxbloom_config::Secret;
    use oxbloom_web::{App, HttpMethod, Request, Route};
    use serde_json::{Value, json};

    use super::*;
    use crate::{
        AuthenticationError, AuthenticationProvider, AuthenticationRequest, HttpSecurity,
        RequestSecurityExt,
    };

    const RSA_PRIVATE: &str = include_str!("../tests/fixtures/rsa_private.pem");
    const RSA_PUBLIC: &str = include_str!("../tests/fixtures/rsa_public.pem");
    const EC_PRIVATE: &str = include_str!("../tests/fixtures/ec_private.pem");
    const EC_PUBLIC: &str = include_str!("../tests/fixtures/ec_public.pem");
    const SECRET: &[u8] = b"test-only-hs256-secret-32-bytes!";
    /// Base64url of `{"alg":"none","typ":"JWT"}`.
    const NONE_HEADER: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Runs `f` with the current second and retries if the second changed meanwhile, so
    /// boundary tests are not flaky.
    async fn at_stable_second<T, F, Fut>(mut f: F) -> T
    where
        F: FnMut(u64) -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        loop {
            let start = now();
            let out = f(start).await;
            if now() == start {
                return out;
            }
        }
    }

    fn claims(extra: Value) -> Value {
        let mut base = json!({ "sub": "alice", "exp": now() + 300 });
        let map = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            if v.is_null() {
                map.remove(k);
            } else {
                map.insert(k.clone(), v.clone());
            }
        }
        base
    }

    fn sign_hs(alg: Algorithm, claims: &Value) -> String {
        encode(
            &Header::new(alg),
            claims,
            &EncodingKey::from_secret(SECRET).unwrap(),
        )
        .unwrap()
    }

    fn sign_rs(alg: Algorithm, claims: &Value) -> String {
        let key = EncodingKey::from_rsa_pem(RSA_PRIVATE.as_bytes()).unwrap();
        encode(&Header::new(alg), claims, &key).unwrap()
    }

    fn sign_es(claims: &Value) -> String {
        let key = EncodingKey::from_ec_pem(EC_PRIVATE.as_bytes()).unwrap();
        encode(&Header::new(Algorithm::ES256), claims, &key).unwrap()
    }

    /// Changes the first signature character. Every bit of the first base64url character is
    /// significant (unlike the last one), so the decoded signature always differs.
    fn flip_signature(token: &str) -> String {
        let at = token.rfind('.').unwrap() + 1;
        let first = token.as_bytes()[at];
        let replacement = if first == b'A' { 'B' } else { 'A' };
        format!("{}{}{}", &token[..at], replacement, &token[at + 1..])
    }

    fn hs_builder() -> JwtDecoderBuilder {
        JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(SECRET.to_vec()))
    }

    fn rs_builder() -> JwtDecoderBuilder {
        JwtDecoderBuilder::with_public_key_pem(Algorithm::RS256, RSA_PUBLIC).unwrap()
    }

    type Reasons = Arc<Mutex<Vec<FailureReason>>>;

    fn recording(builder: JwtDecoderBuilder) -> (DefaultJwtDecoder, Reasons) {
        let reasons: Reasons = Arc::default();
        let sink = reasons.clone();
        let decoder = builder
            .on_failure(Arc::new(move |r| sink.lock().unwrap().push(r)))
            .build()
            .unwrap();
        (decoder, reasons)
    }

    async fn reject(
        builder: JwtDecoderBuilder,
        token: &str,
    ) -> (AuthenticationError, FailureReason) {
        let (decoder, reasons) = recording(builder);
        let err = decoder.decode(token).await.unwrap_err();
        let reasons = reasons.lock().unwrap().clone();
        assert_eq!(reasons.len(), 1, "exactly one failure reported");
        (err, reasons[0])
    }

    fn bearer(token: &str) -> AuthenticationRequest {
        AuthenticationRequest::BearerToken(Secret::new(token.to_string()))
    }

    fn provider(converter: JwtAuthenticationConverter) -> JwtAuthenticationProvider {
        JwtAuthenticationProvider::new(Arc::new(hs_builder().build().unwrap()), converter)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn valid_hs256_token_decodes() {
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "tenant": "acme" })));
        let jwt = hs_builder().build().unwrap().decode(&token).await.unwrap();
        assert_eq!(jwt.subject(), Some("alice"));
        assert_eq!(jwt.claim("tenant"), Some(&json!("acme")));
        assert!(jwt.claim("missing").is_none());
        assert_eq!(jwt.headers().alg, Algorithm::HS256);
        assert_eq!(jwt.token().expose(), &token);
        assert!(jwt.claims().contains_key("exp"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn valid_rs256_and_es256_tokens_decode() {
        let token = sign_rs(Algorithm::RS256, &claims(json!({})));
        let jwt = rs_builder().build().unwrap().decode(&token).await.unwrap();
        assert_eq!(jwt.subject(), Some("alice"));

        let es = JwtDecoderBuilder::with_public_key_pem(Algorithm::ES256, EC_PUBLIC)
            .unwrap()
            .build()
            .unwrap();
        let jwt = es.decode(&sign_es(&claims(json!({})))).await.unwrap();
        assert_eq!(jwt.subject(), Some("alice"));
        assert_eq!(jwt.headers().alg, Algorithm::ES256);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn expired_token_is_expired_error() {
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "exp": now() - 3600 })));
        let (err, reason) = reject(hs_builder(), &token).await;
        assert_eq!(err, AuthenticationError::Expired);
        assert_eq!(reason, FailureReason::Expired);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn nbf_in_future_rejected() {
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "nbf": now() + 3600 })));
        let (err, reason) = reject(hs_builder(), &token).await;
        assert_eq!(err, AuthenticationError::InvalidToken);
        assert_eq!(reason, FailureReason::NotYetValid);
        let ok = sign_hs(Algorithm::HS256, &claims(json!({ "nbf": now() - 10 })));
        assert!(hs_builder().build().unwrap().decode(&ok).await.is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn clock_skew_boundary_both_sides() {
        let decoder = hs_builder().build().unwrap();
        // Default skew is 60 s: exp 61 s ago fails, 59 s ago passes.
        let (late, in_skew) = at_stable_second(|t| {
            let late = sign_hs(Algorithm::HS256, &claims(json!({ "exp": t - 61 })));
            let in_skew = sign_hs(Algorithm::HS256, &claims(json!({ "exp": t - 59 })));
            let decoder = &decoder;
            async move { (decoder.decode(&late).await, decoder.decode(&in_skew).await) }
        })
        .await;
        assert_eq!(late.unwrap_err(), AuthenticationError::Expired);
        assert!(in_skew.is_ok());

        // Mirror for nbf: 61 s in the future fails, 59 s passes.
        let (early, in_skew) = at_stable_second(|t| {
            let early = sign_hs(Algorithm::HS256, &claims(json!({ "nbf": t + 61 })));
            let in_skew = sign_hs(Algorithm::HS256, &claims(json!({ "nbf": t + 59 })));
            let decoder = &decoder;
            async move { (decoder.decode(&early).await, decoder.decode(&in_skew).await) }
        })
        .await;
        assert_eq!(early.unwrap_err(), AuthenticationError::InvalidToken);
        assert!(in_skew.is_ok());

        // A custom skew moves the boundary.
        let strict = hs_builder()
            .clock_skew(Duration::from_secs(10))
            .build()
            .unwrap();
        let (late, in_skew) = at_stable_second(|t| {
            let late = sign_hs(Algorithm::HS256, &claims(json!({ "exp": t - 11 })));
            let in_skew = sign_hs(Algorithm::HS256, &claims(json!({ "exp": t - 9 })));
            let strict = &strict;
            async move { (strict.decode(&late).await, strict.decode(&in_skew).await) }
        })
        .await;
        assert_eq!(late.unwrap_err(), AuthenticationError::Expired);
        assert!(in_skew.is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_exp_rejected_by_default_and_allowed_when_opted_out() {
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "exp": null })));
        let (err, reason) = reject(hs_builder(), &token).await;
        assert_eq!(err, AuthenticationError::InvalidToken);
        assert_eq!(reason, FailureReason::Malformed);

        let lenient = hs_builder().require_exp(false).build().unwrap();
        assert!(lenient.decode(&token).await.is_ok());
        // Opting out of requiring exp still validates a present exp.
        let expired = sign_hs(Algorithm::HS256, &claims(json!({ "exp": now() - 3600 })));
        assert_eq!(
            lenient.decode(&expired).await.unwrap_err(),
            AuthenticationError::Expired
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wrong_issuer_and_wrong_audience_rejected() {
        let strict = || {
            hs_builder()
                .issuer("https://issuer.example")
                .audience(["my-api"])
        };
        let good = json!({ "iss": "https://issuer.example", "aud": "my-api" });
        let token = sign_hs(Algorithm::HS256, &claims(good));
        assert!(strict().build().unwrap().decode(&token).await.is_ok());
        let multi = json!({ "iss": "https://issuer.example", "aud": ["other", "my-api"] });
        let token = sign_hs(Algorithm::HS256, &claims(multi));
        assert!(strict().build().unwrap().decode(&token).await.is_ok());

        let cases = [
            (
                json!({ "iss": "https://evil.example", "aud": "my-api" }),
                FailureReason::IssuerMismatch,
            ),
            (
                json!({ "iss": "https://issuer.example", "aud": "other" }),
                FailureReason::AudienceMismatch,
            ),
            (json!({ "aud": "my-api" }), FailureReason::Malformed),
            (
                json!({ "iss": "https://issuer.example" }),
                FailureReason::Malformed,
            ),
        ];
        for (extra, expected) in cases {
            let token = sign_hs(Algorithm::HS256, &claims(extra));
            let (err, reason) = reject(strict(), &token).await;
            assert_eq!(err, AuthenticationError::InvalidToken);
            assert_eq!(reason, expected);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tampered_payload_is_bad_signature() {
        let original = sign_hs(Algorithm::HS256, &claims(json!({})));
        let other = sign_hs(Algorithm::HS256, &claims(json!({ "sub": "mallory" })));
        let parts: Vec<&str> = original.split('.').collect();
        let other_payload = other.split('.').nth(1).unwrap();
        let tampered = format!("{}.{}.{}", parts[0], other_payload, parts[2]);
        let (err, reason) = reject(hs_builder(), &tampered).await;
        assert_eq!(err, AuthenticationError::InvalidToken);
        assert_eq!(reason, FailureReason::BadSignature);

        let tampered = flip_signature(&sign_rs(Algorithm::RS256, &claims(json!({}))));
        let (err, reason) = reject(rs_builder(), &tampered).await;
        assert_eq!(err, AuthenticationError::InvalidToken);
        assert_eq!(reason, FailureReason::BadSignature);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn alg_none_rejected() {
        let payload = sign_hs(Algorithm::HS256, &claims(json!({})))
            .split('.')
            .nth(1)
            .unwrap()
            .to_string();
        let real_sig = sign_hs(Algorithm::HS256, &claims(json!({})))
            .rsplit('.')
            .next()
            .unwrap()
            .to_string();
        for token in [
            format!("{NONE_HEADER}.{payload}."),
            format!("{NONE_HEADER}.{payload}.{real_sig}"),
        ] {
            for builder in [hs_builder(), rs_builder()] {
                let (err, reason) = reject(builder, &token).await;
                assert_eq!(err, AuthenticationError::InvalidToken);
                assert!(
                    matches!(
                        reason,
                        FailureReason::Malformed | FailureReason::AlgorithmNotAllowed
                    ),
                    "{reason:?}"
                );
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn crit_header_is_rejected() {
        // No header extensions are understood, so any `crit` must be rejected (RFC 7515 4.1.11).
        let mut header = Header::new(Algorithm::HS256);
        header.crit = Some(vec!["exp".to_string()]);
        let token = encode(
            &header,
            &claims(json!({})),
            &EncodingKey::from_secret(SECRET).unwrap(),
        )
        .unwrap();
        let (err, reason) = reject(hs_builder(), &token).await;
        assert_eq!(err, AuthenticationError::InvalidToken);
        assert_eq!(reason, FailureReason::Malformed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn token_alg_outside_allow_list_rejected() {
        let hs512 = sign_hs(Algorithm::HS512, &claims(json!({})));
        let (err, reason) = reject(hs_builder(), &hs512).await;
        assert_eq!(err, AuthenticationError::InvalidToken);
        assert_eq!(reason, FailureReason::AlgorithmNotAllowed);

        let rs512 = sign_rs(Algorithm::RS512, &claims(json!({})));
        let (_, reason) = reject(rs_builder(), &rs512).await;
        assert_eq!(reason, FailureReason::AlgorithmNotAllowed);

        let rs256 = sign_rs(Algorithm::RS256, &claims(json!({})));
        let (_, reason) = reject(hs_builder(), &rs256).await;
        assert_eq!(reason, FailureReason::AlgorithmNotAllowed);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rs256_public_key_as_hs256_secret_is_rejected() {
        // Classic algorithm confusion: sign HS256 with the RSA public key PEM as the secret.
        let forged = encode(
            &Header::new(Algorithm::HS256),
            &claims(json!({})),
            &EncodingKey::from_secret(RSA_PUBLIC.as_bytes()).unwrap(),
        )
        .unwrap();
        let (err, reason) = reject(rs_builder(), &forged).await;
        assert_eq!(err, AuthenticationError::InvalidToken);
        assert_eq!(reason, FailureReason::AlgorithmNotAllowed);
        let text = format!("{err} {err:?} {reason:?}");
        assert!(!text.contains(&forged));
        assert!(!text.contains("BEGIN PUBLIC KEY"));

        // A PEM key can also not be configured as an HMAC secret.
        let err = JwtDecoderBuilder::with_secret(
            Algorithm::HS256,
            Secret::new(RSA_PUBLIC.as_bytes().to_vec()),
        )
        .build()
        .unwrap_err();
        assert_eq!(err, JwtConfigError::InvalidKey);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn error_and_debug_never_contain_token_or_key() {
        let secret_claim = "very-secret-claim-value";
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "pii": secret_claim })));
        let secret_text = std::str::from_utf8(SECRET).unwrap();
        let rsa_body = RSA_PUBLIC.lines().nth(1).unwrap();
        let forbidden = [token.as_str(), secret_text, rsa_body, secret_claim, "alice"];
        let check = |text: String| {
            for f in forbidden {
                assert!(!text.contains(f), "leaked {f:?} in {text:?}");
            }
        };

        let builder = hs_builder();
        check(format!("{builder:?}"));
        let decoder = builder.build().unwrap();
        check(format!("{decoder:?}"));
        let rs = rs_builder();
        check(format!("{rs:?}"));
        check(format!("{:?}", rs.build().unwrap()));

        let jwt = decoder.decode(&token).await.unwrap();
        let dbg = format!("{jwt:?}");
        check(dbg.clone());
        assert!(dbg.contains("pii"), "claim names are fine: {dbg}");

        let expired = sign_hs(
            Algorithm::HS256,
            &claims(json!({ "exp": now() - 3600, "pii": secret_claim })),
        );
        let tampered = format!("{}x", token);
        for bad in [expired.as_str(), tampered.as_str(), "not-a-jwt", "a.b.c"] {
            let (err, reason) = reject(hs_builder(), bad).await;
            check(format!("{err} {err:?} {reason:?}"));
            assert!(!format!("{err:?}").contains(bad));
        }

        let errors = [
            JwtDecoderBuilder::with_public_key_pem(Algorithm::ES256, RSA_PUBLIC).unwrap_err(),
            JwtDecoderBuilder::with_public_key_pem(Algorithm::RS256, RSA_PRIVATE).unwrap_err(),
            JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(b"short".to_vec()))
                .build()
                .unwrap_err(),
        ];
        for e in errors {
            check(format!("{e} {e:?}"));
            assert!(!format!("{e} {e:?}").contains("PRIVATE"));
            assert!(!format!("{e:?}").contains("short"));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn on_failure_hook_receives_reason_without_token() {
        let (decoder, reasons) = recording(hs_builder());
        let good = sign_hs(Algorithm::HS256, &claims(json!({})));
        decoder.decode(&good).await.unwrap();
        assert!(reasons.lock().unwrap().is_empty(), "no report on success");

        let tampered = flip_signature(&good);
        assert!(decoder.decode(&tampered).await.is_err());
        let seen = reasons.lock().unwrap().clone();
        assert_eq!(seen, [FailureReason::BadSignature]);
        assert!(!format!("{:?}", seen[0]).contains(&tampered));
        assert_eq!(FailureReason::BadSignature.to_string(), "bad signature");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_sub_is_invalid() {
        let p = provider(JwtAuthenticationConverter::default());
        for extra in [
            json!({ "sub": null }),
            json!({ "sub": "" }),
            json!({ "sub": 42 }),
        ] {
            let token = sign_hs(Algorithm::HS256, &claims(extra));
            assert_eq!(
                p.authenticate(&bearer(&token)).await.unwrap_err(),
                AuthenticationError::InvalidToken
            );
        }
        let token = sign_hs(Algorithm::HS256, &claims(json!({})));
        let auth = p.authenticate(&bearer(&token)).await.unwrap();
        assert_eq!(auth.principal(), "alice");
        assert!(p.supports(&bearer(&token)));
        assert_eq!(
            p.authenticate(&bearer("garbage")).await.unwrap_err(),
            AuthenticationError::InvalidToken
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn scope_string_maps_to_scope_authorities() {
        let p = provider(JwtAuthenticationConverter::default());
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "scope": "read  write" })));
        let auth = p.authenticate(&bearer(&token)).await.unwrap();
        let names: Vec<&str> = auth.authorities().iter().map(|a| a.as_str()).collect();
        assert_eq!(names, ["SCOPE_read", "SCOPE_write"]);
        assert_eq!(auth.details()["scope"], "read  write");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn scp_array_maps_to_scope_authorities() {
        let p = provider(JwtAuthenticationConverter::default());
        let token = sign_hs(
            Algorithm::HS256,
            &claims(json!({ "scp": ["read", "admin", 7] })),
        );
        let auth = p.authenticate(&bearer(&token)).await.unwrap();
        let names: Vec<&str> = auth.authorities().iter().map(|a| a.as_str()).collect();
        assert_eq!(names, ["SCOPE_admin", "SCOPE_read"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn custom_roles_claim_maps_with_prefix() {
        let converter = JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_");
        let p = provider(converter);
        let token = sign_hs(
            Algorithm::HS256,
            &claims(json!({ "roles": ["ADMIN", "USER"], "scope": "read" })),
        );
        let auth = p.authenticate(&bearer(&token)).await.unwrap();
        assert!(auth.has_role("ADMIN"));
        assert!(auth.has_role("USER"));
        assert!(auth.has_authority("SCOPE_read"));
        assert!(!auth.has_authority("ADMIN"));
    }

    #[test]
    fn builder_rejects_asymmetric_alg_with_secret() {
        for alg in [
            Algorithm::RS256,
            Algorithm::ES256,
            Algorithm::PS256,
            Algorithm::EdDSA,
        ] {
            let err = JwtDecoderBuilder::with_secret(alg, Secret::new(SECRET.to_vec()))
                .build()
                .unwrap_err();
            assert_eq!(err, JwtConfigError::AlgorithmKeyMismatch);
        }
        let err = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(b"short".to_vec()))
            .build()
            .unwrap_err();
        assert_eq!(err, JwtConfigError::SecretTooShort);
        let err = JwtDecoderBuilder::with_secret(Algorithm::HS512, Secret::new(SECRET.to_vec()))
            .build()
            .unwrap_err();
        assert_eq!(err, JwtConfigError::SecretTooShort);
        assert_eq!(
            JwtConfigError::AlgorithmKeyMismatch.to_string(),
            "algorithm does not match the key type"
        );
    }

    #[test]
    fn builder_rejects_symmetric_alg_with_public_key() {
        for alg in [Algorithm::HS256, Algorithm::HS384, Algorithm::HS512] {
            let err = JwtDecoderBuilder::with_public_key_pem(alg, RSA_PUBLIC).unwrap_err();
            assert_eq!(err, JwtConfigError::AlgorithmKeyMismatch);
        }
        let unsupported = [Algorithm::PS256, Algorithm::EdDSA];
        for alg in unsupported {
            let err = JwtDecoderBuilder::with_public_key_pem(alg, RSA_PUBLIC).unwrap_err();
            assert_eq!(err, JwtConfigError::UnsupportedAlgorithm);
        }
        let invalid = [
            (Algorithm::ES256, RSA_PUBLIC),
            (Algorithm::ES384, EC_PUBLIC),
            (Algorithm::RS256, EC_PUBLIC),
            (Algorithm::RS256, RSA_PRIVATE),
            (Algorithm::ES256, EC_PRIVATE),
            (Algorithm::RS256, "not a pem"),
        ];
        for (alg, pem) in invalid {
            let err = JwtDecoderBuilder::with_public_key_pem(alg, pem).unwrap_err();
            assert_eq!(err, JwtConfigError::InvalidKey, "{alg:?}");
        }
        let err = hs_builder()
            .audience(Vec::<String>::new())
            .build()
            .unwrap_err();
        assert_eq!(err, JwtConfigError::EmptyAudience);
    }

    #[test]
    fn oversized_clock_skew_is_rejected_at_build() {
        let hour = Duration::from_secs(3600);
        assert!(hs_builder().clock_skew(hour).build().is_ok());
        assert!(hs_builder().clock_skew(Duration::ZERO).build().is_ok());
        for skew in [hour + Duration::from_secs(1), Duration::from_secs(u64::MAX)] {
            let err = hs_builder().clock_skew(skew).build().unwrap_err();
            assert_eq!(err, JwtConfigError::ClockSkewTooLarge);
            let err = rs_builder().clock_skew(skew).build().unwrap_err();
            assert_eq!(err, JwtConfigError::ClockSkewTooLarge);
        }
        assert_eq!(
            JwtConfigError::ClockSkewTooLarge.to_string(),
            "clock skew exceeds one hour"
        );
    }

    struct NoKeys;

    #[async_trait]
    impl crate::JwksFetcher for NoKeys {
        async fn fetch(&self, _uri: &str) -> Result<jsonwebtoken::jwk::JwkSet, crate::FetchError> {
            Err(crate::FetchError::Network)
        }
    }

    fn jwks_builder(algs: Vec<Algorithm>) -> JwtDecoderBuilder {
        let source = crate::JwksSource::builder(Arc::new(NoKeys), "https://issuer.example/jwks")
            .build()
            .unwrap();
        JwtDecoderBuilder::with_jwks(source, algs)
    }

    #[test]
    fn mixed_family_algorithms_rejected_for_jwks() {
        use Algorithm::*;
        for ok in [
            vec![RS256],
            vec![RS256, RS384, RS512],
            vec![ES256],
            vec![ES256, ES384],
        ] {
            assert!(jwks_builder(ok.clone()).build().is_ok(), "{ok:?}");
        }
        let cases = [
            (vec![RS256, ES256], JwtConfigError::MixedAlgorithmFamilies),
            (vec![ES384, RS512], JwtConfigError::MixedAlgorithmFamilies),
            (vec![HS256], JwtConfigError::AlgorithmKeyMismatch),
            (vec![RS256, HS256], JwtConfigError::AlgorithmKeyMismatch),
            (vec![PS256], JwtConfigError::UnsupportedAlgorithm),
            (vec![RS256, EdDSA], JwtConfigError::UnsupportedAlgorithm),
            (vec![], JwtConfigError::NoAlgorithms),
        ];
        for (algs, expected) in cases {
            let err = jwks_builder(algs.clone()).build().unwrap_err();
            assert_eq!(err, expected, "{algs:?}");
        }
        assert_eq!(
            JwtConfigError::MixedAlgorithmFamilies.to_string(),
            "algorithms mix RSA and EC key families"
        );
        assert_eq!(
            JwtConfigError::NoAlgorithms.to_string(),
            "empty algorithm list"
        );
        let builder = jwks_builder(vec![RS256]);
        assert!(format!("{builder:?}").contains("jwks"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oauth2_resource_server_gives_200_401_403() {
        let decoder: Arc<dyn JwtDecoder> = Arc::new(hs_builder().build().unwrap());
        let chain = HttpSecurity::new()
            .oauth2_resource_server(|r| r.jwt(decoder))
            .authorize(|a| {
                a.get("/admin")
                    .has_role("ADMIN")
                    .any_request()
                    .authenticated()
            })
            .build()
            .unwrap();
        let me =
            |req: Request| async move { req.authentication().unwrap().principal().to_string() };
        let app = App::new()
            .filter(chain)
            .route(Route::new(HttpMethod::Get, "/me", me))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/admin", me))
            .unwrap();
        let call = |path: &'static str, token: Option<String>| {
            let mut req = Request::new(HttpMethod::Get, path);
            if let Some(t) = token {
                req.headers.insert(
                    header::AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {t}")).unwrap(),
                );
            }
            app.handle(req)
        };
        let good = sign_hs(Algorithm::HS256, &claims(json!({ "scope": "read" })));
        let res = call("/me", Some(good.clone())).await;
        assert_eq!(res.status(), 200);
        assert_eq!(res.body().as_ref(), b"alice");
        assert_eq!(call("/me", None).await.status(), 401);
        let bad = call("/me", Some(format!("{good}x"))).await;
        assert_eq!(bad.status(), 401);
        assert_eq!(
            bad.headers()[header::WWW_AUTHENTICATE],
            "Bearer error=\"invalid_token\""
        );
        assert_eq!(call("/admin", Some(good)).await.status(), 403);

        let err = HttpSecurity::new()
            .oauth2_resource_server(|r| r.converter(JwtAuthenticationConverter::default()))
            .authorize(|a| a.any_request().authenticated())
            .build()
            .err()
            .unwrap();
        assert_eq!(err, crate::SecurityBuildError::ResourceServerWithoutDecoder);
    }

    /// A clock that always answers the same instant. The crate cannot use `oxbloom-test`.
    struct Fixed(SystemTime);

    impl oxbloom_core::Clock for Fixed {
        fn now(&self) -> SystemTime {
            self.0
        }
    }

    fn at_secs(secs: u64) -> Arc<dyn oxbloom_core::Clock> {
        Arc::new(Fixed(UNIX_EPOCH + Duration::from_secs(secs)))
    }

    /// Decodes `token` and returns the failure reason, or `Ok` when it decodes.
    async fn outcome(builder: JwtDecoderBuilder, token: &str) -> Result<(), FailureReason> {
        let (decoder, reasons) = recording(builder);
        match decoder.decode(token).await {
            Ok(_) => Ok(()),
            Err(_) => Err(reasons.lock().unwrap()[0]),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn without_a_clock_the_decoder_still_validates_with_the_system_time() {
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "exp": now() + 3600 })));
        assert_eq!(outcome(hs_builder(), &token).await, Ok(()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_clock_in_the_future_makes_a_valid_token_expired() {
        let real = now();
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "exp": real + 60 })));
        let builder = hs_builder()
            .clock_skew(Duration::ZERO)
            .clock(at_secs(real + 3600));
        assert_eq!(outcome(builder, &token).await, Err(FailureReason::Expired));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_clock_in_the_past_makes_a_token_not_yet_valid() {
        let real = now();
        let nbf = real + 3600;
        let token = sign_hs(
            Algorithm::HS256,
            &claims(json!({ "nbf": nbf, "exp": real + 7200 })),
        );
        let early = hs_builder().clock_skew(Duration::ZERO).clock(at_secs(real));
        assert_eq!(
            outcome(early, &token).await,
            Err(FailureReason::NotYetValid)
        );
        let on_time = hs_builder().clock_skew(Duration::ZERO).clock(at_secs(nbf));
        assert_eq!(outcome(on_time, &token).await, Ok(()));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_skew_applies_to_both_edges_with_a_clock() {
        let clock = 2_000_000_000;
        let skew = Duration::from_secs(30);
        let with_clock_at = |secs| hs_builder().clock_skew(skew).clock(at_secs(secs));

        // `exp` equal to the clock minus the skew still decodes; a second later it is expired.
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "exp": clock - 30 })));
        assert_eq!(outcome(with_clock_at(clock), &token).await, Ok(()));
        assert_eq!(
            outcome(with_clock_at(clock + 1), &token).await,
            Err(FailureReason::Expired)
        );

        // `nbf` equal to the clock plus the skew decodes; a second earlier it is not yet valid.
        let token = sign_hs(
            Algorithm::HS256,
            &claims(json!({ "nbf": clock + 30, "exp": clock + 3600 })),
        );
        assert_eq!(outcome(with_clock_at(clock), &token).await, Ok(()));
        assert_eq!(
            outcome(with_clock_at(clock - 1), &token).await,
            Err(FailureReason::NotYetValid)
        );
    }

    /// Claim sets around the skew edges, plus malformed and float values, relative to `t`.
    fn boundary_table(t: u64) -> Vec<Value> {
        let t = t as f64;
        vec![
            json!({ "exp": t - 61.0 }),
            json!({ "exp": t - 60.0 }),
            json!({ "exp": t - 59.0 }),
            json!({ "exp": t }),
            json!({ "exp": t - 60.5 }),
            json!({ "exp": t - 60.4 }),
            json!({ "exp": t - 60.6 }),
            json!({ "exp": t + 300.0, "nbf": t + 59.0 }),
            json!({ "exp": t + 300.0, "nbf": t + 60.0 }),
            json!({ "exp": t + 300.0, "nbf": t + 61.0 }),
            json!({ "exp": t + 300.0, "nbf": t + 60.4 }),
            json!({ "exp": t + 300.0, "nbf": t + 60.5 }),
            json!({ "exp": t - 3600.0, "nbf": t + 3600.0 }),
            json!({ "exp": "soon" }),
            json!({ "exp": -5 }),
            json!({ "exp": -5.0 }),
            json!({ "exp": 1e30 }),
            json!({ "exp": null }),
            json!({ "exp": [1] }),
            json!({ "exp": t + 300.0, "nbf": "later" }),
            json!({ "exp": t + 300.0, "nbf": -1 }),
            json!({ "exp": u64::MAX }),
            json!({ "exp": t + 300.0, "nbf": u64::MAX }),
            json!({ "exp": t + 300.0, "nbf": 0 }),
        ]
    }

    /// Signs `extra` over a base without `exp`, so the table controls every time claim.
    fn sign_table_row(extra: &Value) -> String {
        let mut base = json!({ "sub": "alice" });
        let map = base.as_object_mut().unwrap();
        for (k, v) in extra.as_object().unwrap() {
            map.insert(k.clone(), v.clone());
        }
        // The table computes in floats; a whole value is sent as an integer, as issuers do.
        for name in ["exp", "nbf"] {
            if let Some(Value::Number(n)) = map.get(name)
                && let Some(f) = n.as_f64()
                && f.fract() == 0.0
                && (0.0..1e15).contains(&f)
            {
                map.insert(name.to_string(), json!(f as u64));
            }
        }
        sign_hs(Algorithm::HS256, &base)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_clock_path_and_the_system_path_agree_on_the_boundary_table() {
        for require_exp in [true, false] {
            let rows = at_stable_second(|t| async move {
                let mut rows = Vec::new();
                for extra in boundary_table(t) {
                    let token = sign_table_row(&extra);
                    let system = outcome(hs_builder().require_exp(require_exp), &token).await;
                    let clock = outcome(
                        hs_builder().require_exp(require_exp).clock(at_secs(t)),
                        &token,
                    )
                    .await;
                    rows.push((extra, system, clock));
                }
                rows
            })
            .await;
            for (extra, system, clock) in rows {
                assert_eq!(system, clock, "require_exp={require_exp}, claims {extra}");
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_non_numeric_exp_or_nbf_is_malformed_with_a_clock() {
        let real = now();
        for extra in [
            json!({ "exp": "soon" }),
            json!({ "exp": real + 300, "nbf": "later" }),
            json!({ "exp": real + 300, "nbf": -1 }),
        ] {
            let token = sign_table_row(&extra);
            for require_exp in [true, false] {
                let system = outcome(hs_builder().require_exp(require_exp), &token).await;
                let clock = outcome(
                    hs_builder().require_exp(require_exp).clock(at_secs(real)),
                    &token,
                )
                .await;
                assert_eq!(clock, Err(FailureReason::Malformed), "{extra}");
                assert_eq!(system, clock, "{extra}");
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_float_exp_is_read_like_jsonwebtoken_does_with_a_clock() {
        let real = now();
        // `jsonwebtoken` rounds a float claim to the nearest second.
        let token = sign_hs(
            Algorithm::HS256,
            &json!({ "sub": "alice", "exp": real as f64 + 300.25 }),
        );
        let system = outcome(hs_builder(), &token).await;
        let clock = outcome(hs_builder().clock(at_secs(real)), &token).await;
        assert_eq!(clock, Ok(()));
        assert_eq!(system, clock);

        // 0.5 rounds up, so `exp` = clock - 60.5 counts as clock - 60 and still decodes with
        // the default skew of 60 seconds, while clock - 60.6 counts as clock - 61.
        let clock_at = 2_000_000_000u64;
        let edge = sign_hs(
            Algorithm::HS256,
            &json!({ "sub": "alice", "exp": clock_at as f64 - 60.5 }),
        );
        assert_eq!(
            outcome(hs_builder().clock(at_secs(clock_at)), &edge).await,
            Ok(())
        );
        let past = sign_hs(
            Algorithm::HS256,
            &json!({ "sub": "alice", "exp": clock_at as f64 - 60.6 }),
        );
        assert_eq!(
            outcome(hs_builder().clock(at_secs(clock_at)), &past).await,
            Err(FailureReason::Expired)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_clock_before_the_unix_epoch_does_not_panic() {
        let before_epoch: Arc<dyn oxbloom_core::Clock> =
            Arc::new(Fixed(UNIX_EPOCH - Duration::from_secs(1)));
        for exp in [0, now() + 3600, u64::MAX] {
            let token = sign_hs(Algorithm::HS256, &json!({ "sub": "alice", "exp": exp }));
            let builder = hs_builder().clock(before_epoch.clone());
            assert_eq!(
                outcome(builder, &token).await,
                Err(FailureReason::Expired),
                "exp {exp}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_config_function_accepts_a_clock() {
        let config = oxbloom_config::Config::from_str(&format!(
            "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{}\"",
            std::str::from_utf8(SECRET).unwrap()
        ))
        .unwrap();
        let real = now();
        let token = sign_hs(Algorithm::HS256, &claims(json!({ "exp": real + 600 })));

        let system = crate::jwt_decoder_from_config_with_clock(&config, None, None).unwrap();
        assert!(system.decode(&token).await.is_ok());

        let later =
            crate::jwt_decoder_from_config_with_clock(&config, None, Some(at_secs(real + 3600)))
                .unwrap();
        assert_eq!(
            later.decode(&token).await.unwrap_err(),
            AuthenticationError::Expired
        );
    }

    #[test]
    fn the_debug_output_does_not_show_the_clock() {
        let decoder = hs_builder().clock(at_secs(1_234_567)).build().unwrap();
        let text = format!("{decoder:?}");
        assert!(!text.contains("1234567"), "{text}");
        assert!(!text.contains("Fixed"), "{text}");
        let clone = decoder.clone();
        assert_eq!(format!("{clone:?}"), text);
    }
}
