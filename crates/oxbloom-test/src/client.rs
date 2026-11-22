//! A test client for an `oxbloom_web::App`, in memory or over real HTTP/1.1.
//!
//! [`TestClient`] has one request builder over three transports: an [`App`] called in memory
//! through [`App::handle_from`], a server the client owns on a free loopback port, and a server
//! that runs elsewhere, over HTTP or, with the feature `tls`, over HTTPS. Every transport
//! answers with a [`TestResponse`], so one assertion style works for all of them.
//!
//! Every [`RequestBuilder::send`] and [`TestClient::shutdown`] is bounded at 10 seconds. A
//! connection error or a timeout panics with the method, the path without its query string and
//! the cause. Header values and bodies never appear in a panic message, so a bearer token
//! cannot leak into a test log.

use std::error::Error as StdError;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, Method, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http1::{self, SendRequest};
use hyper_util::rt::TokioIo;
use oxbloom_web::{App, HttpMethod, Request, Response};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// How long one [`RequestBuilder::send`] may take, from the first byte to the whole answer.
pub(crate) const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// The panic message of a `ca_pem` without a usable certificate. It is fixed, so the PEM text
/// never reaches a test log.
#[cfg(feature = "tls")]
const BAD_CA_PEM: &str = "connect_tls: the CA PEM holds no valid certificate";

/// The server name every HTTPS connection sends and verifies: test certificates are issued for
/// `localhost`, whatever loopback address the server listens on.
#[cfg(feature = "tls")]
const TLS_SERVER_NAME: &str = "localhost";

/// How long [`TestClient::shutdown`] waits for an owned server to stop.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// The peer address an in-memory request comes from unless [`TestClient::peer`] changes it.
const DEFAULT_PEER: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 50000);

/// The keep-alive connection of the served and connected transports.
///
/// It holds at most one idle HTTP/1.1 connection. A request takes it, or opens a new one when
/// there is none or it has closed, and puts it back once the answer is read and the connection
/// can take the next request. So sequential requests reuse one connection deterministically.
/// A pooling client such as hyper-util's legacy `Client` hands a finished connection back to
/// its pool from a background task and meanwhile starts a speculative second connection, so
/// the number of connections it opens depends on timing.
struct Http {
    idle: Mutex<Option<SendRequest<Full<Bytes>>>>,
    /// Wraps each new connection in TLS when set, for [`TestClient::connect_tls`].
    #[cfg(feature = "tls")]
    tls: Option<tokio_rustls::TlsConnector>,
}

/// Creates the connection holder; nothing connects until the first request.
fn http_client() -> Http {
    Http {
        idle: Mutex::new(None),
        #[cfg(feature = "tls")]
        tls: None,
    }
}

/// Builds a TLS connector that trusts only the certificates in `ca_pem`, with the
/// `aws-lc-rs` provider and ALPN `http/1.1`. Returns `None` when `ca_pem` holds a PEM error, a
/// certificate that cannot be a trust anchor, or no certificate at all.
#[cfg(feature = "tls")]
fn tls_connector(ca_pem: &str) -> Option<tokio_rustls::TlsConnector> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;

    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(ca_pem.as_bytes()) {
        roots.add(cert.ok()?).ok()?;
    }
    if roots.is_empty() {
        return None;
    }
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the aws-lc-rs provider supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Some(tokio_rustls::TlsConnector::from(std::sync::Arc::new(
        config,
    )))
}

/// Runs the HTTP/1.1 handshake on `stream` and spawns the connection task.
async fn handshake<S>(stream: S) -> Result<SendRequest<Full<Bytes>>, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sender, conn) = http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|e| cause(&e))?;
    // The connection task ends, and closes the socket, once its sender is dropped.
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

impl Http {
    /// Returns whether new connections are wrapped in TLS.
    #[cfg(feature = "tls")]
    fn is_tls(&self) -> bool {
        self.tls.is_some()
    }

    /// Returns whether new connections are wrapped in TLS; never without the feature `tls`.
    #[cfg(not(feature = "tls"))]
    fn is_tls(&self) -> bool {
        false
    }

