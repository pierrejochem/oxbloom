//! Builds a [`DefaultJwtDecoder`] from the `security.jwt` configuration section.

use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::Algorithm;
use oxbloom_config::{Config, ConfigError, Origin, Secret};
use oxbloom_core::Clock;
use serde::Deserialize;

use crate::{
    DefaultJwtDecoder, JwksError, JwksFetcher, JwksSource, JwtConfigError, JwtDecoderBuilder,
};

/// The configuration key that holds the HMAC secret, `security.jwt.secret`.
///
/// Register it when building the [`Config`] so a command-line argument cannot carry the
/// secret (arguments show up in process listings). Set it from an environment variable
/// (`APP_SECURITY__JWT__SECRET`) or a file instead.
///
/// # Examples
///
/// ```
/// use oxbloom_config::{Config, ConfigError};
/// use oxbloom_security::SECRET_KEY;
/// # let dir = tempfile::tempdir()?;
///
/// // The secret may come from the environment...
/// let config = Config::builder()
///     .dir(dir.path())
///     .env_from([("APP_SECURITY__JWT__SECRET".to_string(), "s".to_string())])
///     .secret_key(SECRET_KEY)
///     .build()?;
/// assert_eq!(config.get::<String>(SECRET_KEY)?, "s");
///
/// // ...but not from the command line.
/// let rejected = Config::builder()
///     .dir(dir.path())
///     .env_from([])
///     .args(["--security.jwt.secret=x"])
///     .secret_key(SECRET_KEY)
///     .build();
/// assert!(matches!(rejected, Err(ConfigError::Invalid { .. })));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub const SECRET_KEY: &str = "security.jwt.secret";

const SECTION: &str = "security.jwt";
const K_ALGORITHMS: &str = "security.jwt.algorithms";
const K_SECRET: &str = SECRET_KEY;
const K_PEM: &str = "security.jwt.public-key-pem";
const K_JWKS: &str = "security.jwt.jwk-set-uri";
const K_AUDIENCE: &str = "security.jwt.audience";
const K_SKEW: &str = "security.jwt.clock-skew-secs";
const K_BUNDLE: &str = "security.jwt.jwks-ssl-bundle";
const BUNDLE_RULE: &str = "must be the name of an SSL bundle";
const DEFAULT_SKEW_SECS: u64 = 60;
const MAX_SKEW_SECS: u64 = 3600;

/// The `security.jwt` section. Keys match case-insensitively ignoring `-` and `_`, so no
/// aliases are needed for environment variable spellings.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JwtSettings {
    issuer: Option<String>,
    #[serde(rename = "jwk-set-uri")]
    jwk_set_uri: Option<String>,
    algorithms: Option<Vec<String>>,
    audience: Option<Vec<String>>,
    #[serde(rename = "clock-skew-secs")]
    clock_skew_secs: Option<u64>,
    secret: Option<Secret<String>>,
    #[serde(rename = "public-key-pem")]
    public_key_pem: Option<Secret<String>>,
    #[serde(rename = "jwks-ssl-bundle")]
    jwks_ssl_bundle: Option<String>,
}

fn invalid(key: &str, message: &str) -> ConfigError {
    ConfigError::invalid(key, message)
}

fn missing(key: &str) -> ConfigError {
    ConfigError::Missing {
        key: key.to_string(),
    }
}

fn parse_algorithm(name: &str) -> Result<Algorithm, ConfigError> {
    Ok(match name {
        "HS256" => Algorithm::HS256,
        "HS384" => Algorithm::HS384,
        "HS512" => Algorithm::HS512,
        "RS256" => Algorithm::RS256,
        "RS384" => Algorithm::RS384,
        "RS512" => Algorithm::RS512,
        "ES256" => Algorithm::ES256,
        "ES384" => Algorithm::ES384,
        _ => {
            return Err(invalid(
                K_ALGORITHMS,
                "unsupported algorithm; expected one of HS256, HS384, HS512, RS256, RS384, RS512, ES256, ES384",
            ));
        }
    })
}

fn is_symmetric(alg: Algorithm) -> bool {
    matches!(alg, Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512)
}

fn is_ec(alg: Algorithm) -> bool {
    matches!(alg, Algorithm::ES256 | Algorithm::ES384)
}

/// Maps a builder error to the configuration key that caused it. `key_source` is the key
/// naming the secret, PEM or algorithm list the failing builder step used.
fn map_jwt_error(error: JwtConfigError, key_source: &str) -> ConfigError {
    match error {
        JwtConfigError::AlgorithmKeyMismatch => invalid(
            K_ALGORITHMS,
            "algorithm does not match the configured key type",
        ),
        JwtConfigError::UnsupportedAlgorithm => {
            invalid(K_ALGORITHMS, "unsupported algorithm for this key type")
        }
        JwtConfigError::InvalidKey => invalid(key_source, "invalid key"),
        JwtConfigError::SecretTooShort => invalid(
            K_SECRET,
            "secret is shorter than the algorithm's hash output",
        ),
        JwtConfigError::EmptyAudience => invalid(K_AUDIENCE, "audience list is empty"),
        JwtConfigError::NoAlgorithms => missing(K_ALGORITHMS),
        JwtConfigError::MixedAlgorithmFamilies => {
            invalid(K_ALGORITHMS, "algorithms mix RSA and EC key families")
        }
        JwtConfigError::ClockSkewTooLarge => invalid(K_SKEW, "clock skew exceeds one hour"),
    }
}

