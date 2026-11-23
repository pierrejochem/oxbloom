//! `HttpClientAutoConfiguration`: the `Arc<dyn HttpClient>` bean of the `http.client` keys, the
//! application's own bean winning, the SSL bundle by name and the failure of a broken section.

#![cfg(feature = "http-client")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxbloom_boot::{AutoConfiguration, HttpClientAutoConfiguration, StartupPhase};
use oxbloom_config::ConfigError;
use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response};
use oxbloom_test::ContextRunner;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A plain HTTP server that answers every request with `ok`, and its URL.
async fn plain_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let answer = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    format!("http://{addr}/")
}

#[tokio::test]
async fn the_http_client_bean_is_registered_with_defaults() {
    let url = plain_server().await;
    let client = Arc::new(std::sync::Mutex::new(None));
    let slot = Arc::clone(&client);
    ContextRunner::new()
        .auto_configuration::<HttpClientAutoConfiguration>()
        .run(move |view| {
            *slot.lock().unwrap() = Some(view.get::<dyn HttpClient>().unwrap());
        })
        .await;
    let client: Arc<dyn HttpClient> = client.lock().unwrap().take().unwrap();
    let response = client.execute(Request::get(&url)).await.unwrap();
    assert_eq!(response.text().unwrap(), "ok");
}

#[tokio::test]
async fn the_keys_configure_the_client() {
    let url = plain_server().await;
    let client = Arc::new(std::sync::Mutex::new(None));
    let slot = Arc::clone(&client);
    ContextRunner::new()
        .config_str("[http.client]\nmax-body-bytes = 1\ntimeout = \"2s\"")
        .auto_configuration::<HttpClientAutoConfiguration>()
        .run(move |view| {
            *slot.lock().unwrap() = Some(view.get::<dyn HttpClient>().unwrap());
        })
        .await;
    let client: Arc<dyn HttpClient> = client.lock().unwrap().take().unwrap();
    assert_eq!(
        client.execute(Request::get(&url)).await.unwrap_err(),
        HttpClientError::TooLarge
    );
}

struct OwnClient(AtomicUsize);

#[async_trait::async_trait]
impl HttpClient for OwnClient {
    async fn execute(&self, _request: Request) -> Result<Response, HttpClientError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(HttpClientError::Network)
    }
}

