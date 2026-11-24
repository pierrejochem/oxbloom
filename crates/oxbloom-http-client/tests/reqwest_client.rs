//! `ReqwestClient` against hand-written HTTP/1.1 servers on loopback sockets.

#![cfg(feature = "reqwest")]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxbloom_http_client::{Body, ClientBuilder, HttpClient, HttpClientError, Request};
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// The requests a server received, each as its head followed by its body, in arrival order.
type Seen = Arc<Mutex<Vec<String>>>;

/// Reads one request (head and, when it has a `Content-Length`, its body). Returns `None`
/// when the connection closes first.
async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Option<String> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while data.len() < head_end + length {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    Some(String::from_utf8_lossy(&data).to_string())
}

/// Starts a loopback server that answers every request with `answer(request)` and closes the
/// connection. Returns the base URL and the received requests.
async fn server<F>(answer: F) -> (String, Seen)
where
    F: Fn(&str) -> String + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            let answer = Arc::clone(&answer);
            tokio::spawn(async move {
                let Some(request) = read_request(&mut stream).await else {
                    return;
                };
                let response = answer(&request);
                log.lock().unwrap().push(request);
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

/// A complete HTTP/1.1 answer with the given status line, extra header lines and body.
fn answer(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The path of a request, taken from its request line.
fn path_of(request: &str) -> &str {
    request.split(' ').nth(1).unwrap_or("")
}

/// Whether the head of `request` has a header called `name`, ignoring case.
fn has_header(request: &str, name: &str) -> bool {
    request
        .lines()
        .take_while(|line| !line.is_empty())
        .any(|line| {
            line.split_once(':')
                .is_some_and(|(n, _)| n.eq_ignore_ascii_case(name))
        })
}

/// The value of header `name` in the head of `request`, ignoring the case of the name.
fn header_of<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request
        .lines()
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.eq_ignore_ascii_case(name).then(|| v.trim())
        })
}

/// The body of a recorded request.
fn body_of(request: &str) -> &str {
    request.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// A loopback address on which nothing listens.
async fn closed_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    format!("http://{addr}/")
}

/// A server that accepts connections and never answers.
async fn silent_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            open.push(stream);
        }
    });
    format!("http://{addr}/")
}

#[tokio::test]
async fn get_returns_status_headers_and_body() {
    let (base, seen) = server(|_| answer("201 Created", "X-Demo: yes\r\n", "hello")).await;
    let client = ClientBuilder::new().build().unwrap();
    let response = client
        .execute(Request::get(format!("{base}/greeting")).header("accept", "text/plain"))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::CREATED);
    assert_eq!(response.headers()["x-demo"], "yes");
    assert_eq!(response.text().unwrap(), "hello");
    let seen = seen.lock().unwrap();
    assert_eq!(path_of(&seen[0]), "/greeting");
    assert!(seen[0].starts_with("GET "));
    assert_eq!(header_of(&seen[0], "accept"), Some("text/plain"));
}

#[tokio::test]
async fn form_body_is_urlencoded_with_the_right_content_type() {
    let (base, seen) = server(|_| answer("200 OK", "", "")).await;
    let client = ClientBuilder::new().build().unwrap();
    let request = Request::post(format!("{base}/token")).body(Body::Form(vec![
        ("grant_type".into(), "client_credentials".into()),
        ("scope".into(), "read write&x=1".into()),
    ]));
    client.execute(request).await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        header_of(&seen[0], "content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        body_of(&seen[0]),
        "grant_type=client_credentials&scope=read+write%26x%3D1"
    );
}

#[cfg(feature = "json")]
#[tokio::test]
async fn json_body_sets_content_type() {
    let (base, seen) = server(|_| answer("200 OK", "", "")).await;
    let client = ClientBuilder::new().build().unwrap();
    let request = Request::post(format!("{base}/items"))
        .body(Body::Json(serde_json::json!({"name": "pen", "count": 2})));
    client.execute(request).await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        header_of(&seen[0], "content-type"),
        Some("application/json")
    );
    let sent: serde_json::Value = serde_json::from_str(body_of(&seen[0])).unwrap();
    assert_eq!(sent, serde_json::json!({"name": "pen", "count": 2}));
}

