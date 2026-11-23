# ADR 0093: Outbound HTTP goes through an HttpClient trait with a reqwest implementation that has safe defaults, retries only idempotent requests and gives errors with fixed text

- **Status:** Accepted
- **Date:** 2026-11-23
- **Source:** the security Slice 9 design spec `2026-11-23-oauth2-design.md`, part A (a local file, not linked) and its plan

## Context

Until now the only outbound HTTP call in Oxbloom was the JWKS fetch of `oxbloom-security`, which used `reqwest` directly. OAuth2 login (Slice 9, groups B and C) adds more calls: the token endpoint, the user-info endpoint, discovery and key sets. Each of them carries secrets in a URL, a header or a body, each needs a timeout, a body limit and a TLS policy, and each must be testable without a network. How does Oxbloom offer one outbound client that is safe by default, that application code and tests can replace, and that cannot leak a secret through an error message?

The JWKS client already takes a rustls `ClientConfig` from an SSL bundle ([ADR 0074](0074-jwks-ssl-bundle-resolved-by-boot.md)), and the new client takes such a configuration the same way. The retry machinery is the `RetryPolicy` of `oxbloom-tasks` ([ADR 0085](0085-retry.md)), which already exists and is reused here.

## Decision

**A trait and one implementation.** `HttpClient` is an object-safe `async` trait with one method, `execute(Request) -> Result<Response, HttpClientError>`, so applications hold an `Arc<dyn HttpClient>`. `ReqwestClient` (built by `ClientBuilder`, feature `reqwest`, on by default) is the implementation. Code that needs a client takes the trait, and a test passes `MockHttpClient` from `oxbloom-test`. A status that is not 2xx is a normal `Response`; `Response::error_for_status` turns it into `HttpClientError::BadStatus` when a caller wants that.

**Safe defaults.** A `ReqwestClient` has a total timeout of 10 seconds, a response body limit of 1 MiB and no redirects. `reqwest` is built without default features, with rustls on `aws-lc-rs`, so the client speaks HTTP/1.1 only; HTTP/2 is not available. The body is read incrementally and the call fails with `TooLarge` as soon as the limit is passed, and a larger `Content-Length` is refused before the body is read. No `Referer` is sent and no cookie jar exists. A configured TLS client configuration (`ClientBuilder::tls`, which is how an SSL bundle reaches the client) replaces the platform verifier.

**A request that cannot be built is `InvalidRequest`, not `Network`.** An unparsable URL is a mistake of the caller and not a transport failure, so it is `HttpClientError::InvalidRequest`. A caller can tell "fix the request" from "try again later".

**Redirects fail closed.** With `follow_redirects(n)`, a chain that needs more than `n` redirects returns the last 3xx response and not an error, the same as the default of `n = 0`, so a caller that treats every non-2xx status as a failure rejects it. Two kinds of redirect are never followed, and the 3xx response is returned as when the budget runs out: a 307 or 308 to another origin (scheme, host or port), because `reqwest` sends the method and the body again on those statuses and a form body can carry a client secret, a refresh token, an authorization code or a PKCE verifier; and any redirect from `https` to `http`, whatever its status. The custom redirect policy decides this with a small pure function, and loopback tests pin both cases. A 307 or 308 upgrade from `http` to `https` is stopped too, because the scheme differs. A cross-origin 301, 302 or 303 is followed. `reqwest` (through `tower-http`) turns only a `POST` into a `GET` without a body on a 301 or 302; a `PUT`, `PATCH` or `DELETE` keeps its method and body, and the pure function cannot see the method. So leave redirects off, the default, for requests other than `POST` that carry a secret in the body. On a followed redirect to another scheme, host or port, `reqwest` drops `Authorization`, `Cookie`, `Cookie2`, `Proxy-Authorization` and `WWW-Authenticate`; a test with two loopback servers pins that. Other headers, such as a custom `X-Api-Key`, are kept.

**Retry is idempotent-only and reuses `oxbloom-tasks`.** `RetryingClient` (feature `retry`, which pulls in `oxbloom-tasks`) wraps another client. Only `GET`, `HEAD`, `PUT`, `DELETE` and `OPTIONS` are retried; a `POST` or `PATCH` is sent once, because repeating it could repeat a side effect. `Network`, `Timeout` and the statuses 502, 503 and 504 are retried. `RetryPolicy::run` retries only on `Err`, so a retryable status is mapped inside `run` to a private error that carries the response; when the attempts run out, the last response is returned and not an error. Each attempt gets the inner client's full timeout, so the worst case is about `max_attempts` times the timeout plus the waits.

**The settings are `http.client.*`, in a feature named `config`.** `HttpClientSettings` (feature `config`, which implies `reqwest` and `retry`) reads `timeout`, `connect-timeout`, `max-body-bytes`, `follow-redirects`, `user-agent`, `ssl-bundle` and `retry.*`. A zero `timeout`, a zero `connect-timeout` and a `max-body-bytes` of zero are rejected, because they would make every call fail or lift every limit by accident. The `retry` settings apply only when `retry.max-attempts` is set, and the client is wrapped in a `RetryingClient` only when that value is above 1; the other `retry` keys are checked and then ignored. An unknown key under `http.client` is an error, and an error names the full key and never the value.

