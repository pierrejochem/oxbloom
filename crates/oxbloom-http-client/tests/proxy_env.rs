//! `ReqwestClient` honors the proxy environment variables.
//!
//! This file holds a single test, so it runs alone in its own test binary and nothing else
//! reads the environment while it changes it.

#![cfg(feature = "reqwest")]

use std::sync::{Arc, Mutex};

use oxbloom_http_client::{ClientBuilder, HttpClient, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Starts a loopback server that records the request line of every request, answers it with
/// 200 and the body `name`, and returns its address and the recorded lines.
async fn recording_server(name: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buf = [0u8; 1024];
                while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&data);
                log.lock()
                    .unwrap()
                    .push(head.lines().next().unwrap_or("").to_owned());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{name}",
                    name.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (addr, seen)
}

#[tokio::test]
async fn http_proxy_and_no_proxy_from_the_environment_are_honored() {
    let (proxy, proxied) = recording_server("proxy").await;
    let (origin, direct) = recording_server("origin").await;

    // SAFETY: this test binary runs this one test only, and no other thread reads or writes
    // the environment while it runs.
    unsafe {
        for name in [
            "ALL_PROXY",
            "all_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "no_proxy",
        ] {
            std::env::remove_var(name);
        }
        std::env::set_var("HTTP_PROXY", format!("http://{proxy}"));
        std::env::set_var("http_proxy", format!("http://{proxy}"));
        std::env::set_var("NO_PROXY", "");
    }

    // With HTTP_PROXY set, a request for another host goes to the proxy in absolute form.
    let client = ClientBuilder::new().build().unwrap();
    let response = client
        .execute(Request::get("http://origin.example.test/via-proxy"))
        .await
        .unwrap();
    assert_eq!(response.text().unwrap(), "proxy");
    // Without NO_PROXY, even the loopback origin is reached through the proxy.
    let response = client
        .execute(Request::get(format!("http://{origin}/before")))
        .await
        .unwrap();
    assert_eq!(response.text().unwrap(), "proxy");
    assert_eq!(
        proxied.lock().unwrap().as_slice(),
        [
            "GET http://origin.example.test/via-proxy HTTP/1.1".to_owned(),
            format!("GET http://{origin}/before HTTP/1.1"),
        ]
    );
    assert!(direct.lock().unwrap().is_empty());

    // With NO_PROXY naming the loopback address, the request goes straight to the origin.
    // The environment is read when the client is built.
    // SAFETY: as above.
    unsafe {
        std::env::set_var("NO_PROXY", "127.0.0.1");
    }
    let client = ClientBuilder::new().build().unwrap();
    let response = client
        .execute(Request::get(format!("http://{origin}/direct")))
        .await
        .unwrap();
    assert_eq!(response.text().unwrap(), "origin");
    assert_eq!(direct.lock().unwrap().as_slice(), ["GET /direct HTTP/1.1"]);
    assert_eq!(proxied.lock().unwrap().len(), 2);
}