#[tokio::test]
async fn a_content_type_set_by_the_caller_is_kept() {
    let (base, seen) = server(|_| answer("200 OK", "", "")).await;
    let client = ClientBuilder::new().build().unwrap();
    let request = Request::post(format!("{base}/raw"))
        .header("content-type", "application/octet-stream")
        .body(Body::Bytes(vec![b'a', b'b']));
    client.execute(request).await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        header_of(&seen[0], "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(body_of(&seen[0]), "ab");
}

#[tokio::test]
async fn a_slow_server_gives_timeout() {
    let url = silent_server().await;
    let client = ClientBuilder::new()
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let started = Instant::now();
    let error = client.execute(Request::get(url)).await.unwrap_err();
    assert_eq!(error, HttpClientError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_per_request_timeout_overrides_the_default() {
    let url = silent_server().await;
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let started = Instant::now();
    let error = client
        .execute(Request::get(url).timeout(Duration::from_millis(200)))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_slow_drip_body_cannot_outlive_the_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head).await;
        while stream.write_all(b" ").await.is_ok() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let client = ClientBuilder::new()
        .timeout(Duration::from_millis(400))
        .build()
        .unwrap();
    let started = Instant::now();
    let error = client
        .execute(Request::get(format!("http://{addr}/")))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_body_over_the_limit_gives_too_large_without_reading_it_all() {
    const TOTAL: usize = 64 * 1024 * 1024;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (written_tx, written_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head).await;
        let chunk = vec![b'x'; 65_536];
        let mut written = 0;
        while written < TOTAL {
            if stream.write_all(b"10000\r\n").await.is_err()
                || stream.write_all(&chunk).await.is_err()
                || stream.write_all(b"\r\n").await.is_err()
            {
                break;
            }
            written += chunk.len();
        }
        let _ = written_tx.send(written);
    });
    let client = ClientBuilder::new().max_body_bytes(1024).build().unwrap();
    let error = client
        .execute(Request::get(format!("http://{addr}/big")))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::TooLarge);
    // The client dropped the connection, so the server could not write the whole body.
    let written = tokio::time::timeout(Duration::from_secs(10), written_rx)
        .await
        .expect("the server should stop writing once the client is gone")
        .unwrap();
    assert!(written < TOTAL, "the server wrote the whole body");
}

#[tokio::test]
async fn a_large_content_length_is_refused_before_reading() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        // Announces 10 MB but sends only a few bytes and then waits: a client that tried
        // to read the body would run into its timeout.
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 10000000\r\nConnection: close\r\n\r\nabc";
        let _ = stream.write_all(head).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let client = ClientBuilder::new()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let started = Instant::now();
    let error = client
        .execute(Request::get(format!("http://{addr}/")))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::TooLarge);
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_body_of_exactly_the_limit_is_accepted() {
    let body = "y".repeat(1024);
    let (base, _) = server(move |_| answer("200 OK", "", &body)).await;
    let client = ClientBuilder::new().max_body_bytes(1024).build().unwrap();
    let response = client.execute(Request::get(base)).await.unwrap();
    assert_eq!(response.bytes().len(), 1024);
}

/// A server where `/r1` redirects to `/r2`, `/r2` to `/r3`, `/r3` to `/end`, and `/end`
/// answers 200.
async fn redirect_chain() -> (String, Seen) {
    server(|request| match path_of(request) {
        "/r1" => answer("302 Found", "Location: /r2\r\n", ""),
        "/r2" => answer("302 Found", "Location: /r3\r\n", ""),
        "/r3" => answer("302 Found", "Location: /end\r\n", ""),
        _ => answer("200 OK", "", "end"),
    })
    .await
}