**Proxies come from the environment only.** The `reqwest` default stays: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` are honored, read when the client is built. `ClientBuilder` has no proxy setter. A test in its own test binary pins the environment path.

**Error text is fixed.** Every `HttpClientError` variant has a fixed message. It never holds the URL, a header, the body or a status code (`BadStatus(StatusCode)` carries the status as data but does not print it). `Debug` of `Request`, `Response`, `Body`, `InterceptedClient` and the clients never prints a URL, a header value or a body. A log line that shows one of these errors cannot leak a token that sat in a query string or a header.

**A request validates itself, and every client checks.** `Request::header` returns the request and cannot fail, so a bad header name or value marks the request invalid instead. `Request::validate()` returns the error, and every client calls it before sending: `ReqwestClient`, `RetryingClient`, `InterceptedClient` (before the interceptors and again after them) and `MockHttpClient`. An implementation of `HttpClient` written by an application should call it too. `Request::set_header` returns the error at once, for interceptors that only hold `&mut Request`.

**Interceptors run before the send.** `Interceptor::before(&self, &mut Request)` runs in registration order inside `InterceptedClient`, for example to add an `Authorization` header. An error stops the request. There is no response interceptor yet.

**The JWKS fetcher keeps its API and runs on the client.** `ReqwestJwksFetcher::new`, `with_timeout` and `with_tls` keep their signatures and limits (5 seconds, 1 MiB, no redirects); they now build a `ReqwestClient` and the fetcher holds an `Arc<dyn HttpClient>`. `with_client(Arc<dyn HttpClient>)` is new: the caller's client then decides the timeout, redirects, proxies and TLS, and the 1 MiB check of the fetcher runs after the body is buffered, so the real memory bound is the limit of the supplied client. The security feature `jwks` depends on `oxbloom-http-client` now and no longer on `reqwest` directly.

**`MockHttpClient` has fixed matching rules.** `on(method, url, response)` matches the method and the exact URL, query included. Several answers for one method and URL are served in order and the last one repeats. `on_fn` rules, which answer from the request or return `None`, are evaluated in the order in which all rules were added, and the first rule with an answer wins. A request that no rule answers is `HttpClientError::Network`. Every valid request is recorded. The mock cannot return an error for a rule, so a test of error mapping uses a small stub client.

**The boot wiring.** `HttpClientAutoConfiguration` (feature `http-client` of `oxbloom-boot`, which enables `oxbloom-http-client/config`) registers one `Arc<dyn HttpClient>` bean from the `http.client.*` keys unless the application registers one of its own. It has no condition: with the feature on, the bean exists with the defaults. With the feature `ssl`, `http.client.ssl-bundle` names a bundle; an unknown name lists the available names. A bad value fails the start with the key in the message.

## Consequences

Good:

- One place sets the timeout, the body limit, the redirect policy and the TLS policy for all outbound calls, and the defaults are the safe ones.
- A secret in a URL, a header or a body cannot reach a log through an error or a `Debug` print.
- Code that takes `Arc<dyn HttpClient>` is tested with `MockHttpClient` without a socket.
- A retry never repeats a `POST`, and the retry policy and its configuration are the ones of `oxbloom-tasks`.
- A cross-origin 307 or 308 is not followed, a `POST` answered by a cross-origin 301 or 302 becomes a body-less `GET`, and the standard credential headers are dropped (a `PUT`, `PATCH` or `DELETE` keeps its body on a 301 or 302, so such requests should not follow redirects when the body is secret, and custom credential headers are still forwarded), a downgrade from `https` to `http` is never followed, and a redirect chain that is too long is a failure for any caller that checks the status.
- The JWKS fetcher stays source compatible and gets the same limits and proxy behavior as every other client.

Bad:

- HTTP/2 is not available. A TLS configuration that offers ALPN `h2` can negotiate it and then the call fails with `Network`.
- There is no proxy setter and no per-host client; a proxy needs the environment variables.
- The retry settings are ignored without `retry.max-attempts`, which can surprise someone who sets only `retry.initial`.
- Retried attempts multiply the worst-case time, because each attempt has the full timeout.
- A client built with `with_client` for the JWKS fetcher is only as bounded as the client the caller gives; the fetcher checks the size after the body is in memory.
- The mock matches exact URLs only and cannot script an error, so tests with query-string variations or injected errors need `on_fn` or a stub.
- `RetryingClient` and the mock reach `oxbloom-tasks` and `oxbloom-test` features that applications must enable.
- Custom headers, such as an `X-Api-Key`, still travel on a followed cross-origin 301, 302 or 303.
- A legitimate 307 or 308 to another origin (for example a token endpoint that moved hosts) is not followed; the caller sees the 3xx and must update the URL.
- The loopback tests honor `HTTP_PROXY`, so on a machine that sets it, also set `NO_PROXY=127.0.0.1,localhost` when running them (testing behind a proxy).

## Alternatives considered

- Using `reqwest::Client` directly in every caller: rejected. Each call site would repeat the limits, nothing would stop a caller from printing a URL with a token, and tests would need a server.
- A trait generic over the body type: rejected for this slice. A buffered `Vec<u8>` body keeps the trait object safe and the limit enforceable; streaming bodies are parked.
- `reqwest`'s own `Policy::limited` for the redirect budget: rejected, because it returns an error when the budget runs out and the error text could carry the URL; a custom policy returns the last 3xx response.
- Retrying every method: rejected, because a repeated `POST` can repeat a payment or a token exchange.
- A separate retry implementation: rejected; `RetryPolicy` already has backoff, jitter and settings.
- Enabling HTTP/2: parked. `reqwest` is built without default features, as in `oxbloom-security`, and turning HTTP/2 on is a separate decision about the TLS surface.
- Reading proxy settings from configuration keys: parked; the environment covers the common case.
