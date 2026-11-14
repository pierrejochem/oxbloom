# C10a: Test support for application contexts (`oxbloom-test`)

Date: 2026-11-08. Status: draft for review. Roadmap entry: C10, first half.

## Purpose

Oxbloom users, and the Oxbloom test suites themselves, need a small API for testing an application context without a hand-built harness. Today each suite rebuilds one. For example, `crates/oxbloom-boot/tests/autoconfigure_web.rs` has its own `app()`, `Capture`, `Snapshot`, `serve()` and `Running`, and `crates/oxbloom-boot/tests/common/mod.rs` holds `SharedBuf` and `Log`.

Success means: a new crate `oxbloom-test` offers a `ContextRunner` that runs the real boot lifecycle to the ready state, lets the test inspect beans, configuration, conditions and captured output, and then shuts down cleanly; startup failures can be asserted; wall-clock and monotonic time can be controlled; and the existing integration-test helpers move onto the crate with no loss of tests and no growth of the stack probes.

## Scope

C10 is split in two, as C5 and C6 were.

- **C10a (this spec):** `Clock` in `oxbloom-core` and its wiring, the `oxbloom-test` crate with `ContextRunner`, `ContextView`, `Captured`, `TempConfigDir`, `FixedClock` and `ManualClock`, and the migration of integration-test helpers.
- **C10b (later):** `TestApp`/`TestClient` (in-memory call through filters and router, real server on port 0), sliced helpers for web, config and security, and migration of the real-HTTP tests that keep their own `serve()` until then.

The roadmap listed a fixed clock under C10. The framework has no clock abstraction today, so C10a adds one (decision of the user, 2026-11-08).

## Clock

### `oxbloom-core::clock`

```rust
pub trait Clock: Send + Sync + 'static {
    /// Wall-clock time.
    fn now(&self) -> SystemTime;
}

pub struct SystemClock; // now() = SystemTime::now()
```

`SystemClock` is the default everywhere. Every public item has a runnable doc example.

The trait has no monotonic reading. Reading the call sites showed that the JWKS cache uses `tokio::time::Instant`, which tests already control with tokio's paused time (`tokio::time::pause` and `advance`, as `crates/oxbloom-security/src/jwks/tests.rs` does). The remaining `Instant::now()` reads only measure elapsed time for reports.

### Wiring

Reading the call sites also showed that the two `SystemTime::now()` hits in `crates/oxbloom-security/src/jwt.rs` and `config.rs` are inside test modules. In production, `jsonwebtoken` reads the system time itself while it validates `exp` and `nbf`. The wall-clock reads a clock must reach are therefore:

- The `exp` and `nbf` validation in `DefaultJwtDecoder` (`crates/oxbloom-security/src/jwt.rs`).
- The JSON log timestamp in `crates/oxbloom-logging/src/json.rs`.

Everything else stays on the real clock.

**JWT.** `JwtDecoderBuilder` gets `clock(Arc<dyn Clock>)`. With no clock set, the decoder behaves exactly as today (`jsonwebtoken` validates `exp` and `nbf`). With a clock set, the decoder turns `validate_exp` and `validate_nbf` off and checks both claims itself against `clock.now()`, using the same arithmetic and the same `FailureReason` values (`Expired`, `NotYetValid`, `Malformed` for a non-numeric claim) as `jsonwebtoken`. One difference is accepted and documented: with a clock set, the time checks run after the issuer and audience checks, so a token that fails several checks may report a different reason. A parity test runs the existing boundary table through both paths. `jwt_decoder_from_config` keeps its signature and delegates to a new `jwt_decoder_from_config_with_clock(config, fetcher, clock: Option<Arc<dyn Clock>>)`.

**Logging.** `LoggingSystem` gets a `#[doc(hidden)]` `init_with_clock(config, writer, clock)` next to `init_with_writer`. `JsonFormat` holds the clock and uses it for the timestamp. `oxbloom-logging` gains a dependency on `oxbloom-core`.

**Boot.** There is no clock auto-configuration and no general builder method. A clock is an ordinary `Arc<dyn Clock>` bean that the application registers. `JwtDecoderAutoConfiguration` takes it as an optional bean, as it already does for `JwksFetcher`. With the `logging` feature, `OxbloomApplicationBuilder::logging_clock(Arc<dyn Clock>)` stores the clock in the lazily created, boxed `Logging` struct, because logging starts before the container exists. Boot gains a dependency on `oxbloom-core`.

Zero-growth rule: no field is added to `OxbloomApplicationBuilder` itself, only to the boxed `Logging` struct. The stack probes (boot 238 KB without features, 239 KB with `logging`; DI 206/277/230/194 KB) must not grow, and each task that touches boot re-runs them.

## The `oxbloom-test` crate

A new workspace crate, intended as a dev-dependency. It depends on `oxbloom-boot`, `oxbloom-core` and `oxbloom-config`. Features follow boot's (`web`, `logging`, `security`).

### `ContextRunner`

A builder that wraps `OxbloomApplicationBuilder`.

Configuration methods mirror boot: `property(key, value)`, `config_str(toml)`, `config_dir(path)`, `profile(name)`, `configure(|builder| ..)`, `auto_configuration::<A>()`, `auto_configurations(list)`, `exclude::<A>()` and `clock(Arc<dyn Clock>)`.

- `property` is a command-line style override (`--key=value`), so `security.jwt.secret` is refused there as in production; secrets go through `config_str` or a `TempConfigDir`.
- `config_str` writes an `application.toml` into a temporary directory owned by the runner and removed after the run.
- The configuration is hermetic: `env_from([])`, so the process environment never leaks into a test.
- `clock` registers the clock as an `Arc<dyn Clock>` bean and, with the `logging` feature, also calls `logging_clock`.