    /// Takes the idle connection if it can take a request, or opens a new one to `addr`.
    async fn take(&self, addr: SocketAddr) -> Result<SendRequest<Full<Bytes>>, String> {
        let idle = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut sender) = idle
            && sender.ready().await.is_ok()
        {
            return Ok(sender);
        }
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("tcp connect error: {}", cause(&e)))?;
        let _ = stream.set_nodelay(true);
        #[cfg(feature = "tls")]
        if let Some(connector) = &self.tls {
            let name = rustls::pki_types::ServerName::try_from(TLS_SERVER_NAME)
                .expect("localhost is a valid server name");
            let stream = connector
                .connect(name, stream)
                .await
                .map_err(|e| format!("tls handshake error: {}", cause(&e)))?;
            return handshake(stream).await;
        }
        handshake(stream).await
    }

    /// Waits until `sender` can take the next request and keeps it as the idle connection.
    /// A connection the server closed is dropped instead.
    async fn give_back(&self, mut sender: SendRequest<Full<Bytes>>) {
        if sender.ready().await.is_ok() {
            let mut idle = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
            if idle.is_none() {
                *idle = Some(sender);
            }
        }
    }
}

/// Sends requests to an [`App`] in memory, through a server the client owns, or to a server
/// that runs elsewhere.
///
/// - [`TestClient::in_memory`] calls [`App::handle_from`], which applies the body limit, the
///   client resolution and the forwarded headers like the server does, without a socket.
/// - [`TestClient::serve`] binds `127.0.0.1:0`, runs [`App::serve`] in a task and sends real
///   HTTP/1.1 with keep-alive. Dropping the client triggers the server's shutdown without
///   waiting; [`TestClient::shutdown`] waits for it.
/// - [`TestClient::connect`] sends real HTTP/1.1 to a server it does not own.
/// - [`TestClient::connect_tls`] (feature `tls`) sends real HTTPS with HTTP/1.1 to a server it
///   does not own, trusting a given CA.
///
/// The socket transports keep one connection alive and reuse it for the next request. They
/// add the transport headers that an in-memory call does not have: requests get `Host`, and
/// `Content-Length` when they carry a body; answers get `Date` and `Content-Length`. A path
/// that does not start with `/`, or holds characters a URI does not allow, makes the socket
/// transports panic, while in memory it is taken as is.
///
/// Test a `413` for a body over `server.max-body-bytes` in memory: over a socket the server
/// answers `413` and closes the connection before it has read the whole body, which can reset
/// the connection while the client is still sending.
///
/// The 10 second bounds are Tokio timers, so they cannot interrupt a handler that blocks the
/// thread on a current-thread runtime, and under `#[tokio::test(start_paused = true)]` Tokio
/// advances paused time on its own when every task waits, which can fire them early.
///
/// # Examples
///
/// ```
/// use oxbloom_test::TestClient;
/// use oxbloom_web::{App, HttpMethod, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
/// let client = TestClient::in_memory(app);
/// let res = client.get("/ping").send().await;
/// assert_eq!(res.status(), 200);
/// assert_eq!(res.text(), "pong");
/// # Ok(())
/// # }
/// ```
pub struct TestClient {
    transport: Transport,
    peer: SocketAddr,
}

/// Where a [`TestClient`] sends its requests.
enum Transport {
    /// Calls [`App::handle_from`] without a socket.
    InMemory(Box<App>),
    /// Real HTTP/1.1 to a server the client owns.
    Served(Served),
    /// Real HTTP/1.1 to a server that runs elsewhere.
    Connected { addr: SocketAddr, http: Http },
}

/// A server owned by a [`TestClient`]. Dropping it signals the server to stop, without waiting.
struct Served {
    addr: SocketAddr,
    http: Http,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<io::Result<()>>>,
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl fmt::Debug for TestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = match &self.transport {
            Transport::InMemory(_) => "in_memory",
            Transport::Served(_) => "serve",
            Transport::Connected { http, .. } => {
                if http.is_tls() {
                    "connect_tls"
                } else {
                    "connect"
                }
            }
        };
        f.debug_struct("TestClient")
            .field("mode", &mode)
            .field("addr", &self.addr())
            .field("peer", &self.peer)
            .finish()
    }
}

impl TestClient {
    /// Creates a client that calls `app` in memory through [`App::handle_from`].
    ///
    /// Requests come from the peer `127.0.0.1:50000` unless [`peer`](Self::peer) changes it.
    /// A handler panic comes back as the 500 answer the `App` renders.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let client = TestClient::in_memory(app);
    /// assert_eq!(client.get("/ping").send().await.text(), "pong");
    /// assert_eq!(client.get("/nothing").send().await.status(), 404);
    /// # Ok(())
    /// # }
    /// ```
    pub fn in_memory(app: App) -> Self {
        Self {
            transport: Transport::InMemory(Box::new(app)),
            peer: DEFAULT_PEER,
        }
    }

