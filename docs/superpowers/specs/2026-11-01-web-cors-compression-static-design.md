# C6b-2: CORS, compression and static resources

Date: 2026-11-01. Builds on C6a (filter order, errors, `server.*`) and C6b-1 (JSON and negotiation). This finishes roadmap slice C6b. Security slice 8 also lists CORS; this slice takes it over.

## Goal

An Oxbloom application can answer browser cross-origin requests, compress responses, and serve static files from a directory or from files embedded in the binary. All three are driven by `server.*` properties in the style of C6a, and each is also available from code.

## Decisions

- Response bodies are `Full<Bytes>`: the web layer has no streaming. Compression and static files therefore work on whole in-memory bodies. Static files have a size cap. Range requests and streaming bodies are out of scope and become a later slice.
- CORS and compression are filters at their own `order` slots. Static resources are a set of routes (a `Controller`), so they sit behind the security chain like any route.
- Compression supports gzip, brotli and zstd, each behind its own Cargo feature, and is off by default at runtime.
- CORS is off until `server.cors.allowed-origins` is set.
- Static resources come from a directory on disk or from a list of embedded files.
- The slice adds no stack-probe growth over the C6b-1 numbers (boot 238 KB default, 239 KB with `logging`, `build()` frame `sub $0x178,%rsp`, DI 206/277/230/194 KB) and the boot builder and run plan do not grow: the new settings are read where `WebServer` is built.

## Out of scope

Streaming bodies, range requests, SPA fallback routing, per-path CORS rule sets, security rules that use CORS, pre-compressed sibling files (`.gz`, `.br`), `Content-Encoding` on request bodies, server-side templates, `If-Match` and `If-Range`, and directory listings.

## Order slots

The `order` module gains `CORS = -350` and `COMPRESSION = -150`. The full table becomes `REQUEST_LOG -400`, `CORS -350`, `ERROR_HANDLING -300`, `NEGOTIATION -250`, `BODY_LIMIT -200`, `COMPRESSION -150`, `SECURITY -100`, `DEFAULT 0`. CORS sits outside the error filter, so error answers carry CORS headers and a preflight never reaches security. Compression sits inside the body limit and outside security, so it compresses every answer produced by the handler and by security. It does not see problem+json: the error filter and the negotiation filter are further out and render their answers after the response has left the compression filter. Those bodies are small, so nothing is lost.

## CORS (`oxbloom-web`)

- `CorsConfig` is a builder: `allowed_origins`, `allowed_methods`, `allowed_headers`, `exposed_headers`, `allow_credentials`, `max_age`. `CorsFilter::new(CorsConfig) -> Result<CorsFilter, CorsError>` validates it. `App::cors(config)` registers the filter at `order::CORS`.
- A request without an `Origin` header passes without CORS headers. A request with an origin that is not allowed passes and gets no CORS headers; it is not turned into a 403.
- A preflight is an `OPTIONS` request with `Origin` and `Access-Control-Request-Method`. For an allowed origin and an allowed method the filter answers 204 with `Access-Control-Allow-Origin`, `Access-Control-Allow-Methods`, `Access-Control-Allow-Headers` (the allowed list, or the request headers when the list is `*`), `Access-Control-Max-Age` and, when credentials are on, `Access-Control-Allow-Credentials: true`. It also sets `Vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers`. A preflight that is not allowed passes to the rest of the chain without CORS headers.
- When the policy lists origins (not `*`), every response that is not a preflight answer, including a request without `Origin` or from a disallowed origin, gets `Vary: Origin`, so a shared cache never serves a copy without CORS headers to an allowed origin. A simple or actual request from an allowed origin gets `Access-Control-Allow-Origin`, `Vary: Origin`, `Access-Control-Expose-Headers` (when set) and `Access-Control-Allow-Credentials` (when on) added to the response.
- `Access-Control-Allow-Origin` echoes the request origin. It is `*` only when the list is exactly `*` and credentials are off.
- Validation: `*` mixed with other origins is invalid; `*` origins with `allow-credentials = true` is invalid, and so is `*` in `allowed-headers` or `allowed-methods` with credentials on. An origin must be `scheme://host[:port]` (no path, no trailing slash, case-insensitive host). Defaults: methods `GET, HEAD, POST`, headers empty, exposed empty, credentials off, max age 30 minutes.

## Compression (`oxbloom-web`)

