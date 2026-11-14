use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use oxbloom_core::HttpMethod;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use crate::catch::{CatchUnwind, panic_response, request_info};
use crate::filter::{BodyLimitFilter, BodyTooLarge, ErrorHandlingFilter, run_pipeline};
use crate::forwarded::resolve_client;
use crate::{
    ClientInfo, ErrorRenderer, Filter, ForwardedConfig, ProblemRenderer, Request, Response, Route,
    RouteError, Router, RouterConfig, Scheme,
};

/// The longest header read timeout: one day.
const MAX_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(86_400);

/// How long a client may take to complete its TLS handshake.
#[cfg(feature = "tls")]
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Application builder and HTTP/1.1 server.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{App, HttpMethod, Request, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new()
///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
///
/// let res = app.handle(Request::new(HttpMethod::Get, "/ping")).await;
/// assert_eq!(res.body(), "pong");
/// // app.run("127.0.0.1:8080".parse()?).await?;   // serve for real
/// # Ok(())
/// # }
/// ```
pub struct App {
    router: Router,
    filters: Vec<Arc<dyn Filter>>,
    renderer: Arc<dyn ErrorRenderer>,
    /// The built-in [`ErrorHandlingFilter`] entry of `filters`, found by identity on replace.
    error_filter: Arc<ErrorHandlingFilter>,
    max_body_bytes: usize,
    header_read_timeout: Duration,
    shutdown_timeout: Duration,
    forwarded: ForwardedConfig,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// An empty app with the default [`RouterConfig`], 1 MiB body cap and 10 s header timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::App;
    ///
    /// let app = App::new();
    /// # let _ = app;
    /// ```
    pub fn new() -> Self {
        Self::with_config(RouterConfig::default())
    }

    /// An empty app with a custom [`RouterConfig`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, RouterConfig};
    ///
    /// let app = App::with_config(RouterConfig { allow_trace: true, ..RouterConfig::default() });
    /// # let _ = app;
    /// ```
    pub fn with_config(config: RouterConfig) -> Self {
        let renderer: Arc<dyn ErrorRenderer> = Arc::new(ProblemRenderer::new(false));
        let error_filter = Arc::new(ErrorHandlingFilter {
            renderer: Arc::clone(&renderer),
        });
        Self {
            router: Router::new(config),
            filters: vec![
                Arc::clone(&error_filter) as Arc<dyn Filter>,
                #[cfg(feature = "json")]
                Arc::new(crate::negotiation::JsonNegotiationFilter),
                Arc::new(BodyLimitFilter),
            ],
            renderer,
            error_filter,
            max_body_bytes: 1024 * 1024,
            header_read_timeout: Duration::from_secs(10),
            shutdown_timeout: Duration::from_secs(30),
            forwarded: ForwardedConfig::default(),
        }
    }

    /// Maximum accepted request body; larger bodies get 413. Default 1 MiB.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::App;
    ///
    /// let app = App::new().max_body_bytes(64 * 1024);
    /// # let _ = app;
    /// ```
    pub fn max_body_bytes(mut self, n: usize) -> Self {
        self.max_body_bytes = n;
        self
    }

    /// How long a client may take to send its request headers. Default 10 s.
    ///
    /// Values above one day are clamped to one day: the HTTP server adds the timeout to the
    /// current time, which panics on overflow for absurd values.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_web::App;
    ///
    /// let app = App::new().header_read_timeout(Duration::from_secs(5));
    /// # let _ = app;
    /// ```
    pub fn header_read_timeout(mut self, d: Duration) -> Self {
        self.header_read_timeout = d.min(MAX_HEADER_READ_TIMEOUT);
        self
    }

    /// How long [`App::serve`] (and `App::serve_tls`, feature `tls`) waits for open connections
    /// after the shutdown signal. When it expires, the remaining connections are dropped and a
    /// warning is logged on target `oxbloom::web`. Default 30 s.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_web::App;
    ///
    /// let app = App::new().shutdown_timeout(Duration::from_secs(5));
    /// # let _ = app;
    /// ```
    pub fn shutdown_timeout(mut self, d: Duration) -> Self {
        self.shutdown_timeout = d;
        self
    }

    /// Sets how [`App::serve`] resolves the client address, scheme and host of a request
    /// ([`Request::client_addr`], [`Request::scheme`], [`Request::host`]). The default,
    /// [`ForwardStrategy::None`](crate::ForwardStrategy::None), ignores forwarded headers.
    ///
    /// With [`ForwardStrategy::Framework`](crate::ForwardStrategy::Framework), the `Forwarded`
    /// header (or, when it is absent, `X-Forwarded-For`, `-Proto` and `-Host`) is honored only
    /// when the TCP peer is in `trusted`. The address list is walked from the right and the
    /// first untrusted hop is the client, so a client cannot spoof its address by prepending
    /// entries.
    ///
    /// Deployment requirement: every trusted proxy must set or strip `Forwarded`, overwrite or
    /// strip `X-Forwarded-Proto` and `X-Forwarded-Host`, and append to `X-Forwarded-For`.
    /// `Forwarded` is read first, so a proxy that only appends `X-Forwarded-For` (a common nginx
    /// setup) and passes a client's `Forwarded` through lets that client choose its address,
    /// scheme and host; the same holds for client-sent `X-Forwarded-Proto`/`-Host`. The
    /// framework cannot detect this.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, ForwardStrategy, ForwardedConfig, TrustedProxies};
    ///
    /// let app = App::new().forwarded(ForwardedConfig {
    ///     strategy: ForwardStrategy::Framework,
    ///     trusted: TrustedProxies::loopback(),
    /// });
    /// # let _ = app;
    /// ```
    pub fn forwarded(mut self, cfg: ForwardedConfig) -> Self {
        self.forwarded = cfg;
        self
    }

    /// The forwarded-header handling set by [`App::forwarded`], or the default (strategy
    /// [`ForwardStrategy::None`](crate::ForwardStrategy::None), loopback trusted).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, ForwardStrategy, ForwardedConfig, TrustedProxies};
    ///
    /// assert_eq!(App::new().forwarded_config(), &ForwardedConfig::default());
    /// let app = App::new().forwarded(ForwardedConfig {
    ///     strategy: ForwardStrategy::Framework,
    ///     trusted: TrustedProxies::loopback(),
    /// });
    /// assert_eq!(app.forwarded_config().strategy, ForwardStrategy::Framework);
    /// ```
    pub fn forwarded_config(&self) -> &ForwardedConfig {
        &self.forwarded
    }

    /// Adds a [`Filter`] that wraps routing. Filters run by [`Filter::order`] (ties in registration order), before the
    /// router, in both [`App::handle`] and the real server.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, Filter, HttpMethod, Next, Request, Response};
    ///
    /// struct Tag;
    ///
    /// #[async_trait::async_trait]
    /// impl Filter for Tag {
    ///     async fn filter(&self, req: Request, next: Next<'_>) -> Response {
    ///         let mut res = next.run(req).await;
    ///         res.headers_mut().insert("x-tag", "1".parse().unwrap());
    ///         res
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let app = App::new().filter(Tag);
    /// let res = app.handle(Request::new(HttpMethod::Get, "/nothing")).await;
    /// assert_eq!(res.status(), 404);
    /// assert_eq!(res.headers()["x-tag"], "1");
    /// # }
    /// ```
    pub fn filter(self, f: impl Filter + 'static) -> Self {
        let order = f.order();
        self.insert_filter(order, Arc::new(f))
    }

    /// Adds a [`Filter`] at an explicit `order`, overriding [`Filter::order`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, Filter, HttpMethod, Next, Request, Response, order};
    ///
    /// struct Tag;
    ///
    /// #[async_trait::async_trait]
    /// impl Filter for Tag {
    ///     async fn filter(&self, req: Request, next: Next<'_>) -> Response {
    ///         let mut res = next.run(req).await;
    ///         res.headers_mut().insert("x-tag", "1".parse().unwrap());
    ///         res
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let app = App::new().filter_ordered(order::SECURITY, Tag);
    /// let res = app.handle(Request::new(HttpMethod::Get, "/nothing")).await;
    /// assert_eq!(res.headers()["x-tag"], "1");
    /// # }
    /// ```
    pub fn filter_ordered(self, order: i32, f: impl Filter + 'static) -> Self {
        self.insert_filter(order, Arc::new(crate::Ordered(order, f)))
    }

    /// Registers a [`CorsFilter`](crate::CorsFilter) built from `config` at
    /// [`order::CORS`](crate::order::CORS), outside error handling and security, so error
    /// answers carry CORS headers and an allowed preflight never reaches security. Fails with
    /// [`CorsError`](crate::CorsError) when `config` is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, CorsConfig, HttpMethod, Request};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new().cors(CorsConfig::new().allowed_origins(["https://a.example"]))?;
    ///
    /// let mut req = Request::new(HttpMethod::Options, "/anything");
    /// req.headers.insert("origin", "https://a.example".parse()?);
    /// req.headers.insert("access-control-request-method", "POST".parse()?);
    /// let res = app.handle(req).await;
    /// assert_eq!(res.status(), 204);
    /// assert_eq!(res.headers()["access-control-allow-methods"], "GET, HEAD, POST");
    /// # Ok(())
    /// # }
    /// ```
    pub fn cors(self, config: crate::CorsConfig) -> Result<Self, crate::CorsError> {
        Ok(self.filter(crate::CorsFilter::new(config)?))
    }

    /// Registers a [`CompressionFilter`](crate::CompressionFilter) built from `config` at
    /// [`order::COMPRESSION`](crate::order::COMPRESSION), inside the body limit and outside
    /// security, so it compresses the answers of handlers and of security filters. It picks gzip,
    /// brotli or zstd from `Accept-Encoding`; without a codec feature nothing is compressed.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, CompressionConfig, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new()
    ///     .compression(CompressionConfig::new().min_size(16))
    ///     .route(Route::new(HttpMethod::Get, "/t", |_| async { "ab".repeat(100) }))?;
    /// let mut req = Request::new(HttpMethod::Get, "/t");
    /// req.headers.insert("accept-encoding", "identity".parse()?);
    /// let res = app.handle(req).await;
    /// assert!(res.headers().get("content-encoding").is_none());
    /// # Ok(())
    /// # }
    /// ```
    pub fn compression(self, config: crate::CompressionConfig) -> Self {
        self.filter(crate::CompressionFilter::new(config))
    }

    /// Replaces the [`ErrorRenderer`] that turns error answers (404, 405, 413, a panic, an
    /// [`HttpError`](crate::HttpError) or a bodiless 4xx/5xx) into responses. The default is
    /// [`ProblemRenderer::new(false)`](ProblemRenderer::new).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, ErrorAttributes, ErrorRenderer, HttpMethod, IntoResponse, Request};
    /// use oxbloom_web::{RequestInfo, Response};
    ///
    /// struct Plain;
    ///
    /// impl ErrorRenderer for Plain {
    ///     fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response {
    ///         let mut res = format!("{} at {}", attrs.status.as_u16(), info.path).into_response();
    ///         *res.status_mut() = attrs.status;
    ///         res
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let app = App::new().error_renderer(Plain);
    /// let res = app.handle(Request::new(HttpMethod::Get, "/missing")).await;
    /// assert_eq!(res.status(), 404);
    /// assert_eq!(res.body(), "404 at /missing");
    /// # }
    /// ```
    pub fn error_renderer(mut self, r: impl ErrorRenderer + 'static) -> Self {
        let renderer: Arc<dyn ErrorRenderer> = Arc::new(r);
        let error_filter = Arc::new(ErrorHandlingFilter {
            renderer: Arc::clone(&renderer),
        });
        let old = Arc::as_ptr(&self.error_filter).cast::<()>();
        // Same slot, same order: replacing in place keeps the vector sorted.
        if let Some(slot) = self
            .filters
            .iter_mut()
            .find(|f| Arc::as_ptr(f).cast::<()>() == old)
        {
            *slot = Arc::clone(&error_filter) as Arc<dyn Filter>;
        }
        self.renderer = renderer;
        self.error_filter = error_filter;
        self
    }

    fn insert_filter(mut self, order: i32, f: Arc<dyn Filter>) -> Self {
        let at = self.filters.partition_point(|x| x.order() <= order);
        self.filters.insert(at, f);
        self
    }

    /// Adds one route. Fails with [`RouteError`] on a conflict.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// let app = App::new()
    ///     .route(Route::new(HttpMethod::Get, "/a", |_| async { "a" }))?
    ///     .route(Route::new(HttpMethod::Get, "/b", |_| async { "b" }))?;
    /// # let _ = app;
    /// # Ok::<(), oxbloom_web::RouteError>(())
    /// ```
    pub fn route(mut self, route: Route) -> Result<Self, RouteError> {
        self.router.add(route)?;
        Ok(self)
    }

    /// Adds many routes, typically `controller.routes()`. Fails with [`RouteError`] on a conflict.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_web::{App, IntoResponse, Request, controller};
    ///
    /// struct Users;
    ///
    /// #[controller("/users")]
    /// impl Users {
    ///     #[get("")]
    ///     async fn list(&self, _req: Request) -> impl IntoResponse { "all users" }
    /// }
    ///
    /// let app = App::new().routes(Arc::new(Users).routes())?;
    /// # let _ = app;
    /// # Ok::<(), oxbloom_web::RouteError>(())
    /// ```
    pub fn routes(mut self, routes: impl IntoIterator<Item = Route>) -> Result<Self, RouteError> {
        for r in routes {
            self.router.add(r)?;
        }
        Ok(self)
    }

    /// Resolves `C` from the container and registers its routes.
    ///
    /// `C` must have been registered with the container (for example via `#[component]`).
    /// Requires the `di` feature.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, component};
    /// use oxbloom_web::{App, HttpMethod, IntoResponse, Request, controller};
    ///
    /// #[component]
    /// struct Greeter {}
    ///
    /// #[component]
    /// struct GreetController { greeter: Arc<Greeter> }
    ///
    /// #[controller("/greet")]
    /// impl GreetController {
    ///     #[get("")]
    ///     async fn greet(&self, _req: Request) -> impl IntoResponse { "hi" }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let container = ContainerBuilder::new()
    ///     .component::<Greeter>()
    ///     .component::<GreetController>()
    ///     .build()?;
    /// let app = App::new().controller::<GreetController>(&container)?;
    /// let res = app.handle(Request::new(HttpMethod::Get, "/greet")).await;
    /// assert_eq!(res.body(), "hi");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "di")]
    pub fn controller<C>(self, container: &oxbloom_di::Container) -> Result<Self, AppError>
    where
        C: crate::Controller + Send + Sync + 'static,
    {
        let controller = container.get::<C>()?;
        Ok(self.routes(controller.routes())?)
    }

    /// Dispatch a request without a network; for tests.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new()
    ///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let res = app.handle(Request::new(HttpMethod::Get, "/ping")).await;
    /// assert_eq!(res.status(), 200);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn handle(&self, req: Request) -> Response {
        run_guarded(&self.filters, &self.router, &*self.renderer, req).await
    }

    /// Dispatch a request without a network, the way the server would for a connection from `peer`.
    ///
    /// Unlike [`App::handle`], this applies the body limit ([`App::max_body_bytes`]) and resolves
    /// the client address, scheme and host from `peer` and the forwarded headers (see
    /// [`App::forwarded`]). With `peer` set to `None` the client address is unknown, as for a
    /// request that [`App::handle`] receives. Unlike the server, which starts each request with
    /// empty extensions, `handle_from` keeps whatever extensions the caller put on the request.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new()
    ///     .max_body_bytes(4)
    ///     .route(Route::new(HttpMethod::Post, "/echo", |_| async { "ok" }))?;
    /// let mut req = Request::new(HttpMethod::Post, "/echo");
    /// req.body = "too long".into();
    /// let peer = "192.0.2.1:40000".parse().ok();
    /// let res = app.handle_from(req, peer).await;
    /// assert_eq!(res.status(), 413);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn handle_from(&self, mut req: Request, peer: Option<SocketAddr>) -> Response {
        let too_large = req.body.len() > self.max_body_bytes;
        if too_large {
            req.body = Bytes::new();
        }
        let client = peer.map(|p| resolve_client(p, &req.headers, &self.forwarded, Scheme::Http));
        run_request(
            &self.filters,
            &self.router,
            &*self.renderer,
            req,
            client,
            too_large,
        )
        .await
    }

    /// Bind `addr` and serve until Ctrl-C, then drain open connections (see [`App::serve`]).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use oxbloom_web::{App, HttpMethod, Route};
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// App::new()
    ///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?
    ///     .run("127.0.0.1:8080".parse()?)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(self, addr: SocketAddr) -> io::Result<()> {
        let listener = TcpListener::bind(addr).await?;
        self.serve(listener, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    }

    /// Serve on `listener` until `shutdown` completes, then drain open connections for at most
    /// [`App::shutdown_timeout`]; connections still open after it are dropped and a warning is
    /// logged on target `oxbloom::web`. Dropping the `serve` future aborts the connections it
    /// is still serving; they do not keep running detached.
    ///
    /// Every request carries a [`ClientInfo`] in its extensions (see [`Request::client_addr`]
    /// and [`App::forwarded`]).
    ///
    /// Bind port 0 to get a free port, which makes this the right choice for tests.
    ///
    /// Values of the `Authorization`, `Proxy-Authorization` and `Cookie` headers are marked
    /// sensitive, so `Debug` output of a [`Request`] shows `Sensitive` instead of them. Other
    /// headers, including custom credential headers, are printed as sent.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, HttpMethod, Route};
    /// use tokio::io::{AsyncReadExt, AsyncWriteExt};
    /// use tokio::net::{TcpListener, TcpStream};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let app = App::new()
    ///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let listener = TcpListener::bind("127.0.0.1:0").await?;
    /// let addr = listener.local_addr()?;
    /// let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    /// let server = tokio::spawn(app.serve(listener, async { let _ = stopped.await; }));
    ///
    /// let mut conn = TcpStream::connect(addr).await?;
    /// conn.write_all(b"GET /ping HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").await?;
    /// let mut reply = String::new();
    /// conn.read_to_string(&mut reply).await?;
    /// assert!(reply.starts_with("HTTP/1.1 200"));
    ///
    /// stop.send(()).unwrap();
    /// server.await??;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        self.serve_on(listener, Plain, shutdown).await
    }

    /// Serves HTTPS on `listener` until `shutdown` completes: [`App::serve`] with a TLS
    /// handshake in front of each connection. Requires the `tls` feature.
    ///
    /// The shutdown and its [`App::shutdown_timeout`], the header read timeout, the body limit,
    /// the filters and the router work exactly as in [`App::serve`]. Each accepted connection
    /// is handshaken in its own task, so a slow client never delays the others. A client that
    /// has not finished its handshake after 10 seconds is dropped, and a handshake still running
    /// when shutdown starts is dropped at once. A failed handshake is logged at `debug` on target
    /// `oxbloom::web` with the peer address and the error kind, never with bytes the client sent.
    ///
    /// The connection speaks HTTP/1.1 only. Set `alpn_protocols` of the `rustls::ServerConfig`
    /// behind `acceptor` to `http/1.1` alone, so a client that offers only `h2` fails the
    /// handshake; a connection that still negotiates another protocol is closed. A client that
    /// offers no ALPN is served HTTP/1.1.
    ///
    /// Every request reports the scheme [`Scheme::Https`] through [`Request::scheme`], unless a
    /// trusted proxy's forwarded headers say otherwise (see [`App::forwarded`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_web::{App, HttpMethod, Route};
    /// use tokio_rustls::rustls::ServerConfig;
    /// use tokio_rustls::rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    /// use tokio::net::{TcpListener, TcpStream};
    /// use tokio_rustls::TlsAcceptor;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let key = rcgen::KeyPair::generate()?;
    /// # let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])?.self_signed(&key)?;
    /// # let (certs, key) = (
    /// #     vec![cert.der().clone()],
    /// #     PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    /// # );
    /// // `certs` and `key` are the server's certificate chain and private key.
    /// let mut config = ServerConfig::builder()
    ///     .with_no_client_auth()
    ///     .with_single_cert(certs, key)?;
    /// config.alpn_protocols = vec![b"http/1.1".to_vec()];
    ///
    /// let app = App::new()
    ///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    /// let listener = TcpListener::bind("127.0.0.1:0").await?;
    /// let acceptor = TlsAcceptor::from(Arc::new(config));
    /// // Serve until Ctrl-C; this example stops at once.
    /// // app.serve_tls(listener, acceptor, async { let _ = tokio::signal::ctrl_c().await; })
    /// app.serve_tls(listener, acceptor, async {}).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "tls")]
    pub async fn serve_tls(
        self,
        listener: TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        self.serve_on(listener, Tls(acceptor), shutdown).await
    }

    /// The accept loop of [`App::serve`] and `App::serve_tls`. Each accepted stream is opened by
    /// `transport` inside its connection task, then served by the same HTTP/1.1 connection.
    async fn serve_on<T: Transport>(
        self,
        listener: TcpListener,
        transport: T,
        shutdown: impl Future<Output = ()>,
    ) -> io::Result<()> {
        let pipeline = Arc::new((self.filters, self.router, self.renderer, self.forwarded));
        let max_body = self.max_body_bytes;
        let graceful = GracefulShutdown::new();
        let mut http = hyper::server::conn::http1::Builder::new();
        http.timer(TokioTimer::new())
            .header_read_timeout(self.header_read_timeout);
        // Tells connections still opening their stream (a TLS handshake) to give up, so they do
        // not hold the graceful shutdown open: they have no request in flight yet.
        let (stopping, stop) = tokio::sync::watch::channel(());
        // Connection tasks live in this set, so they can be aborted when the shutdown timeout
        // expires; dropping the set aborts every task still in it.
        let mut conns = JoinSet::new();
        tokio::pin!(shutdown);

        loop {
            let (stream, peer) = tokio::select! {
                res = listener.accept() => match res {
                    Ok(c) => c,
                    Err(_) => continue, // transient accept error (e.g. EMFILE)
                },
                Some(_) = conns.join_next(), if !conns.is_empty() => continue, // reap finished
                () = &mut shutdown => break,
            };
            let pipeline = Arc::clone(&pipeline);
            let svc = service_fn(move |req| {
                let pipeline = Arc::clone(&pipeline);
                async move {
                    let (filters, router, renderer, forwarded) = &*pipeline;
                    let client = resolve_client(peer, req.headers(), forwarded, T::SCHEME);
                    let res = dispatch(filters, router, &**renderer, req, max_body, client).await;
                    Ok::<_, Infallible>(res)
                }
            });
            // Taken now, before any later shutdown signal, so a connection whose stream opens
            // after shutdown started is told to close at once.
            let watcher = graceful.watcher();
            let http = http.clone();
            let transport = transport.clone();
            let mut stop = stop.clone();
            conns.spawn(async move {
                let io = tokio::select! {
                    biased;
                    io = transport.open(stream, peer) => io,
                    _ = stop.changed() => None,
                };
                if let Some(io) = io {
                    let _ = watcher
                        .watch(http.serve_connection(TokioIo::new(io), svc))
                        .await;
                }
            });
        }
        drop(listener);
        let _ = stopping.send(());
        if tokio::time::timeout(self.shutdown_timeout, graceful.shutdown())
            .await
            .is_err()
        {
            tracing::warn!(
                target: "oxbloom::web",
                timeout_ms = u64::try_from(self.shutdown_timeout.as_millis()).unwrap_or(u64::MAX),
                "graceful shutdown timed out; dropping the remaining connections"
            );
        }
        conns.abort_all();
        Ok(())
    }
}

