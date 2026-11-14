# C6b-2: CORS, compression and static resources Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `oxbloom-web` gets a CORS filter, a compression filter (gzip, brotli, zstd) and static resources (directory and embedded), all configurable from code and from `server.*` properties read by `WebServer::new`.

**Architecture:** `CorsFilter` (`order::CORS = -350`) and `CompressionFilter` (`order::COMPRESSION = -150`) are ordinary `Filter`s registered with `App::cors` and `App::compression`. `StaticResources` is a `Controller` over a `ResourceSource` trait with `DiskResources` and `EmbeddedResources`. `WebServer::new` in `oxbloom-boot` reads the new keys and applies them to the `App`, so the auto-configuration, the boot builder and the run plan do not change except for property metadata.

**Tech Stack:** Rust, `flate2`, `brotli`, `zstd`, `tokio` (`fs`), `http`, `bytes`, the existing `Filter`, `Controller`, `HttpError` and `ErrorRenderer` of `oxbloom-web`.

**Spec:** `docs/superpowers/specs/2026-11-01-web-cors-compression-static-design.md`. The repo is not a git repo: there are no commits. Where a skill says "commit", run the task's verification commands instead.

## Global Constraints

- Order table: `REQUEST_LOG -400`, `CORS -350`, `ERROR_HANDLING -300`, `NEGOTIATION -250`, `BODY_LIMIT -200`, `COMPRESSION -150`, `SECURITY -100`, `DEFAULT 0`. `order::CORS` and `order::COMPRESSION` exist in every feature combination.
- Features of `oxbloom-web`: `gzip`, `brotli`, `zstd` (each enables its codec crate), `compression = ["gzip", "brotli", "zstd"]` in `default` next to `json`. A codec whose feature is off is never chosen. Static resources and CORS need no feature.
- CORS defaults: methods `GET, HEAD, POST`, headers empty, exposed empty, credentials off, max age 30 minutes; off until `server.cors.allowed-origins` is set. Compression defaults: `min_size` 1024, `max-body-bytes` 8 MiB, level `default`, off until `server.compression.enabled = true`, mime types `text/*`, `application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml`, `application/xml`. Static defaults: off, directory `static`, path `/`, index `index.html`, `cache-max-age` 0s (sends `no-cache`), `max-file-bytes` 10 MiB.
- Invalid property values fail startup with `ConfigError::invalid` that names the key and never echoes the value. Code-level settings win when their key is absent (the C6a rule for `WebServer::new`).
- No streaming bodies, no range requests, no `If-Match`, no directory listings. No dependency beyond `flate2`, `brotli`, `zstd` and tokio's `fs` feature; content types and the hash (fixed FNV-1a 64) are written in the crate.
- Every public item has a runnable doc example (`AGENT.md`). Docs, comments and test names are plain full sentences. "Oxbloom" in prose, `oxbloom` in code.
- Stack probes must not grow: boot 238 KB default, 239 KB with `logging`, `build()` frame `sub $0x178,%rsp`, DI 206/277/230/194 KB. The boot builder and run plan do not grow.
- Never set `CARGO_TARGET_DIR` or `--target-dir`. Run `graphify update .` after code changes. Use literal absolute paths with `rm`.

## Review Focus

- CORS: `Origin: null`, an origin with a different case or a trailing slash, a preflight with a disallowed origin (passes through, no CORS headers), `OPTIONS` without `Access-Control-Request-Method` (not a preflight), CORS headers on a 401 from a security-slot filter and on a 413.
- Static path traversal: `..`, `%2e%2e`, `%2E%2E`, `%252e%252e` (double encoding stays literal and misses), `%2f`, backslash, NUL (`%00`), a symlink that leaves the root, dotfiles, a very long path, a path equal to the prefix with and without a trailing slash.
- Compression of `HEAD` (same headers as `GET`, empty body, no `Content-Length` lie), an `Accept-Encoding` with `q=0` entries, `*`, upper case names and duplicates, and a static response that is both compressed and answered 304.
- A response that already has `Content-Length` set by a handler and is then compressed; a handler body over `max-body-bytes` of the compression filter; `Cache-Control: no-transform`.
- `WebServer::new` with `server.static.enabled = true` and a missing directory; `server.compression.enabled = true` with no codec feature; `server.cors.allowed-origins = ["*"]` with credentials on.

