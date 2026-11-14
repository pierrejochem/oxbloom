# Errors and filter order

`oxbloom-web` has one error model and one ordered filter chain. Both are on by default; no feature or key switches them on.

## Filter order

Every `Filter` has a slot, a plain `i32`. A lower number runs first (it is the outer layer), and filters with the same number keep their registration order. The slots are constants in `oxbloom_web::order`:

| Constant | Value | Filter |
|---|---|---|
| `REQUEST_LOG` | -400 | `RequestLogFilter` |
| `CORS` | -350 | `CorsFilter`; outside error handling and security |
| `ERROR_HANDLING` | -300 | built in; renders every error answer |
| `NEGOTIATION` | -250 | built in with the feature `json`; answers 406 |
| `BODY_LIMIT` | -200 | built in; answers 413 |
| `COMPRESSION` | -150 | `CompressionFilter`; outside security |
| `SECURITY` | -100 | `SecurityFilterChain` |
| `DEFAULT` | 0 | every filter that declares no slot |

A filter declares its slot by overriding the provided method `Filter::order()`, which returns `order::DEFAULT` unless you override it. `App::filter(f)` places `f` at `f.order()`. To choose a slot from code without changing the filter, use `App::filter_ordered(order, f)`, which wraps the filter in `Ordered`. Boot registers the security chain and every `dyn Filter` bean with plain `App::filter`, so each one lands at its own slot; the order of beans in the container only breaks ties.

```rust
use oxbloom_web::{App, Filter, Next, Request, Response, order};

struct Audit;

#[async_trait::async_trait]
impl Filter for Audit {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        next.run(req).await
    }
    // After security: sees only the requests the chain let through, authenticated.
    fn order(&self) -> i32 {
        order::SECURITY + 50
    }
}

let app = App::new().filter(Audit);
// Same effect for a filter that does not know its slot:
// App::new().filter_ordered(order::SECURITY + 50, Audit)
```

Because the request log is outermost, it records what the security chain and the body limit answer. The request log event now exists for 401, 403, 404, 405, 413 and 500. An oversized request body gets 413 before security runs, so a client without credentials sees 413, not 401. A plain filter at `DEFAULT` runs after the security chain and never sees a request that the chain rejects.

Two zones need care. A filter ordered below `order::SECURITY` runs before authentication: it sees unauthenticated requests (no `Authentication` in the extensions), and a response it returns itself is sent without any authorization check. A filter ordered below `order::BODY_LIMIT` sees an empty body for a request whose body was over `server.max-body-bytes`, and it can tell why only by calling `Request::body_too_large()`, which is `true` exactly when the server cut the body.

## Errors

A handler reports an error by returning an `HttpError`, alone or inside a `Result`:

```rust
use http::StatusCode;
use oxbloom_web::{HttpError, Request};

async fn get_todo(req: Request) -> Result<String, HttpError> {
    let id = req.param("id").unwrap_or_default();
    Err(HttpError::new(StatusCode::NOT_FOUND)
        .detail(format!("no todo {id}"))
        .with("todo", id))
}
```

`IntoResponse` is implemented for `Result<T, E>` when both sides implement it. An application error enum maps to a response by implementing `IntoResponse` itself, usually by turning its variants into an `HttpError`. The builder methods are `title`, `detail`, `type_uri` and `with(key, value)`; `with` adds an extension member whose value is a string, an integer or a boolean.

The answer is `application/problem+json` (RFC 9457):

```json
{"type":"about:blank","title":"Not Found","status":404,"detail":"no todo 7","instance":"/api/todos/7","todo":"7"}
```

