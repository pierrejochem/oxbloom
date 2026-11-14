//! `ReqwestJwksFetcher::with_tls` against a local HTTPS JWKS server, and the
//! `security.jwt.jwks-ssl-bundle` key.
//!
//! The server is plain tokio with tokio-rustls, so this crate needs no web server dependency
//! for it. Its certificate is signed by a private CA that `rcgen` generates when the tests run;
//! no key is committed.

#![cfg(feature = "jwks")]

use std::sync::Arc;

use oxbloom_config::{Config, ConfigError};
use oxbloom_security::{
    FetchError, JwksFetcher, ReqwestJwksFetcher, jwks_ssl_bundle, jwt_decoder_from_config,
};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

const KEY: &str = "security.jwt.jwks-ssl-bundle";
const JWKS: &str = r#"{"keys":[{"kty":"oct","kid":"k1","k":"c2VjcmV0"}]}"#;

/// A private test CA.
struct Ca {
    issuer: Issuer<'static, KeyPair>,
    der: CertificateDer<'static>,
}

impl Ca {
    fn new() -> Ca {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let cert = params.self_signed(&key).unwrap();
        Ca {
            der: cert.der().clone(),
            issuer: Issuer::new(params, key),
        }
    }

    /// A rustls client configuration that trusts this CA only.
    fn client_config(&self) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots.add(self.der.clone()).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    }

    /// A rustls server configuration with a `localhost` certificate signed by this CA.
    fn server_config(&self) -> Arc<ServerConfig> {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&key, &self.issuer)
            .unwrap();
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], private)
            .unwrap();
        Arc::new(config)
    }
}

/// Serves `JWKS` over HTTPS on a loopback port with a certificate of `ca` and returns the URL.
/// Each connection gets one answer; a failed handshake only drops that connection.
async fn https_jwks_server(ca: &Ca) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = TlsAcceptor::from(ca.server_config());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{JWKS}",
                    JWKS.len()
                );
                let _ = tls.write_all(response.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    format!("https://localhost:{port}/jwks.json")
}

#[tokio::test]
async fn fetcher_trusts_a_custom_ca_from_a_client_config() {
    let ca = Ca::new();
    let uri = https_jwks_server(&ca).await;
    let fetcher = ReqwestJwksFetcher::with_tls(ca.client_config()).unwrap();
    let set = fetcher.fetch(&uri).await.unwrap();
    assert_eq!(set.keys.len(), 1);
    assert_eq!(set.keys[0].common.key_id.as_deref(), Some("k1"));
}

#[tokio::test]
async fn fetcher_without_the_custom_ca_fails_with_network_error() {
    let ca = Ca::new();
    let uri = https_jwks_server(&ca).await;

    // The platform verifier does not know the private CA.
    let platform = ReqwestJwksFetcher::new().unwrap();
    assert_eq!(platform.fetch(&uri).await.unwrap_err(), FetchError::Network);

    // Neither does a client configuration that trusts another CA.
    let other = ReqwestJwksFetcher::with_tls(Ca::new().client_config()).unwrap();
    assert_eq!(other.fetch(&uri).await.unwrap_err(), FetchError::Network);
}

/// Whether `result` is a `ConfigError::Invalid` on the bundle key whose text does not contain
/// `value`.
fn invalid_on_the_key<T: std::fmt::Debug>(result: Result<T, ConfigError>, value: &str) {
    let err = result.unwrap_err();
    assert!(
        matches!(&err, ConfigError::Invalid { key, .. } if key == KEY),
        "{err:?}"
    );
    if !value.trim().is_empty() {
        assert!(!err.to_string().contains(value), "{err}");
        assert!(!format!("{err:?}").contains(value), "{err:?}");
    }
}

#[test]
fn invalid_bundle_key_value_is_a_config_error_on_the_key() {
    let cases = [
        ("\"\"", ""),
        ("\"   \"", ""),
        ("[\"marker-value-71\"]", "marker-value-71"),
        ("{ name = \"marker-value-72\" }", "marker-value-72"),
    ];
    for (value, marker) in cases {
        let config =
            Config::from_str(&format!("[security.jwt]\njwks-ssl-bundle = {value}")).unwrap();
        invalid_on_the_key(jwks_ssl_bundle(&config), marker);
    }

    // A bundle without a JWKS URI has nothing to apply to.
    let config = Config::from_str(
        "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"an-example-secret-of-32-bytes!!!\"\n\
         jwks-ssl-bundle = \"corp\"",
    )
    .unwrap();
    invalid_on_the_key(jwt_decoder_from_config(&config, None), "corp");

    // An empty name fails the decoder as well.
    let config = Config::from_str(
        "[security.jwt]\nalgorithms = [\"RS256\"]\njwk-set-uri = \"https://issuer.example/jwks\"\n\
         jwks-ssl-bundle = \"\"",
    )
    .unwrap();
    let fetcher: Arc<dyn JwksFetcher> = Arc::new(ReqwestJwksFetcher::new().unwrap());
    invalid_on_the_key(jwt_decoder_from_config(&config, Some(fetcher)), "");
}

#[test]
fn bundle_key_names_the_bundle_in_every_spelling() {
    let config = Config::from_str("[security.jwt]\njwks-ssl-bundle = \"corp\"").unwrap();
    assert_eq!(jwks_ssl_bundle(&config).unwrap().as_deref(), Some("corp"));

    let dir = tempfile::tempdir().unwrap();
    let config = Config::builder()
        .dir(dir.path())
        .env_from([(
            "APP_SECURITY__JWT__JWKS_SSL_BUNDLE".to_string(),
            "corp".to_string(),
        )])
        .build()
        .unwrap();
    assert_eq!(jwks_ssl_bundle(&config).unwrap().as_deref(), Some("corp"));

    // The key is part of the section, so a decoder with a JWKS URI accepts it; the caller
    // resolves the name and passes the fetcher.
    let config = Config::from_str(
        "[security.jwt]\nalgorithms = [\"RS256\"]\njwk-set-uri = \"https://issuer.example/jwks\"\n\
         jwks-ssl-bundle = \"corp\"",
    )
    .unwrap();
    let fetcher: Arc<dyn JwksFetcher> = Arc::new(ReqwestJwksFetcher::new().unwrap());
    assert!(jwt_decoder_from_config(&config, Some(fetcher)).is_ok());
}

#[test]
fn no_key_keeps_the_platform_verifier() {
    assert_eq!(
        jwks_ssl_bundle(&Config::from_str("").unwrap()).unwrap(),
        None
    );
    let config = Config::from_str(
        "[security.jwt]\nalgorithms = [\"RS256\"]\njwk-set-uri = \"https://issuer.example/jwks\"",
    )
    .unwrap();
    assert_eq!(jwks_ssl_bundle(&config).unwrap(), None);
    assert!(ReqwestJwksFetcher::new().is_ok());
}