    /// Serves `app` on `127.0.0.1:0` in a task and returns a client that sends real HTTP/1.1
    /// to it.
    ///
    /// The process environment is not read. Dropping the client signals the server to stop
    /// without waiting; [`shutdown`](Self::shutdown) waits for it.
    ///
    /// # Panics
    ///
    /// Panics when `127.0.0.1:0` cannot be bound, and when called outside a Tokio runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let client = TestClient::serve(app).await;
    /// assert!(client.addr().is_some());
    /// assert_eq!(client.get("/ping").send().await.text(), "pong");
    /// client.shutdown().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn serve(app: App) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap_or_else(|e| panic!("could not bind 127.0.0.1:0: {e}"));
        let addr = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("could not read the bound address: {e}"));
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(app.serve(listener, async move {
            let _ = stopped.await;
        }));
        Self {
            transport: Transport::Served(Served {
                addr,
                http: http_client(),
                stop: Some(stop),
                task: Some(task),
            }),
            peer: DEFAULT_PEER,
        }
    }

    /// Creates a client that sends real HTTP/1.1 to a server at `addr`, which it does not own.
    ///
    /// Nothing connects until the first request. A server that is not there makes
    /// [`RequestBuilder::send`] panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let server = TestClient::serve(app).await;
    /// let addr = server.addr().expect("a served client has an address");
    ///
    /// let client = TestClient::connect(addr);
    /// assert_eq!(client.get("/ping").send().await.text(), "pong");
    /// server.shutdown().await;
    /// # Ok(())
    /// # }
    /// ```
    pub fn connect(addr: SocketAddr) -> Self {
        Self {
            transport: Transport::Connected {
                addr,
                http: http_client(),
            },
            peer: DEFAULT_PEER,
        }
    }

    /// Creates a client that sends real HTTPS, with HTTP/1.1, to a server at `addr`, which it
    /// does not own, and trusts only the CA certificates in `ca_pem`.
    ///
    /// Every connection sends and verifies the server name `localhost`, whatever the IP of
    /// `addr` is, so the server certificate needs `localhost` among its DNS names. The client
    /// offers ALPN `http/1.1` and uses the `aws-lc-rs` crypto provider. Like
    /// [`connect`](Self::connect) it keeps one connection alive, nothing connects until the
    /// first request, and the TLS handshake counts toward the 10 second bound of
    /// [`RequestBuilder::send`]. A certificate the CA did not sign makes `send` panic with the
    /// method, the path and the handshake error.
    ///
    /// # Panics
    ///
    /// Panics with the fixed message `connect_tls: the CA PEM holds no valid certificate` when
    /// `ca_pem` holds no certificate, a PEM block that does not decode, or a certificate that
    /// cannot be parsed. The message never contains the PEM text.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    /// use rustls::ServerConfig;
    /// use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    /// use tokio_rustls::TlsAcceptor;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// // A self-signed `localhost` certificate is its own CA.
    /// let key = rcgen::KeyPair::generate()?;
    /// let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])?.self_signed(&key)?;
    /// let mut tls = ServerConfig::builder().with_no_client_auth().with_single_cert(
    ///     vec![cert.der().clone()],
    ///     PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    /// )?;
    /// tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    ///
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    /// let addr = listener.local_addr()?;
    /// let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    /// let server = tokio::spawn(app.serve_tls(listener, TlsAcceptor::from(Arc::new(tls)), async {
    ///     let _ = stopped.await;
    /// }));
    ///
    /// let client = TestClient::connect_tls(addr, &cert.pem());
    /// assert_eq!(client.get("/ping").send().await.text(), "pong");
    /// drop(client);
    /// let _ = stop.send(());
    /// server.await??;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "tls")]
    pub fn connect_tls(addr: SocketAddr, ca_pem: &str) -> Self {
        let Some(connector) = tls_connector(ca_pem) else {
            panic!("{BAD_CA_PEM}");
        };
        let mut http = http_client();
        http.tls = Some(connector);
        Self {
            transport: Transport::Connected { addr, http },
            peer: DEFAULT_PEER,
        }
    }

    /// Sets the peer address that in-memory requests come from (default `127.0.0.1:50000`).
    ///
    /// The served and connected transports use the real socket address and ignore it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ip", |r: Request| async move {
    ///     r.client_addr().map(|a| a.ip().to_string()).unwrap_or_default()
    /// }))?;
    /// let client = TestClient::in_memory(app).peer("192.0.2.7:40000".parse()?);
    /// assert_eq!(client.get("/ip").send().await.text(), "192.0.2.7");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn peer(mut self, peer: SocketAddr) -> Self {
        self.peer = peer;
        self
    }

    /// Returns the server address of a served or connected client, and `None` in memory.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::App;
    ///
    /// let client = TestClient::in_memory(App::new());
    /// assert_eq!(client.addr(), None);
    /// let client = TestClient::connect("127.0.0.1:8080".parse().unwrap());
    /// assert_eq!(client.addr(), "127.0.0.1:8080".parse().ok());
    /// ```
    pub fn addr(&self) -> Option<SocketAddr> {
        match &self.transport {
            Transport::InMemory(_) => None,
            Transport::Served(served) => Some(served.addr),
            Transport::Connected { addr, .. } => Some(*addr),
        }
    }

    /// Stops the server a served client owns and waits for it. For the other transports it
    /// does nothing.
    ///
    /// The client's idle connections close first, then the server drains its connections and
    /// stops.
    ///
    /// # Panics
    ///
    /// Panics with `the server did not stop within 10 seconds` when it takes longer, with
    /// `the server failed: ` and the error when [`App::serve`] returned one, and with
    /// `the server task panicked` when it panicked.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let client = TestClient::serve(app).await;
    /// let addr = client.addr().unwrap();
    /// client.shutdown().await;
    /// assert!(tokio::net::TcpStream::connect(addr).await.is_err());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn shutdown(self) {
        let Transport::Served(mut served) = self.transport else {
            return;
        };
        let task = served.task.take();
        // Dropping `served` closes the client's idle connection and signals the server.
        drop(served);
        let Some(task) = task else { return };
        let abort = task.abort_handle();
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, task).await {
            Err(_) => {
                abort.abort();
                panic!("the server did not stop within 10 seconds")
            }
            Ok(Err(_)) => panic!("the server task panicked"),
            Ok(Ok(Err(e))) => panic!("the server failed: {e}"),
            Ok(Ok(Ok(()))) => {}
        }
    }

    /// Starts a `GET` request to `path`, which may carry a `?query`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/search", |r: Request| async move {
    ///     r.query.unwrap_or_default()
    /// }))?;
    /// let client = TestClient::in_memory(app);
    /// assert_eq!(client.get("/search?q=rust").send().await.text(), "q=rust");
    /// # Ok(())
    /// # }
    /// ```
    pub fn get(&self, path: &str) -> RequestBuilder<'_> {
        self.request(Method::GET, path)
    }

    /// Starts a `POST` request to `path`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Post, "/echo", |r: Request| async move {
    ///     String::from_utf8_lossy(&r.body).into_owned()
    /// }))?;
    /// let client = TestClient::in_memory(app);
    /// assert_eq!(client.post("/echo").body("hello").send().await.text(), "hello");
    /// # Ok(())
    /// # }
    /// ```
    pub fn post(&self, path: &str) -> RequestBuilder<'_> {
        self.request(Method::POST, path)
    }

    /// Starts a `PUT` request to `path`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Put, "/item", |_| async { "stored" }))?;
    /// let client = TestClient::in_memory(app);
    /// assert_eq!(client.put("/item").body("v").send().await.text(), "stored");
    /// # Ok(())
    /// # }
    /// ```
    pub fn put(&self, path: &str) -> RequestBuilder<'_> {
        self.request(Method::PUT, path)
    }

    /// Starts a `PATCH` request to `path`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Patch, "/item", |_| async { "patched" }))?;
    /// let client = TestClient::in_memory(app);
    /// assert_eq!(client.patch("/item").send().await.text(), "patched");
    /// # Ok(())
    /// # }
    /// ```
    pub fn patch(&self, path: &str) -> RequestBuilder<'_> {
        self.request(Method::PATCH, path)
    }

    /// Starts a `DELETE` request to `path`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/item", |_| async { "here" }))?;
    /// let client = TestClient::in_memory(app);
    /// assert_eq!(client.delete("/item").send().await.status(), 405);
    /// # Ok(())
    /// # }
    /// ```
    pub fn delete(&self, path: &str) -> RequestBuilder<'_> {
        self.request(Method::DELETE, path)
    }

    /// Starts a `HEAD` request to `path`. The answer has the status and headers of the `GET`
    /// route and no body.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let client = TestClient::in_memory(app);
    /// let res = client.head("/ping").send().await;
    /// assert_eq!(res.status(), 200);
    /// assert!(res.bytes().is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn head(&self, path: &str) -> RequestBuilder<'_> {
        self.request(Method::HEAD, path)
    }

    /// Starts an `OPTIONS` request to `path`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let client = TestClient::in_memory(app);
    /// let res = client.options("/ping").send().await;
    /// assert!(res.header("allow").unwrap_or_default().contains("GET"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn options(&self, path: &str) -> RequestBuilder<'_> {
        self.request(Method::OPTIONS, path)
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder<'_> {
        RequestBuilder {
            client: self,
            method,
            path: path.to_owned(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
        }
    }
}

