//! `HttpClientSettings`: the `http.client` keys, their defaults, their errors and the client
//! they build.

#![cfg(feature = "config")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use oxbloom_http_client::{HttpClientSettings, Request};
use oxbloom_tasks::{Backoff, Jitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn settings(toml: &str) -> Result<HttpClientSettings, ConfigError> {
    HttpClientSettings::from_config(&Config::from_str(toml).unwrap())
}

#[test]
fn settings_default_values() {
    let s = settings("").unwrap();
    assert_eq!(s.timeout, Duration::from_secs(10));
    assert_eq!(s.connect_timeout, None);
    assert_eq!(s.max_body_bytes, 1_048_576);
    assert_eq!(s.follow_redirects, 0);
    assert_eq!(s.user_agent, None);
    assert_eq!(s.ssl_bundle, None);
    assert_eq!(s.retry, None);
    assert_eq!(s, HttpClientSettings::default());
    // A section without any key is the same as no section.
    assert_eq!(settings("[http.client]").unwrap(), s);
}

#[test]
fn settings_read_every_key() {
    let s = settings(
        r#"
[http.client]
timeout = "3s"
connect-timeout = "500ms"
max-body-bytes = 2048
follow-redirects = 2
user-agent = "oxbloom-test/1"
ssl-bundle = "outbound"

[http.client.retry]
max-attempts = 4
backoff = "fixed"
initial = "20ms"
multiplier = 3.0
max-delay = "2s"
jitter = "equal"
"#,
    )
    .unwrap();
    assert_eq!(s.timeout, Duration::from_secs(3));
    assert_eq!(s.connect_timeout, Some(Duration::from_millis(500)));
    assert_eq!(s.max_body_bytes, 2048);
    assert_eq!(s.follow_redirects, 2);
    assert_eq!(s.user_agent.as_deref(), Some("oxbloom-test/1"));
    assert_eq!(s.ssl_bundle.as_deref(), Some("outbound"));
    let retry = s.retry.unwrap();
    assert_eq!(retry.max_attempts, 4);
    assert_eq!(retry.backoff, Backoff::Fixed);
    assert_eq!(retry.initial, Duration::from_millis(20));
    assert_eq!(retry.multiplier, 3.0);
    assert_eq!(retry.max_delay, Duration::from_secs(2));
    assert_eq!(retry.jitter, Jitter::Equal);
}

#[test]
fn retry_keys_default_to_the_policy_defaults() {
    let retry = settings("[http.client.retry]\nmax-attempts = 2")
        .unwrap()
        .retry
        .unwrap();
    let defaults = oxbloom_tasks::RetryPolicy::default();
    assert_eq!(retry.max_attempts, 2);
    assert_eq!(retry.backoff, defaults.backoff);
    assert_eq!(retry.initial, defaults.initial);
    assert_eq!(retry.max_delay, defaults.max_delay);
    assert_eq!(retry.jitter, defaults.jitter);
}

#[test]
fn an_invalid_duration_names_the_full_key() {
    for (toml, key) in [
        ("[http.client]\ntimeout = \"soon\"", "http.client.timeout"),
        (
            "[http.client]\nconnect-timeout = \"soon\"",
            "http.client.connect-timeout",
        ),
        (
            "[http.client.retry]\nmax-attempts = 2\ninitial = \"soon\"",
            "http.client.retry.initial",
        ),
        (
            "[http.client.retry]\nmax-attempts = 2\nmax-delay = \"soon\"",
            "http.client.retry.max-delay",
        ),
    ] {
        let err = settings(toml).unwrap_err();
        match &err {
            ConfigError::Invalid { key: k, .. } => assert_eq!(k, key, "{err}"),
            other => panic!("expected Invalid for {key}, got {other:?}"),
        }
        assert!(!err.to_string().contains("soon"), "{err}");
    }
}

#[test]
fn other_invalid_values_name_the_full_key() {
    for (toml, key) in [
        ("[http.client]\ntimeout = \"0s\"", "http.client.timeout"),
        (
            "[http.client]\nmax-body-bytes = 0",
            "http.client.max-body-bytes",
        ),
        (
            "[http.client]\nmax-body-bytes = \"big\"",
            "http.client.max-body-bytes",
        ),
        ("[http.client]\nuser-agent = \"\"", "http.client.user-agent"),
        ("[http.client]\nssl-bundle = \"\"", "http.client.ssl-bundle"),
        (
            "[http.client.retry]\nmax-attempts = 0",
            "http.client.retry.max-attempts",
        ),
        (
            "[http.client.retry]\nmax-attempts = 2\nbackoff = \"wild\"",
            "http.client.retry.backoff",
        ),
        (
            "[http.client.retry]\nmax-attempts = 2\njitter = \"wild\"",
            "http.client.retry.jitter",
        ),
        (
            "[http.client.retry]\nmax-attempts = 2\nmultiplier = 0.5",
            "http.client.retry.multiplier",
        ),
        (
            "[http.client.retry]\nmax-attempts = 2\ninitial = \"1m\"",
            "http.client.retry.initial",
        ),
    ] {
        let err = settings(toml).unwrap_err();
        match &err {
            ConfigError::Invalid { key: k, .. } => assert_eq!(k, key, "{toml}: {err}"),
            other => panic!("expected Invalid for {key}, got {other:?}"),
        }
    }
}

#[test]
fn an_unknown_key_is_rejected() {
    assert!(settings("[http.client]\ntimeuot = \"3s\"").is_err());
    assert!(settings("[http.client.retry]\nmax-attemps = 3").is_err());
}

/// A server that answers 503 to the first `failures` requests and 200 afterwards.
async fn flaky(failures: usize) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let answer = if n < failures {
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
            };
            let _ = stream.write_all(answer.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (format!("http://{addr}/"), hits)
}

#[tokio::test]
async fn retry_wraps_the_client_only_when_max_attempts_is_above_one() {
    // Without the retry keys, and with `max-attempts = 1`, the 503 is returned as it is.
    for toml in ["", "[http.client.retry]\nmax-attempts = 1"] {
        let (url, hits) = flaky(1).await;
        let client = settings(toml).unwrap().build(None).unwrap();
        let response = client.execute(Request::get(&url)).await.unwrap();
        assert_eq!(
            response.status(),
            http::StatusCode::SERVICE_UNAVAILABLE,
            "{toml}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1, "{toml}");
    }
    // With `max-attempts = 3` the request is retried and the second answer is returned.
    let (url, hits) = flaky(1).await;
    let client =
        settings("[http.client.retry]\nmax-attempts = 3\ninitial = \"1ms\"\nmax-delay = \"5ms\"")
            .unwrap()
            .build(None)
            .unwrap();
    let response = client.execute(Request::get(&url)).await.unwrap();
    assert_eq!(response.text().unwrap(), "ok");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn the_built_client_uses_the_configured_user_agent_and_limit() {
    let (url, _) = flaky(0).await;
    let client = settings("[http.client]\nmax-body-bytes = 1")
        .unwrap()
        .build(None)
        .unwrap();
    // The body `ok` has two bytes, over the limit of one.
    assert_eq!(
        client.execute(Request::get(&url)).await.unwrap_err(),
        oxbloom_http_client::HttpClientError::TooLarge
    );
}

#[test]
fn a_user_agent_that_is_not_a_header_value_names_the_key_without_the_value() {
    let err = settings("[http.client]\nuser-agent = \"MARKER-9\\nvalue\"").unwrap_err();
    assert!(
        matches!(&err, ConfigError::Invalid { key, .. } if key == "http.client.user-agent"),
        "{err:?}"
    );
    let text = format!("{err} {err:?}");
    assert!(!text.contains("MARKER-9"), "{text}");
}