#[tokio::test]
async fn a_redirect_is_not_followed_by_default() {
    let (base, seen) = redirect_chain().await;
    let client = ClientBuilder::new().build().unwrap();
    let response = client
        .execute(Request::get(format!("{base}/r1")))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::FOUND);
    assert_eq!(response.headers()["location"], "/r2");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn follow_redirects_n_follows_up_to_n() {
    let (base, seen) = redirect_chain().await;
    let client = ClientBuilder::new().follow_redirects(2).build().unwrap();
    // Two redirects are followed; the third 302 is returned as the response.
    let response = client
        .execute(Request::get(format!("{base}/r1")))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::FOUND);
    assert_eq!(response.headers()["location"], "/end");
    let paths: Vec<String> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|r| path_of(r).to_owned())
        .collect();
    assert_eq!(paths, ["/r1", "/r2", "/r3"]);

    // Starting one step later, two redirects reach the end.
    let response = client
        .execute(Request::get(format!("{base}/r2")))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.text().unwrap(), "end");
}

#[tokio::test]
async fn no_referer_and_no_cookies_are_sent() {
    let (base, seen) = server(|request| match path_of(request) {
        "/login" => answer(
            "302 Found",
            "Set-Cookie: session=abc123; Path=/\r\nLocation: /home\r\n",
            "",
        ),
        _ => answer("200 OK", "Set-Cookie: other=xyz; Path=/\r\n", "home"),
    })
    .await;
    let client = ClientBuilder::new().follow_redirects(1).build().unwrap();
    client
        .execute(Request::get(format!("{base}/login")))
        .await
        .unwrap();
    client
        .execute(Request::get(format!("{base}/home")))
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for request in seen.iter() {
        assert!(!has_header(request, "cookie"), "{request}");
        assert!(!has_header(request, "referer"), "{request}");
    }
}

#[tokio::test]
async fn a_redirect_to_another_origin_drops_the_authorization_header() {
    let (target, received) = server(|_| answer("200 OK", "", "moved")).await;
    let location = format!("Location: {target}/landing\r\n");
    let (origin, first) = server(move |_| answer("302 Found", &location, "")).await;
    let client = ClientBuilder::new().follow_redirects(2).build().unwrap();
    let response = client
        .execute(
            Request::get(format!("{origin}/start"))
                .header("authorization", "Bearer secret")
                .header("x-api-key", "key-1"),
        )
        .await
        .unwrap();
    assert_eq!(response.text().unwrap(), "moved");
    // The first server, on the original origin, gets the credentials.
    assert_eq!(
        header_of(&first.lock().unwrap()[0], "authorization"),
        Some("Bearer secret")
    );
    // The second server, on another port, gets the custom header but no credentials.
    let received = received.lock().unwrap();
    assert_eq!(path_of(&received[0]), "/landing");
    assert!(
        !has_header(&received[0], "authorization"),
        "{}",
        received[0]
    );
    assert_eq!(header_of(&received[0], "x-api-key"), Some("key-1"));
}

