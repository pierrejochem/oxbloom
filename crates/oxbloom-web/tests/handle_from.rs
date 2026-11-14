//! `App::handle_from` must answer like the served application does.

use std::net::SocketAddr;

use oxbloom_core::HttpMethod;
use oxbloom_web::{
    App, ClientInfo, ForwardStrategy, ForwardedConfig, Request, Route, TrustedProxies,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

fn echo_app() -> App {
    App::new()
        .max_body_bytes(8)
        .route(Route::new(HttpMethod::Post, "/x", |_| async { "ok" }))
        .unwrap()
}

fn post(body: &'static [u8]) -> Request {
    let mut req = Request::new(HttpMethod::Post, "/x");
    req.body = body.into();
    req
}

#[tokio::test]
async fn a_body_over_the_limit_is_413_in_memory() {
    let res = echo_app().handle_from(post(b"123456789"), None).await;
    assert_eq!(res.status(), 413);
}

#[tokio::test]
async fn a_body_at_the_limit_is_accepted() {
    let res = echo_app().handle_from(post(b"12345678"), None).await;
    assert_eq!(res.status(), 200);
}

fn who_app(forwarded: Option<ForwardedConfig>) -> App {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/who", |r| async move {
            let info = r.extensions().get::<ClientInfo>();
            format!("{:?}", info.map(|c| c.addr))
        }))
        .unwrap();
    match forwarded {
        Some(cfg) => app.forwarded(cfg),
        None => app,
    }
}

fn forwarded_for(addr: &str) -> Request {
    let mut req = Request::new(HttpMethod::Get, "/who");
    req.headers.insert("x-forwarded-for", addr.parse().unwrap());
    req
}

#[tokio::test]
async fn the_client_address_is_the_peer() {
    let peer: SocketAddr = "192.0.2.7:40000".parse().unwrap();
    let res = who_app(None)
        .handle_from(Request::new(HttpMethod::Get, "/who"), Some(peer))
        .await;
    assert_eq!(res.body(), &format!("Some({peer:?})"));
    let res = who_app(None)
        .handle_from(Request::new(HttpMethod::Get, "/who"), None)
        .await;
    assert_eq!(res.body(), "None");
}

#[tokio::test]
async fn forwarded_headers_from_an_untrusted_peer_are_ignored() {
    let cfg = ForwardedConfig {
        strategy: ForwardStrategy::Framework,
        trusted: TrustedProxies::parse(&["192.0.2.0/24".to_string()]).unwrap(),
    };
    let peer: SocketAddr = "198.51.100.5:40000".parse().unwrap();
    let res = who_app(Some(cfg))
        .handle_from(forwarded_for("203.0.113.9"), Some(peer))
        .await;
    assert_eq!(res.body(), &format!("Some({peer:?})"));
}

#[tokio::test]
async fn forwarded_headers_from_a_trusted_proxy_are_used() {
    let cfg = ForwardedConfig {
        strategy: ForwardStrategy::Framework,
        trusted: TrustedProxies::loopback(),
    };
    let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
    let res = who_app(Some(cfg))
        .handle_from(forwarded_for("203.0.113.9"), Some(peer))
        .await;
    assert_eq!(res.body(), "Some(203.0.113.9:0)");
}

fn agree_app() -> App {
    App::new()
        .max_body_bytes(8)
        .route(Route::new(HttpMethod::Post, "/x", |_| async { "ok" }))
        .unwrap()
}

/// Sends one request over a raw socket (a hyper client would hide the wire form) and returns
/// the status code and the body.
async fn served(body: &str, path: &str) -> (u16, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        agree_app()
            .serve(listener, async {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });
    let mut s = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8_lossy(&out).into_owned();
    let status = text[9..12].parse().unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
    let _ = tx.send(());
    task.await.unwrap();
    (status, body)
}

async fn in_memory(body: &'static str, path: &str) -> (u16, String) {
    let mut req = Request::new(HttpMethod::Post, path);
    req.body = body.as_bytes().into();
    let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
    let res = agree_app().handle_from(req, Some(peer)).await;
    (
        res.status().as_u16(),
        String::from_utf8_lossy(res.body()).into_owned(),
    )
}

#[tokio::test]
async fn handle_from_and_the_served_answer_agree() {
    for (body, path, status) in [
        ("12345678", "/x", 200),
        ("1", "/missing", 404),
        ("123456789", "/x", 413),
    ] {
        let wire = served(body, path).await;
        let mem = in_memory(body, path).await;
        assert_eq!(wire.0, status, "served status for {path}");
        assert_eq!(wire, mem, "served and in-memory answers differ for {path}");
    }
}
