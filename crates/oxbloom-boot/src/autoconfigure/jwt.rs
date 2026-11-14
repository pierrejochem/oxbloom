//! `JwtDecoderAutoConfiguration`: a `dyn JwtDecoder` from the `security.jwt` section.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_security::{
    DefaultJwtDecoder, JwksFetcher, JwtDecoder, jwt_decoder_from_config_with_clock,
};

use crate::autoconfig::AutoConfiguration;
use crate::metadata::PropertyMetadata;

/// Registers a `dyn JwtDecoder` built by `oxbloom_security::jwt_decoder_from_config_with_clock`
/// when the configuration has a `security.jwt` section.
///
/// The group runs only with `security.jwt` present. The decoder backs off when the
/// application registered its own `dyn JwtDecoder`; an optional `dyn JwksFetcher` bean is
/// passed on for `security.jwt.jwk-set-uri`, and an optional `dyn Clock` bean
/// (`oxbloom_core::Clock`) replaces the system time in the `exp` and `nbf` checks, which lets
/// tests move the time. A broken section fails the build with
/// `DiError::Creation` whose source is the `ConfigError` naming the key; no message carries
/// the secret.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     Banner, JwtDecoderAutoConfiguration, OxbloomApplication, RunListener, StartupPhase,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_security::JwtDecoder;
///
/// #[derive(Clone, Default)]
/// struct Decoders(Arc<Mutex<usize>>);
/// impl RunListener for Decoders {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get_all::<dyn JwtDecoder>().unwrap().len();
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let run = |toml: &str, seen: Decoders| {
///     OxbloomApplication::builder()
///         .with_config(Config::from_str(toml).unwrap())
///         .banner(Banner::off())
///         .output(std::io::sink())
///         .listener(seen)
///         .auto_configuration::<JwtDecoderAutoConfiguration>()
///         .run()
/// };
///
/// let seen = Decoders::default();
/// let toml = "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"an-example-secret-of-32-bytes!!!\"";
/// run(toml, seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), 1);
///
/// // Without the section there is no decoder.
/// let seen = Decoders::default();
/// run("", seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), 0);
///
/// // A HMAC algorithm without a secret fails the container, naming the key.
/// let err = run("[security.jwt]\nalgorithms = [\"HS256\"]", Decoders::default())
///     .await
///     .unwrap_err();
/// assert_eq!(err.phase(), StartupPhase::Container);
/// assert!(err.to_string().contains("security.jwt.secret"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct JwtDecoderAutoConfiguration;

impl Configuration for JwtDecoderAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(jwt_decoder)
            .provides(|d: Arc<DefaultJwtDecoder>| -> Arc<dyn JwtDecoder> { d })
            .on_missing_bean::<dyn JwtDecoder>()
    }
}

impl AutoConfiguration for JwtDecoderAutoConfiguration {
    fn conditions() -> Conditions {
        Conditions::new().on_property("security.jwt")
    }

    fn properties() -> &'static [PropertyMetadata] {
        &JWT
    }
}

/// The decoder of the `security.jwt` section; the `ConfigError` stays the typed source of a `DiError::Creation` that names the decoder.
/// A `dyn Clock` bean, when there is one, replaces the system time in the `exp` and `nbf`
/// checks.
fn jwt_decoder(
    config: Arc<Config>,
    fetcher: Option<Arc<dyn JwksFetcher>>,
    clock: Option<Arc<dyn Clock>>,
) -> Result<DefaultJwtDecoder, DiError> {
    jwt_decoder_from_config_with_clock(&config, fetcher, clock)
        .map_err(DiError::creation_from::<DefaultJwtDecoder>)
}

/// Exactly the keys `jwt_decoder_from_config` reads (pinned by a test).
static JWT: [PropertyMetadata; 7] = [
    PropertyMetadata::new(
        "security.jwt.algorithms",
        "array of String",
        None,
        "The accepted signing algorithms: HS256, HS384, HS512, RS256, RS384, RS512, ES256 or \
         ES384. Required; HMAC algorithms cannot be combined with RSA or EC ones.",
    ),
    PropertyMetadata::new(
        "security.jwt.audience",
        "array of String",
        None,
        "The accepted `aud` values; a token must name one of them. Unset, the audience is not \
         checked.",
    ),
    PropertyMetadata::new(
        "security.jwt.clock-skew-secs",
        "u64",
        Some("60"),
        "The tolerated clock difference for `exp` and `nbf`, in seconds; at most 3600.",
    ),
    PropertyMetadata::new(
        "security.jwt.issuer",
        "String",
        None,
        "The required `iss` claim. Unset, the issuer is not checked.",
    ),
    PropertyMetadata::new(
        "security.jwt.jwk-set-uri",
        "String",
        None,
        "The https URI of the JWK set for RSA or EC algorithms; needs a `JwksFetcher` bean. \
         Excludes `public-key-pem` and `secret`.",
    ),
    PropertyMetadata::new(
        "security.jwt.public-key-pem",
        "String",
        None,
        "The PEM-encoded RSA or EC public key for exactly one algorithm. Excludes \
         `jwk-set-uri` and `secret`.",
    ),
    PropertyMetadata::new(
        "security.jwt.secret",
        "String",
        None,
        "The HMAC secret for exactly one HMAC algorithm, at least as long as its hash output. \
         A secret: its value is never printed, and it is refused on the command line; set it \
         in a file or an environment variable.",
    ),
];