- Features `gzip` (flate2), `brotli` (brotli crate) and `zstd` (zstd crate) enable the codecs. A fourth feature `compression` is in `default` and enables all three. `CompressionConfig` has `min_size` (default 1024), `mime_types` (default `text/*`, `application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml`, `application/xml`), and `level` (`fast`, `default`, `best`, mapped per codec). `CompressionFilter::new(CompressionConfig)` and `App::compression(config)` register it at `order::COMPRESSION`.
- The codec is chosen from `Accept-Encoding` with q-values. Ties go to zstd, then brotli, then gzip. `identity;q=0` with no usable codec answers 406 as an `HttpError`. A codec whose feature is off is never chosen.
- A response is compressed only when all of these hold: the codec exists; the body length is at least `min_size`; the `Content-Type` matches `mime_types` (parameters ignored); the response has no `Content-Encoding`; the status is not 1xx, 204 or 304; and `Cache-Control` has no `no-transform`. Otherwise it passes unchanged.
- A compressed response gets `Content-Encoding`, drops `Content-Length` (the server sets it), adds `Vary: Accept-Encoding` (kept when already present), and turns a strong `ETag` into a weak one. A `HEAD` response gets the same headers as the matching `GET` but keeps an empty body.
- The compression filter has no streaming mode. A body larger than `max-compress-bytes` (default 8 MiB) passes unchanged, to bound memory and CPU.
- The README warns about BREACH: do not compress responses that mix secrets with attacker-controlled input over TLS.

## Static resources (`oxbloom-web`)

- `ResourceSource` is a trait: `async fn read(&self, path: &str) -> Result<Resource, ResourceError>`. `Resource` has `bytes`, `content_type`, `etag` and `last_modified`. `DiskResources::new(root)` reads with `tokio::fs`; `EmbeddedResources::new(&'static [(&'static str, &'static [u8])])` serves a list the application provides (no macro, no new dependency).
- `StaticResources::new(prefix, source)` implements `Controller`: it registers `GET` and `HEAD` under `prefix` (default `/`). The app mounts it like any controller bean.
- Path handling: the request path after the prefix must already be in canonical form, because the security chain matches the raw path and the static handler must not see a different path than the chain did. A request path with an empty segment (`//`) or a `.` segment, other than one single trailing `/`, gives 404. A raw byte in the path must be visible ASCII and must not be one of the bytes that may only be written as an escape (`"`, `{`, `}`, space, `#`, `<`, `>`, `?` and the backtick); non-ASCII characters and those bytes must be written as escapes, because that is how browsers send them. An escape is allowed only when it decodes to a non-ASCII byte or to one of space, `"`, `#`, `<`, `>`, `?`, a backtick, `{`, `}` or `%`; an escape of any other byte (including `[`, `]`, `^`, `|`, the unreserved characters and the sub-delimiters) gives 404, and so does an escape written with lower-case hex digits. Escapes use upper-case hex digits, so every file has exactly one raw spelling. An index file is the one exception: it is served at its own path and also at the path of its directory, so a security rule must protect a directory with `/dir/**`. The path is then split on `/` and every segment is percent-decoded once. A `..` segment, a NUL byte, a backslash, or a decoded `/` inside a segment gives 404. For disk, the final path is canonicalized and must stay under the canonical root, so a symlink that leaves the root gives 404. Names that start with `.` give 404. A directory path serves `index` (default `index.html`) when it exists, otherwise 404.
- Content type comes from a built-in table for common extensions (html, css, js, mjs, json, xml, txt, svg, png, jpg, jpeg, gif, webp, avif, ico, woff, woff2, ttf, wasm, map, pdf, csv) and falls back to `application/octet-stream`. Text types carry `charset=utf-8`.
- Validators: embedded resources get a strong `ETag` from a hash of the bytes; disk resources get a strong `ETag` from size and modification time. `Last-Modified` is sent for disk resources. `Cache-Control: public, max-age=N` is sent with `N` from `cache-max-age` (default 0, which sends `no-cache`).
- Conditional requests follow RFC 9110: `If-None-Match` (weak comparison, list, `*`) gives 304 for `GET` and `HEAD`; `If-Modified-Since` is checked only when there is no `If-None-Match`. A 304 carries `ETag`, `Last-Modified` and `Cache-Control`, and no body.
- A file larger than `max-file-bytes` (default 10 MiB) answers 404 and logs a warning at level `warn` on target `oxbloom::web` with the path. Other I/O errors answer 500 as an `HttpError` and log the error.
- Methods other than `GET` and `HEAD` get 405 from the router, whose `Allow` header lists `GET, HEAD` and `OPTIONS`.

