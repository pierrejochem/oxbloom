# C10b: Test support for web applications (`oxbloom-test`)

Date: 2026-11-08. Status: draft for review. Roadmap entry: C10, second half. It builds on C10a (`docs/superpowers/specs/2026-11-08-test-support-context-design.md`).

## Purpose

Users, and the Oxbloom test suites themselves, need to test HTTP behavior without hand-written sockets. Today `crates/oxbloom-boot/tests/autoconfigure_web.rs` and `autoconfigure_http.rs` carry their own `serve()`, `Running` and raw `get()` helpers.

Success means: a test can send a request through the real filter chain and router, either in memory or over a port-0 socket, with one fluent API and one response type; a test can start a whole boot application with a web server and talk to it; slices of the auto-configuration list are one call away; and the boot tests that kept their `serve()` harness move onto the new API with no loss of tests and no growth of the stack probes.

## Scope

- **In:** `TestClient` and `TestResponse` (web layer, any `oxbloom_web::App`), `TestApp` (boot layer, real server), slice presets, one small change to `oxbloom-web` (`App::handle_from`), and the migration of boot and example integration tests.
- **Out:** a bean-peeking hook on a running application (the container is only borrowed inside listener callbacks, so beans stay a `ContextRunner` job); HTTP/2, TLS and WebSocket clients; migration of `oxbloom-web`'s own tests, which exercise the crate itself, and of any test that needs a malformed request or a raw socket (slow headers, an oversize body that a real client refuses). Those keep their `TcpStream`.

## The `oxbloom-web` change

`App::handle(&self, Request)` runs only the filters and the router. The server path (`dispatch` in `crates/oxbloom-web/src/app.rs`) also applies the body limit, resolves the client address and the trusted-proxy forwarded headers, and reads the hyper body. A bare `handle` call in a test would skip all of that.

C10b adds `App::handle_from(&self, req: Request, peer: Option<SocketAddr>) -> Response`. It runs the same steps as the server path (body limit, client resolution, filters, router) on an already buffered `Request`. `handle` is unchanged. The implementer shares code with `dispatch` instead of copying it, and every public item has a runnable doc example.

## `TestClient` and `TestResponse` (web layer)

A new cargo feature `web` of `oxbloom-test` (default off) adds the optional dependencies `oxbloom-web`, `hyper` (client, `http1`), `hyper-util` (client) and `http-body-util`.

- `TestClient::in_memory(app: App) -> TestClient` calls `handle_from` with a fake peer `127.0.0.1:50000`, changeable with `.peer(SocketAddr)`.
- `async TestClient::serve(app: App) -> TestClient` binds `127.0.0.1:0`, runs `App::serve` in a task and sends real HTTP/1.1 through a hyper client with keep-alive. It has `addr()` and an async `shutdown()`, which stops the server and panics if it does not stop within 10 seconds. Dropping the client triggers shutdown without waiting.
- `TestClient::connect(addr: SocketAddr) -> TestClient` sends real HTTP/1.1 to an existing server and does not own it. `TestApp` uses it.
- One fluent request builder for all three modes: `get`, `post`, `put`, `patch`, `delete`, `head`, `options` (each takes a path), then `.header(name, value)`, `.body(impl Into<Bytes>)`, `.json(&T)` (feature `json`) and `.send().await -> TestResponse`.
- `TestResponse` wraps `oxbloom_web::Response` (`http::Response<Bytes>`) and offers `status()`, `header(name) -> Option<&str>`, `bytes()`, `text()`, `json::<T>()` (feature `json`) and `into_inner()`. All modes return the same type, so one assertion style works everywhere.
- Every `send()` is bounded at 10 seconds. A connection error or a timeout panics with the method, the path and the cause. A handler panic in memory comes back as the 500 problem+json that `App` already renders.

## `TestApp` (boot layer)

`TestApp` (feature `web`) runs a whole boot application with a web server. It is real-server only. Boot's `WebServer` takes the `App` out of itself on `start` and exposes only `local_addr()`, so an in-memory call would need a boot change for little gain. In-memory calls belong to `TestClient::in_memory`.

