# C6a Filter Ordering, Uniform Errors and `server.*` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `oxbloom-web` one error model (`HttpError`, `ErrorRenderer`, problem+json), one ordered filter chain that also wraps framework answers, and the `server.*` properties (limits, shutdown timeout, error switch, forwarded headers) in `oxbloom-boot`.

**Architecture:** `oxbloom-web` gains an `order` module and a defaulted `Filter::order()`, two built-in filters (`BodyLimitFilter`, `ErrorHandlingFilter`) installed in every `App`, an `ErrorRenderer` trait with the default `ProblemRenderer`, panic catching, shutdown timeout, and client-address resolution from trusted proxies. `oxbloom-security` and the request log declare their slot through `Filter::order()`. `oxbloom-boot` reads the new keys and wires a replaceable `ErrorRenderer` bean.

**Tech Stack:** Rust (pinned by `rust-toolchain.toml`), `hyper 1`, `http`, `tokio`, `tracing`. No new dependency.

**Spec:** `docs/superpowers/specs/2026-11-01-web-errors-ordering-design.md`.

## Global Constraints

- The repo is not a git repository. There are no commits; each task ends with a passing gate. The review package is a snapshot diff.
- Never set `CARGO_TARGET_DIR` or `--target-dir`; `~/.cargo/config.toml` already points cargo at `/tmp/cargo-target`.
- Prose says "Oxbloom"; code, crates and commands say `oxbloom`. Every public item has a runnable doc example (AGENT.md). Run `graphify update .` after code changes.
- Task gate: `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`. Baseline: 1663 passed, 0 failed, 8 ignored (87 suites).
- Order constants (all `i32`, lower runs first): `REQUEST_LOG = -400`, `ERROR_HANDLING = -300`, `BODY_LIMIT = -200`, `SECURITY = -100`, `DEFAULT = 0`. Equal orders keep registration order.
- `server.*` defaults: `shutdown-timeout` 30s, `max-body-bytes` 1 MiB (1048576), `header-read-timeout` 10s, `forward-headers.strategy` `none`, `forward-headers.trusted-proxies` loopback only when the strategy is `framework`, `error.include-message` `never`.
- Problem body: `application/problem+json`, members `type` (default `about:blank`), `title` (author's, else the status reason phrase), `status`, `detail` (when set), `instance` (request path, never the query), then extension members. Existing response headers (`WWW-Authenticate`, `Allow`) are kept. A response that already has a body is never rewritten.
- Error and analyzer text for invalid config names the key and never echoes the value. `HttpError` extension values are strings, integers and booleans; the renderer escapes JSON by hand.
- Stack probes must not grow: DI 206/277/230/194 KB, boot 238 KB (default) / 239 KB (`logging`), `build()` frame `sub $0x178,%rsp`. They are measured with the ignored probe tests driven by `OXBLOOM_DI_STACK_KB` and `OXBLOOM_BOOT_STACK_KB`.
- No new crate dependency; the panic catcher is a small std-only future wrapper.

## Review Focus

- A handler panic whose message holds a secret: with `include-message = never` the response and the problem body must not contain it; the request log still records a 500. Pinned in Task 3 and Task 5.
- An oversized body sent without credentials gets 413, not 401, and the request log records it. Pinned in Task 2 and Task 5.
- A spoofed leftmost `X-Forwarded-For` entry behind a trusted proxy must not become the client address; an untrusted peer's forwarded headers are ignored. Pinned in Task 4.
- Control characters, quotes and a query string in the path or in `title`/`detail`: the problem body is valid JSON, one value per member, and `instance` has no query. Pinned in Task 1.
- A `HEAD` request that hits an error and a handler that returns its own error body: no body added to `HEAD` answers, and the handler's body is left alone. Pinned in Task 3.
- A connection that stays open past `server.shutdown-timeout`: the server stops, does not hang, and logs a warning. Pinned in Task 4 and Task 6.

---

### Task 1: `HttpError`, `ErrorAttributes`, `ErrorRenderer`, `ProblemRenderer`

**Files:**
- Create: `crates/oxbloom-web/src/error.rs`, `crates/oxbloom-web/src/render.rs`
- Modify: `crates/oxbloom-web/src/lib.rs` (modules and re-exports), `crates/oxbloom-web/src/response.rs` (`IntoResponse for Result<T, E>`)
- Test: unit tests in both new files

**Interfaces:**
- Consumes: `Response = http::Response<Bytes>`, `IntoResponse`, `HttpMethod`.
- Produces (public, re-exported from the crate root): `HttpError` with `HttpError::new(status: StatusCode) -> Self`, `title(self, impl Into<String>) -> Self`, `detail(self, impl Into<String>) -> Self`, `type_uri(self, impl Into<String>) -> Self`, `with(self, key: impl Into<String>, value: impl Into<ErrorValue>) -> Self`; `ErrorValue` (enum `Str(String)`, `Int(i64)`, `Bool(bool)` with `From` for `&str`, `String`, `i64`, `u32`, `bool`); `ErrorAttributes { status: StatusCode, title: Option<String>, detail: Option<String>, type_uri: Option<String>, extensions: Vec<(String, ErrorValue)>, internal: Option<String> }` (public fields, `ErrorAttributes::from_status(StatusCode)`); `RequestInfo { method: HttpMethod, path: String }`; `trait ErrorRenderer: Send + Sync { fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response; }`; `ProblemRenderer::new(include_internal: bool) -> Self` implementing `ErrorRenderer`. `IntoResponse for HttpError` returns an empty-bodied response with the status and an `ErrorAttributes` value in `res.extensions()`. `IntoResponse for Result<T, E>` where `T: IntoResponse, E: IntoResponse`.

- [ ] **Step 1: Write failing tests.** `http_error_response_carries_attributes_and_no_body`; `result_ok_and_err_map_through_into_response`; `problem_body_has_required_members` (status 404 renders `type` `about:blank`, `title` `Not Found`, `status` 404, `instance` the path, content type `application/problem+json`); `author_detail_and_extensions_are_rendered` (`.detail("x").with("limit", 5).with("ok", true).with("who", "me")`); `title_defaults_to_reason_phrase_and_author_title_wins`; `include_internal_false_hides_internal_and_true_shows_it_as_detail` (attrs with `internal: Some("boom")` and no author detail); `author_detail_is_kept_when_internal_is_set`; `json_is_escaped` (quotes, backslash, newline, `\u{1}`, non-ASCII in title, detail, a key and a string value parse back with `serde_json` in the test); `existing_headers_are_kept` (the renderer given a response with `WWW-Authenticate` — see Step 3 — keeps it); `instance_never_contains_a_query` (a `RequestInfo` path cannot contain one; assert `instance` equals the path given).
- [ ] **Step 2:** Run `cargo test -p oxbloom-web error render`. Expect: compile failure or FAIL.
- [ ] **Step 3: Implement** the types above. `ErrorRenderer::render` returns a fresh `Response`; `ErrorHandlingFilter` (Task 3) copies the original response's headers other than `content-type` and `content-length` onto it. Write the JSON by hand with a small `escape_json` helper (no `serde`). `ProblemRenderer` writes `detail` as the author's detail if set, else `internal` only when `include_internal`. Add doc examples for each public item.
- [ ] **Step 4:** Run `cargo test -p oxbloom-web`. Expect: PASS, including doc tests.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 2: Ordering, `BodyLimitFilter`, 413 inside the pipeline

**Files:**
- Create: `crates/oxbloom-web/src/order.rs`
- Modify: `crates/oxbloom-web/src/filter.rs` (`Filter::order`, `Ordered`, forwarding in `impl Filter for Arc<F>`, `run_pipeline` unchanged), `crates/oxbloom-web/src/app.rs` (`filter_ordered`, sorted stable insertion, built-in `BodyLimitFilter`, `dispatch` no longer answers 413), `crates/oxbloom-web/src/lib.rs`
- Test: unit tests in `filter.rs` and `app.rs`; `crates/oxbloom-web/tests/server.rs` (extend)

**Interfaces:**
- Consumes: `Filter`, `Next`, `App`, `HttpError` (Task 1).
- Produces: `pub mod order` with the five `pub const ...: i32` constants; `Filter::order(&self) -> i32` (provided method returning `order::DEFAULT`); `pub struct Ordered<F>(pub i32, pub F)` implementing `Filter` (delegates `filter`, `order` returns `.0`); `App::filter_ordered(self, order: i32, f: impl Filter + 'static) -> Self`; `App::filter` unchanged in signature and now inserts by `f.order()`; a `pub(crate) struct BodyTooLarge` marker placed in the request extensions by `dispatch`; `BodyLimitFilter` (crate-private) with `order()` = `order::BODY_LIMIT` that answers `HttpError::new(StatusCode::PAYLOAD_TOO_LARGE)` when the marker is present. `App::new()` and `with_config` install `BodyLimitFilter`.

- [ ] **Step 1: Write failing tests.** `constants_are_in_the_documented_order` (-400 < -300 < -200 < -100 < 0); `plain_filter_runs_at_default_and_keeps_registration_order_on_ties`; `filter_ordered_runs_before_default_filters`; `a_filter_declaring_order_via_the_trait_is_sorted` (a test filter overriding `order()`); `arc_filter_forwards_order`; `oversized_body_is_answered_413_by_the_body_limit_filter` (over `serve` with `max_body_bytes(8)`: a 9-byte body gets 413, a filter at `REQUEST_LOG` sees the 413 response, a filter at `DEFAULT` never runs); `oversized_body_gets_413_before_security_style_filter` (a filter at `SECURITY` that returns 401 is not reached for an oversized body); `handle_with_a_body_over_the_limit_is_not_limited` (limits apply to `serve` only; `App::handle` has no marker).
- [ ] **Step 2:** Run `cargo test -p oxbloom-web`. Expect: FAIL.
- [ ] **Step 3: Implement** as specified. Sorted insertion uses `partition_point` on the order so equal orders keep registration order. In `dispatch`, replace the early 413 return with: build an empty-body `Request` with the `BodyTooLarge` marker in its extensions and run the pipeline. Keep the existing header-sensitivity marking.
- [ ] **Step 4:** Run `cargo test -p oxbloom-web`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green; fix any existing test that asserted 413 behavior outside the pipeline.

### Task 3: Error pipeline: `ErrorHandlingFilter`, framework answers, panics

**Files:**
- Modify: `crates/oxbloom-web/src/app.rs` (`error_renderer`, built-in `ErrorHandlingFilter`, outer panic catch in `dispatch`), `crates/oxbloom-web/src/router.rs` (404 and 405 carry `ErrorAttributes`), `crates/oxbloom-web/src/filter.rs` or a new `crates/oxbloom-web/src/catch.rs` (std-only catching future), `crates/oxbloom-web/src/lib.rs`
- Test: unit tests in `app.rs`; `crates/oxbloom-web/tests/errors.rs` (new)

**Interfaces:**
- Consumes: `ErrorAttributes`, `ErrorRenderer`, `ProblemRenderer`, `RequestInfo` (Task 1); `Filter::order`, `order::ERROR_HANDLING`, sorted insertion (Task 2).
- Produces: `App::error_renderer(self, r: impl ErrorRenderer + 'static) -> Self`; crate-private `ErrorHandlingFilter` with `order()` = `order::ERROR_HANDLING`, installed by `App::new()`/`with_config` with `ProblemRenderer::new(false)`; `pub(crate) struct CatchUnwind<F>` future returning `Result<F::Output, Box<dyn Any + Send>>`. Router 404 and 405 responses are bodiless with `ErrorAttributes::from_status`; 405 keeps its `Allow` header.

- [ ] **Step 1: Write failing tests** (in `tests/errors.rs`, through `App::handle` unless noted): `unknown_path_is_problem_json_404`; `wrong_method_is_problem_json_405_with_allow`; `handler_http_error_is_rendered_with_detail_and_extensions`; `handler_result_err_is_rendered`; `bodiless_500_from_a_handler_is_rendered_from_status`; `handler_own_error_body_is_not_rewritten` (a handler returning `(StatusCode::BAD_REQUEST, "plain")` keeps `text/plain` and its body); `head_request_error_has_no_body` (HEAD to a GET route that returns an error keeps the status, content type and no body); `panic_in_handler_is_500_and_hides_message_by_default` (secret `hunter2` in the panic message is absent from the response body and headers); `panic_in_handler_with_include_message_shows_it` (app built with `error_renderer(ProblemRenderer::new(true))`); `panic_in_a_filter_inside_error_handling_is_500`; `panic_in_a_filter_outside_error_handling_is_still_500_over_serve` (a filter at `REQUEST_LOG` that panics: `serve` answers 500, the connection task does not die); `custom_error_renderer_replaces_the_default`; `panic_is_logged_at_error_on_oxbloom_web` (capture with a thread-local `tracing` subscriber; `tracing-subscriber` with `fmt` is already a dev-dependency); `413_is_rendered_as_problem_json` (Task 2 answer). Unit test `catch_unwind_returns_the_payload`.
- [ ] **Step 2:** Run `cargo test -p oxbloom-web --test errors`. Expect: FAIL.
- [ ] **Step 3: Implement.** `ErrorHandlingFilter::filter` runs `next.run(req)` inside `CatchUnwind`; on a panic it builds `ErrorAttributes { status: 500, internal: Some(message), .. }` (message from a `&str` or `String` payload, else `"panic"`) and logs it with `tracing::error!(target: "oxbloom::web", ...)`; on a normal response it renders when the response has `ErrorAttributes` in its extensions, or when it has a 4xx/5xx status and an empty body, and otherwise passes it through. When rendering it copies the original headers except `content-type` and `content-length`, and for a request whose method is `HEAD` it keeps the headers and drops the body. `dispatch` additionally wraps the whole pipeline in `CatchUnwind` and answers a bare 500 problem+json through the app's renderer (so a panic outside `ERROR_HANDLING` still yields a response). `RequestInfo` comes from the `Request` before it is moved (clone method and path first).
- [ ] **Step 4:** Run `cargo test -p oxbloom-web`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green; update existing web tests that assert empty 404/405/413 bodies.

### Task 4: Client info, forwarded headers, shutdown timeout

**Files:**
- Create: `crates/oxbloom-web/src/forwarded.rs`
- Modify: `crates/oxbloom-web/src/app.rs` (keep the peer address, `ForwardedConfig`, `App::forwarded`, `App::shutdown_timeout`, timeout in `serve`), `crates/oxbloom-web/src/request.rs` (`client_addr`, `scheme`, `host`), `crates/oxbloom-web/src/lib.rs`
- Test: unit tests in `forwarded.rs`; `crates/oxbloom-web/tests/server.rs` (extend)

**Interfaces:**
- Consumes: `App`, `Request::extensions`.
- Produces (public): `Scheme { Http, Https }`; `ClientInfo { addr: SocketAddr, scheme: Scheme, host: Option<String> }` stored in the request extensions by `serve`; `Request::client_addr(&self) -> Option<SocketAddr>`, `scheme(&self) -> Scheme` (default `Http`), `host(&self) -> Option<&str>`; `TrustedProxies` with `TrustedProxies::parse(entries: &[String]) -> Result<Self, ProxyParseError>` (IP or CIDR, IPv4 and IPv6, prefix range checked), `TrustedProxies::loopback()`, `contains(&self, IpAddr) -> bool`; `ForwardedConfig { strategy: ForwardStrategy, trusted: TrustedProxies }` with `ForwardStrategy { None, Framework }`; `App::forwarded(self, ForwardedConfig) -> Self`; `App::shutdown_timeout(self, Duration) -> Self` (default 30s); crate-private `resolve_client(peer: SocketAddr, headers: &HeaderMap, cfg: &ForwardedConfig) -> ClientInfo`. `ProxyParseError` implements `Display` without echoing the entry text (it says which index failed).

- [ ] **Step 1: Write failing tests** (unit): `cidr_parsing_and_matching` (`10.0.0.0/8`, `192.168.1.5`, `::1`, `fd00::/8`, `/0`, `/32`, `/128`; invalid `/33`, `/129`, `x`, empty); `strategy_none_ignores_headers`; `untrusted_peer_headers_are_ignored`; `trusted_peer_single_hop_xff`; `multi_hop_xff_stops_at_first_untrusted_from_the_right`; `spoofed_leftmost_entry_is_not_the_client` (peer trusted, XFF `6.6.6.6, 203.0.113.9` with only the peer trusted gives `203.0.113.9`); `all_hops_trusted_uses_leftmost`; `ipv6_and_ports_and_brackets`; `malformed_value_falls_back_to_peer` (any invalid element makes the header unusable); `forwarded_header_takes_precedence_over_x_forwarded`; `proto_accepts_http_https_only`; `host_is_taken_when_a_valid_authority`. Integration: `request_exposes_peer_addr_without_forwarded_config`; `shutdown_timeout_drops_a_stuck_connection_and_warns` (open a TCP connection, send a partial request, trigger shutdown with `shutdown_timeout(200ms)`; `serve` returns within 2s; a `warn` event on target `oxbloom::web` is captured).
- [ ] **Step 2:** Run `cargo test -p oxbloom-web forwarded`. Expect: FAIL.
- [ ] **Step 3: Implement.** Parse `Forwarded` (`for=`, `proto=`, `host=` pairs, comma-separated elements, quoted values, bracketed IPv6) by hand; `X-Forwarded-For` walks right to left; no new dependency. `serve` wraps the graceful shutdown in `tokio::time::timeout(self.shutdown_timeout, graceful.shutdown())` and logs `tracing::warn!(target: "oxbloom::web", ...)` when it expires. Doc examples for every public item.
- [ ] **Step 4:** Run `cargo test -p oxbloom-web`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 5: Security chain and request log take their slots

**Files:**
- Modify: `crates/oxbloom-security/src/chain.rs` (`impl Filter for SecurityFilterChain` overrides `order`), `crates/oxbloom-web/src/request_log.rs` (`order` = `REQUEST_LOG`), existing tests that assert empty 401/403 bodies or the old ordering in `crates/oxbloom-security/tests/` and `crates/oxbloom-boot/tests/`
- Test: `crates/oxbloom-boot/tests/request_log.rs` (extend), `crates/oxbloom-security` tests (extend)

**Interfaces:**
- Consumes: `Filter::order`, `order::SECURITY`, `order::REQUEST_LOG`.
- Produces: `SecurityFilterChain::order()` returns `order::SECURITY`; `RequestLogFilter::order()` returns `order::REQUEST_LOG`. Nothing else changes in the security API.

- [ ] **Step 1: Write failing tests.** In `request_log.rs` (boot, features `web`, `security`, `logging`): replace `request_log_misses_security_rejections` with `request_log_records_security_rejections` (a 401 from the chain now appears in the request log with status 401, and its body is problem+json with `WWW-Authenticate` kept); `request_log_records_413` (oversized body: event with status 413); `request_log_records_404_and_panic_500`; `panic_secret_is_not_in_response_or_log_fields` (handler panics with `hunter2`: the response has no `hunter2`; the request event has none either). In `oxbloom-security`: `chain_declares_the_security_order`.
- [ ] **Step 2:** Run `cargo test -p oxbloom-boot --all-features --test request_log` and the security test. Expect: FAIL.
- [ ] **Step 3: Implement** the two `order` overrides, then fix every existing test that assumed empty bodies for 401/403 or the old chain-first placement (assert the status, the headers and a problem+json body instead).
- [ ] **Step 4:** Run `cargo test --workspace --all-features`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 6: Boot: `server.*` keys, renderer bean, wiring

**Files:**
- Modify: `crates/oxbloom-boot/src/web.rs` (`ServerProperties`, passing values to `App`), `crates/oxbloom-boot/src/autoconfigure/server.rs` (register all filters, `ErrorRenderer` bean, metadata), `crates/oxbloom-boot/src/autoconfigure/mod.rs`, `crates/oxbloom-boot/src/lib.rs`, `crates/oxbloom-boot/src/analyzers/` (config analyzer covers the new `server.*` keys if it routes by prefix), `crates/oxbloom-boot/tests/metadata.rs`
- Test: `crates/oxbloom-boot/tests/server_properties.rs` (new, feature `web`), `crates/oxbloom-boot/tests/autoconfigure_web.rs` (extend)

**Interfaces:**
- Consumes: `App::{max_body_bytes, header_read_timeout, shutdown_timeout, forwarded, error_renderer}`, `TrustedProxies::parse`, `ProblemRenderer::new`, the config duration and size parsers in `oxbloom-config` (public API; use whatever `Config::get` supports for `Duration` and the size type).
- Produces: the keys `server.shutdown-timeout`, `server.max-body-bytes`, `server.header-read-timeout`, `server.forward-headers.strategy`, `server.forward-headers.trusted-proxies`, `server.error.include-message` in `WebServerAutoConfiguration::properties()` metadata (defaults from Global Constraints); an `ErrorRenderer` bean (`ProblemRenderer::new(include_message)`) registered as `Arc<dyn ErrorRenderer>` with `on_missing_bean::<dyn ErrorRenderer>()`; `WebServerAutoConfiguration` injects `Option<Arc<dyn ErrorRenderer>>` and applies it with `App::error_renderer`; filters are registered with plain `App::filter`, and each filter's own `order()` places it (the security chain is still injected as `Option<Arc<SecurityFilterChain>>`).

- [ ] **Step 1: Write failing tests.** `defaults_for_every_new_key`; `each_key_is_applied` (a body over `server.max-body-bytes` gets 413; `server.header-read-timeout` applied; renderer `include-message = always` shows a panic message); `invalid_values_name_the_key_and_not_the_value` (`server.shutdown-timeout = "hunter2"`, `server.max-body-bytes = "hunter2"`, `server.forward-headers.strategy = "hunter2"`, `server.error.include-message = "hunter2"`, `trusted-proxies = ["hunter2"]` each give a `ConfigError::Invalid` whose key is the property and whose text lacks `hunter2`, through `WebServer::new` and through startup); `user_error_renderer_bean_wins` (an application `dyn ErrorRenderer` replaces `ProblemRenderer`, conditions report shows the skip); `forwarded_headers_reach_the_request` (strategy `framework`, trusted `127.0.0.1`: a client request with `X-Forwarded-For` shows the forwarded address in `Request::client_addr()` seen by a controller); `untrusted_forwarded_headers_are_ignored_by_default`; `shutdown_timeout_from_config_stops_a_stuck_connection`; `metadata_pins_defaults` (update the pin test in `autoconfigure/server.rs` and `tests/metadata.rs` key lists and counts).
- [ ] **Step 2:** Run `cargo test -p oxbloom-boot --features web,security,logging`. Expect: FAIL.
- [ ] **Step 3: Implement.** `ServerProperties` gets the new fields with `serde` defaults; unknown values map to `ConfigError::invalid(key, fixed text)` without the value (parse the strategy and the proxies by hand so no serde message leaks). `WebServer::new` applies the values to the `App`. Keep new data in `WebServer`/`ServerProperties` (not on the application builder or run plan) so the boot stack probe does not grow. Doc examples for new public items.
- [ ] **Step 4:** Run `cargo test -p oxbloom-boot --no-default-features`, `--features web`, `--features logging`, `--all-features`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 7: Docs, example, roadmap, final gate

**Files:**
- Modify: `README.md` (new section "Errors and filter order", `server.*` keys), `docs/API.md` (every new public item and key), `docs/ROADMAP-core.md` (C6a done 2026-11-01 with spec and plan paths, delivered list, limits, follow-ups; C6b open), crate docs of `oxbloom-web`, `examples/todos` (use `HttpError` for its 404 and 422 answers; keep `make test-todos` green)
- Test: doc tests, `make check-examples`

**Interfaces:**
- Consumes: everything above.
- Produces: user-facing docs only.

- [ ] **Step 1:** Write the docs, stating: the order constants and that lower runs first; `Filter::order()` versus `App::filter_ordered`; that 401/403 now have problem+json bodies with headers kept; the request log records them and 413; `HttpError`, `Result` handlers, `ErrorRenderer` replacement; `include-message` semantics; the forwarded-header algorithm (right-to-left, trusted list, `Forwarded` first, malformed falls back to the peer); that nothing in security uses the client address yet; shutdown timeout behavior; and the limits: the outermost panic path loses a request-log event for a panic in a filter ordered before the request log, `HEAD` error answers carry no body, extensions are strings, integers and booleans only.
- [ ] **Step 2:** Run `make check-examples`. Expect: pass.
- [ ] **Step 3:** Run the full gate: `cargo test --workspace --all-features`, clippy `-D warnings`, `cargo fmt --check`, `cargo test -p oxbloom-boot --no-default-features`, `--features web`, `--features logging`. Record the counts.
- [ ] **Step 4:** Re-measure the stack probes with the ignored probe tests and `objdump` for the `build()` frame. Expect: zero growth over the Global Constraints numbers.
- [ ] **Step 5:** Run `graphify update .`.
