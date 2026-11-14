//! `TestClient::connect_tls` and `RunningApp::client_tls` send real HTTPS to a server whose
//! certificate is signed by a CA the test trusts.
//!
//! The certificates are generated with `rcgen` when the tests run; no key is committed.

#![cfg(all(feature = "web", feature = "tls"))]

use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use oxbloom_boot::SslAutoConfiguration;
use oxbloom_test::{TestApp, TestClient};
use oxbloom_web::{App, Controller, HttpMethod, Request, Route};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;

/// The fixed panic message of a `ca_pem` that holds no usable certificate.
const BAD_CA_MESSAGE: &str = "connect_tls: the CA PEM holds no valid certificate";

/// The text of a panic payload, for a `&str` or a `String` payload.
fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(text) => *text,
        Err(payload) => payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .unwrap_or_default(),
    }
}

/// A test CA that signs `localhost` server certificates.
struct Ca {
    issuer: Issuer<'static, KeyPair>,
    pem: String,
}

/// A server certificate and its private key.
struct Leaf {
    cert_pem: String,
    key_pem: String,
    der: CertificateDer<'static>,
    key_der: Vec<u8>,
}

impl Ca {
    fn new() -> Ca {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let cert = params.self_signed(&key).unwrap();
        Ca {
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
            key_der: key.serialize_der(),
        }
    }
}

/// The application the HTTPS server runs: a text route and an echo route.
fn app() -> App {
    App::new()
        .route(Route::new(HttpMethod::Get, "/hello", |_| async { "hi" }))
        .unwrap()
        .route(Route::new(
            HttpMethod::Post,
            "/echo",
            |r: Request| async move { String::from_utf8_lossy(&r.body).into_owned() },
        ))
        .unwrap()
}

/// An HTTPS server on a free loopback port that serves `leaf`. Dropping the sender stops it.
async fn https_server(leaf: &Leaf) -> (SocketAddr, oneshot::Sender<()>) {
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf.key_der.clone())),
        )
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(
        app().serve_tls(listener, TlsAcceptor::from(Arc::new(config)), async move {
            let _ = stopped.await;
        }),
    );
    (addr, stop)
}

#[tokio::test]
async fn connect_tls_sends_a_request_over_https() {
    let ca = Ca::new();
    let (addr, _stop) = https_server(&ca.leaf()).await;

    let client = TestClient::connect_tls(addr, &ca.pem);
    assert_eq!(client.addr(), Some(addr));
    let res = client.get("/hello").send().await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.text(), "hi");
    // The second request reuses the kept-alive connection.
    let res = client.post("/echo").body("over tls").send().await;
    assert_eq!(res.text(), "over tls");
    assert_eq!(res.header("content-length"), Some("8"));
}

#[tokio::test]
async fn wrong_ca_panics_with_method_and_path_only() {
    let ca = Ca::new();
    let other = Ca::new();
    let (addr, _stop) = https_server(&ca.leaf()).await;

    let pem = other.pem.clone();
    let task = tokio::spawn(async move {
        let client = TestClient::connect_tls(addr, &pem);
        client
            .get("/hello?token=q5ecret")
            .header("authorization", "Bearer h5ecret")
            .body("b5ecret")
            .send()
            .await
    });
    let text = panic_text(task.await.unwrap_err().into_panic());
    assert!(text.starts_with("GET /hello failed: "), "{text}");
    assert!(text.contains("certificate"), "{text}");
    for secret in ["q5ecret", "h5ecret", "b5ecret", "token", "BEGIN"] {
        assert!(!text.contains(secret), "{text}");
    }
    let body = other.pem.lines().nth(1).unwrap();
    assert!(!text.contains(body), "{text}");
}

#[test]
fn malformed_ca_pem_panics_with_a_fixed_message_without_the_pem_text() {
    let addr: SocketAddr = "127.0.0.1:8443".parse().unwrap();
    let cases = [
        // Not PEM at all.
        "no pem here m4rker".to_owned(),
        // A PEM block whose body is not base64.
        "-----BEGIN CERTIFICATE-----\nm4rker not base64 !!\n-----END CERTIFICATE-----\n".to_owned(),
        // Valid base64 that is not a certificate.
        "-----BEGIN CERTIFICATE-----\nbTRya2VyIGJ1dCBub3QgYSBjZXJ0aWZpY2F0ZQ==\n-----END CERTIFICATE-----\n"
            .to_owned(),
        // A private key instead of a certificate.
        Ca::new().leaf().key_pem,
    ];
    for pem in cases {
        let payload = catch_unwind(AssertUnwindSafe(|| TestClient::connect_tls(addr, &pem)))
            .expect_err("a malformed CA PEM panics");
        let text = panic_text(payload);
        assert_eq!(text, BAD_CA_MESSAGE);
        assert!(!text.contains("m4rker"), "{text}");
    }
}

struct Hello;

impl Controller for Hello {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/hello", |_| async { "hello" })]
    }
}

#[tokio::test]
async fn client_tls_of_a_running_app_reaches_the_server() {
    let ca = Ca::new();
    let leaf = ca.leaf();
    let app = TestApp::web()
        .auto_configuration::<SslAutoConfiguration>()
        .config_str(format!(
            "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
             [server.ssl]\nbundle = \"web\"\n",
            leaf.cert_pem, leaf.key_pem
        ))
        .configure(|b| {
            b.instance(Hello)
                .provides(|c: Arc<Hello>| -> Arc<dyn Controller> { c })
        })
        .start()
        .await;

    let client = app.client_tls(&ca.pem);
    assert_eq!(client.addr(), Some(app.addr()));
    let res = client.get("/hello").send().await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.text(), "hello");
    assert_eq!(client.get("/hello").send().await.text(), "hello");

    // The port speaks only HTTPS: the plain client of the app gets no answer.
    let plain = tokio::spawn({
        let addr = app.addr();
        async move { TestClient::connect(addr).get("/hello").send().await }
    });
    assert!(plain.await.unwrap_err().is_panic());

    // The client still holds its idle connection; the shutdown closes it.
    assert_eq!(app.stop().await.status().code(), 0);
    drop(client);
}
