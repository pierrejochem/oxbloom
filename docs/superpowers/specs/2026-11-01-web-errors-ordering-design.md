# C6a: Filter ordering, uniform errors and `server.*` properties

Date: 2026-11-01. Builds on C3 (boot), C4b (web and security auto-configuration) and C5b (request logging). This is the first half of roadmap slice C6; C6b (`Json<T>`, content negotiation, static resources, compression) follows.

## Goal

The web layer behaves like a Boot server: its properties come from `server.*`, every error answer has one shape (`application/problem+json`), and the request log, error rendering, body limit and security run in a defined order. The request log then sees security rejections and body-limit answers, which C5b could not log.

## Decisions

- Handlers report errors with `HttpError` and `Result`. Framework answers (404, 405, 413, a panic in a handler) use the same path and are rendered by one replaceable `ErrorRenderer`.
- Filter ordering is one ordered chain with public order constants. The security chain becomes an ordinary `dyn Filter` bean. The 413 body-limit answer is produced inside the pipeline.
- `server.*` gains shutdown timeout, limits, forwarded-header handling with a trusted-proxy list, and one error switch.
- The slice adds no stack-probe growth over the C5b numbers (boot 238 KB default, 239 KB with `logging`, `build()` frame `sub $0x178,%rsp`, DI 206/277/230/194 KB).

## Out of scope

`Json<T>`, content negotiation, static resources, compression, CORS, security rules that use the client address, exception-type handlers (Spring `@ExceptionHandler`), and per-route error renderers.

## Errors (`oxbloom-web`)