/// How the server turns an accepted TCP stream into the stream it speaks HTTP/1.1 on.
trait Transport: Clone + Send + 'static {
    /// The stream HTTP/1.1 is served on.
    type Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static;

    /// The scheme of a request on this transport when no trusted proxy states another.
    const SCHEME: Scheme;

    /// Opens `stream`, accepted from `peer`. `None` drops the connection.
    fn open(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
    ) -> impl Future<Output = Option<Self::Io>> + Send + 'static;
}

/// Plain HTTP: the TCP stream as it is.
#[derive(Clone)]
struct Plain;

impl Transport for Plain {
    type Io = TcpStream;
    const SCHEME: Scheme = Scheme::Http;

    fn open(
        &self,
        stream: TcpStream,
        _peer: SocketAddr,
    ) -> impl Future<Output = Option<TcpStream>> + Send + 'static {
        std::future::ready(Some(stream))
    }
}

/// HTTPS: a TLS handshake with a timeout, then HTTP/1.1 inside the TLS stream.
#[cfg(feature = "tls")]
#[derive(Clone)]
struct Tls(tokio_rustls::TlsAcceptor);

#[cfg(feature = "tls")]
impl Transport for Tls {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    const SCHEME: Scheme = Scheme::Https;

    fn open(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
    ) -> impl Future<Output = Option<Self::Io>> + Send + 'static {
        let accept = self.0.accept(stream);
        async move {
            let err = match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, accept).await {
                Ok(Ok(tls)) => match tls.get_ref().1.alpn_protocol() {
                    None | Some(b"http/1.1") => return Some(tls),
                    // The caller's config offered another protocol and the client chose it.
                    Some(_) => io::Error::from(io::ErrorKind::Unsupported),
                },
                Ok(Err(e)) => e,
                Err(_) => io::Error::from(io::ErrorKind::TimedOut),
            };
            // The kind only: the error text of a failed handshake may describe what was sent.
            tracing::debug!(
                target: "oxbloom::web",
                %peer,
                kind = ?err.kind(),
                "TLS handshake failed; connection dropped"
            );
            None
        }
    }
}

