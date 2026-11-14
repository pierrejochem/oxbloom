# C10a: Test support for application contexts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new crate `oxbloom-test` offers `ContextRunner` (real boot lifecycle to ready, inspect, shut down), `Captured`, `TempConfigDir`, `FixedClock` and `ManualClock`; a `Clock` trait in `oxbloom-core` reaches JWT `exp`/`nbf` validation and the JSON log timestamp; existing integration-test helpers move onto the crate.

**Architecture:** `Clock` is one method (`now() -> SystemTime`). The JWT decoder checks `exp` and `nbf` itself only when a clock was set; without one it behaves exactly as today. `ContextRunner` wraps `OxbloomApplicationBuilder`, runs the check inside a `RunListener::ready` callback (boot isolates listener panics, so the runner catches and re-throws them after shutdown), and triggers shutdown from the same callback.

**Tech Stack:** Rust, `oxbloom-boot`, `oxbloom-core`, `oxbloom-config`, `oxbloom-di`, `oxbloom-security`, `oxbloom-logging`, `tokio`, `jsonwebtoken`, `tempfile`.

**Spec:** `docs/superpowers/specs/2026-11-08-test-support-context-design.md`. The repo is not a git repo: there are no commits. Where a skill says "commit", run the task's verification commands instead.

## Global Constraints

- `Clock` has exactly one method, `fn now(&self) -> SystemTime`, and `Clock: Send + Sync + 'static`. `SystemClock` is the default everywhere. There is no `instant()`.
- JWT with no clock set behaves exactly as today. With a clock set, `validate_exp` and `validate_nbf` are off and the decoder checks both against `clock.now()` after the issuer and audience checks; failure reasons are `FailureReason::Expired`, `FailureReason::NotYetValid`, and `FailureReason::Malformed` for a non-numeric claim.
- `jwt_decoder_from_config(config, fetcher)` keeps its signature. The new function is `jwt_decoder_from_config_with_clock(config, fetcher, clock: Option<Arc<dyn Clock>>)`.
- No field is added to `OxbloomApplicationBuilder` itself. The only new boot state is a field in the boxed `Logging` struct (`crates/oxbloom-boot/src/app.rs`). No clock auto-configuration is added.
- `ContextRunner` is hermetic: `env_from([])`, banner off, logging off, output captured, `server.port = 0`, failure report on. Shutdown waits at most 10 seconds.
- `ContextRunner::property` is a command-line style override, so `security.jwt.secret` is refused through it, as in production.
- Test count never drops. Last gate: `cargo test --workspace --all-features` = 2029 passed, 0 failed, 8 ignored.
- Stack probes must not grow: boot 238 KB default, 239 KB with `logging`, `build()` frame `sub $0x178,%rsp`, DI 206/277/230/194 KB. Probe commands: `OXBLOOM_BOOT_STACK_KB=238 cargo test -p oxbloom-boot --test conditions_report -- --ignored probe` (and `--features logging` with 239), `OXBLOOM_DI_STACK_KB=<kb> cargo test -p oxbloom-di --test depth -- --ignored probe`.
- In-`src` unit tests keep their local `Buf`/`Log` helpers; only integration tests under `tests/` migrate to `oxbloom-test`.
- Every public item has a runnable doc example (`AGENT.md`). Docs, comments and test names are plain full sentences. "Oxbloom" in prose, `oxbloom` in code.
- Never set `CARGO_TARGET_DIR` or `--target-dir`. Run `graphify update .` after code changes. Use literal absolute paths with `rm`.

## Review Focus

- A failed assertion inside the `run` check must fail the test, not pass silently because boot swallows listener panics. Pinned in Task 4.
- A check that panics while a web server is running must still shut down and re-throw within the 10 second bound, never hang. Pinned in Task 4.
- A process environment variable such as `SERVER__PORT` or `OXBLOOM_PROFILES_ACTIVE` must not change a runner's configuration. Pinned in Task 4.
- A token with a non-numeric `exp` or `nbf`, a float `exp`, or a clock earlier than the Unix epoch must not panic and must reject or accept as `jsonwebtoken` does. Pinned in Task 2.
- A `ManualClock` clone must see `advance` made through another clone, and `TempConfigDir` must not fail when its directory vanished before drop. Pinned in Task 3.

