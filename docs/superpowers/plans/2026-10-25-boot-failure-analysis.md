# Startup failure analysis (C3b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `oxbloom-boot` a `FailureAnalyzer` that turns a `StartupError` into a short report (description plus action), with built-in analyzers for a port in use, a missing config key, bean wiring errors and security build errors, and switch the `oxbloom new` template to `OxbloomApplication`.

**Architecture:** `DiError::Creation` keeps an optional typed source so the real error survives the container. `StartupError` gains `find_cause` (walk the `source()` chain) and an attached `FailureAnalysis`. `run` computes the analysis once after a failure (builder analyzers, then `Arc<dyn FailureAnalyzer>` beans if a container exists, then built-ins), attaches it before the `failed` listeners run, and prints it according to a flag. Analyzers are small structs in `crates/oxbloom-boot/src/analyzers/`.

**Tech Stack:** Rust 2024. No new dependency outside crates already in `Cargo.lock`. Features: `oxbloom-boot` gets `security = ["dep:oxbloom-security", "oxbloom-security/di"]`; `oxbloom-config` stays optional-`di`; `oxbloom-boot` depends on `oxbloom-config` with feature `di`.

**Spec:** `docs/superpowers/specs/2026-10-25-boot-failure-analysis-design.md`

## Spec amendments decided while planning

Task 8 writes these into the spec.

1. The new field of `DiError::Creation` is `source: ErrorSource`, a public newtype around `Option<Arc<dyn Error + Send + Sync>>` (constructors `none()`, `new(err)`, `Default`; `as_error() -> Option<&dyn Error>`) whose `Debug` prints `ErrorSource(..)`, whose `PartialEq` is always true (so neither the content nor the presence of a source affects equality), and which is `Clone`. This keeps `DiError`'s derives (`Debug`, `Clone`, `PartialEq`, `Eq`) intact instead of hand-written impls. `DiError::creation_from` takes any `Error + Send + Sync + 'static`.
2. The `From<ConfigError>` and `From<SecurityBuildError>` impls call `creation_from::<ConfigError>` and `creation_from::<SecurityBuildError>`; `try_bean` and `#[bean(fallible)]` then rewrite the type name to the failing bean's type and keep the source.
3. The built-in analyzer structs are crate-private; users see only the trait and `FailureAnalysis`.
4. `oxbloom-boot` enables `oxbloom-config/di` unconditionally so `?` on a `ConfigError` works inside `try_bean` for every boot user.

## Global Constraints