/// A request under construction, started by a [`TestClient`] method such as
/// [`TestClient::get`] and sent with [`send`](Self::send).
///
/// Its `Debug` output shows the method, the path without its query string, the number of
/// headers and the body length, never header values or the body.
///
/// # Examples
///
/// ```
/// use oxbloom_test::TestClient;
/// use oxbloom_web::{App, HttpMethod, Request, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new().route(Route::new(HttpMethod::Post, "/len", |r: Request| async move {
///     r.body.len().to_string()
/// }))?;
/// let client = TestClient::in_memory(app);
/// let res = client
///     .post("/len")
///     .header("content-type", "text/plain")
///     .body("four")
///     .send()
///     .await;
/// assert_eq!(res.text(), "4");
/// # Ok(())
/// # }
/// ```
pub struct RequestBuilder<'a> {
    client: &'a TestClient,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Bytes,
}

impl fmt::Debug for RequestBuilder<'_> {
    /// Shows counts only: header values and the body may contain secrets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBuilder")
            .field("method", &self.method)
            .field("path", &path_only(&self.path))
            .field("headers", &self.headers.len())
            .field("body", &self.body.len())
            .finish_non_exhaustive()
    }
}

impl RequestBuilder<'_> {
    /// Adds a header. A name that was set before keeps its earlier values too.
    ///
    /// # Panics
    ///
    /// Panics when the name or the value is not a valid header name or value. The message
    /// names the method and the path, never the value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/me", |r: Request| async move {
    ///     let auth = r.headers.get("authorization").and_then(|v| v.to_str().ok());
    ///     if auth == Some("Bearer t0ken") { "alice" } else { "nobody" }
    /// }))?;
    /// let client = TestClient::in_memory(app);
    /// let res = client.get("/me").header("authorization", "Bearer t0ken").send().await;
    /// assert_eq!(res.text(), "alice");
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn header<K, V>(mut self, name: K, value: V) -> Self
    where
        K: TryInto<HeaderName>,
        V: TryInto<HeaderValue>,
    {
        let Ok(name) = name.try_into() else {
            panic!(
                "{} {}: the header name is not valid",
                self.method,
                path_only(&self.path)
            );
        };
        let Ok(value) = value.try_into() else {
            panic!(
                "{} {}: the value of header {name} is not valid",
                self.method,
                path_only(&self.path)
            );
        };
        self.headers.append(name, value);
        self
    }

    /// Sets the request body.
    ///
    /// Test a `413` for a body over the limit with [`TestClient::in_memory`]: over a socket the
    /// server answers `413` and closes the connection before it has read the whole body, which
    /// can reset the connection while the client is still sending.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new()
    ///     .max_body_bytes(8)
    ///     .route(Route::new(HttpMethod::Post, "/echo", |r: Request| async move {
    ///         String::from_utf8_lossy(&r.body).into_owned()
    ///     }))?;
    /// let client = TestClient::in_memory(app);
    /// assert_eq!(client.post("/echo").body("short").send().await.text(), "short");
    /// let res = client.post("/echo").body(vec![b'x'; 64]).send().await;
    /// assert_eq!(res.status(), 413);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        self.body = body.into();
        self
    }

    /// Sets the body to `value` as JSON and the `Content-Type` to `application/json`.
    ///
    /// # Panics
    ///
    /// Panics when `value` cannot be serialized as JSON, such as a map with keys that are not
    /// strings. The message names the method, the path and the kind of error, never the value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Post, "/echo", |r: Request| async move {
    ///     String::from_utf8_lossy(&r.body).into_owned()
    /// }))?;
    /// let client = TestClient::in_memory(app);
    /// let res = client.post("/echo").json(&serde_json::json!({"n": 1})).send().await;
    /// assert_eq!(res.text(), r#"{"n":1}"#);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "json")]
    #[must_use]
    pub fn json<T: serde::Serialize + ?Sized>(mut self, value: &T) -> Self {
        match serde_json::to_vec(value) {
            Ok(body) => self.body = body.into(),
            Err(e) => panic!(
                "{} {}: the JSON body could not be serialized ({:?} error)",
                self.method,
                path_only(&self.path),
                e.classify()
            ),
        }
        self.headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        self
    }

    /// Sends the request and waits at most 10 seconds for the whole answer.
    ///
    /// # Panics
    ///
    /// Panics with `{METHOD} {path} failed: ` and the cause when the connection fails, the
    /// path does not start with `/` on a socket transport, or no answer arrives within 10
    /// seconds. The path in the message has no query string, and no header value or body
    /// is part of it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let client = TestClient::serve(app).await;
    /// let res = client.get("/ping").send().await;
    /// assert_eq!(res.status(), 200);
    /// client.shutdown().await;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send(self) -> TestResponse {
        let RequestBuilder {
            client,
            method,
            path,
            headers,
            body,
        } = self;
        let target = path_only(&path).to_owned();
        let label = method.clone();
        let exchange = async {
            match &client.transport {
                Transport::InMemory(app) => {
                    let mut req = Request::new(HttpMethod::from(&method), target.clone());
                    req.query = path.split_once('?').map(|(_, q)| q.to_owned());
                    req.headers = headers;
                    req.body = body;
                    Ok(app.handle_from(req, Some(client.peer)).await)
                }
                Transport::Served(served) => {
                    remote(&served.http, served.addr, method, &path, headers, body).await
                }
                Transport::Connected { addr, http } => {
                    remote(http, *addr, method, &path, headers, body).await
                }
            }
        };
        match tokio::time::timeout(SEND_TIMEOUT, exchange).await {
            Ok(Ok(inner)) => TestResponse { inner },
            Ok(Err(cause)) => panic!("{label} {target} failed: {cause}"),
            Err(_) => panic!("{label} {target} failed: no answer within 10 seconds"),
        }
    }
}

