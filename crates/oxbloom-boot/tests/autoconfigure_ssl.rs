//! `SslAutoConfiguration`, `server.ssl.bundle` and the SSL failure report, run in process
//! through `TestApp` and `ContextRunner` and over real HTTPS with a rustls client.
//!
//! The certificates are generated with `rcgen` when the tests run; no key is committed.

#![cfg(feature = "ssl")]

use std::any::type_name;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use oxbloom_boot::{
    OxbloomApplication, SslAutoConfiguration, StartupPhase, WebServer, auto_configurations,
    boot_properties,
};
use oxbloom_config::{Config, ConfigError};
use oxbloom_core::Clock;
use oxbloom_ssl::{SslBundles, SslError};
use oxbloom_test::{ContextRunner, TestApp};
use oxbloom_web::{App, Controller, HttpMethod, Route};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// A key text that must never reach an error, a report or the output.
const SECRET: &str = "hunter2hunter2hunter2";

/// A test CA that signs `localhost` server certificates.
struct Ca {
    issuer: Issuer<'static, KeyPair>,
    der: CertificateDer<'static>,
    pem: String,
}

/// A server certificate and its private key, as PEM, with the certificate's DER.
struct Leaf {
    cert_pem: String,
    key_pem: String,
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
            pem: cert.pem(),
            issuer: Issuer::new(params, key),
        }
    }

    fn leaf(&self) -> Leaf {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&key, &self.issuer)
            .unwrap();
        Leaf {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            der: cert.der().clone(),
        }
    }

    /// A rustls client that trusts this CA only, with ALPN `http/1.1`.
    fn connector(&self) -> TlsConnector {
        let mut roots = RootCertStore::empty();
        roots.add(self.der.clone()).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut client = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.alpn_protocols = vec![b"http/1.1".to_vec()];
        TlsConnector::from(Arc::new(client))
    }
}

/// Writes `leaf` as `web.crt` and `web.key` into `dir`, replacing the files at once with a
/// rename each, and returns their paths.
fn write_leaf(dir: &Path, leaf: &Leaf) -> (PathBuf, PathBuf) {
    let cert = dir.join("web.crt");
    let key = dir.join("web.key");
    let tmp_cert = dir.join("web.crt.tmp");
    let tmp_key = dir.join("web.key.tmp");
    std::fs::write(&tmp_cert, &leaf.cert_pem).unwrap();
    std::fs::write(&tmp_key, &leaf.key_pem).unwrap();
    std::fs::rename(&tmp_key, &key).unwrap();
    std::fs::rename(&tmp_cert, &cert).unwrap();
    (cert, key)
}

/// The `[ssl.bundle.pem.web]` section for the files of `write_leaf`.
fn web_bundle(cert: &Path, key: &Path) -> String {
    format!(
        "[ssl.bundle.pem.web]\ncertificate = '{}'\nprivate-key = '{}'\n",
        cert.display(),
        key.display()
    )
}

struct Hello;

impl Controller for Hello {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/hello", |_| async { "hello" })]
    }
}

/// `TestApp::web()` with the SSL auto-configuration and the `Hello` controller.
fn ssl_app() -> TestApp {
    TestApp::web()
        .auto_configuration::<SslAutoConfiguration>()
        .configure(|b| {
            b.instance(Hello)
                .provides(|c: Arc<Hello>| -> Arc<dyn Controller> { c })
        })
}

/// Sends `GET /hello` over HTTPS and returns the response text and the server's leaf.
async fn https_hello(addr: SocketAddr, ca: &Ca) -> (String, CertificateDer<'static>) {
    let tcp = TcpStream::connect(addr).await.unwrap();
    let name = ServerName::try_from("localhost").unwrap();
    let mut tls = tokio::time::timeout(Duration::from_secs(5), ca.connector().connect(name, tcp))
        .await
        .expect("the handshake timed out")
        .expect("the handshake failed");
    let leaf = tls.get_ref().1.peer_certificates().unwrap()[0].clone();
    tls.write_all(b"GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    // A peer that closes without close_notify ends the read with `UnexpectedEof`.
    let _ = tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut buf)).await;
    (String::from_utf8_lossy(&buf).into_owned(), leaf)
}