/// Runs the pipeline and answers a panic that escaped every filter (one ordered before
/// [`order::ERROR_HANDLING`](crate::order::ERROR_HANDLING)) with a rendered 500.
async fn run_guarded(
    filters: &[Arc<dyn Filter>],
    router: &Router,
    renderer: &dyn ErrorRenderer,
    req: Request,
) -> Response {
    let info = request_info(&req);
    match CatchUnwind::new(Box::pin(run_pipeline(filters, router, req))).await {
        Ok(res) => res,
        Err(payload) => panic_response(renderer, &*payload, &info),
    }
}

/// The steps both the server and [`App::handle_from`] run once the request is buffered:
/// credential headers are marked sensitive, the client and the body-limit marker are attached,
/// and the pipeline runs.
async fn run_request(
    filters: &[Arc<dyn Filter>],
    router: &Router,
    renderer: &dyn ErrorRenderer,
    mut request: Request,
    client: Option<ClientInfo>,
    too_large: bool,
) -> Response {
    // Credentials must never show up in `Debug` output of the request.
    for name in [
        http::header::AUTHORIZATION,
        http::header::PROXY_AUTHORIZATION,
        http::header::COOKIE,
    ] {
        if let http::header::Entry::Occupied(mut entry) = request.headers.entry(name) {
            entry.iter_mut().for_each(|v| v.set_sensitive(true));
        }
    }
    if let Some(client) = client {
        request.extensions_mut().insert(client);
    }
    if too_large {
        request.extensions_mut().insert(BodyTooLarge);
    }
    run_guarded(filters, router, renderer, request).await
}