---

### Task 1: `Clock` and `SystemClock` in `oxbloom-core`

**Files:**
- Create: `crates/oxbloom-core/src/clock.rs`
- Modify: `crates/oxbloom-core/src/lib.rs`

**Interfaces:**
- Produces: `oxbloom_core::Clock` (`fn now(&self) -> SystemTime`), `oxbloom_core::SystemClock` (unit struct, `Clone + Copy + Default + Debug`, implements `Clock`). `impl<C: Clock + ?Sized> Clock for Arc<C>` so `Arc<dyn Clock>` can be passed where `impl Clock` is wanted.

- [ ] **Step 1: Write the failing tests** in `crates/oxbloom-core/src/clock.rs`: `system_clock_reads_the_system_time` (the value lies between two `SystemTime::now()` reads), `an_arc_of_a_clock_is_a_clock` (an `Arc<dyn Clock>` and an `Arc<SystemClock>` both answer `now()`).
- [ ] **Step 2: Run** `cargo test -p oxbloom-core clock`. Expected: FAIL, `Clock` not defined.
- [ ] **Step 3: Implement** the trait, `SystemClock` and the `Arc` impl in `clock.rs`; export both from `lib.rs`. Doc examples: one on `Clock` implementing a custom clock, one on `SystemClock`.
- [ ] **Step 4: Run** `cargo test -p oxbloom-core` and `cargo test -p oxbloom-core --doc`. Expected: PASS.
- [ ] **Step 5: Run** `cargo clippy -p oxbloom-core --all-targets -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 2: Wire the clock into JWT validation, the JSON log timestamp and boot

**Files:**
- Modify: `crates/oxbloom-security/src/jwt.rs` (builder, `DefaultJwtDecoder`, `verify`), `crates/oxbloom-security/src/config.rs`, `crates/oxbloom-security/src/lib.rs` (export)
- Modify: `crates/oxbloom-logging/Cargo.toml` (dependency `oxbloom-core`), `crates/oxbloom-logging/src/json.rs`, `crates/oxbloom-logging/src/system.rs`
- Modify: `crates/oxbloom-boot/Cargo.toml` (dependency `oxbloom-core`), `crates/oxbloom-boot/src/autoconfigure/jwt.rs`, `crates/oxbloom-boot/src/app.rs` (`Logging` struct, `logging_clock`, the `init_with_writer` call near line 1200)
- Test: `crates/oxbloom-security/src/jwt.rs` (tests module), `crates/oxbloom-logging/tests/` (new `clock.rs`), `crates/oxbloom-boot/tests/` (new `clock_wiring.rs`)

**Interfaces:**
- Consumes: `oxbloom_core::Clock` from Task 1.
- Produces:
  - `JwtDecoderBuilder::clock(self, clock: Arc<dyn Clock>) -> Self`.
  - `oxbloom_security::jwt_decoder_from_config_with_clock(config: &Config, fetcher: Option<Arc<dyn JwksFetcher>>, clock: Option<Arc<dyn Clock>>) -> Result<DefaultJwtDecoder, ConfigError>`; `jwt_decoder_from_config` delegates with `None`.
  - `LoggingSystem::init_with_clock<W>(config: &Config, writer: W, clock: Arc<dyn Clock>) -> Result<LoggingSystem, LoggingError>` (`#[doc(hidden)]`, same `W` bounds as `init_with_writer`).
  - `OxbloomApplicationBuilder::logging_clock(self, clock: Arc<dyn Clock>) -> Self` (feature `logging`).
  - `JwtDecoderAutoConfiguration` passes an optional `Arc<dyn Clock>` bean to `jwt_decoder_from_config_with_clock`.

