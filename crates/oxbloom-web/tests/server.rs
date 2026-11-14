use std::cell::RefCell;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use oxbloom_core::HttpMethod;
use oxbloom_web::{App, ForwardStrategy, ForwardedConfig, Route, RouterConfig, TrustedProxies};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tracing_subscriber::fmt::MakeWriter;

struct TestServer {
    addr: std::net::SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(app: App) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            app.serve(listener, async {
                let _ = rx.await;
            })
            .await
            .unwrap();
        });
        Self {
            addr,
            stop: Some(tx),
            task,
        }
    }

    async fn stop(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        self.task.await.unwrap();
    }
}

/// Send raw request, return the full response text (server closes after one exchange).
async fn raw(addr: std::net::SocketAddr, request: &str) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    String::from_utf8_lossy(&out).into_owned()
}

fn req(method: &str, path: &str, extra: &str) -> String {
    format!("{method} {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\n{extra}\r\n")
}

fn app() -> App {
    App::with_config(RouterConfig::default())
        .max_body_bytes(16)
        .route(Route::new(HttpMethod::Get, "/hello", |_| async { "hi" }))
        .unwrap()
        .route(Route::new(HttpMethod::Post, "/echo", |r| async move {
            String::from_utf8_lossy(&r.body).into_owned()
        }))
        .unwrap()
        .route(Route::new(HttpMethod::Trace, "/hello", |_| async { "t" }))
        .unwrap()
}

#[tokio::test]
async fn get_head_options_and_errors() {
    let srv = TestServer::start(app()).await;

    let res = raw(srv.addr, &req("GET", "/hello", "")).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(res.ends_with("hi"), "{res}");

    let res = raw(srv.addr, &req("HEAD", "/hello", "")).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(!res.contains("\r\n\r\nhi"), "{res}");

    let res = raw(srv.addr, &req("OPTIONS", "/hello", "")).await;
    assert!(res.starts_with("HTTP/1.1 204"), "{res}");
    assert!(
        res.to_lowercase().contains("allow: get, head, options"),
        "{res}"
    );

    let res = raw(srv.addr, &req("TRACE", "/hello", "")).await;
    assert!(res.starts_with("HTTP/1.1 405"), "{res}");

    let res = raw(srv.addr, &req("GET", "/missing", "")).await;
    assert!(res.starts_with("HTTP/1.1 404"), "{res}");

    let res = raw(srv.addr, &req("DELETE", "/hello", "")).await;
    assert!(res.starts_with("HTTP/1.1 405"), "{res}");

    srv.stop().await;
}

#[tokio::test]
async fn credential_headers_are_sensitive_in_request_debug() {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/debug", |req| async move {
            format!("{req:?}")
        }))
        .unwrap();
    let srv = TestServer::start(app).await;

    let extra = "Authorization: Bearer SECRET-TOKEN-123\r\n\
                 Proxy-Authorization: Basic SECRET-PROXY-456\r\n\
                 Cookie: session=SECRET-COOKIE-789\r\n\
                 X-Visible: shown\r\n";
    let res = raw(srv.addr, &req("GET", "/debug", extra)).await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    for secret in ["SECRET-TOKEN-123", "SECRET-PROXY-456", "SECRET-COOKIE-789"] {
        assert!(!res.contains(secret), "{res}");
    }
    assert_eq!(res.matches("Sensitive").count(), 3, "{res}");
    assert!(res.contains("shown"), "{res}");

    srv.stop().await;
}

#[tokio::test]
async fn body_is_passed_and_limited() {
    let srv = TestServer::start(app()).await;

    let res = raw(
        srv.addr,
        &(req("POST", "/echo", "Content-Length: 4\r\n") + "ping"),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert!(res.ends_with("ping"), "{res}");

    let big = "x".repeat(100);
    let res = raw(
        srv.addr,
        &(req("POST", "/echo", "Content-Length: 100\r\n") + &big),
    )
    .await;
    assert!(res.starts_with("HTTP/1.1 413"), "{res}");

    srv.stop().await;
}

#[tokio::test]
async fn shuts_down_gracefully() {
    let srv = TestServer::start(app()).await;
    let addr = srv.addr;
    srv.stop().await;
    assert!(TcpStream::connect(addr).await.is_err());
}

fn client_app(forwarded: Option<ForwardedConfig>) -> App {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/who", |r| async move {
            format!(
                "addr={:?} scheme={:?} host={:?}",
                r.client_addr(),
                r.scheme(),
                r.host()
            )
        }))
        .unwrap();
    match forwarded {
        Some(cfg) => app.forwarded(cfg),
        None => app,
    }
}