async fn dispatch(
    filters: &[Arc<dyn Filter>],
    router: &Router,
    renderer: &dyn ErrorRenderer,
    req: hyper::Request<Incoming>,
    max_body: usize,
    client: ClientInfo,
) -> hyper::Response<Full<Bytes>> {
    let (parts, body) = req.into_parts();
    let (body, too_large) = match Limited::new(body, max_body).collect().await {
        Ok(b) => (b.to_bytes(), false),
        Err(_) => (Bytes::new(), true),
    };
    let mut request = Request::new(HttpMethod::from(&parts.method), parts.uri.path());
    request.query = parts.uri.query().map(str::to_owned);
    request.headers = parts.headers;
    request.body = body;
    let res = run_request(filters, router, renderer, request, Some(client), too_large).await;
    to_hyper(res)
}

fn to_hyper(res: Response) -> hyper::Response<Full<Bytes>> {
    res.map(Full::new)
}

/// Failure wiring a controller from the container into the app.
///
/// Requires the `di` feature. `DiError` and `RouteError` convert into it with `?`.
///
/// # Examples
///
/// ```
/// use oxbloom_di::ContainerBuilder;
/// use oxbloom_web::{App, AppError, Controller, Route};
/// use std::sync::Arc;
///
/// struct Unregistered;
/// impl Controller for Unregistered {
///     fn routes(self: Arc<Self>) -> Vec<Route> { Vec::new() }
/// }
///
/// let empty = ContainerBuilder::new().build()?;
/// match App::new().controller::<Unregistered>(&empty) {
///     Err(AppError::Di(e)) => println!("not registered: {e}"),
///     Err(AppError::Route(e)) => println!("route conflict: {e}"),
///     Ok(_) => unreachable!(),
/// }
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
#[cfg(feature = "di")]
#[derive(Debug)]
pub enum AppError {
    Di(oxbloom_di::DiError),
    Route(RouteError),
}

