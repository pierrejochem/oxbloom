//! The `server.cors.*`, `server.compression.*` and `server.static.*` properties of
//! `WebServer::new`, over real HTTP.

#![cfg(feature = "web")]

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use oxbloom_boot::{AutoConfiguration, Service, WebServer, WebServerAutoConfiguration};
use oxbloom_config::{Config, ConfigError};
use oxbloom_web::{App, CorsConfig, HttpMethod, Route};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), f)
        .await
        .expect("test timed out")
}

/// A body of pseudo-random words, large and compressible but not trivially so.
fn words(n: usize) -> String {
    let mut state = 12345_u32;
    let mut out = String::new();
    while out.len() < n {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
        out.push_str(&format!("w{} ", (state >> 16) % 500));
    }
    out
}

fn app() -> App {
    App::new()
        .route(Route::new(HttpMethod::Get, "/big", |_| async {
            words(20_000)
        }))
        .unwrap()
        .route(Route::new(HttpMethod::Get, "/small", |_| async { "hi" }))
        .unwrap()
}

fn config(extra: &str) -> Config {
    Config::from_str(&format!("[server]\nport = 0\n{extra}")).unwrap()
}

async fn start_app(app: App, extra: &str) -> WebServer {
    let server = WebServer::new(app, &config(extra)).unwrap();
    server.start().await.unwrap();
    server
}

async fn start(extra: &str) -> WebServer {
    start_app(app(), extra).await
}

/// The raw reply: the head (lower-cased) and the body bytes.
async fn send(addr: SocketAddr, request: &str) -> (String, Vec<u8>) {
    let mut conn = TcpStream::connect(addr).await.unwrap();
    conn.write_all(request.as_bytes()).await.unwrap();
    let mut reply = Vec::new();
    within(conn.read_to_end(&mut reply)).await.unwrap();
    let split = reply
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a header end");
    let head = String::from_utf8_lossy(&reply[..split]).to_lowercase();
    (head, reply[split + 4..].to_vec())
}

fn get(path: &str, extra_headers: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{extra_headers}\r\n")
}

fn status(head: &str) -> u16 {
    head[9..12].parse().unwrap()
}

fn toml_path(path: &Path) -> String {
    format!("'{}'", path.display())
}

#[tokio::test]
async fn no_keys_change_nothing() {
    let server = start("").await;
    let addr = server.local_addr().unwrap();
    let (head, body) = send(
        addr,
        &get(
            "/big",
            "Origin: https://a.example\r\nAccept-Encoding: gzip\r\n",
        ),
    )
    .await;
    assert_eq!(status(&head), 200);
    assert!(!head.contains("access-control-"), "{head}");
    assert!(!head.contains("content-encoding"), "{head}");
    assert_eq!(body.len(), words(20_000).len());
    let (head, _) = send(addr, &get("/index.html", "")).await;
    assert_eq!(status(&head), 404);
    server.stop().await;
}

#[tokio::test]
async fn cors_keys_answer_preflights_and_simple_requests() {
    let server = start(
        "[server.cors]\nallowed-origins = [\"https://a.example\"]\n\
         allowed-methods = [\"GET\", \"PUT\"]\nallowed-headers = [\"x-token\"]\n\
         exposed-headers = [\"x-count\"]\nallow-credentials = true\nmax-age = \"10m\"\n",
    )
    .await;
    let addr = server.local_addr().unwrap();
    let preflight = "OPTIONS /small HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
         Origin: https://a.example\r\nAccess-Control-Request-Method: PUT\r\n\
         Access-Control-Request-Headers: x-token\r\n\r\n";
    let (head, _) = send(addr, preflight).await;
    assert!(status(&head) < 300, "{head}");
    assert!(head.contains("access-control-allow-origin: https://a.example"));
    assert!(
        head.contains("access-control-allow-methods: get, put\r\n")
            || head.ends_with("access-control-allow-methods: get, put"),
        "{head}"
    );
    assert!(head.contains("access-control-allow-headers: x-token"));
    assert!(head.contains("access-control-max-age: 600"), "{head}");
    assert!(head.contains("access-control-allow-credentials: true"));

    let (head, _) = send(addr, &get("/small", "Origin: https://a.example\r\n")).await;
    assert_eq!(status(&head), 200);
    assert!(head.contains("access-control-allow-origin: https://a.example"));
    assert!(head.contains("access-control-expose-headers: x-count"));

    let (head, _) = send(addr, &get("/small", "Origin: https://evil.example\r\n")).await;
    assert!(!head.contains("access-control-allow-origin"), "{head}");
    server.stop().await;
}