---

### Task 1: Order slots and the CORS filter

**Files:**
- Modify: `crates/oxbloom-web/src/order.rs`, `crates/oxbloom-web/src/app.rs`, `crates/oxbloom-web/src/lib.rs`
- Create: `crates/oxbloom-web/src/cors.rs`, `crates/oxbloom-web/tests/cors.rs`

**Interfaces:**
- Consumes: `Filter`, `Next`, `Request`, `Response`, `HttpError`, `order`, `App::filter_ordered`.
- Produces: `order::CORS = -350`, `order::COMPRESSION = -150`; `#[derive(Debug, Clone)] pub struct CorsConfig` with a builder (`new()`, `allowed_origins(impl IntoIterator<Item = impl Into<String>>)`, `allowed_methods(..)`, `allowed_headers(..)`, `exposed_headers(..)`, `allow_credentials(bool)`, `max_age(Duration)`); `pub struct CorsFilter` with `CorsFilter::new(CorsConfig) -> Result<CorsFilter, CorsError>` implementing `Filter` with `order()` = `order::CORS`; `pub struct CorsError` (Debug, Display, Error; the message names the setting, never an origin value); `App::cors(self, config: CorsConfig) -> Result<Self, CorsError>`. All re-exported from the crate root.

- [ ] **Step 1: Write the failing tests** in `crates/oxbloom-web/tests/cors.rs` using `App::handle`:
  - `order_table_is_complete`: the full table of Global Constraints is strictly increasing.
  - `request_without_origin_passes_unchanged`; `disallowed_origin_gets_no_cors_headers_and_is_not_blocked`.
  - `simple_request_from_allowed_origin_gets_allow_origin_and_vary`, including `Access-Control-Expose-Headers` and `Access-Control-Allow-Credentials: true` when configured.
  - `allow_origin_echoes_the_origin_and_is_star_only_for_star_without_credentials`.
  - Preflight matrix: allowed (204 with `Access-Control-Allow-Origin`, `-Methods`, `-Headers`, `-Max-Age` in seconds, `Vary: Origin, Access-Control-Request-Method, Access-Control-Request-Headers`); wrong method and wrong request header and disallowed origin pass through with no CORS headers; `OPTIONS` without `Access-Control-Request-Method` is not a preflight; `allowed-headers = *` echoes the requested headers.
  - `origin_null_is_allowed_only_when_listed_literally`; `origin_comparison_ignores_host_case_but_rejects_trailing_slash_and_path` for configuration (`CorsFilter::new` fails for `https://a.example/`, `https://a.example/x`, `a.example`) and for requests.
  - Validation: `*` mixed with other origins fails; `*` origins with credentials fails; `*` in headers or methods with credentials fails; the error text names the setting and contains no origin value.
  - Ordering: CORS headers are on a 404, on a 413 (use `max_body_bytes`), on a 401 from a `filter_ordered(order::SECURITY, ..)` filter, and the preflight never reaches that security filter (assert with a counting filter).

- [ ] **Step 2: Run to verify failure:** `cargo test -p oxbloom-web --test cors` fails to compile.

- [ ] **Step 3: Implement** `order.rs` constants with doc examples, `cors.rs`, `App::cors` (registers at `order::CORS`), and the re-exports. Origins are compared as `scheme://host[:port]` with the host lower-cased. The filter appends to `Vary` instead of replacing an existing value. Origin and method lists are stored in small sorted or plain `Vec`s; no dependency.

- [ ] **Step 4: Run:** `cargo test -p oxbloom-web --all-features`, `cargo test -p oxbloom-web --no-default-features`, clippy `-D warnings` for both feature sets, `cargo fmt --check`.

### Task 2: Compression filter