#[tokio::test]
async fn server_ssl_bundle_serves_https() {
    let dir = tempfile::tempdir().unwrap();
    let ca = Ca::new();
    let leaf = ca.leaf();
    let (cert, key) = write_leaf(dir.path(), &leaf);
    let app = ssl_app()
        .config_str(format!(
            "{}[server.ssl]\nbundle = \"web\"\n",
            web_bundle(&cert, &key)
        ))
        .start()
        .await;

    let (response, served) = https_hello(app.addr(), &ca).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("hello"), "{response}");
    assert_eq!(served, leaf.der);

    // The port speaks only HTTPS: a plaintext request gets no HTTP answer.
    let mut plain = TcpStream::connect(app.addr()).await.unwrap();
    plain
        .write_all(b"GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), plain.read_to_end(&mut buf)).await;
    assert!(
        !buf.starts_with(b"HTTP/"),
        "{:?}",
        String::from_utf8_lossy(&buf)
    );

    assert_eq!(app.stop().await.status().code(), 0);
}

#[tokio::test]
async fn unknown_bundle_is_a_config_error_naming_server_ssl_bundle_and_listing_names() {
    let ca = Ca::new();
    let leaf = ca.leaf();
    let toml = format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
         [ssl.bundle.pem.api]\ntrust = '''\n{}'''\n[server.ssl]\nbundle = \"nope\"\n",
        leaf.cert_pem, leaf.key_pem, ca.pem
    );
    let error = ssl_app().config_str(toml).start_failing().await;
    assert_eq!(error.phase(), StartupPhase::Container);
    let config = error
        .find_cause::<ConfigError>()
        .expect("a configuration error");
    assert!(
        matches!(config, ConfigError::Invalid { key, .. } if key == "server.ssl.bundle"),
        "{config:?}"
    );
    let text = config.to_string();
    assert!(text.contains("api, web"), "{text}");
    let report = error.analysis().expect("a failure report").to_string();
    assert!(report.contains("server.ssl.bundle"), "{report}");
    assert!(report.contains("api, web"), "{report}");
    assert!(!report.contains("BEGIN"), "{report}");
}

#[tokio::test]
async fn trust_only_bundle_is_a_config_error() {
    let ca = Ca::new();
    let toml = format!(
        "[ssl.bundle.pem.client]\ntrust = '''\n{}'''\n[server.ssl]\nbundle = \"client\"\n",
        ca.pem
    );
    let error = ssl_app().config_str(toml).start_failing().await;
    assert_eq!(error.phase(), StartupPhase::Container);
    let config = error
        .find_cause::<ConfigError>()
        .expect("a configuration error");
    assert!(
        matches!(config, ConfigError::Invalid { key, .. } if key == "server.ssl.bundle"),
        "{config:?}"
    );
    assert!(config.to_string().contains("client"), "{config}");
    let report = error.analysis().expect("a failure report").to_string();
    assert!(report.contains("server.ssl.bundle"), "{report}");
    assert!(report.contains("certificate"), "{report}");
    assert!(!report.contains("BEGIN"), "{report}");
}

#[tokio::test]
async fn analyzer_report_names_the_key_and_never_a_secret() {
    let ca = Ca::new();
    let leaf = ca.leaf();
    let toml = format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\n\
         private-key = '''\n-----BEGIN PRIVATE KEY-----\n{SECRET}\n-----END PRIVATE KEY-----\n'''\n",
        leaf.cert_pem
    );
    let failure = ContextRunner::new()
        .config_str(toml)
        .property("oxbloom.main.conditions-report", "console")
        .auto_configuration::<SslAutoConfiguration>();
    failure
        .run_failing(|view| {
            let error = view.error();
            assert_eq!(error.phase(), StartupPhase::Container);
            assert!(
                matches!(error.find_cause::<SslError>(), Some(SslError::Pem { key, .. }) if key == "ssl.bundle.pem.web.private-key"),
                "{error:?}"
            );
            let report = error.analysis().expect("a failure report").to_string();
            assert!(
                report.contains("ssl.bundle.pem.web.private-key"),
                "{report}"
            );
            for text in [
                report,
                error.to_string(),
                format!("{error:?}"),
                view.output().contents(),
            ] {
                assert!(!text.contains(SECRET), "{text}");
                assert!(!text.contains("BEGIN"), "{text}");
            }
        })
        .await;
}