#[tokio::test]
async fn an_application_bean_wins() {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let slot = Arc::clone(&seen);
    ContextRunner::new()
        .configure(|b| {
            b.instance(OwnClient(AtomicUsize::new(0)))
                .provides(|c: Arc<OwnClient>| -> Arc<dyn HttpClient> { c })
        })
        .auto_configuration::<HttpClientAutoConfiguration>()
        .run(move |view| {
            *slot.lock().unwrap() = Some((
                view.get::<dyn HttpClient>().unwrap(),
                view.get::<OwnClient>().unwrap(),
            ));
        })
        .await;
    let (client, own) = seen.lock().unwrap().take().unwrap();
    // The bean is the application's: its call counter moves and the error is its own.
    let result = client.execute(Request::get("http://127.0.0.1:1/")).await;
    assert_eq!(result.unwrap_err(), HttpClientError::Network);
    assert_eq!(own.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_broken_section_fails_the_build_with_the_key() {
    ContextRunner::new()
        .config_str("[http.client]\ntimeout = \"MARKER-7\"")
        .auto_configuration::<HttpClientAutoConfiguration>()
        .run_failing(|view| {
            let error = view.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            let config = error.find_cause::<ConfigError>().expect("a config error");
            assert!(
                matches!(config, ConfigError::Invalid { key, .. } if key == "http.client.timeout"),
                "{config:?}"
            );
            let report = error.analysis().expect("a failure report").to_string();
            assert!(report.contains("http.client.timeout"), "{report}");
            for text in [report, error.to_string(), view.output().contents()] {
                assert!(!text.contains("MARKER-7"), "{text}");
            }
        })
        .await;
}

#[test]
fn the_metadata_lists_the_twelve_keys() {
    let keys: Vec<_> = HttpClientAutoConfiguration::properties()
        .iter()
        .map(|p| p.key)
        .collect();
    assert_eq!(
        keys,
        [
            "http.client.timeout",
            "http.client.connect-timeout",
            "http.client.max-body-bytes",
            "http.client.follow-redirects",
            "http.client.user-agent",
            "http.client.ssl-bundle",
            "http.client.retry.max-attempts",
            "http.client.retry.backoff",
            "http.client.retry.initial",
            "http.client.retry.multiplier",
            "http.client.retry.max-delay",
            "http.client.retry.jitter",
        ]
    );
}

#[cfg(feature = "ssl")]
mod ssl {
    use super::*;
    use oxbloom_boot::SslAutoConfiguration;
    use oxbloom_web::{App, HttpMethod, Route};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    use rustls::ServerConfig;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio_rustls::TlsAcceptor;

    /// Serves `ok` over HTTPS with a certificate for `localhost` signed by a private CA, and
    /// returns the URL and the PEM of the CA.
    async fn https_server() -> (String, String) {
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
        let app = App::new()
            .route(Route::new(HttpMethod::Get, "/", |_| async { "ok" }))
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(app.serve_tls(
            listener,
            TlsAcceptor::from(Arc::new(server)),
            std::future::pending::<()>(),
        ));
        (format!("https://localhost:{port}/"), ca_cert.pem())
    }

    fn trust_bundle(name: &str, pem: &str) -> String {
        format!("[ssl.bundle.pem.{name}]\ntrust = '''\n{pem}'''\n")
    }

    #[tokio::test]
    async fn ssl_bundle_names_a_bundle_and_unknown_name_lists_the_available_ones() {
        let (url, ca) = https_server().await;
        let toml = format!(
            "{}{}[http.client]\nssl-bundle = \"idp\"\n",
            trust_bundle("idp", &ca),
            trust_bundle("api", &ca)
        );
        let client = Arc::new(std::sync::Mutex::new(None));
        let slot = Arc::clone(&client);
        ContextRunner::new()
            .config_str(toml)
            .auto_configuration::<SslAutoConfiguration>()
            .auto_configuration::<HttpClientAutoConfiguration>()
            .run(move |view| {
                *slot.lock().unwrap() = Some(view.get::<dyn HttpClient>().unwrap());
            })
            .await;
        let client: Arc<dyn HttpClient> = client.lock().unwrap().take().unwrap();
        let response = client.execute(Request::get(&url)).await.unwrap();
        assert_eq!(response.text().unwrap(), "ok");

        // Without the bundle the platform roots do not know the private CA.
        let client = Arc::new(std::sync::Mutex::new(None));
        let slot = Arc::clone(&client);
        ContextRunner::new()
            .auto_configuration::<HttpClientAutoConfiguration>()
            .run(move |view| {
                *slot.lock().unwrap() = Some(view.get::<dyn HttpClient>().unwrap());
            })
            .await;
        let client: Arc<dyn HttpClient> = client.lock().unwrap().take().unwrap();
        assert_eq!(
            client.execute(Request::get(&url)).await.unwrap_err(),
            HttpClientError::Network
        );

        // An unknown name lists the available bundles.
        let toml = format!(
            "{}{}[http.client]\nssl-bundle = \"nope\"\n",
            trust_bundle("idp", &ca),
            trust_bundle("api", &ca)
        );
        ContextRunner::new()
            .config_str(toml)
            .auto_configuration::<SslAutoConfiguration>()
            .auto_configuration::<HttpClientAutoConfiguration>()
            .run_failing(|view| {
                let error = view.error();
                assert_eq!(error.phase(), StartupPhase::Container);
                let config = error.find_cause::<ConfigError>().expect("a config error");
                assert!(
                    matches!(config, ConfigError::Invalid { key, .. } if key == "http.client.ssl-bundle"),
                    "{config:?}"
                );
                assert!(config.to_string().contains("api, idp"), "{config}");
                let report = error.analysis().expect("a failure report").to_string();
                assert!(report.contains("http.client.ssl-bundle"), "{report}");
                for text in [report, error.to_string(), view.output().contents()] {
                    assert!(!text.contains("BEGIN"), "{text}");
                }
            })
            .await;

        // With no bundles at all the list says none.
        ContextRunner::new()
            .config_str("[http.client]\nssl-bundle = \"nope\"\n")
            .auto_configuration::<HttpClientAutoConfiguration>()
            .run_failing(|view| {
                let config = view.error().find_cause::<ConfigError>().unwrap();
                assert!(
                    config.to_string().contains("available bundles: none"),
                    "{config}"
                );
            })
            .await;
    }
}