- Crates touched: `crates/oxbloom-di`, `crates/oxbloom-config` (small, feature `di`), `crates/oxbloom-security` (small, feature `di`), `crates/oxbloom-boot`, `crates/oxbloom-cli`, docs. Workspace package fields via `.workspace = true`.
- `DiError` stays `#[non_exhaustive]` and keeps `Debug, Clone, PartialEq, Eq`. Equality, `Display` and the fixed texts of every variant stay as today; `Display` of `Creation` still prints type name and message only (never the source).
- Analyzer, report and `Display` text contains no configuration values, argument values or secrets. Allowed: type names, config key names, candidate and source labels, and in the port-in-use report one socket address. Analyzer text is printed as given; analyzers must not emit the block markers (documented, not enforced).
- Report text, exactly (the spec's "Built-in analyzers" section is the source of truth for every description and action string; copy them verbatim):
  - Block: a line of 27 `*`, `APPLICATION FAILED TO START`, a line of 27 `*`, a blank line, `Description:`, a blank line, the description, then (when an action exists) a blank line, `Action:`, a blank line, the action. No trailing newline in `Display`; the output target adds one line break.
  - A failure with no matching analyzer renders the same block with the `StartupError` `Display` as description and no Action section.
- Analyzer order: builder analyzers (call order), then, if a container was built, `Arc<dyn FailureAnalyzer>` beans in DI order (a failing `get_all` is ignored), then built-ins (config key, bean wiring, then `web` port, then `security`). First `Some` wins. Each analyzer call is inside `catch_unwind`; a panic writes `analyzer {name} panicked` to the output target and moves on. `find_cause` stops after 64 levels.
- Printing: `run()` prints the report only when `failure_report(true)` was set; `run_and_exit()` prints the report unless `failure_report(false)` was set, in which case it writes the bare error line as before. Never both. The report is printed as the last step before returning `Err`, after `failed` listeners and cleanup. The analysis is attached before `failed` listeners run.
- Every public item has a runnable doc example (`AGENT.md`). Brand: "Oxbloom" in prose, "oxbloom" in code. Persisted docs use plain full sentences.
- The repo is not a git repository: no commit steps. Never set `CARGO_TARGET_DIR` or `--target-dir`; `/tmp` is a RAM-backed tmpfs shared with the build target: no new target dirs. Delete any probe crate or file you create.
- Done gate: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, each also with `-p oxbloom-boot --features web`, `-p oxbloom-boot --features "web security"`, `-p oxbloom-di`, `-p oxbloom-web --features di`, `-p oxbloom-config --features di`, `-p oxbloom-security --all-features`; `cargo build -p hello`; `graphify update .`.

## Review Focus

- A failure with no matching analyzer never panics, including an empty `required_by`, empty `candidates`, empty `sources` (Task 4 tests).
- A description containing a newline does not break the surrounding block (Task 3 test; the contract is documented).
- A config failure before any container still gets a builder analyzer and a built-in analysis (Task 3 and 4).
- Cloning a cached `DiError::Creation` stays cheap: the source is an `Arc` (Task 1 test using `Arc::strong_count`).
- `failure_report(true)` on `run()` together with `run_and_exit()` does not print twice (Task 3).

---

### Task 1: `oxbloom-di`: typed source on `DiError::Creation`

**Files:**
- Modify: `crates/oxbloom-di/src/error.rs`, `crates/oxbloom-di/src/beans.rs` (`as_creation`), `crates/oxbloom-di/src/lifecycle.rs` (`as_creation`), `crates/oxbloom-di/src/lib.rs` (re-export `ErrorSource`), plus every construction site of `DiError::Creation { .. }` that the compiler reports (src and tests); `crates/oxbloom-di/tests/` (new test file `source.rs`).

**Interfaces:**
- Produces: `#[derive(Clone)] pub struct ErrorSource` (newtype over `Arc<dyn std::error::Error + Send + Sync>`) with `pub fn new(error: impl std::error::Error + Send + Sync + 'static) -> Self`, `pub fn as_error(&self) -> &(dyn std::error::Error + 'static)`, `Debug` printing `ErrorSource(..)`, `PartialEq`/`Eq` always equal. `DiError::Creation { type_name: String, message: String, source: Option<ErrorSource> }`. `DiError::creation_from<T>(source: impl std::error::Error + Send + Sync + 'static) -> DiError` (message = `source.to_string()`, type name of `T`). `impl std::error::Error for DiError` gains `source()` returning the `Creation` source. `DiError::creation::<T>(message)` sets `source: None`.
- Both `as_creation<T>` functions keep an existing `Creation` source when they rewrite the type name; other variants still become the fixed text with `source: None`.

- [ ] **Step 1: Write the failing tests** in `tests/source.rs`: `creation_from_keeps_message_and_source` (custom `MyErr`, `find` via `std::error::Error::source` and `downcast_ref::<MyErr>()`), `equality_ignores_source`, `debug_does_not_print_source_content` (marker string in the source's `Display` absent from `format!("{:?}")` and `format!("{}")`), `clone_shares_the_arc` (`Arc::strong_count` observed through a source type holding an `Arc<()>` marker), `try_bean_keeps_source_and_rewrites_type_name` (factory returns `Err(DiError::creation_from::<MyErr>(MyErr))`; resulting `Creation.type_name` is the bean type and `source` still downcasts), `bean_fallible_keeps_source` (a `#[configuration]` with `#[bean(fallible)]` returning a `Result<T, DiError>` built with `creation_from`), `post_construct_error_keeps_source`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-di --test source`. Expected: compile errors (`ErrorSource`, `creation_from` missing).
- [ ] **Step 3: Implement** `ErrorSource`, the new field, `creation_from`, `Error::source`, and the `as_creation` source preservation. Fix every construction site; sites that match with `..` keep compiling.
- [ ] **Step 4: Doc examples** for `ErrorSource` and `creation_from` (success shows `source()` downcast). Update the `DiError` variant docs. Run `cargo test -p oxbloom-di` and `-p oxbloom-web --features di`, `-p oxbloom-config --features di`, `-p oxbloom-security --all-features`, `-p oxbloom-boot`. Expected: all pass, no existing test changed except construction sites.

### Task 2: `From<ConfigError>` and `From<SecurityBuildError>` for `DiError`

**Files:**
- Modify: `crates/oxbloom-config/src/di.rs`, `crates/oxbloom-security/src/di.rs`, `crates/oxbloom-boot/Cargo.toml` (depend on `oxbloom-config` with `features = ["di"]`)
- Test: `crates/oxbloom-config/tests/` (new `di_error.rs`, gated on feature `di`), `crates/oxbloom-security/tests/` (new `di_error.rs`, gated on feature `di`)

**Interfaces:**
- Consumes: `DiError::creation_from` (Task 1).
- Produces: `impl From<ConfigError> for DiError` (feature `di`) and `impl From<SecurityBuildError> for DiError` (feature `di`), each `DiError::creation_from::<Source>(e)`.

- [ ] **Step 1: Write the failing tests**: in config, `config_error_converts_with_downcastable_source` (a `Config::from_str("")`, `get::<u16>("x")` fails, `?` inside a `try_bean` factory gives a `build()` error whose `source()` downcasts to `ConfigError` and whose `type_name` is the bean type); `message_is_value_free` (a config with the marker as an invalid value: the `Creation.message` does not contain it). In security, `security_build_error_converts` (`HttpSecurity::new().build()` error through `?` in `try_bean`; `source()` downcasts to `SecurityBuildError`; equality of the kind).
- [ ] **Step 2: Run** `cargo test -p oxbloom-config --features di --test di_error` and the security equivalent. Expected: compile error (no `From`).
- [ ] **Step 3: Implement** both impls, with docs.
- [ ] **Step 4: Doc examples** on both impls (a `try_bean` factory using `?`). Run the feature test suites named in the Done gate for config, security and boot. Expected: PASS.

### Task 3: Boot core: `FailureAnalysis`, `FailureAnalyzer`, matching, printing

**Files:**
- Create: `crates/oxbloom-boot/src/failure.rs`, `crates/oxbloom-boot/src/analyzers/mod.rs` (`pub(crate) fn builtin() -> Vec<Arc<dyn FailureAnalyzer>>`, empty for now), `crates/oxbloom-boot/tests/failure.rs`
- Modify: `crates/oxbloom-boot/src/error.rs` (`StartupError`), `crates/oxbloom-boot/src/app.rs` (builder methods, `run_with_output`, `run_and_exit`), `crates/oxbloom-boot/src/lib.rs`

**Interfaces:**
- Produces in `failure.rs`: `#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)] pub struct FailureAnalysis` with `new(description: impl Into<String>, action: impl Into<String>)`, `with_no_action(description: impl Into<String>)`, `description() -> &str`, `action() -> Option<&str>`, `Display` rendering the block (Global Constraints). `pub trait FailureAnalyzer: Send + Sync + 'static { fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis>; fn name(&self) -> &'static str }` (default `type_name::<Self>()`). Crate-private: `fn analyze_failure(builder: &[Arc<dyn FailureAnalyzer>], beans: &[Arc<dyn FailureAnalyzer>], failure: &StartupError, out: &Output) -> Option<FailureAnalysis>` (builder, then beans, then `analyzers::builtin()`; `catch_unwind` per call) and `fn report_text(failure: &StartupError) -> String` (analysis block, or the generic block).
- `StartupError`: private field `analysis: Option<FailureAnalysis>`; `pub fn find_cause<E: std::error::Error + 'static>(&self) -> Option<&E>` (walks `self.source` then its `source()` chain, max 64 levels); `pub fn analysis(&self) -> Option<&FailureAnalysis>`; `pub(crate) fn set_analysis(&mut self, FailureAnalysis)`. `Display`/`Debug` unchanged (`Debug` does not print the analysis).
- Builder: `failure_analyzer(impl FailureAnalyzer)`, `failure_report(bool)`. In `run_with_output(print_default: bool)` after `execute` fails: collect bean analyzers with `container.get_all::<dyn FailureAnalyzer>()` (ignore errors; only if a container was built), compute the analysis, `set_analysis`, then fire `failed`, run cleanup, then print (`flag.unwrap_or(print_default)`). `run()` passes `print_default = false`; `run_and_exit()` passes `true` and, when the flag is `Some(false)`, writes the bare error line itself.

- [ ] **Step 1: Write failing tests** in `tests/failure.rs` (plus unit tests in `failure.rs` for `Display` and `find_cause`): `display_with_action`, `display_without_action`, `generic_report_for_unanalyzed_failure`, `find_cause_returns_first_match`, `find_cause_none_for_absent_type`, `find_cause_stops_on_cyclic_chain` (custom error whose `source()` returns itself), `builder_analyzers_run_in_call_order`, `bean_analyzers_follow_builder_analyzers` (`provides(|a: Arc<X>| -> Arc<dyn FailureAnalyzer> { a })`), `bean_analyzer_ignored_without_container` (config failure: only builder analyzer applies; Review Focus 3), `failing_get_all_for_analyzer_beans_is_ignored`, `panicking_analyzer_is_skipped_and_named`, `failed_listener_sees_analysis`, `run_is_silent_by_default`, `run_prints_report_once_when_flag_true`, `run_and_exit_prints_report_not_bare_line` (compare the output buffer), `run_and_exit_with_flag_false_prints_bare_line`, `no_double_print` (flag true plus `run_and_exit`: exactly one block), `newline_in_description_keeps_block_structure` (single-line text keeps the markers exactly once; the doc states analyzers must not emit markers), `report_does_not_change_error_or_exit_code`. All async tests under the existing `within` helper style and `SharedBuf`/`Log`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --test failure`. Expected: compile errors.
- [ ] **Step 3: Implement** the types, `StartupError` additions, builder methods and the `run_with_output` flow. Keep `Run::cleanup` the only container shutdown point; bean analyzers are collected before cleanup shuts the container down.
- [ ] **Step 4: Doc examples** for every public item (a user analyzer registered on the builder and rendered through `run_and_exit`-style output capture, `find_cause`, `analysis`, `FailureAnalysis::new`). Run `cargo test -p oxbloom-boot` and with `--features web`. Expected: PASS.

### Task 4: Config-key and bean-wiring analyzers

**Files:**
- Create: `crates/oxbloom-boot/src/analyzers/config.rs`, `crates/oxbloom-boot/src/analyzers/beans.rs`, `crates/oxbloom-boot/tests/analyzers.rs`
- Modify: `crates/oxbloom-boot/src/analyzers/mod.rs` (register in `builtin()` in the order config, beans)

**Interfaces:**
- Consumes: `StartupError::find_cause`, `FailureAnalysis`, `FailureAnalyzer`, `ConfigError::Missing { key }`, `DiError` variants.
- Produces: crate-private `MissingConfigKeyAnalyzer` (matches `find_cause::<ConfigError>()` with `Missing { key }`) and `BeanWiringAnalyzer` (matches `find_cause::<DiError>()`: `Cycle`, `Missing`, `Ambiguous`, `Duplicate`; other variants `None`). Texts exactly as in the spec's "Built-in analyzers" 2 and 3. For `Missing` with an empty `required_by` the first sentence is "A bean of type `<requested>` was requested, but none is registered."

- [ ] **Step 1: Write failing tests**: unit tests in each module asserting exact description and action strings for `Missing` key; `Cycle` (path `A -> B -> A`), `Missing` with and without `required_by`, `Ambiguous` (candidates joined by `, `), `Duplicate` (sources joined), plus `None` for unrelated errors and for `Creation`. Review Focus 1: empty `required_by`, `candidates`, `sources` do not panic. Integration in `tests/analyzers.rs`: `missing_key_through_try_bean` (a `try_bean` factory does `config.get::<u16>("db.port")?` with a real `Config` bean; the `run_and_exit`-style captured report contains the key and the three action routes), `cycle_report`, `missing_bean_report`, `ambiguous_report`, `duplicate_report`, `user_analyzer_overrides_builtin`, `config_failure_before_container_gets_builtin_analysis` (Review Focus 3: unparsable file, `ConfigError` not `Missing`, so the generic block; and a `ConfigError::Missing` raised by `with_config` flow through `config(|b| ...)` if reachable, else covered by the try_bean case), `marker_value_never_in_report`.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --test analyzers`. Expected: FAIL.
- [ ] **Step 3: Implement** both analyzers and register them.
- [ ] **Step 4: Doc examples** are not needed for crate-private types; add a README-style table later (Task 8). Run `cargo test -p oxbloom-boot`. Expected: PASS.

### Task 5: `BindError` and the port-in-use analyzer (feature `web`)

**Files:**
- Create: `crates/oxbloom-boot/src/analyzers/port.rs`
- Modify: `crates/oxbloom-boot/src/web.rs` (public `BindError`), `crates/oxbloom-boot/src/analyzers/mod.rs`, `crates/oxbloom-boot/src/lib.rs` (re-export `BindError` under the feature), `crates/oxbloom-boot/tests/web.rs`

**Interfaces:**
- Produces: `pub struct BindError` (feature `web`) with `address() -> SocketAddr`, `kind() -> std::io::ErrorKind`; `Display` unchanged ("the web server cannot bind its listener: <OS error text>", no address or port); `Error::source()` unchanged. Crate-private `PortInUseAnalyzer` matching `find_cause::<BindError>()` with kind `AddrInUse`; description "The web server could not listen on `<address>` because the port is already in use."; action "Stop the process that uses the port, or set `server.port` to a free port (0 picks one)." Registered in `builtin()` only under `cfg(feature = "web")`, after beans.

- [ ] **Step 1: Write failing tests**: unit `addr_in_use_text_is_exact`, `other_kinds_return_none` (PermissionDenied), `bind_error_display_has_no_address` (existing assertion kept); integration in `tests/web.rs`: `port_in_use_report_shows_address` (hold a `TcpListener`, run with `server.port` set to its port and `failure_report(true)` and a captured output: the report contains `127.0.0.1:<port>` and the action; `StartupError::analysis()` is `Some`; phase `Service`), `port_report_is_the_only_place_with_the_address` (the `Display` of the error and the generic block never contain the port).
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --features web --test web`. Expected: FAIL.
- [ ] **Step 3: Implement** `BindError` (public, accessors) and the analyzer; box `BindError` inside the `ServiceError` as today.
- [ ] **Step 4: Doc examples** for `BindError` accessors. Run `cargo test -p oxbloom-boot --features web` and without the feature. Expected: PASS.

### Task 6: Security analyzer (feature `security`)

**Files:**
- Create: `crates/oxbloom-boot/src/analyzers/security.rs`, `crates/oxbloom-boot/tests/security.rs` (`#![cfg(feature = "security")]`)
- Modify: `crates/oxbloom-boot/Cargo.toml` (`security = ["dep:oxbloom-security", "oxbloom-security/di"]`, optional dependency `oxbloom-security` by path), `crates/oxbloom-boot/src/analyzers/mod.rs`

**Interfaces:**
- Consumes: `SecurityBuildError` (exhaustive `match`, no wildcard), `From<SecurityBuildError> for DiError` (Task 2).
- Produces: crate-private `SecurityAnalyzer` with the five descriptions and actions from the spec's "Built-in analyzers" 4, registered last in `builtin()` under `cfg(feature = "security")`.

- [ ] **Step 1: Write failing tests**: unit tests for the exact text of each of the five variants; integration `bad_pattern_through_try_bean` (`.try_bean(|| -> Result<SecurityFilterChain, DiError> { Ok(HttpSecurity::new().authorize(|a| a.path("/users/{id}").authenticated()).build()?) })`; the report contains the Ant-syntax explanation and not the pattern; phase `Container`), `missing_authorize_section_report`, `no_security_feature_means_generic_report` is covered by building without the feature in the gate.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --features security --test security`. Expected: FAIL.
- [ ] **Step 3: Implement** the feature, the analyzer and the registration.
- [ ] **Step 4: Run** the feature matrix: `-p oxbloom-boot`, `--features web`, `--features security`, `--features "web security"`; clippy for each. Expected: PASS.

### Task 7: CLI template

**Files:**
- Modify: `crates/oxbloom-cli/src/templates.rs` (MAIN_RS, Cargo manifest template, tests), `crates/oxbloom-cli/src/cli.rs` (tests), `crates/oxbloom-cli/tests/new_project.rs` (drop the `CARGO_TARGET_DIR` override in the ignored test)

**Interfaces:**
- Produces: a generated project whose `src/main.rs` uses `OxbloomApplication::builder()` with a `WebServer` (registered with `try_bean` + `provides`, as in `examples/hello`) and `run_and_exit` returning `ExitCode`, whose `Cargo.toml` depends on `oxbloom-boot` with `features = ["web"]` (same dependency style as the existing `oxbloom-web` entry) and `oxbloom-di`/`oxbloom-config` as needed, and whose `src/lib.rs` still exports `pub fn app() -> App` (the generated test calls it).

- [ ] **Step 1: Update the failing unit tests** in `templates.rs`/`cli.rs`: the manifest contains `oxbloom-boot` with the `web` feature and a pinned tokio with the features `main()` needs; `main.rs` contains `OxbloomApplication::builder()` and `run_and_exit`, not `app().run(`; `lib.rs` still has `pub fn app`. Run `cargo test -p oxbloom-cli`. Expected: FAIL.
- [ ] **Step 2: Implement** the template changes.
- [ ] **Step 3: Verify the generated project once for real**: generate it with the built `oxbloom` binary into a directory under the scratchpad, run `cargo check` and `cargo test` inside it without setting any target-directory variable (the user-level cargo config already points at the shared target directory), then run its binary with `--server.port=0` for two seconds, send SIGINT, and report the real output and exit code. Delete the generated project afterwards.
- [ ] **Step 4: Run** `cargo test -p oxbloom-cli` (the ignored slow test stays ignored). Expected: PASS.

### Task 8: Documentation, spec amendments and final gate

**Files:**
- Modify: `examples/hello/**` (Cargo.toml, src, `application.toml`, `application-dev.toml`, tests), `README.md` (section "Application lifecycle": failure report, analyzers, `find_cause`, flag, typed sources), `docs/API.md` (one entry with an example per new public item: `FailureAnalysis`, `FailureAnalyzer`, `StartupError::find_cause`/`analysis`, builder `failure_analyzer`/`failure_report`, `ErrorSource`, `DiError::creation_from`, both `From` impls, `BindError`), `AGENT.md` (crate table: `oxbloom-boot` features `web`, `security`), `docs/ROADMAP-core.md` (C3 done; remaining follow-ups), `docs/superpowers/specs/2026-10-25-boot-failure-analysis-design.md` (append an "Amendments" section with the four amendments above in plain prose).

- [ ] **Step 1: Docs.** Add a table of the built-in analyzers (what each matches and says). State the limits: analyzers do not see failures after startup; the port-in-use report shows the address; the bad-pattern report cannot show the pattern; analyzers must not emit the block markers; `Creation.source` is not part of equality.
- [ ] **Step 1b: Update the examples to the current features** (user request). Rework `examples/hello` into a small application that shows what exists today, keeping it runnable with `cargo run -p hello` and covered by at least one test (an in-process test in `examples/hello/tests/` driving `OxbloomApplication` with a `ShutdownHandle` and port 0): a `#[configuration]` with `#[bean]` methods, trait injection with `Vec<Arc<dyn ..>>` plus `primary`/`order`, a lazy bean, a `Lifecycle` hook with `pre_destroy`, config from an `application.toml` with a profile file (`application-dev.toml`) and `server.port`, an `ApplicationRunner`, a `RunListener`, an `ExitCodeGenerator`, the `WebServer`, a security filter chain from `oxbloom-security` (static JWT key or a simple authenticated route, feature `security` of boot is not needed for the example), and a custom `FailureAnalyzer`. Also refresh the code snippets in `README.md` and `docs/API.md` that still show pre-DI-v2/pre-boot style so every snippet matches the current API. Keep the example under about 200 lines per file, with comments that say which feature each part shows. Update `AGENT.md`'s example row.
- [ ] **Step 2: Verify examples**: compile every new `docs/API.md` snippet once in a temporary test file (delete it), as in earlier slices.
- [ ] **Step 3: Final gate** per Global Constraints, plus `tests/signal.rs` and `tests/web.rs` 5 runs each, then `graphify update .`. Report real counts.