#[tokio::test]
async fn bad_certificate_file_fails_the_boot_with_the_ssl_report() {
    let dir = tempfile::tempdir().unwrap();
    let ca = Ca::new();
    let (cert, key) = write_leaf(dir.path(), &ca.leaf());
    std::fs::write(&cert, format!("not a certificate {SECRET}")).unwrap();
    let error = ssl_app()
        .config_str(format!(
            "{}[server.ssl]\nbundle = \"web\"\n",
            web_bundle(&cert, &key)
        ))
        .start_failing()
        .await;
    assert_eq!(error.phase(), StartupPhase::Container);
    assert!(
        matches!(error.find_cause::<SslError>(), Some(SslError::Pem { key, .. }) if key == "ssl.bundle.pem.web.certificate"),
        "{error:?}"
    );
    let report = error.analysis().expect("a failure report").to_string();
    assert!(
        report.contains("ssl.bundle.pem.web.certificate"),
        "{report}"
    );
    assert!(report.contains("`web`"), "{report}");
    // Neither the path (it may itself be a misplaced secret) nor the file content.
    let path = dir.path().display().to_string();
    for text in [report, error.to_string()] {
        assert!(!text.contains(&path), "{text}");
        assert!(!text.contains(SECRET), "{text}");
    }

    // A missing file names the key and the error kind.
    std::fs::remove_file(&cert).unwrap();
    let error = ssl_app()
        .config_str(format!(
            "{}[server.ssl]\nbundle = \"web\"\n",
            web_bundle(&cert, &key)
        ))
        .start_failing()
        .await;
    let report = error.analysis().expect("a failure report").to_string();
    assert!(
        report.contains("ssl.bundle.pem.web.certificate"),
        "{report}"
    );
    assert!(report.contains("not found"), "{report}");
    assert!(!report.contains(&path), "{report}");
}

#[tokio::test]
async fn replaced_certificate_reaches_a_new_connection_without_restart() {
    let dir = tempfile::tempdir().unwrap();
    let ca = Ca::new();
    let first = ca.leaf();
    let (cert, key) = write_leaf(dir.path(), &first);
    let app = ssl_app()
        .config_str(format!(
            "[ssl]\nreload-interval = \"50ms\"\n{}[server.ssl]\nbundle = \"web\"\n",
            web_bundle(&cert, &key)
        ))
        .start()
        .await;
    let (response, served) = https_hello(app.addr(), &ca).await;
    assert!(response.ends_with("hello"), "{response}");
    assert_eq!(served, first.der);

    let second = ca.leaf();
    write_leaf(dir.path(), &second);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (response, served) = https_hello(app.addr(), &ca).await;
        assert!(response.ends_with("hello"), "{response}");
        if served == second.der {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the new certificate was not served within 5 seconds"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(app.stop().await.status().code(), 0);
}

#[tokio::test]
async fn ssl_auto_configuration_backs_off_for_a_user_bean() {
    let ca = Ca::new();
    let mine = SslBundles::from_config(
        &Config::from_str(&format!(
            "[ssl.bundle.pem.mine]\ntrust = '''\n{}'''",
            ca.pem
        ))
        .unwrap(),
    )
    .unwrap();
    ContextRunner::new()
        .config_str(format!(
            "[ssl.bundle.pem.theirs]\ntrust = '''\n{}'''",
            ca.pem
        ))
        .auto_configuration::<SslAutoConfiguration>()
        .configure(move |b| b.instance(mine))
        .run(|view| {
            assert_eq!(view.get::<SslBundles>().unwrap().names(), ["mine"]);
            assert!(
                view.conditions()
                    .records()
                    .iter()
                    .any(|r| r.condition() == "on_missing_bean" && !r.matched()),
                "{:?}",
                view.conditions()
            );
        })
        .await;
}

#[tokio::test]
async fn conditions_report_shows_the_ssl_auto_configuration() {
    let group = type_name::<SslAutoConfiguration>();
    // Without any `ssl.bundle.*` key the group is skipped, and the report says why.
    ContextRunner::new()
        .property("ssl.reload-interval", "5s")
        .auto_configuration::<SslAutoConfiguration>()
        .run(move |view| {
            let record = view
                .conditions()
                .records()
                .iter()
                .find(|r| r.registration() == group)
                .expect("a record of the SSL auto-configuration");
            assert_eq!(record.condition(), "on_property");
            assert!(!record.matched());
            assert!(
                record.reason().contains("ssl.bundle"),
                "{}",
                record.reason()
            );
            assert!(!view.contains::<SslBundles>());
        })
        .await;

    // With a bundle it matches, and the printed report carries no key material.
    let ca = Ca::new();
    let leaf = ca.leaf();
    ContextRunner::new()
        .config_str(format!(
            "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n",
            leaf.cert_pem, leaf.key_pem
        ))
        .property("oxbloom.main.conditions-report", "console")
        .auto_configuration::<SslAutoConfiguration>()
        .run(move |view| {
            assert!(
                view.conditions()
                    .records()
                    .iter()
                    .any(|r| r.registration() == group
                        && r.condition() == "on_property"
                        && r.matched()),
                "{:?}",
                view.conditions()
            );
            assert_eq!(view.get::<SslBundles>().unwrap().names(), ["web"]);
            let output = view.output().contents();
            assert!(output.contains(group), "{output}");
            assert!(!output.contains("BEGIN"), "{output}");
        })
        .await;
}

#[test]
fn property_metadata_lists_every_ssl_key() {
    let list = OxbloomApplication::builder()
        .auto_configurations(auto_configurations())
        .property_metadata();
    for &(key, type_name, default, _) in oxbloom_ssl::PROPERTIES {
        let found = list
            .iter()
            .find(|p| p.key == key)
            .unwrap_or_else(|| panic!("{key} is listed"));
        assert_eq!(
            (found.type_name, found.default),
            (type_name, default),
            "{key}"
        );
    }
    let bundle = boot_properties()
        .iter()
        .find(|p| p.key == "server.ssl.bundle")
        .expect("server.ssl.bundle is a boot property");
    assert_eq!(bundle.type_name, "String");
    assert_eq!(bundle.default, None);
    assert!(list.iter().any(|p| p.key == "server.ssl.bundle"));

    // Excluding the auto-configuration removes the `ssl.*` keys.
    let without = OxbloomApplication::builder()
        .auto_configurations(auto_configurations())
        .exclude::<SslAutoConfiguration>()
        .property_metadata();
    assert!(!without.iter().any(|p| p.key.starts_with("ssl.")));
}

#[tokio::test]
async fn no_ssl_keys_means_no_bean_and_plain_http() {
    ContextRunner::new()
        .auto_configurations(auto_configurations())
        .run(|view| assert!(!view.contains::<SslBundles>()))
        .await;

    let app = ssl_app().start().await;
    let response = app.client().get("/hello").send().await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.text(), "hello");
    assert_eq!(app.stop().await.status().code(), 0);
}