Test-safe defaults: banner off, logging off, output captured, `server.port = 0`, failure report on. `server.port = 0` is passed as a command-line style argument, so it also overrides a port set in `config_str` or a config file; only `property` can change it. Signal handling cannot be turned off: boot installs Ctrl-C and SIGTERM handlers whenever a service runs, and disabling them would need a new builder field, so the runner documents the limit instead.

`async fn run(self, check: impl FnOnce(&ContextView))`: runs the real boot lifecycle to the ready event, calls `check` with a `ContextView`, then triggers shutdown and waits for the exit status. The check runs inside the `ready` listener callback. Boot isolates listener panics, so the runner catches a panic from `check`, stores the payload, and resumes it with `resume_unwind` after shutdown completes. No change to boot's listener behavior is needed.

`async fn run_failing(self, check: impl FnOnce(&FailureView))`: expects startup to fail. `FailureView` offers the `StartupError` (`error()`) and the captured output (`output()`), which holds the rendered failure-analyzer report; there is no separate report method. It fails the test if startup succeeded.

Both methods are `async` and use the caller's tokio runtime.

### `ContextView`

- `get::<T>() -> Result<Arc<T>, DiError>`, `contains::<T>() -> bool`.
- The condition report, the loaded `Config`, and the captured output (`Captured`).
- No unwrapping helpers that hide the cause of a failure.

### `Captured`

A cloneable in-memory writer that implements `Write` and can be read back: `contents()`, `lines()`, `contains(&str)`. It is the promoted form of `SharedBuf` from `crates/oxbloom-boot/tests/common/mod.rs`.

### `TempConfigDir`

`TempConfigDir::new()`, `.file(name, text)`, `.path()`. It removes only the directory it created, on drop, and ignores removal errors.

### `FixedClock` and `ManualClock`

`FixedClock::at(SystemTime)` never moves. `ManualClock::at(SystemTime)` starts there, and `advance(Duration)` moves it forward. Both are `Clone` and share their state between clones.

## Errors and limits

- `run` panics with a clear message when startup fails but was expected to succeed. The message includes the failure report and the captured output.
- `run_failing` panics when startup succeeds.
- A panic in `check` is re-thrown with its original payload after a clean shutdown.
- The whole run, startup and shutdown, is bounded at 10 seconds and never hangs. Boot reports a failed shutdown (a service that panics or misses its stop deadline, a panicking `pre_destroy`) only as a line of output, not as an error. The runner therefore sets boot's shutdown timeout to 5 seconds and panics with the captured output when it finds one of those lines. The match is on boot's wording, and tests pin that wording. `run_failing` does not check for these lines.

## Migration of existing helpers

Cargo allows a dev-dependency cycle (`oxbloom-boot` dev-depends on `oxbloom-test`, which depends on `oxbloom-boot`) only for integration tests under `tests/`, where the library is linked once. Unit tests inside `src/` would see two builds of the crate, so they keep their local `Buf` and `Log` helpers.

- `crates/oxbloom-boot/tests/common/mod.rs`: `SharedBuf`, `Log` and `quiet` move onto `Captured` and a `Log` helper in `oxbloom-test`.
- `autoconfigure_web.rs`, `autoconfigure_security.rs`: context-only assertions move to `ContextRunner::run`. Tests that make real HTTP calls keep their `serve()` and `Running` until C10b.
- `examples/hello/tests/{app,auto}.rs` and `examples/todos/tests/logging.rs`: replace their `SharedBuf` copies with `Captured`.
- `crates/oxbloom-logging/tests/system_*.rs` are left alone. Logging does not depend on boot, and a dev-dependency on `oxbloom-test` would pull boot in for a tiny buffer.

Rule: migrate by moving helpers, never by rewriting assertions. The test count must not drop.

## Testing the crate

- Unit tests for `Captured`, `TempConfigDir`, `FixedClock` and `ManualClock`.
- Integration tests for the runner: success, `run_failing` with a failure analyzer, panic propagation, property and profile overrides, `exclude`, a conditional bean, and a clock override reaching JWT validation. The runner keeps logging off, so the clock reaching the JSON log timestamp is tested in `oxbloom-logging` and `oxbloom-boot` (Task 2) instead.
- Gate: `cargo test --workspace --all-features` (the 2029 count only grows), `cargo clippy -D warnings`, `cargo fmt --check`, `make check-examples`, the `oxbloom-web` and boot feature-matrix runs, and the stack probes with zero growth.

## Documentation

- Every public item has a runnable doc example.
- `docs/API.md` gets a "Testing" section and `README.md` a short mention.
- `docs/ROADMAP-core.md` splits C10 into C10a (done) and C10b, and records the added `Clock`.

## Task outline for the plan

1. `Clock` and `SystemClock` in core.
2. Wire the clock into security (JWT) and the logging JSON timestamp, and into boot (`JwtDecoderAutoConfiguration`, `logging_clock`).
3. The `oxbloom-test` crate: `Captured`, `TempConfigDir`, `FixedClock`, `ManualClock`.
4. `ContextRunner`: `run`, `run_failing`, `ContextView`, `FailureView`.
5. Migrate boot, hello and todos test helpers.
6. Documentation, roadmap, final gate and stack probes.

## Deferred to C10b

`TestApp`, `TestClient`, web, config and security slicing, and migration of the real-HTTP tests.
