//! The `server.*` properties of `WebServer::new`, over real HTTP.

#![cfg(feature = "web")]

use std::future::Future;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use oxbloom_boot::{Service, WebServer};
use oxbloom_config::{Config, ConfigError};
use oxbloom_web::{App, ForwardStrategy, ForwardedConfig, HttpMethod, Route, TrustedProxies};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), f)
        .await
        .expect("test timed out")
}

fn app() -> App {
    App::new()
        .route(Route::new(HttpMethod::Post, "/echo", |_| async { "ok" }))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/who", |req| async move {
            req.client_addr()
                .map(|a| a.ip().to_string())
                .unwrap_or_default()
        }))
        .unwrap()
}

async fn start(config: &str) -> WebServer {
    let config = Config::from_str(&format!("[server]\nport = 0\n{config}")).unwrap();
    let server = WebServer::new(app(), &config).unwrap();
    server.start().await.unwrap();
    server
}

async fn send(addr: SocketAddr, request: &str) -> String {
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(request.as_bytes()).await.unwrap();
    let mut reply = String::new();
    within(conn.read_to_string(&mut reply)).await.unwrap();
    reply
}

fn status(reply: &str) -> u16 {
    reply[9..12].parse().unwrap()
}

fn post(len: usize) -> String {
    format!(
        "POST /echo HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: {len}\r\n\r\n{}",
        "a".repeat(len)
    )
}

#[test]
fn defaults_for_every_new_key() {
    // The default values themselves are asserted in `web.rs` (`limit_defaults_...`) and over
    // HTTP below; here an explicit empty `server.forward-headers` table must still be accepted.
    assert!(WebServer::new(App::new(), &Config::from_str("").unwrap()).is_ok());
    let config = Config::from_str("[server]\nport = 1\n[server.forward-headers]\n").unwrap();
    assert!(WebServer::new(App::new(), &config).is_ok());
}

#[tokio::test]
async fn zero_max_body_rejects_any_body() {
    let server = start("max-body-bytes = 0").await;
    let addr = server.local_addr().unwrap();
    assert_eq!(status(&send(addr, &post(1)).await), 413);
    server.stop().await;
}

