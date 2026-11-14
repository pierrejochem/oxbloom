# C10b: Test support for web applications Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `oxbloom-test` gets `TestClient`/`TestResponse` (in-memory, served, connected) and `TestApp` with slice presets; `oxbloom-web` gets `App::handle_from`; boot's real-HTTP integration tests and the examples' request helpers move onto them.

**Architecture:** `App::handle_from` runs the server's dispatch steps (body limit, client resolution, filters, router) on a buffered `Request`, so in-memory calls match served calls. `TestClient` has one request builder over three transports (in-memory, owned server on port 0, connect to an address) and always answers with `oxbloom_web::Response`. `TestApp` reuses a lifecycle module extracted from `ContextRunner`, starts the boot application, waits for `WebServer::local_addr()`, and hands a connected `TestClient`.

**Tech Stack:** Rust, `oxbloom-web`, `oxbloom-boot`, `hyper` (client, `http1`), `hyper-util` (client), `http-body-util`, `tokio`.

**Spec:** `docs/superpowers/specs/2026-11-08-test-support-app-design.md` (builds on `docs/superpowers/specs/2026-11-08-test-support-context-design.md`). The plan has seven tasks where the spec's outline has six: the spec's "extract the shared lifecycle, add `TestApp`" is split into a behavior-preserving refactor (Task 3) and the new feature (Task 4). The repo is not a git repo: there are no commits. Where a skill says "commit", run the task's verification commands instead.

## Global Constraints

- `App::handle_from(&self, req: Request, peer: Option<SocketAddr>) -> Response`. `handle` is unchanged. Both share code with the server's `dispatch`; no copy of the body-limit or client-resolution logic.
- `oxbloom-test` feature `web` (default off) adds the optional dependencies `oxbloom-web`, `hyper` (features `client`, `http1`), `hyper-util` (feature `client-legacy`, `tokio`, `http1`) and `http-body-util`, and still forwards `oxbloom-boot/web`. Feature `json` of `oxbloom-test` forwards `oxbloom-web/json` and adds `serde`, `serde_json`; `TestClient::json`/`TestResponse::json` exist only with it.
- `TestClient::in_memory(app: App)`; `async TestClient::serve(app: App)`; `TestClient::connect(addr: SocketAddr)`; `.peer(SocketAddr)` default `127.0.0.1:50000`. Request builder: `get|post|put|patch|delete|head|options(path)`, `.header(name, value)`, `.body(impl Into<Bytes>)`, `.json(&T)`, `.send().await -> TestResponse`. `TestResponse`: `status()`, `header(name) -> Option<&str>`, `bytes()`, `text()`, `json::<T>()`, `into_inner() -> Response`.
- Every `send()` and `shutdown()` is bounded at 10 seconds and never hangs. A connection error or timeout panics with the method, the path (no query string) and the cause; headers and bodies are never part of a panic message.
- `TestClient::serve` and `TestApp` bind only `127.0.0.1` and never read the process environment. Dropping a served client or a `RunningApp` triggers shutdown without waiting.
- `TestApp` is real-server only. `TestApp::new()` registers nothing. Presets: `web()` = `RequestLogAutoConfiguration` + `WebServerAutoConfiguration`; `secured()` (needs `security`) = web + `JwtDecoderAutoConfiguration` + `SecurityAutoConfiguration`; `full()` = `oxbloom_boot::auto_configurations()`. `start()` panics with the failure report and captured output when startup fails, and when the application has no web server (no controller) instead of waiting 10 seconds. `start_failing()` mirrors it.
- The C10a rules carry over: hermetic defaults (`env_from([])`, port 0, logging off through `--oxbloom.logging.enabled=false`, captured output), 5 s boot stop deadline, shutdown-failure text guard in `stop`, 10 s run bound.
- Migration: move helpers, never rewrite assertions; test names and assertions stay; the passed-test count never drops (baseline 2132 passed, 0 failed, 8 ignored). Only integration tests under `tests/` migrate. `oxbloom-web`'s own tests and any test that needs a raw socket keep their `TcpStream`, with a comment saying why.
- Feature unification hazard: giving boot's `oxbloom-test` dev-dependency features (`web`, `security`) turns those boot features on for every `cargo test -p oxbloom-boot` build. Task 5 re-baselines the boot feature-matrix counts and checks the feature-off library builds with `cargo build -p oxbloom-boot --no-default-features` (and `--features logging`), which does not see dev-dependencies. Do not hide the change.
- Stack probes must not grow: boot 238 KB default, 239 KB with `logging`, DI 206/277/230/194 KB. No field is added to `OxbloomApplicationBuilder`; no boot code changes are planned.
- Every public item has a runnable doc example (`AGENT.md`). Docs, comments and test names are plain full sentences. "Oxbloom" in prose, `oxbloom` in code.
- Never set `CARGO_TARGET_DIR` or `--target-dir`. Run `graphify update .` after code changes. Use literal absolute paths with `rm`.