/// Returns `path` without its query string.
fn path_only(path: &str) -> &str {
    path.split_once('?').map_or(path, |(p, _)| p)
}

/// Sends one request over real HTTP/1.1 and buffers the answer. The error is the cause
/// chain as text, which never holds header values or the body.
async fn remote(
    http: &Http,
    addr: SocketAddr,
    method: Method,
    path: &str,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, String> {
    if !path.starts_with('/') {
        return Err("the path does not start with '/'".to_owned());
    }
    let uri: http::Uri = path.parse().map_err(|e| cause(&e))?;
    let mut req = http::Request::new(Full::new(body));
    *req.method_mut() = method;
    *req.uri_mut() = uri;
    *req.headers_mut() = headers;
    if !req.headers().contains_key(http::header::HOST) {
        let host = HeaderValue::try_from(addr.to_string()).map_err(|e| cause(&e))?;
        req.headers_mut().insert(http::header::HOST, host);
    }
    let mut sender = http.take(addr).await?;
    let res = sender.send_request(req).await.map_err(|e| cause(&e))?;
    let (parts, incoming) = res.into_parts();
    let bytes = incoming.collect().await.map_err(|e| cause(&e))?.to_bytes();
    http.give_back(sender).await;
    Ok(Response::from_parts(parts, bytes))
}

/// The error and its sources, joined with `: `.
fn cause(error: &dyn StdError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(e) = source {
        text.push_str(": ");
        text.push_str(&e.to_string());
        source = e.source();
    }
    text
}

/// The answer to a [`TestClient`] request, the same type for every transport.
///
/// It wraps an [`oxbloom_web::Response`] (`http::Response<Bytes>`) whose body is fully read.
///
/// # Examples
///
/// ```
/// use oxbloom_test::TestClient;
/// use oxbloom_web::{App, HttpMethod, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
/// let res = TestClient::in_memory(app).get("/ping").send().await;
/// assert_eq!(res.status(), 200);
/// assert_eq!(res.header("content-type"), Some("text/plain; charset=utf-8"));
/// assert_eq!(res.text(), "pong");
/// # Ok(())
/// # }
/// ```
pub struct TestResponse {
    inner: Response,
}

impl fmt::Debug for TestResponse {
    /// Shows the status and counts only: headers such as `Set-Cookie` may contain secrets.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestResponse")
            .field("status", &self.inner.status())
            .field("headers", &self.inner.headers().len())
            .field("body", &self.inner.body().len())
            .finish()
    }
}

