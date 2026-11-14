//! `App::serve_tls` end to end with a rustls client and certificates made at test time.
#![cfg(feature = "tls")]

use std::cell::RefCell;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use oxbloom_core::HttpMethod;
use oxbloom_web::{App, ForwardStrategy, ForwardedConfig, Route, TrustedProxies};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tracing_subscriber::fmt::MakeWriter;

/// A test CA, a `localhost` server certificate it signed, and the configs built from them.
struct Pki {
    acceptor: TlsAcceptor,
    ca: CertificateDer<'static>,
}

impl Pki {
    fn new() -> Self {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::new(ca_params, ca_key);

        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&key, &issuer)
            .unwrap();

        let mut server = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap();
        server.alpn_protocols = vec![b"http/1.1".to_vec()];
        Self {
            acceptor: TlsAcceptor::from(Arc::new(server)),
            ca: ca.der().clone(),
        }
    }

    fn connector(&self, alpn: &[&[u8]]) -> TlsConnector {
        let mut roots = RootCertStore::empty();
        roots.add(self.ca.clone()).unwrap();
        let mut client = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        TlsConnector::from(Arc::new(client))
    }

    async fn connect(
        &self,
        addr: SocketAddr,
        alpn: &[&[u8]],
    ) -> io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let tcp = TcpStream::connect(addr).await?;
        let name = ServerName::try_from("localhost").unwrap();
        self.connector(alpn).connect(name, tcp).await
    }
}

struct TestServer {
    addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<io::Result<()>>,
}

impl TestServer {
    async fn start(app: App, acceptor: TlsAcceptor) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let task = tokio::spawn(app.serve_tls(listener, acceptor, async {
            let _ = rx.await;
        }));
        Self {
            addr,
            stop: Some(tx),
            task,
        }
    }

    /// Signals shutdown and waits at most `limit` for `serve_tls` to return.
    async fn stop_within(mut self, limit: Duration) {
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(limit, self.task)
            .await
            .expect("serve_tls did not return in time")
            .unwrap()
            .unwrap();
    }
}

fn app() -> App {
    App::new()
        .route(Route::new(HttpMethod::Get, "/hello", |_| async { "hi" }))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/who", |r| async move {
            format!("scheme={:?}", r.scheme())
        }))
        .unwrap()
}

/// Reads until the peer closes. A peer that closes without a TLS close_notify ends the read
/// with `UnexpectedEof`, which counts as the end here.
async fn read_all(mut s: impl AsyncRead + Unpin) -> String {
    let mut out = Vec::new();
    match s.read_to_end(&mut out).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {}
        Err(e) => panic!("read failed: {e}"),
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn get(path: &str, extra: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra}\r\n")
}

fn body(res: &str) -> &str {
    res.split("\r\n\r\n").nth(1).unwrap_or("")
}