#[tokio::test]
async fn cors_without_allowed_origins_keeps_a_filter_set_in_code() {
    let app = app()
        .cors(CorsConfig::new().allowed_origins(["https://code.example"]))
        .unwrap();
    // Other cors keys without `allowed-origins` do not turn CORS on or off.
    let server = start_app(app, "[server.cors]\nmax-age = \"1m\"\n").await;
    let addr = server.local_addr().unwrap();
    let (head, _) = send(addr, &get("/small", "Origin: https://code.example\r\n")).await;
    assert!(head.contains("access-control-allow-origin: https://code.example"));
    server.stop().await;
}

#[tokio::test]
async fn compression_enabled_compresses_a_large_body() {
    let server = start("[server.compression]\nenabled = true\n").await;
    let addr = server.local_addr().unwrap();
    let (head, body) = send(addr, &get("/big", "Accept-Encoding: gzip\r\n")).await;
    assert!(head.contains("content-encoding: gzip"), "{head}");
    assert!(body.len() < words(20_000).len() / 2);
    let (head, _) = send(addr, &get("/small", "Accept-Encoding: gzip\r\n")).await;
    assert!(
        !head.contains("content-encoding"),
        "below the default min-size"
    );
    let (head, _) = send(addr, &get("/big", "")).await;
    assert!(!head.contains("content-encoding"), "no Accept-Encoding");
    server.stop().await;
}

#[tokio::test]
async fn compression_enabled_false_compresses_nothing() {
    let server = start("[server.compression]\nenabled = false\nmin-size = 1\n").await;
    let addr = server.local_addr().unwrap();
    let (head, _) = send(addr, &get("/big", "Accept-Encoding: gzip\r\n")).await;
    assert!(!head.contains("content-encoding"), "{head}");
    server.stop().await;
}

#[tokio::test]
async fn compression_min_size_changes_which_bodies_are_compressed() {
    let server = start("[server.compression]\nenabled = true\nmin-size = 1\n").await;
    let addr = server.local_addr().unwrap();
    // A two-byte body is still too small to gain anything only if the filter ignores the key;
    // with `min-size = 1` it is compressed.
    let (head, _) = send(addr, &get("/small", "Accept-Encoding: gzip\r\n")).await;
    assert!(head.contains("content-encoding: gzip"), "{head}");
    server.stop().await;

    let server = start("[server.compression]\nenabled = true\nmin-size = \"1MiB\"\n").await;
    let addr = server.local_addr().unwrap();
    let (head, _) = send(addr, &get("/big", "Accept-Encoding: gzip\r\n")).await;
    assert!(!head.contains("content-encoding"), "{head}");
    server.stop().await;
}

#[tokio::test]
async fn compression_mime_types_limit_the_compressed_types() {
    let server =
        start("[server.compression]\nenabled = true\nmime-types = [\"application/wasm\"]\n").await;
    let addr = server.local_addr().unwrap();
    let (head, _) = send(addr, &get("/big", "Accept-Encoding: gzip\r\n")).await;
    assert!(
        !head.contains("content-encoding"),
        "text/plain is not listed: {head}"
    );
    server.stop().await;
}

#[tokio::test]
async fn compression_max_body_bytes_skips_larger_bodies() {
    let server = start("[server.compression]\nenabled = true\nmax-body-bytes = 1000\n").await;
    let addr = server.local_addr().unwrap();
    let (head, _) = send(addr, &get("/big", "Accept-Encoding: gzip\r\n")).await;
    assert!(!head.contains("content-encoding"), "{head}");
    server.stop().await;
}

