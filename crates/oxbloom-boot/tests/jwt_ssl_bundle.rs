//! `security.jwt.jwks-ssl-bundle`: `JwtDecoderAutoConfiguration` fetches the JWK set with the
//! trust of a named SSL bundle. The JWK set is served over HTTPS by a local `App::serve_tls`
//! whose certificate is signed by a private CA; no external network is used.
//!
//! The CA, the server certificate and the signing key are generated with `rcgen` when the
//! tests run; no key is committed.

#![cfg(all(feature = "ssl", feature = "security"))]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use oxbloom_boot::{JwtDecoderAutoConfiguration, SslAutoConfiguration, StartupPhase};
use oxbloom_config::ConfigError;
use oxbloom_security::{FetchError, JwksFetcher, JwtDecoder};
use oxbloom_test::ContextRunner;
use oxbloom_web::{App, HttpMethod, Route};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const KEY: &str = "security.jwt.jwks-ssl-bundle";

/// A private test CA, its PEM text, and a TLS acceptor for a `localhost` certificate it signed.
/// The certificate and its key are kept as PEM to serve as a client identity in a bundle.
struct Ca {
    pem: String,
    acceptor: TlsAcceptor,
    leaf_cert_pem: String,
    leaf_key_pem: String,
}

impl Ca {
    fn new() -> Ca {
        let ca_key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(params, ca_key);

        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&key, &issuer)
            .unwrap();
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut server = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], private)
            .unwrap();
        server.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ca {
            pem: ca_cert.pem(),
            acceptor: TlsAcceptor::from(Arc::new(server)),
            leaf_cert_pem: cert.pem(),
            leaf_key_pem: key.serialize_pem(),
        }
    }
}

/// An ES256 signing key with the JWK set that publishes its public half under `kid` `k1`.
fn signing_key() -> (EncodingKey, String) {
    let key = KeyPair::generate().unwrap();
    let encoding = EncodingKey::from_ec_pem(key.serialize_pem().as_bytes()).unwrap();
    let mut jwk = Jwk::from_encoding_key(&encoding, Algorithm::ES256).unwrap();
    jwk.common.key_id = Some("k1".to_owned());
    let body = serde_json::to_string(&JwkSet { keys: vec![jwk] }).unwrap();
    (encoding, body)
}

/// Serves `body` at `/jwks.json` over HTTPS with the certificate of `ca` and returns the URL.
async fn https_jwks(ca: &Ca, body: String) -> String {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/jwks.json", move |_| {
            let body = body.clone();
            async move { body }
        }))
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(app.serve_tls(listener, ca.acceptor.clone(), std::future::pending::<()>()));
    format!("https://localhost:{port}/jwks.json")
}

fn token(key: &EncodingKey) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("k1".to_owned());
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let claims = serde_json::json!({ "sub": "alice", "exp": exp });
    encode(&header, &claims, key).unwrap()
}

/// The `security.jwt` section for an ES256 JWK set at `uri` fetched with the bundle `bundle`.
fn jwt_section(uri: &str, bundle: &str) -> String {
    format!(
        "[security.jwt]\nalgorithms = [\"ES256\"]\njwk-set-uri = \"{uri}\"\n\
         jwks-ssl-bundle = \"{bundle}\"\n"
    )
}

/// A trust-only bundle section named `name` for `pem`.
fn trust_bundle(name: &str, pem: &str) -> String {
    format!("[ssl.bundle.pem.{name}]\ntrust = '''\n{pem}'''\n")
}

#[tokio::test]
async fn jwt_auto_configuration_uses_the_named_bundle() {
    let ca = Ca::new();
    let (key, jwks) = signing_key();
    let uri = https_jwks(&ca, jwks).await;
    let toml = format!(
        "{}[security.jwt]\nalgorithms = [\"ES256\"]\njwk-set-uri = \"{uri}\"\n\
         jwks-ssl-bundle = \"idp\"\n",
        trust_bundle("idp", &ca.pem)
    );
    let seen: Arc<Mutex<Option<Arc<dyn JwtDecoder>>>> = Arc::default();
    let slot = Arc::clone(&seen);
    ContextRunner::new()
        .config_str(toml)
        .auto_configuration::<SslAutoConfiguration>()
        .auto_configuration::<JwtDecoderAutoConfiguration>()
        .run(move |view| {
            *slot.lock().unwrap() = Some(view.get::<dyn JwtDecoder>().unwrap());
        })
        .await;
    let decoder = seen.lock().unwrap().take().expect("a decoder");
    let jwt = tokio::time::timeout(Duration::from_secs(10), decoder.decode(&token(&key)))
        .await
        .expect("the decode timed out")
        .expect("the token is valid");
    assert_eq!(jwt.subject(), Some("alice"));
}