- `HttpError` carries `status: StatusCode`, `title: Option<String>`, `detail: Option<String>`, `type_uri: Option<String>` and extension members (a map of string keys to `serde_json`-free values: strings, integers and booleans). It is built with `HttpError::new(status)` and the methods `title`, `detail`, `type_uri` and `with(key, value)`. It needs no `serde` dependency in the public API; the renderer writes the JSON by hand with escaping.
- `IntoResponse for HttpError` returns a response with that status, an empty body and an `ErrorAttributes` value in the response extensions. `ErrorAttributes` holds the same fields plus an `internal: Option<String>` message that only the framework sets (the panic payload).
- `IntoResponse for Result<T, E>` is implemented where `T: IntoResponse` and `E: IntoResponse`. Application error enums map to responses by implementing `IntoResponse`, usually by converting into an `HttpError`.
- Framework answers use `ErrorAttributes` too: 404 (no route), 405 (route exists for other methods, with the `Allow` header kept), 413 (body limit), and 500 for a panic. A panic in the handler or in a filter is caught in `dispatch` with `catch_unwind` over the pipeline future. It is logged at level `error` on target `oxbloom::web` with the panic message, and answered as 500 with `internal` set to the message.
- `ErrorHandlingFilter` (built in, at `order::ERROR_HANDLING`) renders every response that carries `ErrorAttributes`, and every bodiless response with a 4xx or 5xx status (attributes derived from the status). It calls `ErrorRenderer::render`. A response that already has a body is left alone, so a handler can still return its own error body.
- `ErrorRenderer` is a trait: `fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response`, where `RequestInfo` has the method and path. `ProblemRenderer` is the default. It writes RFC 9457 `application/problem+json` with `type` (default `about:blank`), `title` (the author's, else the status reason phrase), `status`, `detail` (when set), `instance` (the request path, never the query) and the extension members. Headers already on the response, such as `WWW-Authenticate` and `Allow`, are kept.
- `App::error_renderer(impl ErrorRenderer + 'static)` sets the renderer. Boot registers `ProblemRenderer` as `dyn ErrorRenderer` with `on_missing_bean`, so an application bean replaces it.
- `detail` set by an author on an `HttpError` is always shown, because the author chose it for the client. `internal` is shown as `detail` only when `server.error.include-message = always`; the default `never` leaves `detail` out for framework answers that have no author detail.

## Ordering

- Public constants in `oxbloom_web::order` (all `i32`): `REQUEST_LOG = -400`, `ERROR_HANDLING = -300`, `BODY_LIMIT = -200`, `SECURITY = -100`, `DEFAULT = 0`. A lower number runs first (outer). Filters with equal order keep registration order.
- `Filter` gains a provided method `fn order(&self) -> i32` that returns `order::DEFAULT`. `App::filter(f)` inserts the filter at `f.order()`, keeping registration order for equal values, and `App::filter_ordered(order, f)` wraps `f` in an `Ordered` adapter that overrides the order. A filter therefore carries its own slot, whether it comes from code or from a bean.
- `ErrorHandlingFilter` and `BodyLimitFilter` are built into every `App` at their slots. The router's own 404 and 405 answers are produced at the end of the chain, so every filter wraps them.
- `dispatch` no longer answers 413. When the body exceeds the limit it builds the request with an empty body and a `BodyTooLarge` marker in the request extensions. `BodyLimitFilter` answers 413 when it sees the marker, before security and the application filters run.
- `SecurityFilterChain` overrides `order()` to return `order::SECURITY`, and `RequestLogFilter` overrides it to return `order::REQUEST_LOG`. `WebServerAutoConfiguration` no longer places the chain first by itself: it registers the chain and every `dyn Filter` bean with `App::filter`, and each filter's own `order()` decides its place. Bean `order` still decides the injection order of beans with equal `order()`.
- Effect on existing behavior: 401 and 403 answers from the security chain are rendered as problem+json (headers unchanged), and the request log now records 401, 403, 404, 405, 413 and 500 answers. This changes those bodies from empty to JSON. Existing tests that assert empty bodies are updated.

## `server.*` properties (`oxbloom-boot`)

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `server.address`, `server.port` | existing | unchanged | |
| `server.shutdown-timeout` | duration | `30s` | Graceful shutdown waits this long for open connections, then drops them and `oxbloom-web` logs a warning at level `warn` on target `oxbloom::web`. |
| `server.max-body-bytes` | size | `1MiB` | Request body limit; larger bodies get 413. |
| `server.header-read-timeout` | duration | `10s` | Time allowed to read the request headers. |
| `server.forward-headers.strategy` | `none` \| `framework` | `none` | Whether forwarded headers are honored. |
| `server.forward-headers.trusted-proxies` | list of IP or CIDR | loopback only when the strategy is `framework` | Peers whose forwarded headers are trusted. |
| `server.error.include-message` | `never` \| `always` | `never` | See Errors. |

- Durations and sizes use the existing config unit parsers. Invalid values fail startup with `ConfigError::invalid` naming the key and never the value. A `trusted-proxies` entry that is not an IP or CIDR names `server.forward-headers.trusted-proxies`.
- `ServerProperties` in `web.rs` reads the new keys; `WebServer` passes them to the `App`. `App::shutdown_timeout`, `max_body_bytes` and `header_read_timeout` are the code-level equivalents.
- All keys are listed in the `PropertyMetadata` of `WebServerAutoConfiguration`.

## Forwarded headers

- `serve` keeps the peer `SocketAddr` of each connection and adds a `ClientInfo { addr: SocketAddr, scheme: Scheme, host: Option<String> }` to the request extensions. `Request::client_addr()`, `scheme()` and `host()` read it. `Scheme` is `Http` or `Https`; the default is `Http` and the peer address.
- With strategy `framework`, forwarded values apply only when the peer address is in the trusted list. The `Forwarded` header (RFC 7239) is read first; if it is absent, `X-Forwarded-For`, `X-Forwarded-Proto` and `X-Forwarded-Host` are read.
- `X-Forwarded-For` and the `for=` parameters are walked from right to left. The walk stops at the first hop that is not in the trusted list, and that hop is the client address. If every hop is trusted, the leftmost is the client. A value that is not a valid IP address (with optional port, and the bracketed IPv6 form) makes the whole header unusable and the peer address stays the client address. `proto` accepts `http` and `https` only; `host` is taken as is when it is a valid authority.
- The Debug output of `Request` and the request log do not print forwarded values beyond what the request log already prints; no new credential surface is added.
- The CIDR parser is small, handles IPv4 and IPv6, and adds no dependency.

## Testing

- Errors: `HttpError` builder; `Result` mapping; problem+json shape for each framework answer (404, 405 with `Allow`, 413, 500 for a panic in a handler and in a filter); an author `detail` is shown; `include-message` `never` hides and `always` shows the panic message; a custom `ErrorRenderer` replaces the default; a handler's own error body is not rewritten; the `instance` field never contains the query.
- Ordering: the order constants and registration-order ties; the request log records 401 (security), 404, 405 and 413 answers; security runs after the body limit (an oversized body gets 413 even without credentials); an application filter at `DEFAULT` runs after security; `filter_ordered` places a filter before the security chain.
- `server.*`: defaults, each key, invalid values name the key and never echo the value, shutdown timeout drops a connection that stays open and logs a warning, body and header limits come from config.
- Forwarded headers: strategy `none` ignores the headers; an untrusted peer is ignored; a trusted peer with `X-Forwarded-For` chains (one hop, several hops, a spoofed leftmost entry behind a trusted proxy, IPv6, ports); malformed values fall back to the peer; `Forwarded` takes precedence over `X-Forwarded-*`; `proto` and `host`; CIDR parsing and matching including `/0`, `/32`, `/128` and invalid prefixes.
- Gate: `cargo test --workspace --all-features`, clippy `-D warnings`, `cargo fmt --check`, the feature combinations of `oxbloom-boot`, `make check-examples`, and the stack probes (zero growth).

## Documentation

README (new section "Errors and filter order" and the `server.*` keys), `docs/API.md` for each new public item, the crate docs of `oxbloom-web`, the roadmap (C6a done, C6b open, follow-ups), and the `examples/todos` error paths (use `HttpError` for 404 and 422).