- [ ] **Step 1: Write the failing JWT tests** in the `jwt.rs` tests module, using a small local fixed clock (the crate cannot depend on `oxbloom-test`):
  - `without_a_clock_the_decoder_still_validates_with_the_system_time` (an existing boundary test covers it; add one that asserts a token with `exp` one hour ahead decodes with no clock set).
  - `a_clock_in_the_future_makes_a_valid_token_expired` (`exp` = real now + 60, clock = real now + 3600, skew 0 → `FailureReason::Expired`).
  - `a_clock_in_the_past_makes_a_token_not_yet_valid` (`nbf` = real now + 3600, clock = real now, skew 0 → `NotYetValid`, and with the clock moved to `nbf` it decodes).
  - `the_skew_applies_to_both_edges_with_a_clock` (`exp` equal to the clock minus the skew decodes; one second later is `Expired`; `nbf` equal to the clock plus the skew decodes; one second earlier is `NotYetValid`). Mirror the arithmetic of `jsonwebtoken-11`'s `validation.rs::validate` (read it in the cargo registry): `exp < now - leeway` expires, `nbf > now + leeway` is immature, with saturating arithmetic so large values cannot panic.
  - `a_clock_path_and_the_system_path_agree_on_the_boundary_table` (run the same table of `exp`/`nbf` offsets through a decoder with no clock and through one whose clock is a fixed clock at the current second, retrying like `at_stable_second`; the outcomes are equal).
  - `a_non_numeric_exp_or_nbf_is_malformed_with_a_clock` and `a_float_exp_is_read_like_jsonwebtoken_does_with_a_clock` (match what the no-clock decoder answers for the same token).
  - `a_clock_before_the_unix_epoch_does_not_panic` (clock at `UNIX_EPOCH - 1s` → `Expired` for any `exp`, no panic).
  - `the_config_function_accepts_a_clock` for `jwt_decoder_from_config_with_clock`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-security jwt`. Expected: FAIL, `clock` not defined.
- [ ] **Step 3: Implement the builder and decoder change.** `JwtDecoderBuilder` stores `Option<Arc<dyn Clock>>`. In `build`, when a clock is set, set `validation.validate_exp = false` and `validation.validate_nbf = false` and keep it on `DefaultJwtDecoder` together with the leeway in seconds. In `verify`, after `jsonwebtoken::decode` succeeds and when a clock is set, read `exp` and `nbf` from the claims map and apply the check described above; the claims `require_exp` setting already makes a missing `exp` fail inside `jsonwebtoken`. Keep `DefaultJwtDecoder: Clone` and its `Debug` output free of the clock value. Add `jwt_decoder_from_config_with_clock` in `config.rs`, delegate the old function, and export both from `lib.rs`.
- [ ] **Step 4: Run** `cargo test -p oxbloom-security --all-features`. Expected: PASS, with the existing boundary tests unchanged.
- [ ] **Step 5: Write the failing logging test** in `crates/oxbloom-logging/tests/clock.rs`: `json_timestamps_follow_the_clock` — build a scoped subscriber the way `tests/system_scoped.rs` does, with a fixed clock at `UNIX_EPOCH + 86_400` seconds and `logging.console.format = "json"`; the line's `timestamp` field equals the RFC 3339 text for that instant (`1970-01-02T00:00:00Z` with the fractional digits `rfc3339` prints today). Also `json_timestamps_use_the_system_time_without_a_clock` (the existing behavior, unchanged).
- [ ] **Step 6: Run** `cargo test -p oxbloom-logging --test clock`. Expected: FAIL.
- [ ] **Step 7: Implement.** `JsonFormat` holds `Arc<dyn Clock>` (default `SystemClock`) and `JsonFormat::new` keeps its call sites by taking the clock as a second argument; thread the clock through `console_layer` and `LoggingSystem::build` (a `pub(crate)` `build_with_clock`) and add `init_with_clock` next to `init_with_writer`, which delegates with `SystemClock`. File output layers are built by the same code path where they use `JsonFormat`.
- [ ] **Step 8: Run** `cargo test -p oxbloom-logging --all-features`. Expected: PASS.
- [ ] **Step 9: Write the failing boot tests** in `crates/oxbloom-boot/tests/clock_wiring.rs` (feature `security`): `the_jwt_auto_configuration_uses_a_clock_bean` — register a fixed `Arc<dyn Clock>` bean through `.configure`, configure `security.jwt` with an HS256 secret in `Config::from_str`, fetch the `dyn JwtDecoder` in a `RunListener::context_ready` snapshot, and decode a token whose `exp` is in the real future but before the fixed clock → rejected as expired. Add `without_a_clock_bean_the_decoder_uses_the_system_time`. Feature `logging`: `logging_clock_reaches_the_json_output` is not testable in-process because the subscriber is global; test instead that `logging_clock` compiles and a run with it succeeds (`logging_clock_is_accepted_by_a_run`).
- [ ] **Step 10: Run** `cargo test -p oxbloom-boot --features security --test clock_wiring`. Expected: FAIL.
- [ ] **Step 11: Implement boot.** In `autoconfigure/jwt.rs`, `jwt_decoder` takes a third parameter `clock: Option<Arc<dyn Clock>>` and calls `jwt_decoder_from_config_with_clock`. In `app.rs`, add `clock: Option<Arc<dyn Clock>>` to `Logging`, the builder method `logging_clock` (via `logging_mut`), and call `LoggingSystem::init_with_clock` when a clock is set (otherwise the existing call). Update the `JwtDecoderAutoConfiguration` doc to mention the optional `dyn Clock` bean.
- [ ] **Step 12: Run the gate for these crates:** `cargo test -p oxbloom-boot --all-features`, `cargo test -p oxbloom-boot --no-default-features`, `cargo test -p oxbloom-boot --features logging`, `cargo test -p oxbloom-boot --features web`, then both boot stack probes (238 and 239 KB, expect pass), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 3: The `oxbloom-test` crate: helpers and clocks

**Files:**
- Create: `crates/oxbloom-test/Cargo.toml`, `crates/oxbloom-test/src/lib.rs`, `crates/oxbloom-test/src/captured.rs`, `crates/oxbloom-test/src/log.rs`, `crates/oxbloom-test/src/config_dir.rs`, `crates/oxbloom-test/src/clock.rs`
- Modify: `/home/jochem/Repositories/rustspring/Cargo.toml` (add `"crates/oxbloom-test"` to `members`)

**Interfaces:**
- Consumes: `oxbloom_core::Clock`.
- Produces:
  - `Captured` (`Clone + Default + Debug`, `std::io::Write`): `new()`, `contents(&self) -> String`, `lines(&self) -> Vec<String>`, `contains(&self, needle: &str) -> bool`, `clear(&self)`.
  - `Log` (`Clone + Default`): `new()`, `push(&self, entry: impl Into<String>)`, `entries(&self) -> Vec<String>`. This is the `Log` helper from `crates/oxbloom-boot/tests/common/mod.rs`, with its same methods; read that file and keep its method names so Task 5 changes imports only.
  - `TempConfigDir`: `new() -> TempConfigDir`, `file(self, name: &str, text: &str) -> Self` (creates parent directories for `a/b.toml`), `path(&self) -> &Path`.
  - `FixedClock::at(SystemTime) -> FixedClock`; `ManualClock::at(SystemTime) -> ManualClock`, `ManualClock::advance(&self, by: Duration)`. Both `Clone` (clones share state) and implement `Clock`.
- Cargo features: `web = ["oxbloom-boot/web"]`, `security = ["oxbloom-boot/security"]`, `logging = ["oxbloom-boot/logging"]`, `default = []`. Dependencies: `oxbloom-core`, `oxbloom-boot`, `oxbloom-config`, `oxbloom-di`, `tempfile`, `tokio` (features `rt`, `time`, `macros`).

- [ ] **Step 1: Write the failing unit tests**, one module each: `captured_collects_writes_from_clones` (write through one clone, read from another, `lines` splits on `\n` without a trailing empty entry), `clear_empties_the_buffer`; `log_keeps_order`; `a_temp_config_dir_holds_its_files_and_is_removed_on_drop` (path exists with `application.toml` inside, gone after drop), `a_temp_config_dir_creates_parent_directories`, `dropping_a_temp_config_dir_whose_directory_vanished_does_not_panic` (remove the directory with `std::fs::remove_dir_all(dir.path())` first); `a_fixed_clock_never_moves`; `a_manual_clock_advances_and_clones_share_it` (advance through one clone, read through the other).
- [ ] **Step 2: Run** `cargo test -p oxbloom-test`. Expected: FAIL (crate does not compile / items missing).
- [ ] **Step 3: Implement** the crate files per the Interfaces block. `TempConfigDir` wraps `tempfile::TempDir`; `drop` ignores errors (use `TempDir`'s default drop, which already does). `ManualClock` keeps its offset in `Arc<Mutex<SystemTime>>`. Doc examples on every public item; the crate docs describe the crate as a dev-dependency.
- [ ] **Step 4: Run** `cargo test -p oxbloom-test`, `cargo test -p oxbloom-test --doc`, `cargo clippy -p oxbloom-test --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`. Expected: PASS and clean.

---

### Task 4: `ContextRunner`, `ContextView`, `FailureView`

**Files:**
- Create: `crates/oxbloom-test/src/runner.rs`, `crates/oxbloom-test/tests/runner.rs`
- Modify: `crates/oxbloom-test/src/lib.rs` (export)

**Interfaces:**
- Consumes: Task 3 types; `OxbloomApplicationBuilder` methods `args`, `config`, `with_config`, `configure`, `auto_configuration`, `auto_configurations`, `exclude`, `output`, `banner`, `failure_report`, `logging`, `listener`, `shutdown_handle`, `logging_clock` (feature `logging`), `run`; `RunListener::ready(&Container)`; `StartupError`.
- Produces:
  - `ContextRunner::new() -> ContextRunner` and `Default`.
  - Builder methods, each `self -> Self`: `property(key: impl Into<String>, value: impl Into<String>)`, `config_str(toml: impl Into<String>)`, `config_dir(path: impl Into<PathBuf>)`, `profile(name: impl Into<String>)`, `configure(f: impl FnOnce(ContainerBuilder) -> ContainerBuilder + Send + 'static)`, `auto_configuration::<A: AutoConfiguration>()`, `auto_configurations(list: AutoConfigurations)`, `exclude::<A: AutoConfiguration>()`, `clock(clock: Arc<dyn Clock>)`.
  - `async fn run(self, check: impl FnOnce(&ContextView<'_>) + Send + 'static)`.
  - `async fn run_failing(self, check: impl FnOnce(&FailureView<'_>))`.
  - `ContextView<'a>`: `get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>, DiError>`, `contains<T: ?Sized + Send + Sync + 'static>(&self) -> bool`, `config(&self) -> Arc<Config>`, `conditions(&self) -> &ConditionReport` (use the type the container returns for its report; read `Container`), `output(&self) -> &Captured`.
  - `FailureView<'a>`: `error(&self) -> &StartupError`, `output(&self) -> &Captured` (holds the printed failure report).

- [ ] **Step 1: Write the failing integration tests** in `tests/runner.rs` (all `#[tokio::test(flavor = "multi_thread")]`):
  - `run_hands_the_check_a_view_with_beans` (`configure(|b| b.instance(7u32))`, `get::<u32>()` is 7, `contains::<String>()` is false).
  - `a_property_override_reaches_the_config` (`property("demo.name", "x")`, `config().get::<String>("demo.name")` — use the `Config::get` form the other tests use).
  - `config_str_and_profiles_load` (`config_str("[demo]\nname = \"a\"\n")` plus `profile("dev")` with a `TempConfigDir`-style file `application-dev.toml` via `config_dir`; the profile value wins).
  - `a_failing_assertion_in_the_check_fails_the_test` (`#[should_panic(expected = "check failed marker")]`, the check does `panic!("check failed marker")`) — Review Focus 1.
  - `a_panicking_check_with_a_running_server_still_shuts_down_and_rethrows` (feature `web`: register a controller so `WebServer` runs on port 0; the check panics; the test wraps `run` in `tokio::time::timeout(Duration::from_secs(12), ..)` and asserts the panic payload message arrives, i.e. no hang) — Review Focus 2.
  - `the_process_environment_does_not_leak_into_the_configuration` (set `OXBLOOM_TEST_LEAK=1` and `SERVER__PORT=1`-style variables with `std::env::set_var` inside an `unsafe` block guarded by a `static Mutex`, build a runner, assert `config().get` of the corresponding key is absent) — Review Focus 3.
  - `a_secret_property_is_refused_like_on_the_command_line` (`property("security.jwt.secret", "…")` with feature `security` and `JwtDecoderAutoConfiguration` → `run_failing`, `error().to_string()` contains `security.jwt.secret`).
  - `run_failing_hands_over_the_error_and_the_report` (a duplicate bean or a missing required property that fails the container; `error()` phase is `StartupPhase::Container`; `output().contains(..)` the analyzer text).
  - `run_failing_panics_when_startup_succeeds` (`#[should_panic]`).
  - `run_panics_with_the_report_when_startup_fails` (`#[should_panic(expected = "startup failed")]`; the panic text also holds the captured output).
  - `exclude_removes_an_auto_configuration` and `a_clock_override_reaches_jwt_validation` (feature `security`: `clock(ManualClock)` at a time after the token's `exp` → `get::<dyn JwtDecoder>()` rejects the token as expired, then `advance` is irrelevant; also assert a clock before `exp` accepts).
- [ ] **Step 2: Run** `cargo test -p oxbloom-test --all-features --test runner`. Expected: FAIL (`ContextRunner` missing).
- [ ] **Step 3: Implement `ContextRunner::run`.** Build the boot builder with the hermetic defaults; the configuration closure is `|b| b.env_from([])` plus `.dir(..)` for `config_dir`/`config_str` (an owned `TempConfigDir` kept alive for the run) and `.profile(..)`; properties go through `args(["--key=value", ..])`. Register a private `Probe` listener holding `Mutex<Option<Box<dyn FnOnce(&ContextView) + Send>>>`, a `Mutex<Option<Box<dyn Any + Send>>>` for a panic payload and a clone of the `ShutdownHandle`. In `Probe::ready`, build the `ContextView`, run the check under `catch_unwind(AssertUnwindSafe(..))`, store a payload on panic, then always call `shutdown.trigger()`. Run the builder in `tokio::time::timeout(10s, ..)`; on success with a stored payload call `std::panic::resume_unwind`; on `Err(StartupError)` panic with `"startup failed: {error}\n{captured output}"`; on timeout panic with `"shutdown did not finish within 10 seconds"`. `run_failing` runs the builder the same way and passes the `Err` to `check` (panics with `"startup succeeded but a failure was expected"` on `Ok`). `clock(c)` registers `c` as an `Arc<dyn Clock>` bean through a `configure` closure (`b.instance(c.clone()).provides(..)` pattern from the boot tests) and, with the `logging` feature, calls `logging_clock`. Doc examples on every public item.
- [ ] **Step 4: Run** `cargo test -p oxbloom-test --all-features`. Expected: PASS.
- [ ] **Step 5: Run** `cargo clippy -p oxbloom-test --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 5: Migrate integration-test helpers

**Files:**
- Modify: `crates/oxbloom-boot/Cargo.toml` (dev-dependency `oxbloom-test` with the features boot's tests need), `crates/oxbloom-boot/tests/common/mod.rs`, `crates/oxbloom-boot/tests/autoconfigure_web.rs`, `crates/oxbloom-boot/tests/autoconfigure_security.rs`, plus every other file under `crates/oxbloom-boot/tests/` that uses `common::SharedBuf` or `common::Log`
- Modify: `examples/hello/Cargo.toml`, `examples/hello/tests/app.rs`, `examples/hello/tests/auto.rs`, `examples/todos/Cargo.toml`, `examples/todos/tests/logging.rs`

**Interfaces:**
- Consumes: `oxbloom_test::{Captured, Log, ContextRunner}`.
- Produces: no new public items. `common::quiet` stays in `tests/common/mod.rs` if it has no equivalent; `SharedBuf` and `Log` are removed from it.

- [ ] **Step 1: Record the baseline.** Run `cargo test --workspace --all-features` and write down the passed count (2029 expected, or higher if Tasks 1 to 4 added tests). Every later count must be at least this.
- [ ] **Step 2: Replace `SharedBuf` and `Log`.** In `common/mod.rs`, delete both types and re-export `pub use oxbloom_test::{Captured as SharedBuf, Log};` so call sites compile unchanged, then run `cargo test -p oxbloom-boot --all-features`. Expected: PASS with the same count. If the dev-dependency cycle fails to compile an integration test, stop and report it; do not work around it in `src`.
- [ ] **Step 3: Replace the aliases** in each suite file by importing `oxbloom_test::{Captured, Log}` directly, rename `SharedBuf` to `Captured`, and delete the alias lines from `common/mod.rs`. Do not rewrite assertions.
- [ ] **Step 4: Move context-only assertions** in `autoconfigure_web.rs` and `autoconfigure_security.rs` that only call `app(..)` plus a `Capture`/`Snapshot` to read beans or conditions onto `ContextRunner::run`. Keep tests that make real HTTP calls on their `serve()` and `Running` (they move in C10b). Keep a test's name and its assertions.
- [ ] **Step 5: Examples.** Replace the `SharedBuf` copies in `examples/hello/tests/{app,auto}.rs` and `examples/todos/tests/logging.rs` with `oxbloom_test::Captured` (dev-dependency in each `Cargo.toml`). Leave `crates/oxbloom-logging/tests/system_*.rs` and all in-`src` test helpers alone.
- [ ] **Step 6: Run** `cargo test --workspace --all-features` (count ≥ baseline), `make check-examples`, both boot stack probes, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `graphify update .`.

---

### Task 6: Documentation, roadmap, final gate

**Files:**
- Modify: `docs/API.md` (new "Testing" section), `README.md` (short mention), `docs/ROADMAP-core.md` (C10 split, Clock note, deferred items), `crates/oxbloom-test/src/lib.rs` (crate docs)

**Interfaces:**
- Consumes: everything from Tasks 1 to 5.
- Produces: documentation only.

- [ ] **Step 1: Write the docs.** `docs/API.md`: "Testing" section showing `ContextRunner::run`, `run_failing`, `property`/`config_str`/`TempConfigDir`, `ManualClock` with a JWT example, and the `Clock` bean convention (`provides(|c: Arc<MyClock>| -> Arc<dyn Clock> { c })`). State that `property` refuses secrets and that `ready` checks are synchronous. `README.md`: two sentences and a link. `docs/ROADMAP-core.md`: mark C10a done with its delivered list, add C10b as the open entry (TestApp, TestClient, slicing, migration of real-HTTP tests), note that the `Clock` landed in C10a and the JWT time checks run after issuer and audience checks when a clock is set, and list the deferred minors reviewers recorded.
- [ ] **Step 2: Final gate.** Run `cargo test --workspace --all-features` (count ≥ the Task 5 baseline), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`, `oxbloom-web --no-default-features`, boot `--no-default-features`, `--features web`, `--features logging`, `make check-examples`, the boot probes (238 and 239 KB) and the DI probes (206/277/230/194 KB). Record every number in the task report, and run `graphify update .`.

---

## Self-review

- **Spec coverage:** Clock and wiring (Tasks 1, 2); crate helpers and clocks (3); runner, views, errors and limits (4); migration (5); docs and roadmap (6); testing section items map to Tasks 2 to 5. The spec's deferral of C10b is in Task 6.
- **Type consistency:** `Clock::now`, `jwt_decoder_from_config_with_clock`, `LoggingSystem::init_with_clock`, `logging_clock`, `Captured`, `Log`, `TempConfigDir`, `FixedClock::at`, `ManualClock::{at, advance}`, `ContextRunner`, `ContextView`, `FailureView` are spelled the same in every task.
- **Known open point for the implementer:** `ContextView::conditions` returns whatever report type `Container` exposes; Task 4 says to read `Container` for it.