#[tokio::test]
async fn tls_request_gets_a_response() {
    let pki = Pki::new();
    let srv = TestServer::start(app(), pki.acceptor.clone()).await;

    let mut tls = pki.connect(srv.addr, &[b"http/1.1"]).await.unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));
    tls.write_all(get("/hello", "").as_bytes()).await.unwrap();
    let res = read_all(tls).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(body(&res), "hi");

    // A client that offers no ALPN at all still speaks HTTP/1.1.
    let mut tls = pki.connect(srv.addr, &[]).await.unwrap();
    tls.write_all(get("/hello", "").as_bytes()).await.unwrap();
    assert!(read_all(tls).await.starts_with("HTTP/1.1 200"));

    srv.stop_within(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn client_info_scheme_is_https() {
    let pki = Pki::new();
    let srv = TestServer::start(app(), pki.acceptor.clone()).await;
    let mut tls = pki.connect(srv.addr, &[b"http/1.1"]).await.unwrap();
    tls.write_all(get("/who", "").as_bytes()).await.unwrap();
    assert_eq!(body(&read_all(tls).await), "scheme=Https");
    srv.stop_within(Duration::from_secs(5)).await;

    // A trusted proxy's forwarded headers keep their meaning: it says the client used http.
    let trusting = app().forwarded(ForwardedConfig {
        strategy: ForwardStrategy::Framework,
        trusted: TrustedProxies::loopback(),
    });
    let srv = TestServer::start(trusting, pki.acceptor.clone()).await;
    let mut tls = pki.connect(srv.addr, &[b"http/1.1"]).await.unwrap();
    let extra = "X-Forwarded-For: 203.0.113.9\r\nX-Forwarded-Proto: http\r\n";
    tls.write_all(get("/who", extra).as_bytes()).await.unwrap();
    assert_eq!(body(&read_all(tls).await), "scheme=Http");
    srv.stop_within(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn graceful_shutdown_drains_an_open_tls_connection() {
    let pki = Pki::new();
    let (started_tx, mut started) = mpsc::channel::<()>(1);
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/slow", move |_| {
            let started_tx = started_tx.clone();
            async move {
                let _ = started_tx.send(()).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                "done"
            }
        }))
        .unwrap();
    let srv = TestServer::start(app, pki.acceptor.clone()).await;

    let mut tls = pki.connect(srv.addr, &[b"http/1.1"]).await.unwrap();
    tls.write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    started.recv().await.unwrap();

    // Shutdown starts while the request is in flight; the answer must still arrive.
    let addr = srv.addr;
    let stopping = tokio::spawn(srv.stop_within(Duration::from_secs(5)));
    let res = read_all(tls).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(res.ends_with("done"), "{res}");
    stopping.await.unwrap();
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn plain_http_on_the_tls_port_is_closed_and_does_not_hang_shutdown() {
    let pki = Pki::new();
    let srv = TestServer::start(app(), pki.acceptor.clone()).await;

    let mut plain = TcpStream::connect(srv.addr).await.unwrap();
    plain.write_all(get("/hello", "").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(2), plain.read_to_end(&mut out)).await;
    assert!(read.is_ok(), "plain HTTP connection was left open");
    assert!(!String::from_utf8_lossy(&out).contains("HTTP/1.1"));

    // A client stuck before its handshake must not hold up shutdown (the timeout is 30 s).
    let _idle = TcpStream::connect(srv.addr).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    srv.stop_within(Duration::from_secs(2)).await;
}

#[tokio::test(start_paused = true)]
async fn client_that_never_finishes_the_handshake_is_dropped_after_ten_seconds() {
    let pki = Pki::new();
    let srv = TestServer::start(app(), pki.acceptor.clone()).await;

    let started = tokio::time::Instant::now();
    let mut silent = TcpStream::connect(srv.addr).await.unwrap();
    let mut buf = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(30), silent.read(&mut buf)).await;
    let elapsed = started.elapsed();

    let n = read
        .expect("the server kept the silent connection open")
        .unwrap_or(0);
    assert_eq!(n, 0, "the server sent bytes to a silent client");
    assert!(elapsed >= Duration::from_secs(10), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(11), "{elapsed:?}");

    srv.stop_within(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn h2_only_alpn_is_refused() {
    let pki = Pki::new();
    let srv = TestServer::start(app(), pki.acceptor.clone()).await;
    let refused = pki.connect(srv.addr, &[b"h2"]).await;
    assert!(
        refused.is_err(),
        "an h2-only client completed the handshake"
    );
    srv.stop_within(Duration::from_secs(5)).await;
}

thread_local! {
    static CAPTURE: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

/// Writes into the buffer of the current thread's capture, or nowhere.
struct ThreadBuf;

impl io::Write for ThreadBuf {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        CAPTURE.with(|c| {
            if let Some(buf) = &*c.borrow() {
                buf.lock().unwrap().extend_from_slice(data);
            }
        });
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for ThreadBuf {
    type Writer = ThreadBuf;

    fn make_writer(&'a self) -> Self::Writer {
        ThreadBuf
    }
}

/// One global subscriber for this test binary; thread-scoped subscribers race with the
/// callsite interest cache when tests run in parallel.
fn init_tracing() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(ThreadBuf)
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
    });
}

#[tokio::test]
async fn failed_handshake_logs_at_debug_without_bytes() {
    init_tracing();
    let buf = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.with(|c| *c.borrow_mut() = Some(buf.clone()));

    let pki = Pki::new();
    let srv = TestServer::start(app(), pki.acceptor.clone()).await;
    let mut plain = TcpStream::connect(srv.addr).await.unwrap();
    let peer = plain.local_addr().unwrap();
    plain
        .write_all(get("/secret-token-4711", "").as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), plain.read_to_end(&mut out)).await;
    srv.stop_within(Duration::from_secs(5)).await;
    CAPTURE.with(|c| *c.borrow_mut() = None);

    let logs = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    let line = logs
        .lines()
        .find(|l| l.contains("oxbloom::web") && l.contains("TLS handshake failed"))
        .unwrap_or_else(|| panic!("no handshake failure event in {logs:?}"));
    assert!(line.contains("DEBUG"), "{line}");
    assert!(line.contains(&peer.to_string()), "{line}");
    assert!(line.contains("kind=InvalidData"), "{line}");
    assert!(!logs.contains("secret-token-4711"), "{logs}");
    assert!(!logs.contains("GET "), "{logs}");
}