## `server.*` properties (`oxbloom-boot`)

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `server.cors.allowed-origins` | list of strings | unset (CORS off) | Allowed origins, or `*`. |
| `server.cors.allowed-methods` | list of strings | `GET,HEAD,POST` | |
| `server.cors.allowed-headers` | list of strings | empty | Or `*`. |
| `server.cors.exposed-headers` | list of strings | empty | |
| `server.cors.allow-credentials` | bool | `false` | |
| `server.cors.max-age` | duration | `30m` | |
| `server.compression.enabled` | bool | `false` | Registers the compression filter. |
| `server.compression.min-size` | size | `1KiB` | |
| `server.compression.mime-types` | list of strings | see Compression | |
| `server.compression.level` | `fast` \| `default` \| `best` | `default` | |
| `server.compression.max-body-bytes` | size | `8MiB` | Larger bodies are not compressed. |
| `server.static.enabled` | bool | `false` | Mounts a `DiskResources` controller. |
| `server.static.directory` | string | `static` | Root directory. |
| `server.static.path` | string | `/` | URL prefix. |
| `server.static.index` | string | `index.html` | |
| `server.static.cache-max-age` | duration | `0s` | |
| `server.static.max-file-bytes` | size | `10MiB` | |

- Invalid values fail startup with `ConfigError::invalid` that names the key and never echoes the value. `server.static.directory` must exist when `enabled` is true, or startup fails naming the key. `server.compression.enabled = true` in a build without any codec feature fails startup naming the key.
- `WebServerAutoConfiguration` reads these keys where it builds the `WebServer`, registers `CorsFilter` and `CompressionFilter` when configured, and mounts `StaticResources` when enabled. A `dyn Filter` or `Controller` bean of the application still works next to them. All keys are listed in the `PropertyMetadata` of `WebServerAutoConfiguration`.
- Embedded resources have no property: the application registers a `StaticResources` bean in code.

## Testing

- CORS: no `Origin`; allowed and disallowed origin; the preflight matrix (allowed, wrong method, wrong header, missing request method); `*` with and without credentials; `Vary`; headers on error answers (404, 413, 401 from a security-slot filter); the preflight is never seen by security; validation errors; an origin with a path or trailing slash is rejected.
- Compression: codec choice from `Accept-Encoding` (q-values, ties, `identity;q=0`, unknown codecs, `*`); each codec round-trips through a decoder in the test; each skip rule; `Vary`, `ETag` weakening and header changes; `HEAD`; the size cap; problem+json rendered by the error filter is not compressed; a handler body above `min-size` behind security is; ordering relative to security and negotiation; builds with each codec feature alone and with none.
- Static: content types; `index`; the traversal matrix (`..`, `%2e%2e`, `%2f`, backslash, NUL, double encoding, symlink out of the root, dotfiles); `ETag` and `Last-Modified`; `If-None-Match` forms and `If-Modified-Since`; `HEAD`; the size cap; unreadable file; embedded source; mounting under a prefix; 405 with `Allow`.
- Properties: defaults, each key, invalid values name the key and never the value, `server.static.directory` missing, no codec feature.
- Gate: `cargo test --workspace --all-features`, `cargo build -p oxbloom-web --no-default-features`, each codec feature alone, clippy `-D warnings`, `cargo fmt --check`, the feature combinations of `oxbloom-boot`, `make check-examples`, and the stack probes (zero growth).

## Documentation

README (new sections "CORS", "Compression", "Static resources", the new `server.*` keys and the extended order table), `docs/API.md` for each new public item, the crate docs of `oxbloom-web` (the new features), the roadmap (C6b done, follow-ups: streaming bodies, range requests, SPA fallback, pre-compressed files), `AGENT.md`, and a commented sample of the new keys in `examples/todos/application.toml`.

## Dependencies

`oxbloom-web`: `flate2` (feature `gzip`), `brotli` (feature `brotli`), `zstd` (feature `zstd`), and `tokio` with the `fs` feature for `DiskResources`. No other new dependency: content types, hashing (a small FNV-1a or std `DefaultHasher` is not stable across releases, so the implementation uses a fixed FNV-1a 64) and CIDR-free origin parsing are written in the crate.