#[tokio::test]
async fn compression_level_changes_the_compressed_size() {
    let mut sizes = Vec::new();
    for level in ["fast", "default", "best"] {
        let server = start(&format!(
            "[server.compression]\nenabled = true\nlevel = \"{level}\"\n"
        ))
        .await;
        let addr = server.local_addr().unwrap();
        let (head, body) = send(addr, &get("/big", "Accept-Encoding: gzip\r\n")).await;
        assert!(head.contains("content-encoding: gzip"), "{head}");
        sizes.push(body.len());
        server.stop().await;
    }
    assert!(
        sizes[0] > sizes[2],
        "fast {} vs best {}",
        sizes[0],
        sizes[2]
    );
}

fn site() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<h1>home</h1>").unwrap();
    std::fs::write(dir.path().join("start.html"), "<h1>start</h1>").unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
    std::fs::write(dir.path().join("big.txt"), "x".repeat(100)).unwrap();
    dir
}

#[tokio::test]
async fn static_enabled_serves_files_from_the_directory() {
    let dir = site();
    let server = start(&format!(
        "[server.static]\nenabled = true\ndirectory = {}\n",
        toml_path(dir.path())
    ))
    .await;
    let addr = server.local_addr().unwrap();
    let (head, body) = send(addr, &get("/a.txt", "")).await;
    assert_eq!(status(&head), 200);
    assert_eq!(body, b"hello");
    assert!(head.contains("cache-control: no-cache"), "{head}");
    let (_, body) = send(addr, &get("/", "")).await;
    assert_eq!(body, b"<h1>home</h1>");
    let (head, _) = send(addr, &get("/%2e%2e/secret", "")).await;
    assert_eq!(status(&head), 404);
    server.stop().await;
}

#[tokio::test]
async fn static_path_index_cache_and_size_keys_apply() {
    let dir = site();
    let server = start(&format!(
        "[server.static]\nenabled = true\ndirectory = {}\npath = \"/assets\"\n\
         index = \"start.html\"\ncache-max-age = \"60s\"\nmax-file-bytes = 50\n",
        toml_path(dir.path())
    ))
    .await;
    let addr = server.local_addr().unwrap();
    let (head, body) = send(addr, &get("/assets/a.txt", "")).await;
    assert_eq!(status(&head), 200);
    assert_eq!(body, b"hello");
    assert!(head.contains("cache-control: public, max-age=60"), "{head}");
    let (_, body) = send(addr, &get("/assets", "")).await;
    assert_eq!(body, b"<h1>start</h1>");
    let (head, _) = send(addr, &get("/assets/big.txt", "")).await;
    assert_eq!(status(&head), 404, "larger than max-file-bytes");
    let (head, _) = send(addr, &get("/a.txt", "")).await;
    assert_eq!(status(&head), 404, "outside the prefix");
    server.stop().await;
}

#[tokio::test]
async fn static_disabled_ignores_a_missing_directory() {
    let c = config("[server.static]\nenabled = false\ndirectory = \"/nonexistent-oxbloom-dir\"\n");
    assert!(WebServer::new(App::new(), &c).is_ok());
}

#[test]
fn static_directory_must_exist_and_be_a_directory() {
    let dir = site();
    for path in [
        "/nonexistent-oxbloom-dir".to_string(),
        dir.path().join("a.txt").display().to_string(),
    ] {
        let c = config(&format!(
            "[server.static]\nenabled = true\ndirectory = '{path}'\n"
        ));
        let err = WebServer::new(App::new(), &c).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.static.directory"),
            "{err}"
        );
        assert!(!err.to_string().contains(&path), "{err}");
    }
}

#[test]
fn static_routes_that_clash_with_the_app_fail_naming_the_path_key() {
    let dir = site();
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/{*rest}", |_| async { "x" }))
        .unwrap();
    let c = config(&format!(
        "[server.static]\nenabled = true\ndirectory = {}\n",
        toml_path(dir.path())
    ));
    let err = WebServer::new(app, &c).unwrap_err();
    assert!(
        matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.static.path"),
        "{err}"
    );
}