- Members, in order: `type` (default `about:blank`), `title` (the author's, else the status reason phrase), `status`, `detail` (only when set), `instance` (the request path, never the query) and then the extension members. An extension whose key equals one of the five built-in members is dropped.
- The framework answers 404, 405 (with `Allow`), 413 and a panic in a handler or filter (500) the same way, and so does the security chain for 401, 403 and 503. Headers already on the response, such as `WWW-Authenticate` and `Allow`, are kept.
- A `detail` set by the author is always shown, because the author wrote it for the client. `server.error.include-message` controls only the internal messages that the framework supplies, which today means the panic message: with `never` (the default) it stays out of the response, with `always` it becomes the `detail` of the 500 answer. The panic message is always in the `error` log event `request panicked: ...` (target `oxbloom::web`), and Rust's default panic hook also prints it to standard error.
- A response that already has a body is never rewritten, not even when it carries error attributes. A handler can still return its own error body.
- A `HEAD` request that ends in an error gets the status and the headers, with no body.

To change the shape, implement `ErrorRenderer` (`fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response`) and register it as a `dyn ErrorRenderer` bean (the default `ProblemRenderer` has `on_missing_bean`, so yours wins), or call `App::error_renderer` without boot. An `Arc` of a renderer, also an `Arc<dyn ErrorRenderer>`, is a renderer too, so a shared one can be passed to `App::error_renderer` as it is. `RequestInfo` holds the method and the path.

```rust
use std::sync::Arc;
use oxbloom_web::{ErrorAttributes, ErrorRenderer, IntoResponse, RequestInfo, Response};

struct PlainRenderer;

impl ErrorRenderer for PlainRenderer {
    fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response {
        let mut res = format!("{} at {}", attrs.status.as_u16(), info.path).into_response();
        *res.status_mut() = attrs.status;
        res
    }
}

// With boot: .instance(PlainRenderer)
//            .provides(|r: Arc<PlainRenderer>| -> Arc<dyn ErrorRenderer> { r })
```

## Server properties

```toml
# application.toml
[server]
address = "127.0.0.1"
port = 8080
shutdown-timeout = "30s"
max-body-bytes = "1MiB"
header-read-timeout = "10s"

[server.error]
include-message = "never"

[server.forward-headers]
strategy = "none"
trusted-proxies = ["127.0.0.1", "::1"]
```

| Key | Default | Meaning |
|---|---|---|
| `server.address`, `server.port` | `127.0.0.1`, `8080` | Where the server listens (`0` picks a free port). |
| `server.shutdown-timeout` | `30s` | How long graceful shutdown waits for open connections. After that they are dropped and a `warn` event is logged on target `oxbloom::web`. `0` drops open connections at once. Dropping the future of `serve` aborts in-flight connections. |
| `server.max-body-bytes` | `1MiB` | Largest request body; a larger one gets 413. `0` rejects every non-empty body. |
| `server.header-read-timeout` | `10s` | Time a client has to send the request headers. Must be above 0 and at most one day; `App::header_read_timeout` clamps to one day. |
| `server.forward-headers.strategy` | `none` | `none` or `framework`; see the next section. |
| `server.forward-headers.trusted-proxies` | loopback only | A list of IP addresses and CIDR networks (IPv4 and IPv6). Used when the strategy is `framework`. |
| `server.error.include-message` | `never` | `never` or `always`; see Errors. |
| `server.cors.allowed-origins` | unset (CORS off) | A list: `*` or entries such as `https://app.example`. Setting it switches CORS on; see [CORS](11-cors.md#cors). An explicitly empty list fails startup. |
| `server.cors.allowed-methods` | `GET`, `HEAD`, `POST` | Methods a preflight allows, upper case; `*` allows every method. |
| `server.cors.allowed-headers` | none | Request headers a preflight allows; `*` allows every header. |
| `server.cors.exposed-headers` | none | Response headers that scripts may read. |
| `server.cors.allow-credentials` | `false` | Cannot be combined with `*` in the three lists above. |
| `server.cors.max-age` | `30m` | How long a browser may cache a preflight answer. |
| `server.compression.enabled` | `false` | Switches compression on; fails startup without a codec feature. See [Compression](12-compression.md#compression). |
| `server.compression.min-size` | `1KiB` | Smaller bodies are not compressed. |
| `server.compression.mime-types` | `text/*`, `application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml`, `application/xml` | Types that are compressed; `family/*` matches a family. |
| `server.compression.level` | `default` | `fast`, `default` or `best`. |
| `server.compression.max-body-bytes` | `8MiB` | Larger bodies are not compressed. |
| `server.static.enabled` | `false` | Serves `server.static.directory` at `server.static.path`; see [Static resources](13-static-resources.md#static-resources). |
| `server.static.directory` | `static` | Must exist when static resources are enabled. |
| `server.static.path` | `/` | The URL prefix. |
| `server.static.index` | `index.html` | The file served for a directory: 1 to 255 bytes, not starting with `.`, without `/`, `\`, `%` or NUL; otherwise startup fails. |
| `server.static.cache-max-age` | `0s` | `max-age` of `Cache-Control`; `0s` sends `no-cache`. |
| `server.static.max-file-bytes` | `10MiB` | Larger files answer 404. |

Durations and sizes use the usual units (`500ms`, `30s`, `1MiB`, or plain bytes). Invalid values fail startup naming the key and never the value, in the `Container` phase; the failure report is the generic container report, because the built-in analyzer explains missing keys only. The same keys exist in code as `App::shutdown_timeout`, `App::max_body_bytes`, `App::header_read_timeout`, `App::forwarded`, `App::cors` and `App::compression` (the static keys build a `StaticResources` controller that you register with `App::routes`). For the CORS, compression and static keys, the `server.*` section is read once by `WebServer::new`; a key that is absent leaves what the code set, and `enabled = true` or a present `server.cors.allowed-origins` adds the filter or the routes. A `server.static.path` that conflicts with a route of the application fails startup naming that key.

The sub-keys (`server.compression.min-size`, `mime-types`, `level` and `max-body-bytes`; the `server.cors.*` keys other than `allowed-origins`; the `server.static.*` keys other than `enabled`) are always validated, but they have no effect without their switch key (`server.compression.enabled = true`, `server.cors.allowed-origins`, `server.static.enabled = true`). They never tune a filter or route set that the code registered with `App::compression`, `App::cors` or `App::routes`.

`WebServer::new(app, &config)` applies only the keys that are present. A setting made in code wins when its key is absent, so `WebServer::new(App::new().max_body_bytes(16), &config)` keeps the 16 bytes unless the configuration has `server.max-body-bytes`; the defaults in the table are the `App`'s own. `strategy` and `trusted-proxies` each replace only their own part of `App::forwarded`, so `trusted-proxies` alone keeps the strategy set in code. `server.error.include-message` only configures the `ProblemRenderer` that the auto-configuration registers: `WebServer::new` validates it but sets no renderer, so without the auto-configuration choose one with `App::error_renderer`.

## Forwarded headers

Behind a reverse proxy the TCP peer is the proxy, not the user. With `server.forward-headers.strategy = "framework"` (or `App::forwarded(ForwardedConfig { strategy: ForwardStrategy::Framework, .. })`) the server resolves the real client and stores a `ClientInfo { addr, scheme, host }` in the request extensions. `Request::client_addr()`, `scheme()` (`Scheme::Http` or `Scheme::Https`) and `host()` read it. The default strategy is `none`: the headers are ignored, the client is the peer, the scheme is `Http` and `host()` is `None`.

**Deployment requirement: the trusted proxy must set or strip `Forwarded`, must overwrite or strip `X-Forwarded-Proto` and `X-Forwarded-Host`, and must append to `X-Forwarded-For`. Otherwise clients control their own address, scheme and host, and the framework cannot detect this.** Enable the strategy only behind a proxy you have checked for this, and list only that proxy in `trusted-proxies`.

The algorithm, when the strategy is `framework`:

1. The headers count only when the TCP peer is in the trusted list. Otherwise they are ignored.
2. The `Forwarded` header (RFC 7239) is read first. Only when it is absent are `X-Forwarded-For`, `X-Forwarded-Proto` and `X-Forwarded-Host` read. A `Forwarded` header that is present but unusable does not fall back to the `X-Forwarded-*` headers.
3. The hops (`X-Forwarded-For` or the `for=` parameters) are walked from right to left. The first hop that is not trusted is the client. If every hop is trusted, the leftmost one is the client. The leftmost entry is client-controlled, so it is never used just because it is first. With `Forwarded`, `proto` and `host` come from the client's element. A list in `X-Forwarded-Proto` or `X-Forwarded-Host` is read at the client's hop when it has as many entries as `X-Forwarded-For`; otherwise (also without `X-Forwarded-For`) its rightmost entry, written by the proxy nearest the server, is used.
4. Any element that is not a valid IP address (with optional port, and the bracketed IPv6 form) makes the whole header unusable. Then any `proto` and `host` are dropped too: the peer stays the client, the scheme is the connection's own and the host is `None`.
5. `proto` gives the scheme `Https` when it is `https` (in any case) and `Http` for any other value. Without a `proto` (also without any forwarded header) the scheme is the connection's own: `Http`, or `Https` when the server terminated TLS. `host` must be a valid authority (a host name or IP address with an optional port), otherwise the host is `None`.

The default `server.forward-headers.trusted-proxies` list is loopback only (`127.0.0.0/8` and `::1`), so a proxy on another host must be listed. An empty list trusts nobody. A forwarded address without a port has port `0`. `Request::client_addr()` is `None` for a request built by hand and run with `App::handle`, because there is no socket.

Nothing in `oxbloom-security` uses the client address yet; rules by address are a follow-up.

## Limits

- A panic in a handler or in a filter ordered after `ERROR_HANDLING` is answered 500 by the built-in error filter, and the request log records that 500. A panic in a filter ordered before `ERROR_HANDLING` is caught by an outer catch and also answered 500 (with the `request panicked` event), but it unwinds through every filter outside it. So the request log event of that request is lost when the request log itself panics or when any filter ordered between `REQUEST_LOG` and `ERROR_HANDLING` (for example at -350) panics. A filter ordered before `REQUEST_LOG` loses the event only when it panics before it calls `next.run`; a panic after that comes when the event is already written.
- The panic message in the `request panicked` event has its control characters escaped (`\n`, `\r`, `\u{1b}` and so on), so it stays on one line; `ErrorAttributes.internal` keeps the raw message.
- `HttpError` extension values are strings, integers and booleans. Two `with` calls with the same key can produce two members with that name.
- A status without a reason phrase (for example 499 or 599) renders an empty `title`.
- A rendered response drops the extensions of the original response.
- A renderer that panics escapes the outer catch.
- `ErrorAttributes` on a 2xx or 3xx response is rendered as problem+json.
- There are no per-route error renderers and no handlers keyed by error type.