fn map_jwks_error(error: JwksError) -> ConfigError {
    match error {
        JwksError::InvalidUri => invalid(K_JWKS, "invalid JWKS URI"),
        JwksError::InsecureUri => invalid(K_JWKS, "JWKS URI must use https"),
        JwksError::ZeroTtl => invalid(K_JWKS, "JWKS TTL must be greater than zero"),
        JwksError::ZeroTimeout => invalid(K_JWKS, "JWKS fetch timeout must be greater than zero"),
    }
}

/// The name of the SSL bundle whose trust the JWKS client uses, from
/// `security.jwt.jwks-ssl-bundle`, or `None` when the key is not set.
///
/// This crate does not load SSL bundles. The caller resolves the name, builds the fetcher
/// with `ReqwestJwksFetcher::with_tls` (cargo feature `jwks`) and
/// passes it to [`jwt_decoder_from_config`]; `oxbloom-boot` does this with its `ssl` feature.
/// Without the key nothing changes: the fetcher keeps the platform certificate verifier. The
/// JWKS client is built once, when the decoder is built, so a later reload of the bundle's
/// files does not reach it.
///
/// # Errors
///
/// [`ConfigError::Invalid`] on `security.jwt.jwks-ssl-bundle` when the value is not a string
/// or is empty. The message never contains the value.
///
/// # Examples
///
/// ```
/// use oxbloom_config::{Config, ConfigError};
/// use oxbloom_security::jwks_ssl_bundle;
///
/// let config = Config::from_str("[security.jwt]\njwks-ssl-bundle = \"corporate\"")?;
/// assert_eq!(jwks_ssl_bundle(&config)?.as_deref(), Some("corporate"));
///
/// // Without the key the platform verifier stays in use.
/// assert_eq!(jwks_ssl_bundle(&Config::from_str("")?)?, None);
///
/// // An empty name is an error on the key.
/// let empty = Config::from_str("[security.jwt]\njwks-ssl-bundle = \"\"")?;
/// assert!(matches!(
///     jwks_ssl_bundle(&empty),
///     Err(ConfigError::Invalid { key, .. }) if key == "security.jwt.jwks-ssl-bundle"
/// ));
/// # Ok::<(), ConfigError>(())
/// ```
pub fn jwks_ssl_bundle(config: &Config) -> Result<Option<String>, ConfigError> {
    let name = match config.get::<String>(K_BUNDLE) {
        Ok(name) => name,
        Err(ConfigError::Missing { .. }) => return Ok(None),
        Err(ConfigError::Invalid { .. }) => return Err(invalid(K_BUNDLE, BUNDLE_RULE)),
        Err(other) => return Err(other),
    };
    check_bundle_name(&name)?;
    Ok(Some(name))
}

/// Refuses an empty or whitespace-only bundle name.
fn check_bundle_name(name: &str) -> Result<(), ConfigError> {
    if name.trim().is_empty() {
        return Err(invalid(
            K_BUNDLE,
            "must not be empty; give the name of an SSL bundle",
        ));
    }
    Ok(())
}

/// Builds a [`DefaultJwtDecoder`] from the `security.jwt` section of `config`.
///
/// Keys: `algorithms` (required list of `HS256`, `HS384`, `HS512`, `RS256`, `RS384`, `RS512`,
/// `ES256`, `ES384`), exactly one key source (`secret` for HMAC, `public-key-pem` or
/// `jwk-set-uri` for RSA/EC), and optional `issuer`, `audience` (list) and `clock-skew-secs`
/// (default 60, at most 3600).
///
/// Keys match case-insensitively, ignoring `-` and `_`: `jwk-set-uri`, `jwk_set_uri`,
/// `jwkSetUri` and the environment variable `APP_SECURITY__JWT__JWK_SET_URI` are the same key.
/// Lists can come from the environment as comma-separated values
/// (`APP_SECURITY__JWT__AUDIENCE=a,b`) or indexed (`..._AUDIENCE__0=a`, `..._AUDIENCE__1=b`).
/// Register [`SECRET_KEY`] with `ConfigBuilder::secret_key` so the HMAC secret cannot be passed
/// as a command-line argument; this function also refuses a secret whose origin is the command
/// line, even when the key was not registered. Empty or whitespace-only items in `audience` and
/// `algorithms` are rejected.
///
/// A `jwk-set-uri` needs a `fetcher` and must use `https`. `jwks-ssl-bundle` names the SSL
/// bundle whose trust that fetcher uses; this function only checks that it is not empty and
/// that `jwk-set-uri` is set, and the caller resolves it (see [`jwks_ssl_bundle`]).
///
/// # Errors
///
/// [`ConfigError::Missing`] or [`ConfigError::Invalid`], naming the offending key. Messages
/// never contain the secret or the PEM text.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::jwt_decoder_from_config;
///
/// let config = Config::from_str(
///     r#"
///     [security.jwt]
///     algorithms = ["HS256"]
///     secret = "an-example-secret-of-32-bytes!!!"
///     issuer = "https://issuer.example"
///     "#,
/// )?;
/// let decoder = jwt_decoder_from_config(&config, None)?;
/// # let _ = decoder;
///
/// // A HMAC algorithm without a secret names the missing key.
/// let bad = Config::from_str("[security.jwt]\nalgorithms = [\"HS256\"]")?;
/// let err = jwt_decoder_from_config(&bad, None).unwrap_err();
/// assert!(err.to_string().contains("security.jwt.secret"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
pub fn jwt_decoder_from_config(
    config: &Config,
    fetcher: Option<Arc<dyn JwksFetcher>>,
) -> Result<DefaultJwtDecoder, ConfigError> {
    jwt_decoder_from_config_with_clock(config, fetcher, None)
}