/// A clock at the current time that counts its reads.
struct CountingClock(AtomicUsize);

impl Clock for CountingClock {
    fn now(&self) -> SystemTime {
        self.0.fetch_add(1, Ordering::SeqCst);
        SystemTime::now()
    }
}

#[tokio::test]
async fn clock_bean_reaches_the_ssl_bundles() {
    let ca = Ca::new();
    let clock = Arc::new(CountingClock(AtomicUsize::new(0)));
    let seen = clock.clone();
    ContextRunner::new()
        .config_str(format!(
            "[ssl]\nreload-interval = \"0s\"\n[ssl.bundle.pem.client]\ntrust = '''\n{}'''",
            ca.pem
        ))
        .clock(clock)
        .auto_configuration::<SslAutoConfiguration>()
        .run(move |view| {
            assert!(view.contains::<SslBundles>());
            // `with_clock` checks every certificate against the clock at once.
            assert!(seen.0.load(Ordering::SeqCst) > 0);
        })
        .await;
}

#[test]
fn web_server_new_refuses_server_ssl_bundle() {
    // `new` has no bundles, so a set `server.ssl.bundle` never falls back to plain HTTP.
    let config = Config::from_str("[server.ssl]\nbundle = \"web\"").unwrap();
    let err = WebServer::new(App::new(), &config).unwrap_err();
    assert!(
        matches!(&err, ConfigError::Invalid { key, .. } if key == "server.ssl.bundle"),
        "{err:?}"
    );
    let err = WebServer::with_ssl(App::new(), &config, None).unwrap_err();
    assert!(
        matches!(&err, ConfigError::Invalid { key, .. } if key == "server.ssl.bundle"),
        "{err:?}"
    );
    assert!(err.to_string().contains("none"), "{err}");

    // Without the key both build a plain HTTP server.
    let plain = Config::from_str("[server]\nport = 0").unwrap();
    assert!(WebServer::new(App::new(), &plain).is_ok());
    assert!(WebServer::with_ssl(App::new(), &plain, None).is_ok());
}