## Review Focus

- The same request must give the same answer in memory and over the socket: a body over `server.max-body-bytes` is 413, and an `X-Forwarded-For` from an untrusted peer is ignored in both. Pinned in Tasks 1 and 2.
- A `HEAD` answer, a `204` and a `304` have no body and must not hang or error in the hyper client; a large body (above 64 KiB) arrives whole. Pinned in Task 2.
- `TestResponse::header` on a header value that is not valid UTF-8 returns `None` and does not panic. Pinned in Task 2.
- A served client dropped without `shutdown()` must free its port and task (a second bind to the same address succeeds shortly after). Pinned in Task 2.
- `TestApp::start` on an application without a controller must fail fast with a clear message, not hang for 10 seconds. A bearer token must never appear in a panic message. Pinned in Task 4.

---

### Task 1: `App::handle_from` in `oxbloom-web`

**Files:**
- Modify: `crates/oxbloom-web/src/app.rs` (new method next to `handle` at ~489; share code with `dispatch` at ~630)
- Test: `crates/oxbloom-web/src/app.rs` (tests module) and `crates/oxbloom-web/tests/handle_from.rs` (new)

**Interfaces:**
- Consumes: `resolve_client(peer, headers, forwarded)`, `run_guarded`, `BodyTooLarge`, `ClientInfo`, `Request` fields `query`, `headers`, `body` as used by `dispatch`.
- Produces: `pub async fn handle_from(&self, req: Request, peer: Option<SocketAddr>) -> Response`. With `peer = None` the client address is unknown, as for a connection without a peer.

- [ ] **Step 1: Write the failing tests** in `tests/handle_from.rs`: `a_body_over_the_limit_is_413_in_memory` (`App::new().max_body_bytes(8)`, a 9-byte body, route answers 200 on `POST /x`; expect 413), `a_body_at_the_limit_is_accepted`, `the_client_address_is_the_peer` (handler reads `ClientInfo` from `req.extensions()`; `handle_from(req, Some(addr))` shows `addr`), `forwarded_headers_from_an_untrusted_peer_are_ignored` and `forwarded_headers_from_a_trusted_proxy_are_used` (build the `ForwardedConfig` the way `tests/forwarded.rs` does), `handle_from_and_the_served_answer_agree` (same request through `handle_from` and through `App::serve` on port 0 with a raw `TcpStream`: equal status and body for a 200, a 404 and a 413).
- [ ] **Step 2: Run** `cargo test -p oxbloom-web --test handle_from`. Expected: FAIL, `handle_from` not found.
- [ ] **Step 3: Implement** `handle_from` in `app.rs`. Extract the part of `dispatch` after the body is collected (header sensitivity marking, `ClientInfo` insertion, `BodyTooLarge` marker, `run_guarded`) into one private function used by both `dispatch` and `handle_from`; `handle_from` computes `too_large = req.body.len() > self.max_body_bytes` and resolves the client with `self.forwarded`. A doc example on `handle_from` shows the 413 case.
- [ ] **Step 4: Run** `cargo test -p oxbloom-web --all-features`, `cargo test -p oxbloom-web --no-default-features`, `cargo test -p oxbloom-web --doc`. Expected: PASS, with the 349 no-default-features count unchanged except for the new tests.
- [ ] **Step 5: Run** `cargo clippy -p oxbloom-web --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 2: `TestClient`, `TestResponse` and the request builder

**Files:**
- Create: `crates/oxbloom-test/src/client.rs`, `crates/oxbloom-test/tests/client.rs`
- Modify: `crates/oxbloom-test/Cargo.toml` (features and optional dependencies per Global Constraints), `crates/oxbloom-test/src/lib.rs` (`#[cfg(feature = "web")] mod client;` and exports)

