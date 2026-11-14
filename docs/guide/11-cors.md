# CORS

`CorsFilter` answers browser cross-origin requests. It sits at `order::CORS` (-350), outside error handling and security, so error answers (401, 403, 404, 413, 500) carry the CORS headers and an allowed preflight never reaches the security chain. It needs no Cargo feature. Register it with `App::cors(CorsConfig)`, which returns a `CorsError` for an unusable setting, or switch it on with `server.cors.allowed-origins`.

```rust
use std::time::Duration;
use oxbloom_web::{App, CorsConfig};

let app = App::new().cors(
    CorsConfig::new()
        .allowed_origins(["https://app.example"])
        .allowed_methods(["GET", "POST", "DELETE"])
        .allowed_headers(["Content-Type", "Authorization"])
        .exposed_headers(["X-Total"])
        .allow_credentials(true)
        .max_age(Duration::from_secs(600)),
)?;
# Ok::<(), oxbloom_web::CorsError>(())
```

Defaults: methods `GET`, `HEAD` and `POST`, no allowed and no exposed headers, credentials off, max age 30 minutes. CORS is off until an origin list is given.

- A request without an `Origin` header, or from an origin that is not allowed, passes and gets no CORS headers. The filter does not block it; the browser enforces the policy. When the allowed origins are a list (not `*`), every answer that is not a preflight answer, including these, gets `Vary: Origin`, so a shared cache never serves a copy without CORS headers to an allowed origin.
- A preflight is an `OPTIONS` request with `Origin` and `Access-Control-Request-Method`. From an allowed origin that asks for an allowed method and allowed headers, it is answered 204 by the filter with `Access-Control-Allow-Origin`, `-Allow-Methods`, `-Allow-Headers` (when there are any), `-Max-Age`, `-Allow-Credentials` (when on) and a `Vary` header. Any other preflight passes through to the rest of the chain with no CORS headers, and an `OPTIONS` request without `Access-Control-Request-Method` is not a preflight.
- Every other request from an allowed origin gets `Access-Control-Allow-Origin`, `Vary: Origin` and, when configured, `Access-Control-Expose-Headers` and `Access-Control-Allow-Credentials`. An existing `Vary` that lists `Origin` or `*` is not repeated.
- `Access-Control-Allow-Origin` echoes the request origin, or is `*` when the allowed origins are `*`.

Rules for the settings:

- The origin list must not be empty. An entry is `scheme://host[:port]` with no path and no trailing slash, or `null`, or the single entry `*`. The scheme and host are compared without regard to case. Default ports are not stripped: list `https://a.example`, not `https://a.example:443`, because a browser does not send the default port.
- `*` must be the only entry of its list. This holds for origins, methods and headers. `*` in any of the three cannot be combined with `allow-credentials`; the error names `allow-credentials`.
- Methods are compared exactly and case-sensitively, so write them in upper case. Header names are compared without regard to case. `*` for headers allows and echoes every requested header; `*` for methods allows every method.
- A request with two `Origin` headers counts as not allowed.
- Listing `null` together with `allow-credentials` is a foot-gun: sandboxed frames and local files send `Origin: null`, so any such page could then make credentialed calls. The filter accepts it; do not do it.
- There are no per-path rules; one filter applies to the whole application.

From the properties: `server.cors.allowed-origins` (a list) switches CORS on, and `allowed-methods`, `allowed-headers`, `exposed-headers`, `allow-credentials` and `max-age` refine it; the keys are in [Web server properties](09-errors-and-filter-order.md#server-properties). A bad list names the matching key. A present `server.cors.allowed-origins` adds a CORS filter next to one set in code with `App::cors`, so both run; use one or the other.