impl TestResponse {
    /// Returns the status code.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::App;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let res = TestClient::in_memory(App::new()).get("/nothing").send().await;
    /// assert_eq!(res.status(), StatusCode::NOT_FOUND);
    /// # }
    /// ```
    pub fn status(&self) -> StatusCode {
        self.inner.status()
    }

    /// Returns the first value of the header `name` as text, or `None` when the header is
    /// missing or its value is not valid UTF-8 text.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let res = TestClient::in_memory(app).get("/ping").send().await;
    /// assert_eq!(res.header("Content-Type"), Some("text/plain; charset=utf-8"));
    /// assert_eq!(res.header("x-missing"), None);
    /// # Ok(())
    /// # }
    /// ```
    pub fn header(&self, name: &str) -> Option<&str> {
        self.inner.headers().get(name)?.to_str().ok()
    }

    /// Returns every value of the header `name` as text, in order, such as each `Set-Cookie`
    /// of a response. A value that is not valid UTF-8 text is left out.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/two", |_| async {
    ///     http::Response::builder()
    ///         .header("set-cookie", "a=1")
    ///         .header("set-cookie", "b=2")
    ///         .body(bytes::Bytes::new())
    ///         .unwrap()
    /// }))?;
    /// let res = TestClient::in_memory(app).get("/two").send().await;
    /// assert_eq!(res.header_all("set-cookie"), ["a=1", "b=2"]);
    /// assert!(res.header_all("x-missing").is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub fn header_all(&self, name: &str) -> Vec<&str> {
        self.inner
            .headers()
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect()
    }

    /// Returns the body bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let res = TestClient::in_memory(app).get("/ping").send().await;
    /// assert_eq!(res.bytes().as_ref(), b"pong");
    /// # Ok(())
    /// # }
    /// ```
    pub fn bytes(&self) -> &Bytes {
        self.inner.body()
    }

    /// Returns the body as text, with invalid UTF-8 replaced by `U+FFFD`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let res = TestClient::in_memory(app).get("/ping").send().await;
    /// assert_eq!(res.text(), "pong");
    /// # Ok(())
    /// # }
    /// ```
    pub fn text(&self) -> String {
        String::from_utf8_lossy(self.inner.body()).into_owned()
    }

    /// Parses the body as JSON into `T`.
    ///
    /// # Panics
    ///
    /// Panics when the body is not JSON of type `T`. The message names the type, the kind of
    /// error and its line and column, never the body.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Json, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/n", |_| async {
    ///     Json(serde_json::json!({"n": 1}))
    /// }))?;
    /// let res = TestClient::in_memory(app).get("/n").send().await;
    /// let value: serde_json::Value = res.json();
    /// assert_eq!(value["n"], 1);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "json")]
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        match serde_json::from_slice(self.inner.body()) {
            Ok(value) => value,
            Err(e) => panic!(
                "the response body is not JSON of type {} ({:?} error at line {} column {})",
                std::any::type_name::<T>(),
                e.classify(),
                e.line(),
                e.column()
            ),
        }
    }

    /// Returns the wrapped [`Response`], for checks the helpers do not cover.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::TestClient;
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let res = TestClient::in_memory(app).get("/ping").send().await.into_inner();
    /// assert_eq!(res.version(), http::Version::HTTP_11);
    /// assert_eq!(res.body().as_ref(), b"pong");
    /// # Ok(())
    /// # }
    /// ```
    pub fn into_inner(self) -> Response {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_loses_only_its_query_string() {
        assert_eq!(path_only("/a/b?x=1&y=2"), "/a/b");
        assert_eq!(path_only("/a"), "/a");
        assert_eq!(path_only("/a?"), "/a");
    }

    #[test]
    fn the_send_bound_is_ten_seconds() {
        assert_eq!(SEND_TIMEOUT, Duration::from_secs(10));
    }
}