#[tokio::test]
async fn zero_shutdown_timeout_stops_at_once() {
    let server = start("shutdown-timeout = \"0s\"\nheader-read-timeout = \"60s\"").await;
    let addr = server.local_addr().unwrap();
    let mut stuck = TcpStream::connect(addr).await.unwrap();
    stuck
        .write_all(b"GET /who HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    within(server.stop()).await;
}

#[tokio::test]
async fn default_body_limit_is_one_mib() {
    let server = start("").await;
    let addr = server.local_addr().unwrap();
    assert_eq!(status(&send(addr, &post(1024)).await), 200);
    assert_eq!(status(&send(addr, &post((1 << 20) + 1)).await), 413);
    server.stop().await;
}

#[tokio::test]
async fn max_body_bytes_is_applied() {
    let server = start("max-body-bytes = \"1KiB\"").await;
    let addr = server.local_addr().unwrap();
    assert_eq!(status(&send(addr, &post(1024)).await), 200);
    assert_eq!(status(&send(addr, &post(1025)).await), 413);
    server.stop().await;
}

#[tokio::test]
async fn header_read_timeout_is_applied() {
    let server = start("header-read-timeout = \"200ms\"").await;
    let addr = server.local_addr().unwrap();
    // Headers never complete: the server closes the connection.
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(b"GET /who HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    let mut reply = Vec::new();
    let started = Instant::now();
    let _ = within(conn.read_to_end(&mut reply)).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    server.stop().await;
}

#[tokio::test]
async fn shutdown_timeout_from_config_stops_a_stuck_connection() {
    let server = start("shutdown-timeout = \"200ms\"\nheader-read-timeout = \"60s\"").await;
    let addr = server.local_addr().unwrap();
    let mut stuck = TcpStream::connect(addr).await.unwrap();
    stuck
        .write_all(b"GET /who HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    within(server.stop()).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(stuck);
}

fn who(headers: &str) -> String {
    format!("GET /who HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{headers}\r\n")
}

fn body(reply: &str) -> &str {
    reply.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("")
}

#[tokio::test]
async fn forwarded_headers_reach_the_request() {
    let server = start(
        "[server.forward-headers]\nstrategy = \"framework\"\ntrusted-proxies = [\"127.0.0.1\"]",
    )
    .await;
    let addr = server.local_addr().unwrap();
    let reply = send(addr, &who("X-Forwarded-For: 203.0.113.9\r\n")).await;
    assert_eq!(body(&reply), "203.0.113.9", "{reply}");
    server.stop().await;
}

#[tokio::test]
async fn untrusted_forwarded_headers_are_ignored_by_default() {
    let server = start("").await;
    let addr = server.local_addr().unwrap();
    let reply = send(addr, &who("X-Forwarded-For: 203.0.113.9\r\n")).await;
    assert_eq!(body(&reply), "127.0.0.1", "{reply}");
    server.stop().await;

    // Strategy `framework`, but the peer is not a trusted proxy.
    let server = start(
        "[server.forward-headers]\nstrategy = \"framework\"\ntrusted-proxies = [\"10.0.0.0/8\"]",
    )
    .await;
    let addr = server.local_addr().unwrap();
    let reply = send(addr, &who("X-Forwarded-For: 203.0.113.9\r\n")).await;
    assert_eq!(body(&reply), "127.0.0.1", "{reply}");
    server.stop().await;
}

/// Starts `app` with `[server]`, port 0 and `config`.
async fn start_app(app: App, config: &str) -> WebServer {
    let config = Config::from_str(&format!("[server]\nport = 0\n{config}")).unwrap();
    let server = WebServer::new(app, &config).unwrap();
    server.start().await.unwrap();
    server
}

#[tokio::test]
async fn code_body_limit_wins_when_the_key_is_absent() {
    let server = start_app(app().max_body_bytes(16), "").await;
    let addr = server.local_addr().unwrap();
    assert_eq!(status(&send(addr, &post(16)).await), 200);
    assert_eq!(status(&send(addr, &post(17)).await), 413);
    server.stop().await;
}

#[tokio::test]
async fn a_present_key_overrides_the_code_body_limit() {
    let server = start_app(app().max_body_bytes(16), "max-body-bytes = \"1KiB\"").await;
    let addr = server.local_addr().unwrap();
    assert_eq!(status(&send(addr, &post(17)).await), 200);
    assert_eq!(status(&send(addr, &post(1025)).await), 413);
    server.stop().await;
}

#[tokio::test]
async fn code_header_read_timeout_wins_when_the_key_is_absent() {
    let app = app().header_read_timeout(Duration::from_millis(200));
    let server = start_app(app, "").await;
    let addr = server.local_addr().unwrap();
    // Headers never complete: the 200 ms from code close the connection, not the 10 s default.
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(b"GET /who HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    let mut reply = Vec::new();
    let started = Instant::now();
    let _ = within(conn.read_to_end(&mut reply)).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    server.stop().await;
}

#[tokio::test]
async fn code_shutdown_timeout_wins_when_the_key_is_absent() {
    let app = app().shutdown_timeout(Duration::from_millis(200));
    let server = start_app(app, "header-read-timeout = \"60s\"").await;
    let addr = server.local_addr().unwrap();
    let mut stuck = TcpStream::connect(addr).await.unwrap();
    stuck
        .write_all(b"GET /who HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The 200 ms from code apply, not the 30 s default.
    let started = Instant::now();
    within(server.stop()).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(stuck);
}

fn framework(trusted: &[&str]) -> ForwardedConfig {
    let entries: Vec<String> = trusted.iter().map(|s| (*s).to_owned()).collect();
    ForwardedConfig {
        strategy: ForwardStrategy::Framework,
        trusted: TrustedProxies::parse(&entries).unwrap(),
    }
}

#[tokio::test]
async fn code_forwarded_config_wins_when_the_keys_are_absent() {
    let server = start_app(app().forwarded(framework(&["127.0.0.1"])), "").await;
    let addr = server.local_addr().unwrap();
    let reply = send(addr, &who("X-Forwarded-For: 203.0.113.9\r\n")).await;
    assert_eq!(body(&reply), "203.0.113.9", "{reply}");
    server.stop().await;
}

#[tokio::test]
async fn trusted_proxies_alone_keep_the_code_strategy() {
    // Strategy `framework` from code, only the list from the configuration.
    let server = start_app(
        app().forwarded(framework(&["10.0.0.0/8"])),
        "[server.forward-headers]\ntrusted-proxies = [\"127.0.0.1\"]",
    )
    .await;
    let addr = server.local_addr().unwrap();
    let reply = send(addr, &who("X-Forwarded-For: 203.0.113.9\r\n")).await;
    assert_eq!(body(&reply), "203.0.113.9", "{reply}");
    server.stop().await;

    // Strategy `none` from the configuration overrides `framework` from code.
    let server = start_app(
        app().forwarded(framework(&["127.0.0.1"])),
        "[server.forward-headers]\nstrategy = \"none\"",
    )
    .await;
    let addr = server.local_addr().unwrap();
    let reply = send(addr, &who("X-Forwarded-For: 203.0.113.9\r\n")).await;
    assert_eq!(body(&reply), "127.0.0.1", "{reply}");
    server.stop().await;
}

#[test]
fn invalid_values_name_the_key_and_not_the_value() {
    let cases = [
        ("shutdown-timeout = \"hunter2\"", "server.shutdown-timeout"),
        ("max-body-bytes = \"hunter2\"", "server.max-body-bytes"),
        (
            "header-read-timeout = \"hunter2\"",
            "server.header-read-timeout",
        ),
        (
            "error.include-message = \"hunter2\"",
            "server.error.include-message",
        ),
        (
            "[server.forward-headers]\nstrategy = \"hunter2\"",
            "server.forward-headers.strategy",
        ),
        (
            "[server.forward-headers]\ntrusted-proxies = [\"hunter2\"]",
            "server.forward-headers.trusted-proxies",
        ),
    ];
    for (line, key) in cases {
        let config = Config::from_str(&format!("[server]\n{line}")).unwrap();
        let err = WebServer::new(App::new(), &config).unwrap_err();
        match &err {
            ConfigError::Invalid { key: k, .. } => assert_eq!(k, key),
            other => panic!("{other:?}"),
        }
        assert!(!err.to_string().contains("hunter2"), "{err}");
        assert!(!format!("{err:?}").contains("hunter2"), "{err:?}");
    }
}