**Files:**
- Modify: `crates/oxbloom-web/Cargo.toml`, `crates/oxbloom-web/src/app.rs`, `crates/oxbloom-web/src/lib.rs`
- Create: `crates/oxbloom-web/src/compression.rs`, `crates/oxbloom-web/tests/compression.rs`

**Interfaces:**
- Consumes: Task 1's `order::COMPRESSION`; `Filter`, `HttpError`.
- Produces: features `gzip`, `brotli`, `zstd`, `compression` (in `default`); `#[derive(Debug, Clone)] pub struct CompressionConfig` (`new()`, `min_size(usize)`, `mime_types(impl IntoIterator<Item = impl Into<String>>)`, `level(CompressionLevel)`, `max_body_bytes(usize)`); `#[derive(Debug, Clone, Copy, PartialEq, Eq)] #[non_exhaustive] pub enum CompressionLevel { Fast, Default, Best }`; `pub struct CompressionFilter` with `CompressionFilter::new(CompressionConfig) -> CompressionFilter` implementing `Filter` at `order::COMPRESSION`; `pub fn App::compression(self, config: CompressionConfig) -> Self`; `pub fn codecs() -> &'static [&'static str]` listing the codecs compiled in (`"zstd"`, `"br"`, `"gzip"`), used by Task 4 for validation. Types compile with no codec feature; the filter then never compresses.

- [ ] **Step 1: Write the failing tests** (dev-dependencies: the decoders `flate2`, `brotli`, `zstd` for round trips; tests that need a codec use `#[cfg(feature = ..)]`):
  - Codec choice table from `Accept-Encoding`: `gzip`, `br`, `zstd`, `gzip, br`, `gzip;q=0.5, br;q=0.4`, tie goes zstd > br > gzip, `*`, `*;q=0`, `identity`, `GZIP` (names are case-insensitive), unknown codec, duplicate entries, `br;q=0, gzip`, a malformed entry is skipped. `identity;q=0` with no usable codec answers 406 as an `HttpError` and `Accept-Encoding: identity` or no header passes unchanged.
  - Round trips: each codec's output decodes to the original body.
  - Skip rules (each its own test): body under `min_size`; type not in the list; `text/html; charset=utf-8` matches `text/*`; response already has `Content-Encoding`; status 204 and 304; `Cache-Control: no-transform`; body over `max_body_bytes`.
  - Header effects: `Content-Encoding` set; `Content-Length` removed; `Vary: Accept-Encoding` added and kept when already present; strong `ETag` becomes weak (`W/"x"`), a weak one stays; a handler-set `Content-Length` is dropped.
  - `HEAD` gets the same headers as `GET` and an empty body; a `GET` with no matching codec is unchanged.
  - Ordering: a handler body above `min_size` behind a `filter_ordered(order::SECURITY, ..)` filter is compressed; a problem+json answer rendered by the error filter is not; the 406 for `identity;q=0` is rendered as problem+json.
  - `codecs()` lists exactly the compiled codecs.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.** Add the optional dependencies (`flate2`, `brotli`, `zstd`) with exact versions at least two weeks old that `cargo` resolves offline-compatible with the lockfile; run `cargo update -p <crate>` only if needed. Encoders run synchronously on the response body (it is already in memory); the level maps `Fast`/`Default`/`Best` to each crate's own scale. Register through `App::compression` at `order::COMPRESSION`.

- [ ] **Step 4: Run:** `cargo test -p oxbloom-web --all-features`; `cargo test -p oxbloom-web --no-default-features`; `cargo build -p oxbloom-web --no-default-features --features gzip` and the same for `brotli` and `zstd`; clippy `-D warnings` for all of these; `cargo fmt --check`.

### Task 3: Static resources

**Files:**
- Modify: `crates/oxbloom-web/Cargo.toml` (tokio `fs`), `crates/oxbloom-web/src/lib.rs`
- Create: `crates/oxbloom-web/src/static_files.rs` (optionally `mime.rs` for the content-type table), `crates/oxbloom-web/tests/static_files.rs`