#[test]
fn invalid_values_name_the_key_and_never_echo_the_value() {
    const ORIGINS: &str = "allowed-origins = [\"https://a.example\"]\n";
    let cases: &[(&str, &str, &str, &str)] = &[
        // (table, key, toml value, extra lines in the same table)
        ("cors", "allowed-origins", "{ x = \"hunter2\" }", ""),
        ("cors", "allowed-origins", "[\"hunter2\"]", ""),
        (
            "cors",
            "allowed-origins",
            "[\"https://a.example/hunter2\"]",
            "",
        ),
        ("cors", "allowed-origins", "[]", ""),
        ("cors", "allowed-methods", "{ x = \"hunter2\" }", ORIGINS),
        ("cors", "allowed-methods", "[\"hunter2 x\"]", ORIGINS),
        ("cors", "allowed-headers", "{ x = \"hunter2\" }", ORIGINS),
        ("cors", "allowed-headers", "[\"hunter2 x\"]", ORIGINS),
        ("cors", "exposed-headers", "{ x = \"hunter2\" }", ORIGINS),
        ("cors", "exposed-headers", "[\"hunter2 x\"]", ORIGINS),
        ("cors", "allow-credentials", "\"hunter2\"", ORIGINS),
        ("cors", "max-age", "\"hunter2\"", ORIGINS),
        ("compression", "enabled", "\"hunter2\"", ""),
        ("compression", "min-size", "\"hunter2\"", ""),
        ("compression", "mime-types", "{ x = \"hunter2\" }", ""),
        ("compression", "level", "\"hunter2\"", ""),
        ("compression", "max-body-bytes", "\"hunter2\"", ""),
        ("static", "enabled", "\"hunter2\"", ""),
        ("static", "directory", "{ x = \"hunter2\" }", ""),
        ("static", "path", "{ x = \"hunter2\" }", ""),
        ("static", "index", "{ x = \"hunter2\" }", ""),
        ("static", "index", "\"hunter2/x\"", ""),
        ("static", "index", "\"hunter2\\\\x\"", ""),
        ("static", "index", "\".hunter2\"", ""),
        ("static", "index", "\"hunter2%2e\"", ""),
        ("static", "index", "\"\"", ""),
        ("static", "index", "\"hunter2\\u0000x\"", ""),
        ("static", "cache-max-age", "\"hunter2\"", ""),
        ("static", "max-file-bytes", "\"hunter2\"", ""),
    ];
    for (table, key, value, extra) in cases {
        let c = config(&format!("[server.{table}]\n{extra}{key} = {value}\n"));
        let err = WebServer::new(App::new(), &c).unwrap_err();
        let full = format!("server.{table}.{key}");
        assert!(
            matches!(err, ConfigError::Invalid { key: ref k, .. } if *k == full),
            "{full} = {value}: {err}"
        );
        assert!(!err.to_string().contains("hunter2"), "{full}: {err}");
    }
}

#[test]
fn an_unusable_cors_origin_list_names_the_key_and_the_reason_but_no_value() {
    for (value, reason) in [
        ("[]", "at least one origin is required"),
        (
            "[\"https://a.example/\"]",
            "an origin must be `null` or scheme://host[:port] without a path or trailing slash",
        ),
    ] {
        let c = config(&format!("[server.cors]\nallowed-origins = {value}\n"));
        let err = WebServer::new(App::new(), &c).unwrap_err();
        assert!(
            matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.cors.allowed-origins"),
            "{err}"
        );
        let text = err.to_string();
        assert!(text.contains(reason), "{value}: {text}");
        assert!(!text.contains("a.example"), "{value}: {text}");
    }
}

#[test]
fn wildcard_origin_with_credentials_names_the_credentials_key() {
    let c = config("[server.cors]\nallowed-origins = [\"*\"]\nallow-credentials = true\n");
    let err = WebServer::new(App::new(), &c).unwrap_err();
    assert!(
        matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.cors.allow-credentials"),
        "{err}"
    );
}