/// Builds a [`DefaultJwtDecoder`] from the `security.jwt` section of `config`, checking `exp`
/// and `nbf` against `clock` when one is given.
///
/// It reads the same keys and reports the same errors as [`jwt_decoder_from_config`], which
/// calls it with no clock. A clock is passed on to [`JwtDecoderBuilder::clock`].
///
/// # Errors
///
/// [`ConfigError::Missing`] or [`ConfigError::Invalid`], naming the offending key, exactly as
/// [`jwt_decoder_from_config`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::{Duration, SystemTime};
/// use oxbloom_config::Config;
/// use oxbloom_core::Clock;
/// use oxbloom_security::jwt_decoder_from_config_with_clock;
///
/// struct AtTwoBillion;
///
/// impl Clock for AtTwoBillion {
///     fn now(&self) -> SystemTime {
///         SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000)
///     }
/// }
///
/// let config = Config::from_str(
///     r#"
///     [security.jwt]
///     algorithms = ["HS256"]
///     secret = "an-example-secret-of-32-bytes!!!"
///     "#,
/// )?;
/// let decoder = jwt_decoder_from_config_with_clock(&config, None, Some(Arc::new(AtTwoBillion)))?;
/// # let _ = decoder;
///
/// // Without a clock it behaves like `jwt_decoder_from_config`.
/// let bad = Config::from_str("[security.jwt]\nalgorithms = [\"HS256\"]")?;
/// let err = jwt_decoder_from_config_with_clock(&bad, None, None).unwrap_err();
/// assert!(err.to_string().contains("security.jwt.secret"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
pub fn jwt_decoder_from_config_with_clock(
    config: &Config,
    fetcher: Option<Arc<dyn JwksFetcher>>,
    clock: Option<Arc<dyn Clock>>,
) -> Result<DefaultJwtDecoder, ConfigError> {
    let settings: JwtSettings = config.get(SECTION)?;

    if let Some(origin @ Origin::Cli { .. }) = config.origin(K_SECRET) {
        return Err(ConfigError::Invalid {
            key: K_SECRET.to_string(),
            message: "secret keys cannot be set on the command line; use an environment \
                      variable or a configuration file"
                .to_string(),
            origin: Some(origin.clone()),
        });
    }
    if settings
        .audience
        .iter()
        .flatten()
        .any(|item| item.trim().is_empty())
    {
        return Err(invalid(K_AUDIENCE, "audience items must not be empty"));
    }
    if settings
        .algorithms
        .iter()
        .flatten()
        .any(|item| item.trim().is_empty())
    {
        return Err(invalid(K_ALGORITHMS, "algorithm items must not be empty"));
    }

    if let Some(name) = &settings.jwks_ssl_bundle {
        check_bundle_name(name)?;
        if settings.jwk_set_uri.is_none() {
            return Err(invalid(
                K_BUNDLE,
                "only valid with `security.jwt.jwk-set-uri`",
            ));
        }
    }

    let skew = settings.clock_skew_secs.unwrap_or(DEFAULT_SKEW_SECS);
    if skew > MAX_SKEW_SECS {
        return Err(invalid(K_SKEW, "clock skew exceeds one hour"));
    }

    let names = settings.algorithms.as_deref().unwrap_or_default();
    if names.is_empty() {
        return Err(missing(K_ALGORITHMS));
    }
    let algorithms = names
        .iter()
        .map(|name| parse_algorithm(name))
        .collect::<Result<Vec<_>, _>>()?;
    let symmetric = algorithms.iter().any(|a| is_symmetric(*a));
    if symmetric && !algorithms.iter().all(|a| is_symmetric(*a)) {
        return Err(invalid(
            K_ALGORITHMS,
            "HMAC algorithms cannot be combined with RSA or EC algorithms",
        ));
    }
    if !symmetric && algorithms.iter().any(|a| is_ec(*a)) && !algorithms.iter().all(|a| is_ec(*a)) {
        return Err(invalid(
            K_ALGORITHMS,
            "algorithms mix RSA and EC key families",
        ));
    }

    if settings.secret.is_some() && settings.jwk_set_uri.is_some() {
        return Err(invalid(
            K_SECRET,
            "cannot be combined with `security.jwt.jwk-set-uri`",
        ));
    }
    if settings.secret.is_some() && settings.public_key_pem.is_some() {
        return Err(invalid(
            K_SECRET,
            "cannot be combined with `security.jwt.public-key-pem`",
        ));
    }

    let mut builder = if symmetric {
        if settings.jwk_set_uri.is_some() {
            return Err(invalid(K_JWKS, "not allowed with HMAC algorithms"));
        }
        if settings.public_key_pem.is_some() {
            return Err(invalid(K_PEM, "not allowed with HMAC algorithms"));
        }
        let Some(secret) = &settings.secret else {
            return Err(invalid(K_SECRET, "required with HMAC algorithms"));
        };
        if algorithms.len() != 1 {
            return Err(invalid(
                K_ALGORITHMS,
                "exactly one HMAC algorithm is supported",
            ));
        }
        let bytes = Secret::new(secret.expose().as_bytes().to_vec());
        JwtDecoderBuilder::with_secret(algorithms[0], bytes)
    } else {
        if settings.secret.is_some() {
            return Err(invalid(K_SECRET, "only valid with HMAC algorithms"));
        }
        match (&settings.jwk_set_uri, &settings.public_key_pem) {
            (Some(_), Some(_)) => {
                return Err(invalid(
                    K_PEM,
                    "cannot be combined with `security.jwt.jwk-set-uri`",
                ));
            }
            (None, None) => {
                return Err(invalid(
                    K_ALGORITHMS,
                    "RSA and EC algorithms need `security.jwt.public-key-pem` or `security.jwt.jwk-set-uri`",
                ));
            }
            (Some(uri), None) => {
                let Some(fetcher) = fetcher else {
                    return Err(invalid(
                        K_JWKS,
                        "a JWKS fetcher is required to use a JWKS URI",
                    ));
                };
                let source = JwksSource::builder(fetcher, uri.clone())
                    .build()
                    .map_err(map_jwks_error)?;
                JwtDecoderBuilder::with_jwks(source, algorithms)
            }
            (None, Some(pem)) => {
                if algorithms.len() != 1 {
                    return Err(invalid(
                        K_ALGORITHMS,
                        "exactly one algorithm is supported with a public key",
                    ));
                }
                JwtDecoderBuilder::with_public_key_pem(algorithms[0], pem.expose())
                    .map_err(|e| map_jwt_error(e, K_PEM))?
            }
        }
    };

    if let Some(issuer) = &settings.issuer {
        builder = builder.issuer(issuer);
    }
    if let Some(audience) = settings.audience {
        builder = builder.audience(audience);
    }
    if let Some(clock) = clock {
        builder = builder.clock(clock);
    }
    builder
        .clock_skew(Duration::from_secs(skew))
        .build()
        .map_err(|e| {
            map_jwt_error(
                e,
                if settings.public_key_pem.is_some() {
                    K_PEM
                } else {
                    K_SECRET
                },
            )
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use jsonwebtoken::jwk::JwkSet;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use oxbloom_config::{Config, ConfigError};
    use serde_json::json;

    use super::*;
    use crate::{FetchError, JwksFetcher, JwtDecoder};

    const SECRET: &str = "an-example-secret-of-32-bytes!!!";
    const RSA_PUBLIC: &str = include_str!("../tests/fixtures/rsa_public.pem");
    const RSA_PRIVATE: &str = include_str!("../tests/fixtures/rsa_private.pem");
    const EC_PUBLIC: &str = include_str!("../tests/fixtures/ec_public.pem");
    const URI: &str = "https://issuer.example/jwks.json";

    struct Mem;

    #[async_trait]
    impl JwksFetcher for Mem {
        async fn fetch(&self, _uri: &str) -> Result<JwkSet, FetchError> {
            Ok(JwkSet { keys: Vec::new() })
        }
    }

    fn fetcher() -> Option<Arc<dyn JwksFetcher>> {
        Some(Arc::new(Mem))
    }

    fn cfg(body: &str) -> Config {
        Config::from_str(&format!("[security.jwt]\n{body}")).unwrap()
    }

    fn pem_literal(pem: &str) -> String {
        format!("'''\n{pem}'''")
    }

    fn expect_invalid(result: Result<crate::DefaultJwtDecoder, ConfigError>, want: &str) {
        match result {
            Err(ConfigError::Invalid { key, .. }) => assert_eq!(key, want),
            other => panic!("expected Invalid({want}), got {other:?}"),
        }
    }

    fn hs_token(claims: &serde_json::Value) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap()
    }

    fn exp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600
    }

    #[test]
    fn valid_jwks_config_builds_decoder() {
        let config = cfg(&format!(
            "jwk-set-uri = \"{URI}\"\nalgorithms = [\"RS256\", \"RS512\"]\nissuer = \"https://issuer.example\""
        ));
        assert!(jwt_decoder_from_config(&config, fetcher()).is_ok());
    }

    #[tokio::test]
    async fn valid_secret_config_builds_decoder() {
        let config = cfg(&format!("secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]"));
        let decoder = jwt_decoder_from_config(&config, None).unwrap();
        let jwt = decoder
            .decode(&hs_token(&json!({"sub": "alice", "exp": exp()})))
            .await
            .unwrap();
        assert_eq!(jwt.subject(), Some("alice"));
    }

    #[tokio::test]
    async fn valid_public_key_pem_config_builds_decoder() {
        let config = cfg(&format!(
            "public-key-pem = {}\nalgorithms = [\"RS256\"]",
            pem_literal(RSA_PUBLIC)
        ));
        let decoder = jwt_decoder_from_config(&config, None).unwrap();
        let key = EncodingKey::from_rsa_pem(RSA_PRIVATE.as_bytes()).unwrap();
        let token = encode(
            &Header::new(Algorithm::RS256),
            &json!({"sub": "bob", "exp": exp()}),
            &key,
        )
        .unwrap();
        assert_eq!(decoder.decode(&token).await.unwrap().subject(), Some("bob"));

        let ec = cfg(&format!(
            "public_key_pem = {}\nalgorithms = [\"ES256\"]",
            pem_literal(EC_PUBLIC)
        ));
        assert!(jwt_decoder_from_config(&ec, None).is_ok());
    }

    #[test]
    fn secret_and_jwk_set_uri_together_is_invalid_naming_key() {
        let config = cfg(&format!(
            "secret = \"{SECRET}\"\njwk-set-uri = \"{URI}\"\nalgorithms = [\"HS256\"]"
        ));
        match jwt_decoder_from_config(&config, fetcher()) {
            Err(ConfigError::Invalid { key, message, .. }) => {
                assert_eq!(key, "security.jwt.secret");
                assert!(message.contains("security.jwt.jwk-set-uri"));
                assert!(!message.contains(SECRET));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn secret_and_public_key_pem_together_is_invalid() {
        let config = cfg(&format!(
            "secret = \"{SECRET}\"\npublic-key-pem = {}\nalgorithms = [\"RS256\"]",
            pem_literal(RSA_PUBLIC)
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.secret",
        );
    }

    #[test]
    fn no_algorithms_is_missing_naming_key() {
        let config = cfg(&format!("secret = \"{SECRET}\""));
        match jwt_decoder_from_config(&config, None) {
            Err(ConfigError::Missing { key }) => assert_eq!(key, "security.jwt.algorithms"),
            other => panic!("{other:?}"),
        }
        let empty = cfg(&format!("secret = \"{SECRET}\"\nalgorithms = []"));
        assert!(matches!(
            jwt_decoder_from_config(&empty, None),
            Err(ConfigError::Missing { .. })
        ));
    }

    #[test]
    fn missing_section_is_missing() {
        let config = Config::from_str("").unwrap();
        assert!(matches!(
            jwt_decoder_from_config(&config, None),
            Err(ConfigError::Missing { .. })
        ));
    }

    #[test]
    fn symmetric_alg_without_secret_is_invalid() {
        let config = cfg("algorithms = [\"HS256\"]");
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.secret",
        );
    }

    #[test]
    fn hmac_with_pem_and_secret_is_invalid_naming_secret() {
        let config = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]\npublic-key-pem = {}",
            pem_literal(RSA_PUBLIC)
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.secret",
        );
    }

    #[test]
    fn hmac_with_pem_and_no_secret_is_invalid_naming_pem() {
        let config = cfg(&format!(
            "algorithms = [\"HS256\"]\npublic-key-pem = {}",
            pem_literal(RSA_PUBLIC)
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.public-key-pem",
        );
    }

    #[test]
    fn hmac_with_jwk_set_uri_is_invalid_naming_uri() {
        let uri = cfg(&format!(
            "jwk-set-uri = \"{URI}\"\nalgorithms = [\"HS256\"]"
        ));
        expect_invalid(
            jwt_decoder_from_config(&uri, fetcher()),
            "security.jwt.jwk-set-uri",
        );
    }

    #[test]
    fn rsa_with_pem_and_jwk_set_uri_is_invalid_naming_pem() {
        let config = cfg(&format!(
            "jwk-set-uri = \"{URI}\"\nalgorithms = [\"RS256\"]\npublic-key-pem = {}",
            pem_literal(RSA_PUBLIC)
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, fetcher()),
            "security.jwt.public-key-pem",
        );
    }

    #[test]
    fn several_algorithms_with_single_key_are_invalid() {
        let pem = cfg(&format!(
            "public-key-pem = {}\nalgorithms = [\"RS256\", \"RS256\"]",
            pem_literal(RSA_PUBLIC)
        ));
        expect_invalid(
            jwt_decoder_from_config(&pem, None),
            "security.jwt.algorithms",
        );
        let hmac = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\", \"HS384\"]"
        ));
        expect_invalid(
            jwt_decoder_from_config(&hmac, None),
            "security.jwt.algorithms",
        );
    }

    #[test]
    fn empty_audience_is_invalid_naming_key() {
        let config = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]\naudience = []"
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.audience",
        );
    }

    #[test]
    fn unknown_key_under_security_jwt_is_invalid() {
        for typo in ["audiance", "isuer", "algorithm", "jwk-set-url"] {
            let config = cfg(&format!(
                "secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]\n{typo} = [\"x\"]"
            ));
            match jwt_decoder_from_config(&config, None) {
                Err(ConfigError::Invalid { key, message, .. }) => {
                    assert_eq!(key, "security.jwt");
                    assert!(message.contains(typo), "{message}");
                    assert!(!message.contains(SECRET));
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn underscore_aliases_still_work_and_other_security_keys_are_ignored() {
        let config = Config::from_str(&format!(
            "[security.session]\nttl = 5\n[security.jwt]\nalgorithms = [\"RS256\"]\njwk_set_uri = \"{URI}\"\nclock_skew_secs = 5"
        ))
        .unwrap();
        assert!(jwt_decoder_from_config(&config, fetcher()).is_ok());
        let pem = cfg(&format!(
            "public_key_pem = {}\nalgorithms = [\"RS256\"]",
            pem_literal(RSA_PUBLIC)
        ));
        assert!(jwt_decoder_from_config(&pem, None).is_ok());
    }

    #[test]
    fn asymmetric_alg_without_key_source_is_invalid() {
        let config = cfg("algorithms = [\"RS256\"]");
        match jwt_decoder_from_config(&config, fetcher()) {
            Err(ConfigError::Invalid { key, .. }) => assert_eq!(key, "security.jwt.algorithms"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn asymmetric_alg_with_secret_is_invalid() {
        let config = cfg(&format!("secret = \"{SECRET}\"\nalgorithms = [\"RS256\"]"));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.secret",
        );
    }

    #[test]
    fn mixed_families_are_invalid() {
        let hs_rs = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\", \"RS256\"]"
        ));
        expect_invalid(
            jwt_decoder_from_config(&hs_rs, None),
            "security.jwt.algorithms",
        );
        let rs_es = cfg(&format!(
            "jwk-set-uri = \"{URI}\"\nalgorithms = [\"RS256\", \"ES256\"]"
        ));
        expect_invalid(
            jwt_decoder_from_config(&rs_es, fetcher()),
            "security.jwt.algorithms",
        );
    }

    #[test]
    fn unknown_algorithm_name_is_invalid() {
        for name in ["none", "PS256", "EdDSA", "hs256", "HS999", ""] {
            let config = cfg(&format!("secret = \"{SECRET}\"\nalgorithms = [\"{name}\"]"));
            expect_invalid(
                jwt_decoder_from_config(&config, None),
                "security.jwt.algorithms",
            );
        }
    }

    #[test]
    fn underscore_alias_works_for_env_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("application.toml"),
            "[security.jwt]\nalgorithms = [\"RS256\"]\n",
        )
        .unwrap();
        let config = Config::builder()
            .dir(dir.path())
            .env_from([
                (
                    "APP_SECURITY__JWT__JWK_SET_URI".to_string(),
                    URI.to_string(),
                ),
                (
                    "APP_SECURITY__JWT__CLOCK_SKEW_SECS".to_string(),
                    "10".to_string(),
                ),
            ])
            .build()
            .unwrap();
        assert!(jwt_decoder_from_config(&config, fetcher()).is_ok());
        // The alias reaches the fetcher check, proving the URI was read.
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.jwk-set-uri",
        );
    }

    #[test]
    fn secret_value_not_in_any_error() {
        let secret = "short-leaked-secret-value";
        let long = "long-leaked-secret-value-that-is-definitely-over-sixty-four-bytes-long!";
        let bodies = [
            format!("secret = \"{secret}\"\nalgorithms = [\"HS256\"]"),
            format!("secret = \"{long}\"\nalgorithms = [\"RS256\"]"),
            format!("secret = \"{long}\"\njwk-set-uri = \"{URI}\"\nalgorithms = [\"HS256\"]"),
            format!("secret = \"{long}\"\nalgorithms = [\"HS256\", \"RS256\"]"),
            format!("secret = \"{long}\"\nalgorithms = [\"none\"]"),
            format!("secret = \"{long}\"\nalgorithms = [\"HS256\"]\nclock-skew-secs = 99999"),
            format!("secret = \"{long}\"\nalgorithms = [\"HS256\"]\naudience = []"),
            format!("secret = \"-----BEGIN {long}\"\nalgorithms = [\"HS256\"]"),
            format!("secret = {}\nalgorithms = [\"HS256\"]", "5"),
            format!(
                "public-key-pem = {}\nalgorithms = [\"ES256\"]",
                pem_literal(RSA_PUBLIC)
            ),
            "public-key-pem = '''leaked-pem-text'''\nalgorithms = [\"RS256\"]".to_string(),
        ];
        for body in bodies {
            let err = jwt_decoder_from_config(&cfg(&body), None).unwrap_err();
            let text = format!("{err} {err:?}");
            for needle in [secret, long, "leaked-pem-text", "MIIB", "BEGIN"] {
                assert!(!text.contains(needle), "{needle} leaked in {text}");
            }
        }
    }

    #[test]
    fn jwks_uri_without_fetcher_is_invalid() {
        let config = cfg(&format!(
            "jwk-set-uri = \"{URI}\"\nalgorithms = [\"RS256\"]"
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.jwk-set-uri",
        );
    }

    #[test]
    fn non_https_jwks_uri_is_invalid() {
        for uri in ["http://issuer.example/jwks.json", "ftp://x/y", "not a uri"] {
            let config = cfg(&format!(
                "jwk-set-uri = \"{uri}\"\nalgorithms = [\"RS256\"]"
            ));
            expect_invalid(
                jwt_decoder_from_config(&config, fetcher()),
                "security.jwt.jwk-set-uri",
            );
        }
    }

    #[test]
    fn oversized_clock_skew_is_invalid() {
        let config = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]\nclock-skew-secs = 3601"
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.clock-skew-secs",
        );
        let ok = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]\nclock-skew-secs = 3600"
        ));
        assert!(jwt_decoder_from_config(&ok, None).is_ok());
    }

    #[tokio::test]
    async fn issuer_and_audience_are_applied() {
        let config = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]\nissuer = \"https://issuer.example\"\naudience = [\"api\"]"
        ));
        let decoder = jwt_decoder_from_config(&config, None).unwrap();
        let good = json!({"sub": "a", "exp": exp(), "iss": "https://issuer.example", "aud": "api"});
        assert!(decoder.decode(&hs_token(&good)).await.is_ok());
        let bad_iss =
            json!({"sub": "a", "exp": exp(), "iss": "https://evil.example", "aud": "api"});
        assert!(decoder.decode(&hs_token(&bad_iss)).await.is_err());
        let bad_aud =
            json!({"sub": "a", "exp": exp(), "iss": "https://issuer.example", "aud": "other"});
        assert!(decoder.decode(&hs_token(&bad_aud)).await.is_err());
        let no_aud = json!({"sub": "a", "exp": exp(), "iss": "https://issuer.example"});
        assert!(decoder.decode(&hs_token(&no_aud)).await.is_err());
    }

    fn rsa_base_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("application.toml"),
            format!(
                "[security.jwt]\npublic-key-pem = {}\n",
                pem_literal(RSA_PUBLIC)
            ),
        )
        .unwrap();
        dir
    }

    fn rs_token(aud: &str) -> String {
        let key = EncodingKey::from_rsa_pem(RSA_PRIVATE.as_bytes()).unwrap();
        encode(
            &Header::new(Algorithm::RS256),
            &json!({"sub": "bob", "exp": exp(), "aud": aud}),
            &key,
        )
        .unwrap()
    }

    fn env_vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    async fn assert_audience_a_b(config: &Config) {
        let decoder = jwt_decoder_from_config(config, None).unwrap();
        assert!(decoder.decode(&rs_token("b")).await.is_ok());
        assert!(decoder.decode(&rs_token("a")).await.is_ok());
        assert!(decoder.decode(&rs_token("c")).await.is_err());
    }

    #[tokio::test]
    async fn algorithms_and_audience_from_environment() {
        let dir = rsa_base_dir();
        let config = Config::builder()
            .dir(dir.path())
            .env_from(env_vars(&[
                ("APP_SECURITY__JWT__ALGORITHMS", "RS256"),
                ("APP_SECURITY__JWT__AUDIENCE", "a,b"),
            ]))
            .build()
            .unwrap();
        assert_audience_a_b(&config).await;
    }

    #[tokio::test]
    async fn algorithms_and_audience_indexed_environment_form() {
        let dir = rsa_base_dir();
        let config = Config::builder()
            .dir(dir.path())
            .env_from(env_vars(&[
                ("APP_SECURITY__JWT__ALGORITHMS__0", "RS256"),
                ("APP_SECURITY__JWT__AUDIENCE__0", "a"),
                ("APP_SECURITY__JWT__AUDIENCE__1", "b"),
            ]))
            .build()
            .unwrap();
        assert_audience_a_b(&config).await;
    }

    #[test]
    fn secret_key_const_matches_config_key() {
        assert_eq!(SECRET_KEY, "security.jwt.secret");
        assert_eq!(SECRET_KEY, K_SECRET);
    }

    #[test]
    fn cli_secret_argument_is_rejected_when_registered() {
        let dir = tempfile::tempdir().unwrap();
        let result = Config::builder()
            .dir(dir.path())
            .env_from([])
            .args(["--security.jwt.secret=x"])
            .secret_key(SECRET_KEY)
            .build();
        match result {
            Err(ConfigError::Invalid { key, message, .. }) => {
                assert_eq!(key, SECRET_KEY);
                assert!(!message.contains("=x"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn relaxed_keys_bind() {
        // Environment spelling.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("application.toml"),
            "[security.jwt]\nalgorithms = [\"RS256\"]\n",
        )
        .unwrap();
        let env = Config::builder()
            .dir(dir.path())
            .env_from(env_vars(&[("APP_SECURITY__JWT__JWK_SET_URI", URI)]))
            .build()
            .unwrap();
        assert!(jwt_decoder_from_config(&env, fetcher()).is_ok());
        expect_invalid(
            jwt_decoder_from_config(&env, None),
            "security.jwt.jwk-set-uri",
        );
        // File spellings.
        for spelling in ["jwkSetUri", "jwk-set-uri", "jwk_set_uri", "JWK_SET_URI"] {
            let config = cfg(&format!(
                "algorithms = [\"RS256\"]\n{spelling} = \"{URI}\"\nclockSkewSecs = 5"
            ));
            assert!(
                jwt_decoder_from_config(&config, fetcher()).is_ok(),
                "{spelling}"
            );
            expect_invalid(
                jwt_decoder_from_config(&config, None),
                "security.jwt.jwk-set-uri",
            );
        }
    }

    #[test]
    fn hyphen_and_underscore_spelling_of_same_key_in_one_source_is_duplicate_key() {
        // Behaviour change: the config crate now reports both spellings of one key in a
        // single source as `DuplicateKey` (it used to be serde's "duplicate field" error).
        let result = Config::from_str(&format!(
            "[security.jwt]\nalgorithms = [\"RS256\"]\njwk-set-uri = \"{URI}\"\njwk_set_uri = \"{URI}\""
        ));
        assert!(
            matches!(result, Err(ConfigError::DuplicateKey { .. })),
            "{result:?}"
        );
    }

    fn hs_cli_config(args: &[&str], registered: bool) -> Result<Config, ConfigError> {
        let dir = tempfile::tempdir().unwrap();
        let mut b = Config::builder()
            .dir(dir.path())
            .env_from([])
            .args(args.to_vec());
        if registered {
            b = b.secret_key(SECRET_KEY);
        }
        b.build()
    }

    #[test]
    fn cli_sourced_secret_is_rejected_even_when_not_registered() {
        let secret_arg = format!("--security.jwt.secret={SECRET}");
        let config = hs_cli_config(
            &["--security.jwt.algorithms=HS256", secret_arg.as_str()],
            false,
        )
        .unwrap();
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.secret",
        );
    }

    #[test]
    fn cli_secret_error_has_no_value_and_names_the_key_and_cli_origin() {
        let secret_arg = format!("--security.jwt.secret={SECRET}");
        let config = hs_cli_config(
            &["--security.jwt.algorithms=HS256", secret_arg.as_str()],
            false,
        )
        .unwrap();
        match jwt_decoder_from_config(&config, None) {
            Err(ConfigError::Invalid {
                key,
                message,
                origin,
            }) => {
                assert_eq!(key, "security.jwt.secret");
                assert!(matches!(origin, Some(oxbloom_config::Origin::Cli { .. })));
                assert!(message.contains("command line"));
                assert!(!message.contains(SECRET));
            }
            other => panic!("{other:?}"),
        }
        let err = jwt_decoder_from_config(&config, None).unwrap_err();
        assert!(!format!("{err} {err:?}").contains(SECRET));
    }

    #[test]
    fn registered_cli_secret_still_rejected_by_config_crate_first() {
        let result = hs_cli_config(&["--security.jwt.secret=x"], true);
        assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    }

    #[tokio::test]
    async fn env_sourced_secret_still_works_unregistered() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::builder()
            .dir(dir.path())
            .env_from(env_vars(&[
                ("APP_SECURITY__JWT__ALGORITHMS", "HS256"),
                ("APP_SECURITY__JWT__SECRET", SECRET),
            ]))
            .build()
            .unwrap();
        let decoder = jwt_decoder_from_config(&config, None).unwrap();
        assert!(
            decoder
                .decode(&hs_token(&json!({"sub": "a", "exp": exp()})))
                .await
                .is_ok()
        );
        let file = cfg(&format!("secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]"));
        assert!(jwt_decoder_from_config(&file, None).is_ok());
    }

    fn audience_env(value: &str) -> Result<DefaultJwtDecoder, ConfigError> {
        let dir = rsa_base_dir();
        let config = Config::builder()
            .dir(dir.path())
            .env_from(env_vars(&[
                ("APP_SECURITY__JWT__ALGORITHMS", "RS256"),
                ("APP_SECURITY__JWT__AUDIENCE", value),
            ]))
            .build()
            .unwrap();
        jwt_decoder_from_config(&config, None)
    }

    #[test]
    fn empty_audience_item_from_env_is_invalid() {
        for value in ["a,,b", ",", "a,", "a, ,b"] {
            match audience_env(value) {
                Err(ConfigError::Invalid { key, message, .. }) => {
                    assert_eq!(key, "security.jwt.audience", "{value}");
                    assert!(!message.contains(value), "{message}");
                }
                other => panic!("{value}: {other:?}"),
            }
        }
        // An empty value is an empty list and still fails closed.
        expect_invalid(audience_env(""), "security.jwt.audience");
    }

    #[test]
    fn empty_audience_item_in_file_array_is_invalid() {
        let config = cfg(&format!(
            "secret = \"{SECRET}\"\nalgorithms = [\"HS256\"]\naudience = [\"a\", \"\"]"
        ));
        expect_invalid(
            jwt_decoder_from_config(&config, None),
            "security.jwt.audience",
        );
    }

    #[test]
    fn empty_algorithm_item_is_invalid() {
        for list in ["[\"HS256\", \"\"]", "[\" \"]", "[\"\"]"] {
            let config = cfg(&format!("secret = \"{SECRET}\"\nalgorithms = {list}"));
            expect_invalid(
                jwt_decoder_from_config(&config, None),
                "security.jwt.algorithms",
            );
        }
    }

    #[tokio::test]
    async fn non_empty_audience_list_still_works() {
        let decoder = audience_env("a,b").unwrap();
        assert!(decoder.decode(&rs_token("b")).await.is_ok());
        assert!(decoder.decode(&rs_token("c")).await.is_err());
    }
}