**Interfaces:**
- Consumes: `Controller`, `Route`, `Request`, `Response`, `HttpError`.
- Produces: `pub struct Resource { pub bytes: Bytes, pub content_type: String, pub etag: String, pub last_modified: Option<SystemTime> }`; `#[derive(Debug)] pub enum ResourceError { NotFound, TooLarge, Io(std::io::Error) }`; `#[async_trait] pub trait ResourceSource: Send + Sync { async fn read(&self, path: &str) -> Result<Resource, ResourceError>; }` where `path` is the already-cleaned relative path (segments joined by `/`, no leading slash, may be empty); `DiskResources::new(root: impl Into<PathBuf>) -> DiskResources` with `.max_file_bytes(usize)`; `EmbeddedResources::new(files: &'static [(&'static str, &'static [u8])]) -> EmbeddedResources`; `StaticResources::new(prefix: impl Into<String>, source: impl ResourceSource + 'static) -> StaticResources` with `.index(impl Into<String>)` and `.cache_max_age(Duration)`, implementing `Controller` (routes `GET` and `HEAD` under the prefix and for the prefix itself). All re-exported.

- [ ] **Step 1: Write the failing tests** (`tests/static_files.rs`, tempdir via `std::env::temp_dir` plus a unique name, cleaned up at the end):
  - Content types: every extension in the spec table, unknown extension gives `application/octet-stream`, text types carry `charset=utf-8`.
  - `index_is_served_for_the_prefix_and_for_directories`, `directory_without_index_is_404`.
  - Traversal matrix as in Review Focus: each of `/../secret`, `/%2e%2e/secret`, `/%2E%2E/secret`, `/a/%2e%2e/%2e%2e/secret`, `/%2fetc%2fpasswd`, `/a%5c..%5csecret`, `/x%00.html`, `/%252e%252e/secret`, a symlink inside the root pointing outside, `/.hidden`, `/dir/.git/config` all answer 404 and never the content of the outside file (create a `secret` file next to the root); a 4 KB path answers 404 without panic.
  - Validators: disk `ETag` changes when size or mtime changes; embedded `ETag` is the FNV-1a 64 of the bytes (pin one value); `Last-Modified` is a valid HTTP date for disk resources and absent for embedded; `Cache-Control` is `no-cache` for 0 and `public, max-age=N` otherwise.
  - Conditional requests: `If-None-Match` exact, weak (`W/"x"`), list, `*`, and a non-match; 304 has `ETag`, `Last-Modified`, `Cache-Control` and an empty body; `If-Modified-Since` honored only without `If-None-Match`; a bad date is ignored.
  - `head_has_the_get_headers_and_no_body`; `post_is_405_with_allow_get_head`; `file_over_max_file_bytes_is_404_and_logs_a_warning` (tracing capture as in `tests/errors.rs`, event at level `warn` on target `oxbloom::web` containing the path); an unreadable file (permission or a directory read as a file) is a 500 problem+json and logs at `error`.
  - Mounting under `/assets` works at `/assets/app.css` and `/assets`, and `/other/app.css` is 404 from the router.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement.** Check how the router captures a catch-all path parameter (matchit 0.9: `{*rest}`) and whether `Request::param` returns the raw or the decoded value; decode exactly once so `%252e` stays literal. The cleaned path is rejected (404) on any `..`, NUL, backslash or decoded `/` in a segment and on any segment that starts with `.`. `DiskResources` canonicalizes the root once at construction (store the canonical root; a missing root is an error at `StaticResources` use time, not a panic) and canonicalizes each target, which must start with the canonical root. Content types and the FNV-1a 64 hash are written in the crate. `HEAD` uses the same read and drops the body.

- [ ] **Step 4: Run:** `cargo test -p oxbloom-web --all-features`, `--no-default-features`, clippy `-D warnings`, `cargo fmt --check`.

### Task 4: `server.*` properties in `oxbloom-boot`

**Files:**
- Modify: `crates/oxbloom-boot/src/web.rs`, `crates/oxbloom-boot/src/autoconfigure/server.rs` (metadata only), `crates/oxbloom-boot/Cargo.toml` if feature forwarding is needed
- Create: `crates/oxbloom-boot/tests/server_extras.rs`