#[test]
fn every_new_key_is_listed_with_its_default() {
    let rows = WebServerAutoConfiguration::properties();
    let default = |key: &str| {
        rows.iter()
            .find(|p| p.key == key)
            .unwrap_or_else(|| panic!("{key} is not listed"))
            .default
    };
    assert_eq!(rows.len(), 25);
    assert_eq!(default("server.cors.allowed-origins"), None);
    assert_eq!(
        default("server.cors.allowed-methods"),
        Some("GET,HEAD,POST")
    );
    assert_eq!(default("server.cors.allowed-headers"), Some(""));
    assert_eq!(default("server.cors.exposed-headers"), Some(""));
    assert_eq!(default("server.cors.allow-credentials"), Some("false"));
    assert_eq!(default("server.cors.max-age"), Some("30m"));
    assert_eq!(default("server.compression.enabled"), Some("false"));
    assert_eq!(default("server.compression.min-size"), Some("1KiB"));
    assert!(
        default("server.compression.mime-types")
            .unwrap()
            .contains("text/*")
    );
    assert_eq!(default("server.compression.level"), Some("default"));
    assert_eq!(default("server.compression.max-body-bytes"), Some("8MiB"));
    assert_eq!(default("server.static.enabled"), Some("false"));
    assert_eq!(default("server.static.directory"), Some("static"));
    assert_eq!(default("server.static.path"), Some("/"));
    assert_eq!(default("server.static.index"), Some("index.html"));
    assert_eq!(default("server.static.cache-max-age"), Some("0s"));
    assert_eq!(default("server.static.max-file-bytes"), Some("10MiB"));
}

#[tokio::test]
async fn static_path_is_normalized_like_the_resources_prefix() {
    let dir = site();
    let cases = [
        ("\"\"", "/"),
        ("\"assets\"", "/assets"),
        ("\"/a/b/\"", "/a/b"),
        ("\"/x{y}\"", "/x{y}"),
    ];
    for (value, url) in cases {
        let server = start(&format!(
            "[server.static]\nenabled = true\ndirectory = {}\npath = {value}\n",
            toml_path(dir.path())
        ))
        .await;
        let addr = server.local_addr().unwrap();
        let (head, body) = send(addr, &get(url, "")).await;
        assert_eq!(status(&head), 200, "{value}: {head}");
        assert_eq!(body, b"<h1>home</h1>", "{value}");
        server.stop().await;
    }
}

#[tokio::test]
async fn compression_set_in_code_survives_absent_and_disabled_keys() {
    for extra in ["", "[server.compression]\nenabled = false\n"] {
        let app = app().compression(oxbloom_web::CompressionConfig::new());
        let server = start_app(app, extra).await;
        let addr = server.local_addr().unwrap();
        let (head, _) = send(addr, &get("/big", "Accept-Encoding: gzip\r\n")).await;
        assert!(head.contains("content-encoding: gzip"), "{extra}: {head}");
        server.stop().await;
    }
}

#[tokio::test]
async fn static_route_set_in_code_survives_absent_and_disabled_keys() {
    let dir = site();
    for extra in [
        String::new(),
        format!(
            "[server.static]\nenabled = false\ndirectory = {}\n",
            toml_path(dir.path())
        ),
    ] {
        let site = oxbloom_web::StaticResources::new(
            "/code",
            oxbloom_web::EmbeddedResources::new(&[("index.html", b"from code")]),
        );
        let app = app()
            .routes(oxbloom_web::Controller::routes(std::sync::Arc::new(site)))
            .unwrap();
        let server = start_app(app, &extra).await;
        let addr = server.local_addr().unwrap();
        let (_, body) = send(addr, &get("/code", "")).await;
        assert_eq!(body, b"from code", "{extra}");
        let (head, _) = send(addr, &get("/a.txt", "")).await;
        assert_eq!(status(&head), 404, "{extra}");
        server.stop().await;
    }
}