fn body(res: &str) -> &str {
    res.split("\r\n\r\n").nth(1).unwrap_or("")
}

#[tokio::test]
async fn request_exposes_peer_addr_without_forwarded_config() {
    let srv = TestServer::start(client_app(None)).await;
    let mut s = TcpStream::connect(srv.addr).await.unwrap();
    let local = s.local_addr().unwrap();
    let extra = "X-Forwarded-For: 203.0.113.9\r\nX-Forwarded-Proto: https\r\nX-Forwarded-Host: example.com\r\nForwarded: for=198.51.100.1\r\n";
    s.write_all(req("GET", "/who", extra).as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let res = String::from_utf8_lossy(&out).into_owned();
    assert!(res.starts_with("HTTP/1.1 200"), "{res}");
    assert_eq!(
        body(&res),
        format!("addr=Some({local:?}) scheme=Http host=None"),
        "{res}"
    );
    srv.stop().await;
}

#[tokio::test]
async fn handle_has_no_client_info() {
    let res = client_app(None)
        .handle(oxbloom_web::Request::new(HttpMethod::Get, "/who"))
        .await;
    assert_eq!(res.body(), "addr=None scheme=Http host=None");
}

#[tokio::test]
async fn trusted_loopback_proxy_forwards_the_client() {
    let cfg = ForwardedConfig {
        strategy: ForwardStrategy::Framework,
        trusted: TrustedProxies::loopback(),
    };
    let srv = TestServer::start(client_app(Some(cfg))).await;
    let extra = "X-Forwarded-For: 6.6.6.6, 203.0.113.9\r\nX-Forwarded-Proto: https\r\nX-Forwarded-Host: example.com\r\n";
    let res = raw(srv.addr, &req("GET", "/who", extra)).await;
    assert_eq!(
        body(&res),
        "addr=Some(203.0.113.9:0) scheme=Https host=Some(\"example.com\")",
        "{res}"
    );
    // A peer outside the trusted list is the client, whatever it sends.
    let cfg = ForwardedConfig {
        strategy: ForwardStrategy::Framework,
        trusted: TrustedProxies::parse(&["192.0.2.0/24".to_string()]).unwrap(),
    };
    let srv2 = TestServer::start(client_app(Some(cfg))).await;
    let res = raw(srv2.addr, &req("GET", "/who", extra)).await;
    assert!(body(&res).starts_with("addr=Some(127.0.0.1:"), "{res}");
    assert!(body(&res).ends_with("scheme=Http host=None"), "{res}");
    srv.stop().await;
    srv2.stop().await;
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
async fn shutdown_timeout_drops_a_stuck_connection_and_warns() {
    init_tracing();
    let buf = Arc::new(Mutex::new(Vec::new()));
    CAPTURE.with(|c| *c.borrow_mut() = Some(buf.clone()));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut stuck = TcpStream::connect(addr).await.unwrap();
    // Headers never finish, so the connection is busy and graceful shutdown cannot close it.
    stuck
        .write_all(b"GET /hello HTTP/1.1\r\nHost: t\r\nX-Partial: ")
        .await
        .unwrap();

    let app = app().shutdown_timeout(Duration::from_millis(200));
    let started = Instant::now();
    let served = tokio::time::timeout(
        Duration::from_secs(2),
        app.serve(listener, tokio::time::sleep(Duration::from_millis(100))),
    )
    .await;
    let elapsed = started.elapsed();
    CAPTURE.with(|c| *c.borrow_mut() = None);

    served.expect("serve did not return within 2s").unwrap();
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");

    // The connection was dropped: the client sees EOF or a reset, not a hang.
    let mut rest = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(1), stuck.read_to_end(&mut rest)).await;
    assert!(
        read.is_ok(),
        "stuck connection still open after serve returned"
    );

    let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
    let line = out
        .lines()
        .find(|l| l.contains("oxbloom::web") && l.contains("graceful shutdown timed out"))
        .unwrap_or_else(|| panic!("no shutdown-timeout warning in {out:?}"));
    assert!(line.contains("WARN"), "{line}");
    assert!(!line.contains("127.0.0.1"), "{line}");
}