**Interfaces:**
- Consumes: Tasks 1-3 (`CorsConfig`, `CompressionConfig`, `codecs()`, `StaticResources`, `DiskResources`, `App::cors`, `App::compression`, `App::routes(Arc::new(static_resources).routes())`).
- Produces: `WebServer::new` also reads `server.cors.*`, `server.compression.*`, `server.static.*` (keys, types and defaults from the spec table) and applies them to the `App`; seventeen new rows in the `PropertyMetadata` of `WebServerAutoConfiguration`.

- [ ] **Step 1: Write the failing tests** (`tests/server_extras.rs`, as `tests/server_properties.rs` does):
  - Defaults: no keys changes nothing (no CORS headers, no compression, no static route).
  - `server.cors.*` end to end over a real connection: a preflight and a simple request get the configured headers; `allowed-origins` unset leaves a code-set `App::cors` alone.
  - `server.compression.enabled = true` compresses a large JSON handler body with `Accept-Encoding: gzip`; each of `min-size`, `mime-types`, `level` and `max-body-bytes` changes the behavior as documented.
  - `server.static.enabled = true` serves a file from a tempdir with `directory`, `path`, `index`, `cache-max-age`, `max-file-bytes`.
  - Invalid values: for every new key a wrong type or value fails with `ConfigError::Invalid` naming that key and not containing the value (`"hunter2"`); `*` with credentials; an origin with a path; `server.static.directory` that does not exist; `server.compression.enabled = true` when `codecs()` is empty (gate this test with `#[cfg(not(any(feature = "gzip", feature = "brotli", feature = "zstd")))]` or by calling a small internal function with an empty codec list).
  - Metadata: every new key is listed with its default; `metadata_defaults_are_the_server_defaults` still passes.

- [ ] **Step 2: Run to verify failure.**

- [ ] **Step 3: Implement** a private `Extras::from_config(&Config) -> Result<Extras, ConfigError>` in `web.rs` next to `Limits`, applied in `WebServer::new` after `limits.apply`. It must not add fields to the builder or to the run plan, and must keep the stack probes flat (box anything large). Add the metadata rows.

- [ ] **Step 4: Run:** `cargo test -p oxbloom-boot --all-features`, `--no-default-features`, `--features web`, `--features logging`, clippy `-D warnings`, fmt, and the boot stack probe at 238 KB (`OXBLOOM_BOOT_STACK_KB=238 cargo test -q -p oxbloom-boot --test conditions_report -- --ignored probe_boot` passes; 237 overflows).

### Task 5: Documentation, the example and the gate

**Files:**
- Modify: `README.md`, `docs/API.md`, `docs/ROADMAP-core.md`, `AGENT.md`, `crates/oxbloom-web/src/lib.rs` (crate docs), `examples/todos/application.toml`

- [ ] **Step 1:** README gets sections "CORS", "Compression" (with the BREACH note and the feature flags) and "Static resources" (disk and embedded examples, the traversal rules, what is not supported), the new `server.*` keys, and the extended order table. `docs/API.md` lists every new public item. The crate docs name the new features. `AGENT.md` names the new modules and features. The roadmap marks C6b done (C6b-1 and C6b-2), records follow-ups (streaming bodies, range requests, SPA fallback, pre-compressed files, per-path CORS rules) and the deferred minors. `examples/todos/application.toml` gets a commented sample of the new keys.
- [ ] **Step 2:** Check no document still lists CORS, compression or static resources as open (`grep -rn` over `README.md docs AGENT.md examples crates`, ignoring history in plans and specs).
- [ ] **Step 3: Gate.** `cargo test --workspace --all-features`, `cargo test -p oxbloom-web --no-default-features`, each codec feature alone, clippy `--workspace --all-features --all-targets -- -D warnings`, `cargo fmt --check`, `cargo doc -p oxbloom-web --no-default-features --no-deps` with zero warnings, `cargo test -p oxbloom-boot --no-default-features`, `--features web`, `--features logging`, `make check-examples`, the DI and boot stack probes and the `build()` frame as in the C5b task-6 report, then `graphify update .`. Report each result with its count.