**Interfaces:**
- Consumes: `oxbloom_web::{App, Request, Response, HttpMethod}`, `App::handle_from` (Task 1), `App::serve(listener, shutdown)`.
- Produces: `TestClient` (`in_memory`, `serve`, `connect`, `peer`, `addr() -> Option<SocketAddr>`, `async shutdown(self)`, the seven method builders), `RequestBuilder` (`header`, `body`, `json`, `send`), `TestResponse` (`status() -> StatusCode`, `header(&self, name: &str) -> Option<&str>`, `bytes() -> &Bytes`, `text() -> String`, `json<T: DeserializeOwned>() -> T`, `into_inner() -> Response`).

- [ ] **Step 1: Write the failing integration tests** in `tests/client.rs` (feature `web`, `#[tokio::test(flavor = "multi_thread")]`): `in_memory_and_served_answer_the_same` (one table of requests, 200/404/405 and a JSON body, through `in_memory` and `serve`, equal status, content type and body), `a_body_over_the_limit_is_413_in_both_modes`, `an_untrusted_forwarded_header_is_ignored_in_both_modes`, `head_204_and_304_have_no_body_and_do_not_hang` (a route answering each; 3 seconds bound), `a_body_above_64_kib_arrives_whole`, `a_header_that_is_not_utf8_has_no_text_form` (route sets a `HeaderValue::from_bytes(b"\xff")` header; `header(..)` is `None`), `keep_alive_reuses_one_connection` (a `TcpListener` accept counter in front of `App::serve` through `TestClient::connect`: three requests, one accepted connection), `dropping_a_served_client_frees_its_port` (note `addr()`, drop, then `TcpListener::bind(addr)` succeeds within 2 seconds, retrying), `shutdown_stops_the_server` (a later `connect` fails), `connect_to_a_closed_port_panics_with_method_and_path` (`#[should_panic(expected = "GET /missing")]`, query string absent from the message), `a_request_that_never_answers_panics_after_the_bound` (use `tokio::time::pause` is not allowed here; instead a route that sleeps 15 s is too slow: make the bound a `pub(crate) const` and test the panic text with a listener that accepts and never answers, `#[should_panic(expected = "10 seconds")]` run under `tokio::time::timeout(12 s)`), `the_json_helpers_round_trip` (feature `json`), `headers_and_authorization_never_appear_in_panic_messages` (a failing request with an `Authorization` header: the panic message does not contain the token).
- [ ] **Step 2: Run** `cargo test -p oxbloom-test --features web,json --test client`. Expected: FAIL (module missing).
- [ ] **Step 3: Implement** `client.rs`. Transport enum with three variants. In-memory: build the `Request` from the builder (method, path with optional `?query` split at the first `?`, headers, body) and call `app.handle_from(req, Some(peer))`. Served and connected: a hyper-util legacy client (`Client::builder(TokioExecutor::new())`) over `HttpConnector` with the request built from `http::Request<Full<Bytes>>`, collect the body with `http-body-util`, map to `Response` (`http::Response<Bytes>`). `serve` binds `127.0.0.1:0`, spawns `app.serve(listener, rx)` with a `oneshot` shutdown; `Drop` sends the shutdown signal; `shutdown(self)` sends it and awaits the task inside `tokio::time::timeout(10 s)`. Wrap `send()` in `tokio::time::timeout(10 s)` and panic with `"{METHOD} {path} failed: {cause}"`. Doc examples on every public item (use `App::new()` with one `Route` and `TestClient::in_memory`).
- [ ] **Step 4: Run** `cargo test -p oxbloom-test --all-features`, `cargo test -p oxbloom-test --features web` and the plain `cargo test -p oxbloom-test` (no features still compiles and passes), `cargo test -p oxbloom-test --doc --all-features`. Expected: PASS.
- [ ] **Step 5: Run** `cargo clippy -p oxbloom-test --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 3: Extract the shared lifecycle from `ContextRunner` (no behavior change)

**Files:**
- Create: `crates/oxbloom-test/src/lifecycle.rs`
- Modify: `crates/oxbloom-test/src/runner.rs` (use the new module), `crates/oxbloom-test/src/lib.rs` (private `mod lifecycle;`)

**Interfaces:**
- Consumes: the private items of `runner.rs`: `DEFAULT_ARGS`, `RUN_TIMEOUT`, `SHUTDOWN_TIMEOUT`, `RUN_TIMEOUT_MESSAGE`, `SHUTDOWN_FAILURES`, `has_shutdown_failure`, `copy_dir`, `lock`, `RunnerClock`, `Probe`, `ProbeListener`, `TriggerOnReady`, and `ContextRunner::prepare`.
- Produces (all `pub(crate)`): `Setup { builder, properties, config_str, config_dir, profiles }` with the configuration methods `ContextRunner` and `TestApp` share (`property`, `config_str`, `config_dir`, `profile`, `configure`, `auto_configuration`, `auto_configurations`, `exclude`, `clock`) returning `Self`, and `fn prepare(self, output: &Captured) -> (OxbloomApplicationBuilder, Option<TempConfigDir>)`; the constants; `fn lock`; `fn has_shutdown_failure`; `fn finish_run(output: &Captured, result: Result<ExitStatus, StartupError>)` is NOT added (the two callers keep their own control flow).

- [ ] **Step 1: Baseline.** Run `cargo test -p oxbloom-test --all-features` and write down the passed count (78 expected). It must be identical after the refactor.
- [ ] **Step 2: Move** the listed items into `lifecycle.rs` and make `ContextRunner` hold a `Setup`. The public `ContextRunner` API, its `Debug` output and its docs do not change. No test is edited in this task.
- [ ] **Step 3: Run** `cargo test -p oxbloom-test --all-features`, `cargo test -p oxbloom-boot --all-features`, `cargo clippy -p oxbloom-test --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`. Expected: the same pass counts as the baseline, and `git`-less diff shows only moved code in `runner.rs`.

---

### Task 4: `TestApp`, `RunningApp`, `Stopped` and the presets

**Files:**
- Create: `crates/oxbloom-test/src/app.rs`, `crates/oxbloom-test/tests/app.rs`
- Modify: `crates/oxbloom-test/src/lib.rs` (`#[cfg(feature = "web")] mod app;` and exports), `crates/oxbloom-test/Cargo.toml` (dev-dependencies as the tests need)