#[cfg(feature = "di")]
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Di(e) => write!(f, "dependency injection: {e}"),
            Self::Route(e) => write!(f, "route registration: {e}"),
        }
    }
}

/// Returns the wrapped [`oxbloom_di::DiError`] or [`RouteError`], so error reporters can walk
/// the chain.
///
/// # Examples
///
/// ```
/// use std::error::Error;
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError};
/// use oxbloom_web::{App, Controller, Route};
///
/// struct Unregistered;
/// impl Controller for Unregistered {
///     fn routes(self: Arc<Self>) -> Vec<Route> { Vec::new() }
/// }
///
/// let empty = ContainerBuilder::new().build()?;
/// let err = App::new().controller::<Unregistered>(&empty).err().unwrap();
/// let source = err.source().and_then(|s| s.downcast_ref::<DiError>());
/// assert!(matches!(source, Some(DiError::Missing { .. })));
/// # Ok::<(), DiError>(())
/// ```
#[cfg(feature = "di")]
impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Di(e) => Some(e),
            Self::Route(e) => Some(e),
        }
    }
}

#[cfg(feature = "di")]
impl From<oxbloom_di::DiError> for AppError {
    fn from(e: oxbloom_di::DiError) -> Self {
        Self::Di(e)
    }
}

#[cfg(feature = "di")]
impl From<RouteError> for AppError {
    fn from(e: RouteError) -> Self {
        Self::Route(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_read_timeout_is_clamped_to_one_day() {
        let huge = App::new().header_read_timeout(Duration::from_secs(u64::MAX));
        assert_eq!(huge.header_read_timeout, Duration::from_secs(86_400));
        let small = App::new().header_read_timeout(Duration::from_secs(5));
        assert_eq!(small.header_read_timeout, Duration::from_secs(5));
    }

    #[test]
    fn new_has_the_documented_defaults() {
        // `oxbloom-boot` leaves these in place when a `server.*` key is absent.
        let app = App::new();
        assert_eq!(app.max_body_bytes, 1_048_576);
        assert_eq!(app.header_read_timeout, Duration::from_secs(10));
        assert_eq!(app.shutdown_timeout, Duration::from_secs(30));
        assert_eq!(app.forwarded, ForwardedConfig::default());
        assert_eq!(app.forwarded.strategy, crate::ForwardStrategy::None);
        assert_eq!(app.forwarded.trusted, crate::TrustedProxies::loopback());
    }

    fn orders(app: &App) -> Vec<i32> {
        app.filters.iter().map(|f| f.order()).collect()
    }

    #[test]
    fn error_renderer_replaces_the_built_in_filter_in_place() {
        let app = App::new();
        let before = orders(&app);
        #[cfg(feature = "json")]
        let expected = vec![
            crate::order::ERROR_HANDLING,
            crate::order::NEGOTIATION,
            crate::order::BODY_LIMIT,
        ];
        #[cfg(not(feature = "json"))]
        let expected = vec![crate::order::ERROR_HANDLING, crate::order::BODY_LIMIT];
        assert_eq!(before, expected);
        let app = app
            .error_renderer(ProblemRenderer::new(true))
            .error_renderer(ProblemRenderer::new(false));
        assert_eq!(orders(&app), before);
        let installed = Arc::as_ptr(&app.filters[0]).cast::<()>();
        assert_eq!(installed, Arc::as_ptr(&app.error_filter).cast::<()>());
        assert!(Arc::ptr_eq(&app.error_filter.renderer, &app.renderer));
    }
}