#[tokio::test]
async fn unknown_jwks_bundle_is_a_config_error_listing_names() {
    let ca = Ca::new();
    let toml = format!(
        "{}{}[security.jwt]\nalgorithms = [\"ES256\"]\n\
         jwk-set-uri = \"https://localhost:1/jwks.json\"\njwks-ssl-bundle = \"nope\"\n",
        trust_bundle("idp", &ca.pem),
        trust_bundle("api", &ca.pem)
    );
    ContextRunner::new()
        .config_str(toml)
        .auto_configuration::<SslAutoConfiguration>()
        .auto_configuration::<JwtDecoderAutoConfiguration>()
        .run_failing(|view| {
            let error = view.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let config = error.find_cause::<ConfigError>().expect("a config error");
            assert!(
                matches!(config, ConfigError::Invalid { key, .. } if key == KEY),
                "{config:?}"
            );
            assert!(config.to_string().contains("api, idp"), "{config}");
            let report = error.analysis().expect("a failure report").to_string();
            assert!(report.contains(KEY), "{report}");
            assert!(report.contains("api, idp"), "{report}");
            for text in [report, error.to_string(), view.output().contents()] {
                assert!(!text.contains("BEGIN"), "{text}");
            }
        })
        .await;

    // Without any bundle (no `SslBundles` bean) the name is unknown as well.
    let toml = "[security.jwt]\nalgorithms = [\"ES256\"]\n\
                jwk-set-uri = \"https://localhost:1/jwks.json\"\njwks-ssl-bundle = \"nope\"\n";
    ContextRunner::new()
        .config_str(toml)
        .auto_configuration::<SslAutoConfiguration>()
        .auto_configuration::<JwtDecoderAutoConfiguration>()
        .run_failing(|view| {
            let config = view
                .error()
                .find_cause::<ConfigError>()
                .expect("a config error");
            assert!(
                matches!(config, ConfigError::Invalid { key, .. } if key == KEY),
                "{config:?}"
            );
            assert!(
                config.to_string().contains("available bundles: none"),
                "{config}"
            );
        })
        .await;
}

/// A fetcher bean of the application; it is never called in these tests.
struct OwnFetcher;

#[async_trait::async_trait]
impl JwksFetcher for OwnFetcher {
    async fn fetch(&self, _uri: &str) -> Result<jsonwebtoken::jwk::JwkSet, FetchError> {
        Err(FetchError::Network)
    }
}

#[tokio::test]
async fn jwks_bundle_and_a_jwks_fetcher_bean_together_are_a_config_error() {
    let ca = Ca::new();
    let toml = format!(
        "{}{}",
        trust_bundle("idp", &ca.pem),
        jwt_section("https://localhost:1/jwks.json", "idp")
    );
    ContextRunner::new()
        .config_str(toml)
        .configure(|b| {
            b.instance(OwnFetcher)
                .provides(|f: Arc<OwnFetcher>| -> Arc<dyn JwksFetcher> { f })
        })
        .auto_configuration::<SslAutoConfiguration>()
        .auto_configuration::<JwtDecoderAutoConfiguration>()
        .run_failing(|view| {
            let error = view.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let config = error.find_cause::<ConfigError>().expect("a config error");
            assert!(
                matches!(config, ConfigError::Invalid { key, .. } if key == KEY),
                "{config:?}"
            );
            assert!(
                config
                    .to_string()
                    .contains("conflicts with a `JwksFetcher` bean"),
                "{config}"
            );
            assert!(!config.to_string().contains("idp"), "{config}");
            let report = error.analysis().expect("a failure report").to_string();
            assert!(report.contains(KEY), "{report}");
        })
        .await;
}

#[tokio::test]
async fn identity_only_bundle_keeps_the_platform_verifier() {
    // The bundle has a client certificate but no trust store, so the server is checked by the
    // platform verifier, which does not know the private CA: the key set cannot be fetched.
    let ca = Ca::new();
    let (key, jwks) = signing_key();
    let uri = https_jwks(&ca, jwks).await;
    let toml = format!(
        "[ssl.bundle.pem.client]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n{}",
        ca.leaf_cert_pem,
        ca.leaf_key_pem,
        jwt_section(&uri, "client")
    );
    let seen: Arc<Mutex<Option<Arc<dyn JwtDecoder>>>> = Arc::default();
    let slot = Arc::clone(&seen);
    ContextRunner::new()
        .config_str(toml)
        .auto_configuration::<SslAutoConfiguration>()
        .auto_configuration::<JwtDecoderAutoConfiguration>()
        .run(move |view| {
            *slot.lock().unwrap() = Some(view.get::<dyn JwtDecoder>().unwrap());
        })
        .await;
    let decoder = seen.lock().unwrap().take().expect("a decoder");
    let result = tokio::time::timeout(Duration::from_secs(10), decoder.decode(&token(&key)))
        .await
        .expect("the decode timed out");
    assert!(result.is_err(), "the private CA must not be trusted");
}