**Interfaces:**
- Consumes: `lifecycle::Setup`, `TestClient::connect` (Task 2), `oxbloom_boot::{WebServer, RunListener, ShutdownHandle, StartupError, ExitStatus, auto_configurations, RequestLogAutoConfiguration, WebServerAutoConfiguration}` and, with `security`, `JwtDecoderAutoConfiguration`, `SecurityAutoConfiguration`.
- Produces: `TestApp` with the `Setup` configuration methods, `listener(impl RunListener + 'static) -> Self`, constructors `new`, `web`, `secured` (cfg `security`), `full`; `async start(self) -> RunningApp`; `async start_failing(self) -> StartupError`. `RunningApp`: `client(&self) -> &TestClient`, `addr(&self) -> SocketAddr`, `config(&self) -> Arc<Config>`, `output(&self) -> &Captured`, `async stop(self) -> Stopped`. `Stopped`: `status() -> &ExitStatus`, `output() -> &Captured`.

- [ ] **Step 1: Write the failing tests** in `tests/app.rs` (feature `web`): `web_slice_serves_a_controller` (controller registered with `configure`, `client().get("/hello")` answers 200), `new_registers_no_auto_configuration` (a controller without any preset gives the no-web-server panic of the next test, not a hang), `start_without_a_controller_fails_fast` (`#[should_panic(expected = "no web server")]` under a 3 second `timeout`), `web_slice_has_no_security_chain` (a request without credentials is 200), `secured_slice_rejects_without_a_token_and_accepts_a_valid_one` (feature `security`, HS256 secret in `config_str`, `ManualClock` through `.clock(..)`; no token is 401, a token with `exp` after the clock is 200, after `advance` past `exp` it is 401), `a_bearer_token_never_appears_in_a_panic_message` (a failing `start()` with a token in a property: the panic text lacks it), `full_runs_every_enabled_auto_configuration`, `start_failing_returns_the_error_for_a_bad_port` (`property("server.port", "70000")`), `start_failing_panics_when_startup_succeeds`, `config_is_captured_at_context_ready` (`config().get` of a `property`), `stop_reports_a_zero_status_and_output`, `a_service_whose_stop_panics_fails_stop` (reuse the C10a shape from `tests/runner.rs`), `dropping_a_running_app_shuts_it_down` (the port is free again within 5 seconds), `a_listener_passthrough_sees_lifecycle_events`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-test --all-features --test app`. Expected: FAIL (`TestApp` missing).
- [ ] **Step 3: Implement** `app.rs`. `start` builds with `Setup::prepare`, adds a private listener that stores the `Arc<Config>` at `context_ready` (via `container.get::<Config>()`), and runs `builder.run()` in a `tokio::spawn`ed task holding the `ShutdownHandle`. A second private listener records `WebServer::local_addr()` once `started` fires: read the `WebServer` bean from the container in `started(&Container)` and store its `local_addr()`. If `ready` fires with no `WebServer` bean, store a "no web server" marker so `start` panics at once with `"no web server: register a dyn Controller or use TestApp::web()"` plus the captured output. Wait with a 5 ms poll bounded at 10 s; if the run task ends first, panic with `"startup failed: {error}\n{output}"`. `RunningApp` owns the task, the shutdown handle and a `TestClient::connect(addr)`. `stop` triggers shutdown, awaits the task within 10 s, applies `has_shutdown_failure` to the output, and returns `Stopped`. `Drop` triggers shutdown. `start_failing` runs the same task and expects `Err`; with `Ok` it triggers shutdown and panics. The presets call the `Setup` methods.
- [ ] **Step 4: Run** `cargo test -p oxbloom-test --all-features`, `cargo test -p oxbloom-test --features web` (no `security`), `cargo test -p oxbloom-test --doc --all-features`. Expected: PASS.
- [ ] **Step 5: Run** `cargo clippy -p oxbloom-test --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 5: Migrate boot's real-HTTP tests