- Configuration methods and hermetic defaults are those of `ContextRunner` (`property`, `config_str`, `config_dir`, `profile`, `configure`, `auto_configuration`, `auto_configurations`, `exclude`, `clock`; port 0, logging off, captured output, no environment). A `listener(impl RunListener)` passthrough lets a test observe lifecycle events.
- `ContextRunner` and `TestApp` share one private lifecycle module, extracted from `runner.rs` (the `prepare` step, the `ready` probe, the panic, shutdown and timeout handling, the shutdown-failure guard). There is one implementation of those rules.
- `async start(self) -> RunningApp` runs the lifecycle in a task, waits until `WebServer::local_addr()` is bound (bounded at 10 seconds), and panics with the failure report and captured output if startup fails.
- `async start_failing(self) -> StartupError` is the mirror, for a bad `server.*` value or a port clash. It panics if startup succeeds.
- `RunningApp` offers `client() -> &TestClient` (a `connect` client), `addr()`, `config() -> Arc<Config>` (captured at `context_ready`), `output() -> &Captured` and `async stop(self) -> Stopped`. `Stopped` holds the `ExitStatus` and the final output. `stop` applies the C10a rules (10 second bound, shutdown-failure guard). A dropped `RunningApp` triggers shutdown without waiting.

## Slices

Slices are presets of the auto-configurations that run. They are plain constructors that call existing builder methods, and `.exclude::<A>()` still works after them.

- **Config slice:** `ContextRunner::new()` already runs no auto-configuration. C10b documents it with an example and adds no code.
- **`TestApp::web()`:** `RequestLogAutoConfiguration` and `WebServerAutoConfiguration`. Controllers and filters come from `configure`. No security chain, no JWT decoder.
- **`TestApp::secured()`** (features `web` and `security`): the web slice plus `JwtDecoderAutoConfiguration` and `SecurityAutoConfiguration`.
- **`TestApp::full()`:** `oxbloom_boot::auto_configurations()` for every feature `oxbloom-test` was built with.
- **`TestApp::new()`** registers nothing, like `ContextRunner::new()`.
- A web slice without DI is `TestClient::in_memory(app)`.

## Migration

The rule is the C10a one: migrate by moving helpers, never by rewriting assertions. Test names and assertions stay, and the passed-test count never drops.

- `crates/oxbloom-boot/tests/autoconfigure_web.rs` (13 `serve()` uses) and `autoconfigure_http.rs` (7): `serve()`, `Running` and the raw `get()` helper move onto `TestApp::start`/`stop` and `TestClient`.
- `examples/hello` and `examples/todos` API tests: switch to `TestClient` only where they carry a hand-rolled request helper.
- Tests that need a raw socket keep their `TcpStream` and the test says why in a comment.
- The dev-dependency cycle rule of C10a applies: only integration tests under `tests/` migrate; in-`src` unit tests keep their helpers.

## Limits

`send()` and `shutdown()` are bounded at 10 seconds and never hang. `TestClient::serve` and `TestApp` never read the process environment. `TestClient::serve` binds `127.0.0.1`; `TestApp` binds 127.0.0.1 by default (boot's default `server.address`); a user property can override it. The C10a limits (blocking a thread on a current-thread runtime, `start_paused` auto-advance) apply to the 10 second bounds here too, and the docs repeat them.

## Testing the crate

- `TestClient`: in-memory and served modes give the same status, headers and body for the same request; the body limit (413) and forwarded-header resolution work in memory through `handle_from`; keep-alive reuses one connection; the header, body and JSON helpers; the timeout and connection-refused panics.
- `TestApp`: the web, secured and full presets; a JWT bearer request accepted and rejected using `ManualClock`; `start_failing` for a bad `server.port`; the shutdown guard; drop without `stop`.
- `App::handle_from` in `oxbloom-web`: body limit, client resolution, forwarded headers, equality with the served answer for the same request.
- Gate: `cargo test --workspace --all-features` (2132 only grows), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `make check-examples`, the feature-matrix runs, and the stack probes with zero growth (boot 238 KB, 239 KB with `logging`; DI 206/277/230/194 KB). No field is added to `OxbloomApplicationBuilder`.

## Documentation

Every public item has a runnable doc example (`AGENT.md`). `docs/API.md` section 4.10 grows a web-testing part, `README.md` gets a sentence, and `docs/ROADMAP-core.md` marks C10b done with its delivered list and deferred minors.

## Task outline for the plan

1. `oxbloom-web`: `App::handle_from(req, peer)`.
2. `oxbloom-test`: `TestClient`, `TestResponse` and the request builder (in-memory, `serve`, `connect`).
3. Extract the shared lifecycle from `runner.rs`; add `TestApp`, `RunningApp`, `Stopped` and the presets.
4. Migrate boot's real-HTTP tests.
5. Migrate the examples' request helpers.
6. Documentation, roadmap, final gate and stack probes.