#[tokio::test]
async fn a_cross_origin_307_is_not_followed_and_the_body_is_not_resent() {
    let (target, received) = server(|_| answer("200 OK", "", "moved")).await;
    let location = format!("Location: {target}/token\r\n");
    let (origin, first) = server(move |_| answer("307 Temporary Redirect", &location, "")).await;
    let client = ClientBuilder::new().follow_redirects(2).build().unwrap();
    let response = client
        .execute(
            Request::post(format!("{origin}/token")).body(Body::Form(vec![
                ("grant_type".into(), "client_credentials".into()),
                ("client_secret".into(), "top-secret".into()),
            ])),
        )
        .await
        .unwrap();
    // The 307 itself is returned, as when the redirect budget runs out.
    assert_eq!(response.status(), http::StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(first.lock().unwrap().len(), 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    // The other origin never sees a request, so it never sees the secret.
    assert!(received.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_same_origin_307_resends_the_body() {
    let (base, seen) = server(|request| match path_of(request) {
        "/a" => answer("307 Temporary Redirect", "Location: /b\r\n", ""),
        _ => answer("200 OK", "", "done"),
    })
    .await;
    let client = ClientBuilder::new().follow_redirects(2).build().unwrap();
    let response = client
        .execute(
            Request::post(format!("{base}/a"))
                .body(Body::Form(vec![("code".into(), "abc".into())])),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(response.text().unwrap(), "done");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(path_of(&seen[1]), "/b");
    assert!(seen[1].starts_with("POST "), "{}", seen[1]);
    assert_eq!(body_of(&seen[1]), "code=abc");
}

#[tokio::test]
async fn a_cross_origin_302_on_a_post_becomes_a_get_without_body_or_credentials() {
    let (target, received) = server(|_| answer("200 OK", "", "moved")).await;
    let location = format!("Location: {target}/landing\r\n");
    let (origin, _) = server(move |_| answer("302 Found", &location, "")).await;
    let client = ClientBuilder::new().follow_redirects(2).build().unwrap();
    let response = client
        .execute(
            Request::post(format!("{origin}/token"))
                .header("authorization", "Basic c2VjcmV0")
                .body(Body::Form(vec![(
                    "client_secret".into(),
                    "top-secret".into(),
                )])),
        )
        .await
        .unwrap();
    assert_eq!(response.text().unwrap(), "moved");
    let received = received.lock().unwrap();
    assert_eq!(received.len(), 1);
    assert!(received[0].starts_with("GET /landing "), "{}", received[0]);
    assert!(
        !has_header(&received[0], "authorization"),
        "{}",
        received[0]
    );
    assert_eq!(body_of(&received[0]), "");
    assert!(!received[0].contains("top-secret"));
}

#[tokio::test]
async fn a_file_or_ftp_url_is_an_invalid_request() {
    let client = ClientBuilder::new().build().unwrap();
    for url in ["file:///etc/passwd", "ftp://example.com/x"] {
        assert_eq!(
            client.execute(Request::get(url)).await.unwrap_err(),
            HttpClientError::InvalidRequest,
            "{url}"
        );
    }
}

#[tokio::test]
async fn a_followed_redirect_to_a_non_http_scheme_is_an_invalid_request() {
    let (base, _) = server(|_| answer("302 Found", "Location: ftp://example.com/x\r\n", "")).await;
    let client = ClientBuilder::new().follow_redirects(2).build().unwrap();
    assert_eq!(
        client.execute(Request::get(base)).await.unwrap_err(),
        HttpClientError::InvalidRequest
    );
}

#[tokio::test]
async fn credentials_in_the_url_become_a_basic_authorization_header() {
    let (base, seen) = server(|_| answer("200 OK", "", "")).await;
    let url = base.replace("http://", "http://user:pw@");
    let client = ClientBuilder::new().build().unwrap();
    client
        .execute(Request::get(format!("{url}/x")))
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    // "user:pw" in base64.
    assert_eq!(
        header_of(&seen[0], "authorization"),
        Some("Basic dXNlcjpwdw==")
    );
}

#[tokio::test]
async fn the_user_agent_is_sent_when_configured() {
    let (base, seen) = server(|_| answer("200 OK", "", "")).await;
    let client = ClientBuilder::new()
        .user_agent("oxbloom-test/1.0")
        .build()
        .unwrap();
    client.execute(Request::get(base)).await.unwrap();
    assert_eq!(
        header_of(&seen.lock().unwrap()[0], "user-agent"),
        Some("oxbloom-test/1.0")
    );
}

#[tokio::test]
async fn a_bad_user_agent_fails_the_build() {
    let error = ClientBuilder::new()
        .user_agent("bad\nagent")
        .build()
        .unwrap_err();
    assert_eq!(error, HttpClientError::Build);
}

#[tokio::test]
async fn a_refused_connection_gives_network() {
    let client = ClientBuilder::new().build().unwrap();
    let error = client
        .execute(Request::get(closed_port_url().await))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::Network);
}

#[tokio::test]
async fn an_invalid_request_is_rejected_before_sending() {
    let (base, seen) = server(|_| answer("200 OK", "", "")).await;
    let client = ClientBuilder::new().build().unwrap();
    let error = client
        .execute(Request::get(base).header("bad name", "x"))
        .await
        .unwrap_err();
    assert_eq!(error, HttpClientError::InvalidRequest);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn errors_never_contain_the_url_or_the_body() {
    let url = closed_port_url().await + "?code=SECRET-MARKER";
    let client = ClientBuilder::new().build().unwrap();
    let error = client.execute(Request::get(url)).await.unwrap_err();
    let text = format!("{error} {error:?} {client:?}");
    assert!(
        !text.contains("SECRET-MARKER") && !text.contains("127.0.0.1"),
        "{text}"
    );
}

/// A private test CA.
struct Ca {
    issuer: Issuer<'static, KeyPair>,
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
            issuer: Issuer::new(params, key),
        }
    }

    /// A rustls client configuration that trusts this CA only.
    fn client_config(&self) -> Arc<ClientConfig> {
        let mut roots = RootCertStore::empty();
        roots.add(self.der.clone()).unwrap();
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    }

    /// A rustls server configuration with a `localhost` certificate signed by this CA.
    fn server_config(&self) -> Arc<ServerConfig> {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&key, &self.issuer)
            .unwrap();
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], private)
            .unwrap();
        Arc::new(config)
    }
}

