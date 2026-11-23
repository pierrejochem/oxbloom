# HTTP client

Oxbloom calls other services through one outbound client with safe defaults. The crate `oxbloom-http-client` holds the `HttpClient` trait, the `Request` and `Response` types, a `reqwest`-based `ReqwestClient`, a `RetryingClient` and an `InterceptedClient`; `oxbloom-test` has `MockHttpClient` for tests; and `oxbloom-boot` registers a configured client as a bean. This is security Slice 9, part A. OAuth2 login builds on it, and so does the JWKS fetcher of the JWT resource server in `oxbloom-security` (see [Security](08-security.md)).

```toml
# Cargo.toml
oxbloom-http-client = "0.1"                                                   # the client
oxbloom-http-client = { version = "0.1", features = ["retry"] }               # with RetryingClient
oxbloom-boot = { version = "0.1", features = ["http-client"] }                # the bean and http.client.* keys
oxbloom-test = { version = "0.1", features = ["http-client"] }                # MockHttpClient (dev-dependency)
```

The decisions are in [ADR 0093](../adr/0093-http-client.md) and the public items are in the [API reference](../API.md#414-oxbloom-http-client). The design is a local file under `docs/superpowers`, so this page names it without a link.

## A first request

Build a client with `ClientBuilder`, build a `Request`, and call `execute`. A status that is not 2xx is a normal response; only a failure to get any answer is an error.

```rust
use oxbloom_http_client::{ClientBuilder, HttpClient, HttpClientError, Request};

async fn stock() -> Result<u64, HttpClientError> {
    let client = ClientBuilder::new().build()?;
    let response = client
        .execute(Request::get("https://api.example.test/stock").header("accept", "application/json"))
        .await?
        .error_for_status()?; // a 4xx or 5xx becomes HttpClientError::BadStatus
    let body: serde_json::Value = response.json()?;
    Ok(body["stock"].as_u64().unwrap_or(0))
}
```

A `Request` has a method, a URL, headers, a `Body` (`Empty`, `Bytes`, `Form` or, with the default feature `json`, `Json`) and an optional timeout of its own. `Body::Form` is URL-encoded and `Body::Json` is serialized, and the matching `Content-Type` is set unless you set one. A `Response` has the `status`, the `headers`, the body as `bytes`, `text` or `json`, and `error_for_status`.

Code that needs a client should take `Arc<dyn HttpClient>`, not `ReqwestClient`, so a test can pass `MockHttpClient`.

## Defaults and `ClientBuilder`

| Setting | Default | Builder method |
|---|---|---|
| Total timeout of a request, connect included | 10 seconds | `timeout` |
| Connect timeout | none | `connect_timeout` |
| Response body limit | 1 MiB | `max_body_bytes` |
| Redirects followed | 0 | `follow_redirects` |
| `User-Agent` | none | `user_agent` |
| TLS | the platform certificate verifier | `tls(Arc<rustls::ClientConfig>)` |

The body is read incrementally. The call fails with `TooLarge` as soon as the limit is passed, and a `Content-Length` above the limit is refused before the body is read. No `Referer` and no cookies are sent. Proxies come from the environment only: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` are read when the client is built, and there is no setter.

A request can set its own `timeout`, which replaces the client's default for that call.

For TLS with your own trust, pass a `rustls::ClientConfig` built on the `aws-lc-rs` provider, for example one from `oxbloom_ssl::SslBundle::client_config`. A configuration with its own root store trusts only those CAs.

### Redirects

With `follow_redirects(n)`, up to `n` redirects are followed. A chain that needs more returns the last 3xx response instead of an error, so a caller that treats every non-2xx status as a failure rejects it. Some redirects are never followed, whatever the budget, and the 3xx response is returned instead:

- a redirect from `https` to `http`, whatever its status;
- a 307 or 308 to another origin (another scheme, host or port). These two statuses make the client send the method and the body again, and a form body can hold a `client_secret`, a `refresh_token`, a `code` or a `code_verifier`.

A 301, 302 or 303 to another origin is followed as a `GET` without a body, and a redirect within the same origin is followed with any of these statuses, a 307 or 308 with its body. On a followed redirect to another origin, the `Authorization`, `Cookie`, `Cookie2`, `Proxy-Authorization` and `WWW-Authenticate` headers are dropped; other headers, such as a custom `X-Api-Key`, are kept. So credentials in a custom header do travel to another origin: leave redirects off for calls that carry credentials unless you need them.

## Errors

`HttpClientError` is `Timeout`, `Network`, `TooLarge`, `BadStatus(StatusCode)`, `Build`, `Decode` and `InvalidRequest`. The text of each is fixed and never holds a URL, a header, a body or a status code, and the `Debug` output of the request, response and client types never shows them either, so a log line cannot leak a token that sat in a query string.

A request that `reqwest` cannot build, such as one with an unparsable URL or a scheme other than `http` or `https` (`file:///etc/passwd`, `ftp://...`), is `InvalidRequest` and not `Network`: fix the request, do not retry. With redirects on, a server-controlled redirect whose `Location` is malformed or names a scheme other than `http` or `https` also surfaces as `InvalidRequest`, so the variant does not always mean that your request is wrong. Credentials in the URL (`https://user:password@host/`) are turned into an `Authorization: Basic` header; prefer setting the header yourself. A bad header name or value does not fail `Request::header`, which returns the request; it marks the request invalid, and `Request::validate()` (called by every client before sending) returns `InvalidRequest`. `Request::set_header` returns the error at once.

## Interceptors

An `Interceptor` changes a request before it is sent, for example to add an `Authorization` header. `InterceptedClient` runs its interceptors in registration order and then calls the inner client. An error from an interceptor stops the request. There is no response interceptor.

## Retry

`RetryingClient` (feature `retry`) wraps another client and uses the `RetryPolicy` of `oxbloom-tasks`. Only `GET`, `HEAD`, `PUT`, `DELETE` and `OPTIONS` are retried; a `POST` or `PATCH` is sent once. `Network` and `Timeout` errors and the statuses 502, 503 and 504 are retried. When the attempts run out after a retryable status, the last response is returned, not an error. Each attempt gets the inner client's full timeout, so the worst case is about `max_attempts` times the timeout plus the waits.

## Testing with `MockHttpClient`

`MockHttpClient` (feature `http-client` of `oxbloom-test`) is an `HttpClient` with scripted answers. The example wraps a mock in a `RetryingClient` and an `InterceptedClient`, the way an application stacks them, and checks the retry, the header and that a `POST` is not retried. It needs `oxbloom-http-client` with the feature `retry`, `oxbloom-tasks`, `async-trait`, `serde_json`, `http` and `tokio` with the `macros` feature.

```rust
use std::sync::Arc;
use std::time::Duration;

use http::Method;
use oxbloom_http_client::{
    HttpClient, HttpClientError, InterceptedClient, Interceptor, Request, RetryingClient,
};
use oxbloom_tasks::{Jitter, RetryPolicy};
use oxbloom_test::{MockHttpClient, MockResponse};

struct Bearer(String);

#[async_trait::async_trait]
impl Interceptor for Bearer {
    async fn before(&self, request: &mut Request) -> Result<(), HttpClientError> {
        request.set_header("authorization", &format!("Bearer {}", self.0))
    }
}

#[tokio::test]
async fn a_503_is_retried_and_the_bearer_is_sent() -> Result<(), HttpClientError> {
    let url = "https://api.example.test/stock";
    let mock = Arc::new(MockHttpClient::new());
    mock.on(Method::GET, url, MockResponse::status(503))
        .on(Method::GET, url, MockResponse::status(200).json(&serde_json::json!({"stock": 4})));

    let policy = RetryPolicy {
        max_attempts: 3,
        initial: Duration::from_millis(1),
        jitter: Jitter::None,
        ..RetryPolicy::default()
    };
    let retrying = RetryingClient::new(mock.clone(), policy);
    let client = InterceptedClient::new(Arc::new(retrying)).with(Arc::new(Bearer("t0ken".into())));

    let response = client.execute(Request::get(url)).await?;
    let body: serde_json::Value = response.json()?;
    assert_eq!(body["stock"], 4);
    assert_eq!(mock.request_count(), 2);
    assert_eq!(mock.requests()[1].headers()["authorization"], "Bearer t0ken");

    // A POST is never retried.
    let post = "https://api.example.test/orders";
    mock.on(Method::POST, post, MockResponse::status(503));
    let answer = client.execute(Request::post(post)).await?;
    assert_eq!(answer.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(mock.request_count(), 3);
    Ok(())
}
```

The rules of the mock:

- `on(method, url, response)` matches the method and the exact URL, query and fragment included.
- Calling `on` again for the same method and URL adds a later answer. Answers are served in order and the last one repeats.
- `on_fn(|request| ..)` answers from the request, or returns `None` to let a later rule answer. The rules, `on` and `on_fn` together, are tried in the order they were added and the first one with an answer wins.
- A request that no rule answers is `HttpClientError::Network`.
- Every valid request is recorded, answered or not; read them with `requests()` and `request_count()`. An invalid request is an error and is not recorded.
- `unmatched_requests()` returns the requests that no rule answered (they also got `Network`). A test of a failure path should assert that it is empty, so a mistyped URL cannot make the negative test pass for the wrong reason.
- The mock cannot answer with an error. A test that needs `Timeout` or `TooLarge` writes a small stub client.

## Configuration and the bean

With the feature `http-client` of `oxbloom-boot`, `HttpClientAutoConfiguration` registers one `Arc<dyn HttpClient>` bean built from these keys, unless your application registers a `dyn HttpClient` bean of its own. With the feature on, the bean exists even when the section is missing, with the defaults.

```toml
[http.client]
timeout          = "10s"        # total time of a request; must be above zero
connect-timeout  = "2s"         # default: none; must be above zero
max-body-bytes   = 1048576      # response body limit; at least 1
follow-redirects = 0
user-agent       = "billing/2.0"
ssl-bundle       = "corporate"  # needs the feature `ssl`; names a bundle under ssl.bundle.*

[http.client.retry]
max-attempts = 3                # retry settings apply only when this key is set
backoff      = "exponential"    # or "fixed"
initial      = "100ms"
multiplier   = 2.0
max-delay    = "30s"
jitter       = "full"           # or "none", "equal"
```

The `retry` keys are those of `tasks.retry` (see [Retry](18-tasks.md#retry)), with the same defaults, but the two do not share values. The client is wrapped in a `RetryingClient` only when `retry.max-attempts` is above 1; the other `retry` keys are checked and then ignored when `max-attempts` is not set. An unknown key, a bad value, a zero timeout or a zero body limit fails the start with an error that names the full key and never the value. A `user-agent` that is not a valid header value (for example one with a line break) fails the start on `http.client.user-agent`. An unknown `ssl-bundle` name fails the start, and the startup failure report lists the bundles that exist (for example "available bundles: api, idp"). Any other failure to build the client is reported on the section `http.client`.

A bean that needs the client injects `Arc<dyn HttpClient>`. The JWKS fetcher can use the same client with `ReqwestJwksFetcher::with_client(client)`. `TestApp::full` and `ContextRunner` with the feature `http-client` of `oxbloom-test` include the auto-configuration.

## Limits

- **HTTP/1.1 only.** `reqwest` is built without default features, so HTTP/2 is not available. A TLS configuration that offers ALPN `h2` can negotiate it and the call then fails with `Network`.
- **No proxy setter and no per-host clients.** Proxies come from the environment variables. A different policy per host needs a client per policy.
- **The retry settings apply only with `retry.max-attempts`.** Setting `retry.initial` alone does nothing. There is no per-request retry override.
- **Retries multiply the worst-case time**, because every attempt gets the full timeout.
- **Redirects are off by default.** When they are on, a 307 or 308 to another origin and any redirect from `https` to `http` are not followed; on a followed cross-origin redirect the credential headers are dropped, but other headers are kept.
- **Buffered bodies only.** A request body and a response body are held in memory; the response limit bounds the second. There is no streaming.
- **The JWKS fetcher built with `with_client` is bounded by the client you give it.** It checks 1 MiB only after the body is buffered, so the real memory bound is the limit of that client.
- **`MockHttpClient` matches exact URLs and cannot script an error.** Its `on_fn` closure runs while the mock's state lock is held, so the closure must not call the same mock.
- **No metrics and no request logging yet.**
- **Build `tls` configurations on `aws-lc-rs`.** The client does not reject a `ring` provider.

## Testing behind a proxy

The loopback tests of `oxbloom-http-client` and `oxbloom-boot` build real clients, and those honor `HTTP_PROXY`. On a machine that sets it, also set `NO_PROXY=127.0.0.1,localhost`, or the tests send their loopback requests to the proxy and fail.