**Files:**
- Modify: `crates/oxbloom-boot/Cargo.toml` (the `oxbloom-test` dev-dependency gets `features = ["web", "security"]`; keep it path-only), `crates/oxbloom-boot/tests/autoconfigure_web.rs`, `crates/oxbloom-boot/tests/autoconfigure_http.rs`, `crates/oxbloom-boot/tests/common/mod.rs` if helpers become unused

**Interfaces:**
- Consumes: `TestApp`, `RunningApp`, `TestClient`, `TestResponse`.
- Produces: no public items.

- [ ] **Step 1: Record the baselines** before touching anything: `cargo test --workspace --all-features` (2132 + the new C10b tests of Tasks 1 to 4), `cargo test -p oxbloom-boot --no-default-features`, `--features web`, `--features logging`, `--features security`, `--all-features`, both boot stack probes.
- [ ] **Step 2: Enable the dev-dependency features** and re-run the five boot matrix commands. Write down the new counts: with feature unification the `--no-default-features` run now includes the web and security tests. Report the old and new numbers; this is the expected consequence named in Global Constraints, not a failure. Also run `cargo build -p oxbloom-boot --no-default-features` and `cargo build -p oxbloom-boot --features logging` (library builds, no dev-dependencies) and expect success.
- [ ] **Step 3: Migrate** the `serve()`/`Running`/`get()` helpers of `autoconfigure_web.rs` (13 uses) and `autoconfigure_http.rs` (7) onto `TestApp::start`/`stop` and `TestClient`. Lifecycle-event assertions use `TestApp::listener(..)`. Keep every test name and assertion; where an assertion read a `(u16, String)` pair, read `status().as_u16()` and `text()` instead and say nothing else changes. A test that needs a malformed request or a raw socket keeps its `TcpStream` and gets a one-line comment saying why. Do not touch tests that do not use `serve()`.
- [ ] **Step 4: Run** the five matrix commands again and `cargo test --workspace --all-features` (count at least the Step 1 baseline), `make check-examples`, both boot stack probes (238 and 239 KB must pass), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 6: Migrate the examples' request helpers