/// Answers one request on an accepted stream with 200 and the body `secure`.
async fn answer_secure<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) {
    if read_request(&mut stream).await.is_some() {
        let _ = stream
            .write_all(answer("200 OK", "", "secure").as_bytes())
            .await;
        let _ = stream.shutdown().await;
    }
}

/// Serves HTTPS on a loopback port with a `localhost` certificate of `ca`, answering every
/// request with `answer`, and returns the URL.
async fn https_server_answering(ca: &Ca, answer: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = TlsAcceptor::from(ca.server_config());
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let answer = answer.clone();
            tokio::spawn(async move {
                if let Ok(mut tls) = acceptor.accept(tcp).await
                    && read_request(&mut tls).await.is_some()
                {
                    let _ = tls.write_all(answer.as_bytes()).await;
                    let _ = tls.shutdown().await;
                }
            });
        }
    });
    format!("https://localhost:{port}/")
}

#[tokio::test]
async fn a_redirect_from_https_to_http_is_not_followed() {
    let ca = Ca::new();
    let (plain, received) = server(|_| answer("200 OK", "", "plain")).await;
    // A 302 (not only a 307) is refused: the downgrade alone is enough.
    let url = https_server_answering(
        &ca,
        answer("302 Found", &format!("Location: {plain}/landing\r\n"), ""),
    )
    .await;
    let client = ClientBuilder::new()
        .tls(ca.client_config())
        .follow_redirects(2)
        .build()
        .unwrap();
    let response = client
        .execute(Request::get(url).header("x-api-key", "key-1"))
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::FOUND);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(received.lock().unwrap().is_empty());
}

/// Serves HTTPS on a loopback port with a `localhost` certificate of `ca` and returns the URL.
async fn https_server(ca: &Ca) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = TlsAcceptor::from(ca.server_config());
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    answer_secure(tls).await;
                }
            });
        }
    });
    format!("https://localhost:{port}/")
}

#[tokio::test]
async fn a_tls_config_decides_which_ca_is_trusted() {
    let ca = Ca::new();
    let url = https_server(&ca).await;

    let trusting = ClientBuilder::new()
        .tls(ca.client_config())
        .build()
        .unwrap();
    let response = trusting.execute(Request::get(url.clone())).await.unwrap();
    assert_eq!(response.text().unwrap(), "secure");

    // The platform verifier does not know the private CA.
    let platform = ClientBuilder::new().build().unwrap();
    assert_eq!(
        platform
            .execute(Request::get(url.clone()))
            .await
            .unwrap_err(),
        HttpClientError::Network
    );

    // Neither does a configuration that trusts another CA.
    let other = ClientBuilder::new()
        .tls(Ca::new().client_config())
        .build()
        .unwrap();
    assert_eq!(
        other.execute(Request::get(url)).await.unwrap_err(),
        HttpClientError::Network
    );
}

#[tokio::test]
async fn the_client_is_clone_and_the_clones_share_the_settings() {
    let (base, _) = server(|_| answer("200 OK", "", &"z".repeat(100))).await;
    let client = ClientBuilder::new().max_body_bytes(10).build().unwrap();
    let copy = client.clone();
    assert_eq!(
        copy.execute(Request::get(base)).await.unwrap_err(),
        HttpClientError::TooLarge
    );
}