**Files:**
- Modify: `examples/hello/Cargo.toml`, `examples/hello/tests/app.rs`, `examples/hello/tests/auto.rs`, `examples/todos/Cargo.toml`, `examples/todos/tests/api.rs` (only the files that carry a hand-rolled request helper; read each first)

**Interfaces:**
- Consumes: `TestClient`, `TestApp` (if a test starts the whole application).
- Produces: no public items.

- [ ] **Step 1: Baseline.** Run `make check-examples` and `cargo test -p hello -p todos` and write down the counts.
- [ ] **Step 2: Migrate** only helpers that open a `TcpStream` or hand-build HTTP text onto `TestClient`/`TestApp` (dev-dependency `oxbloom-test` with `features = ["web"]` plus `json` where `todos` uses JSON bodies; path-only). Keep every test name and assertion. Leave files without such a helper unchanged.
- [ ] **Step 3: Run** `make check-examples`, `cargo test --workspace --all-features` (count at least the Task 5 final), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 7: Documentation, roadmap, final gate

**Files:**
- Modify: `docs/API.md` (web-testing part of section 4.10), `README.md` (one sentence), `docs/ROADMAP-core.md` (C10b done, deferred minors), `crates/oxbloom-test/src/lib.rs` (crate docs: features `web`, `security`, `logging`, `json`)

**Interfaces:**
- Consumes: everything from Tasks 1 to 6.
- Produces: documentation only.

- [ ] **Step 1: Write the docs.** `docs/API.md`: `TestClient` in all three modes with one runnable-looking example each, `TestApp` with the three presets, the secured example with `ManualClock`, `handle_from` versus `handle`, the 10 second bounds and their limits (blocked thread, `start_paused`), what stays on raw sockets, and the feature-unification note for crates that give the dev-dependency features. Copy names exactly from the source; the snippets are read-checked, not compiled. `README.md`: one sentence and a link. `docs/ROADMAP-core.md`: mark C10b done with its delivered list, the added `App::handle_from`, the migration scope, and the deferred minors from the reviews (read the ledger lines `Task N: minor (deferred)`), under "Deferred minors from the C10b reviews".
- [ ] **Step 2: Final gate.** Run `cargo test --workspace --all-features` (count at least the Task 6 final), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `cargo test -p oxbloom-web --no-default-features`, the five boot matrix commands, `make check-examples`, the boot stack probes (238 and 239 KB) and the DI probes (206/277/230/194 KB, one test name at a time because an overflow aborts the process). Record every number and run `graphify update .`.

---

## Self-review

- **Spec coverage:** `handle_from` (Task 1); `TestClient`, `TestResponse`, builder, modes, bounds (2); shared lifecycle (3); `TestApp`, `RunningApp`, `Stopped`, presets, `start_failing`, listener passthrough (4); boot migration (5); examples migration (6); docs and gate (7). The spec's "Testing the crate" items map to Tasks 1, 2 and 4. The spec's "no bean-peeking hook" is kept out of the plan.
- **Type consistency:** `handle_from`, `TestClient::{in_memory, serve, connect, peer, addr, shutdown}`, `TestResponse::{status, header, bytes, text, json, into_inner}`, `TestApp::{new, web, secured, full, start, start_failing, listener}`, `RunningApp::{client, addr, config, output, stop}`, `Stopped::{status, output}`, `Setup` are spelled the same in every task.
- **Known open points for the implementer:** the exact feature list of the hyper-util client (`client-legacy`), how to read the `WebServer` bean in `started` (Task 4 says to read boot's `web.rs`), and the feature-unification counts (Task 5 records them).
