# C4b: Web, Security and JWT Auto-configuration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `oxbloom-boot` provides auto-configurations for the JWT decoder, the security filter chain and the web server, fed by contributor beans, and every auto-configuration describes its config keys as `PropertyMetadata`.

**Architecture:** New module `crates/oxbloom-boot/src/autoconfigure/` holds three hand-written `AutoConfiguration` types behind the `security` and `web` features, plus `auto_configurations()` and `boot_properties()`. `AutoConfiguration` gets a defaulted `properties()`; the descriptor stores it so the builder can list metadata for the registered, non-excluded auto-configurations. Routes come from the existing `oxbloom_web::Controller` (as `dyn Controller`); rules come from a new `SecurityCustomizer` trait; extra filters are `dyn Filter` beans.

**Tech Stack:** Rust 1.99.1 workspace, `oxbloom-di` conditions and groups (C4a), `oxbloom-security`, `oxbloom-web`, `oxbloom-config`.

**Spec:** `docs/superpowers/specs/2026-10-25-web-security-autoconfig-design.md` (binding; the plan argues from it).

## Global Constraints

- Branding: "Oxbloom" in prose, `oxbloom` in code, crate names and commands. Never "OxBloom".
- Never set `CARGO_TARGET_DIR` or `--target-dir`; no probe crates left behind. The workspace is not a git repo: no commits; the review package is a snapshot diff.
- Every public item has a runnable doc example (AGENT.md). No `unwrap` in library code.
- `DiError` stays 56 bytes (`source_does_not_grow_di_error`). A change to `DiError`, `build`'s frame (0x178) or `Container` size re-runs the stack probes: 256-deep build, debug, minima 206 KB closures, 277 KB `#[component]`, 230 KB `#[bean]`, 194 KB `.bean`; boot probe 241 KB.
- Secrets never appear in output, metadata values or error text (`security.jwt.secret` is described, never shown).
- After code changes run `graphify update .`.
- Gate for every task: `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test -p oxbloom-boot --all-features`, plus the task's own tests. Task 6 runs the full workspace.
- Metadata key lists must equal the keys the code reads (the pin tests in Tasks 2 and 3).

## Review Focus

1. A user `SecurityFilterChain` bean registered in `configure` and the default chain: only one chain exists; the conditions report names the skipped default.
2. `security.jwt` present but broken (HMAC without secret, secret on the command line): startup fails with a report naming the key, never the secret value.
3. No `Controller` bean: no `WebServer`, no `Service`, `run()` returns right after the runners.
4. `exclude::<WebServerAutoConfiguration>()` and `exclude::<SecurityAutoConfiguration>()`: the server still builds without the chain (web only) and metadata drops the excluded keys.
5. Two controllers with the same route: startup fails with a `RouteError` source, not a panic.

---

### Task 1: `PropertyMetadata` and the metadata API

**Files:**
- Create: `crates/oxbloom-boot/src/metadata.rs`
- Modify: `crates/oxbloom-boot/src/autoconfig.rs` (trait method, `Descriptor`), `crates/oxbloom-boot/src/app.rs` (builder method), `crates/oxbloom-boot/src/lib.rs` (module, exports)
- Test: `crates/oxbloom-boot/tests/metadata.rs`

**Interfaces:**
- Consumes: `AutoConfiguration`, `AutoConfigurations`, `Descriptor`, `OxbloomApplicationBuilder::exclude`.
- Produces: `pub struct PropertyMetadata { pub key: &'static str, pub type_name: &'static str, pub default: Option<&'static str>, pub description: &'static str }` (`Clone, Debug, PartialEq, Eq`, plus a `const fn new(key, type_name, default, description)`); `AutoConfiguration::properties() -> &'static [PropertyMetadata]` defaulting to `&[]`; `pub fn boot_properties() -> &'static [PropertyMetadata]`; `PropertyMetadata::to_json(list: &[PropertyMetadata]) -> String`; `OxbloomApplicationBuilder::property_metadata(&self) -> Vec<PropertyMetadata>`. `Descriptor` gains `properties: fn() -> &'static [PropertyMetadata]`, filled by `AutoConfigurations::add`.

- [ ] **Step 1: Write the failing tests** in `tests/metadata.rs`:
  - `to_json_is_stable`: one entry with a default and one with `None`, assert the exact string `[{"key":"a.b","type":"u16","default":"8080","description":"x"},{"key":"c","type":"String","default":null,"description":"y"}]`; and `to_json(&[]) == "[]"`.
  - `to_json_escapes_quotes_backslashes_and_control_chars`: a description with `"`, `\` and a newline yields `\"`, `\\`, `\n`.
  - `builder_lists_boot_keys_sorted`: a builder with no auto-configurations returns exactly the five boot keys `oxbloom.config.import`, `oxbloom.main.banner-mode`, `oxbloom.main.conditions-report`, `oxbloom.profiles.active`, `oxbloom.profiles.group`, sorted.
  - `builder_adds_registered_and_drops_excluded`: a test `AutoConfiguration` with `properties()` returning `[x.one, x.two]`; registered it appears, after `.exclude::<T>()` it does not; a key declared by two auto-configurations appears once.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --test metadata` — fails to compile.
- [ ] **Step 3: Implement** the types in `metadata.rs` (hand-written JSON, no new dependency), the trait method, the descriptor field, `boot_properties()` with the five boot keys (type, default and description text written by you from `app.rs`/`banner.rs` usage: `banner-mode` default `console`, `conditions-report` default `off`), and `property_metadata()` (boot keys plus the non-excluded descriptors' keys, sorted by key, first declaration wins on duplicates).
- [ ] **Step 4: Doc examples** for `PropertyMetadata`, `new`, `to_json`, `boot_properties`, `properties`, `property_metadata`.
- [ ] **Step 5: Run** the gate; the new tests pass.

---

### Task 2: JWT decoder and security chain auto-configuration (feature `security`)

**Files:**
- Create: `crates/oxbloom-boot/src/autoconfigure/mod.rs`, `crates/oxbloom-boot/src/autoconfigure/jwt.rs`, `crates/oxbloom-boot/src/autoconfigure/security.rs`
- Modify: `crates/oxbloom-boot/src/lib.rs` (module, exports under `#[cfg(feature = "security")]`)
- Test: `crates/oxbloom-boot/tests/autoconfigure_security.rs` (feature `security`)

**Interfaces:**
- Consumes: Task 1's `PropertyMetadata`/`properties()`; `jwt_decoder_from_config(&Config, Option<Arc<dyn JwksFetcher>>) -> Result<DefaultJwtDecoder, ConfigError>`; `HttpSecurity`; `DiError::creation_from::<T>`; `From<ConfigError>` and `From<SecurityBuildError>` for `DiError`.
- Produces:
  - `pub trait SecurityCustomizer: Send + Sync { fn customize(&self, http: HttpSecurity) -> HttpSecurity; }`
  - `pub struct JwtDecoderAutoConfiguration;` and `pub struct SecurityAutoConfiguration;`, both `#[component]`, both `AutoConfiguration` (hand-written `Configuration` and `AutoConfiguration` impls).
  - `autoconfigure::mod` exports them; `pub fn auto_configurations() -> AutoConfigurations` (security part only for now: the two, in order).

Behavior (spec sections "Contributor beans" and "Auto-configurations"):
- `JwtDecoderAutoConfiguration`: group conditions `on_property("security.jwt")`; `try_bean` taking `Arc<Config>` and `Option<Arc<dyn JwksFetcher>>`, registered with `on_missing_bean(dyn JwtDecoder)` and `provides(dyn JwtDecoder)`. Errors: `DiError::from(ConfigError)` with typed source.
- `SecurityAutoConfiguration`: `after(JwtDecoderAutoConfiguration)`; `try_bean` taking `Vec<Arc<dyn SecurityCustomizer>>` and `Option<Arc<dyn JwtDecoder>>`, with `on_missing_bean(SecurityFilterChain)`. Customizers applied in the injected order; none registered means `authorize(|a| a.any_request().authenticated())`; a decoder adds `oauth2_resource_server(|r| r.jwt(decoder))`; `build()?`.
- `properties()` of the JWT one lists exactly the `security.jwt.*` keys that `config.rs` of `oxbloom-security` reads (read the file and its `K_*` constants; do not invent keys). The secret key's default is `None` and its description says it is never printed.

- [ ] **Step 1: Write the failing tests** (build containers with `with_config` and a `Config::from_str`, the in-process path used by `tests/security.rs`):
  - `jwt_decoder_is_built_when_section_present` and `jwt_decoder_absent_without_section`: with `[security.jwt]` (HS256 + 32-byte secret) `container.get::<dyn JwtDecoder>()` resolves; without it the report shows the group skipped.
  - `user_decoder_wins`: a user `dyn JwtDecoder` bean in `configure`; the auto bean is skipped, report says so.
  - `default_chain_requires_authentication`: no customizer, no jwt; the chain from the container rejects an unauthenticated request with 401 (via `App::new().filter(chain).route(..)` and `handle`).
  - `customizers_apply_in_order`: two customizers with `order` 1 and 2; the second sees the first's rules (assert through request outcomes: permit `/open/**` from the first, authenticated for the rest from the second).
  - `user_chain_wins`: a user `SecurityFilterChain` bean means exactly one chain, `condition_report` names `SecurityFilterChain` skipped.
  - `broken_jwt_section_fails_with_the_key`: HS256 without secret; `StartupError` phase `Container`, `analysis()` is `Some` and its description contains `security.jwt.secret`. A second case: the secret passed as `--security.jwt.secret=..` fails and the error text and the failure report must not contain the secret value (assert it is absent).
  - `metadata_keys_match_config`: every key in `JwtDecoderAutoConfiguration::properties()` is accepted by a config built from those keys with sample values, and the list contains each `K_*` key (a hand-written expected list in the test, compared as a set).
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --features security --test autoconfigure_security` — fails to compile.
- [ ] **Step 3: Implement** the module and both auto-configurations as described; export from `lib.rs`.
- [ ] **Step 4: Doc examples** for the trait, both types and `auto_configurations`.
- [ ] **Step 5: Run** the gate with `--features security`; run the `DiError` size guard test.

---

### Task 3: Web server auto-configuration and the full entry point

**Files:**
- Create: `crates/oxbloom-boot/src/autoconfigure/server.rs`
- Modify: `crates/oxbloom-boot/src/autoconfigure/mod.rs` (`auto_configurations()` for `web`, feature matrix), `crates/oxbloom-boot/src/lib.rs`; possibly `crates/oxbloom-web/src/controller.rs` (`Send + Sync` supertrait, only if `Arc<dyn Controller>` does not satisfy the DI bounds; check first and note the finding in the report)
- Test: `crates/oxbloom-boot/tests/autoconfigure_web.rs` (features `web`, `security`)

**Interfaces:**
- Consumes: Task 2 types; `WebServer::new(App, &Config) -> Result<WebServer, ConfigError>`; `App::filter`, `App::routes`; `Controller::routes(Arc<Self>) -> Vec<Route>`; `Service` registration (`provides(dyn Service)`).
- Produces: `pub struct WebServerAutoConfiguration;` (`#[component]`, `AutoConfiguration`); `auto_configurations()` returning, by enabled feature, `[JwtDecoderAutoConfiguration, SecurityAutoConfiguration]` (security), `[WebServerAutoConfiguration]` (web), or all three in that order.

Behavior: group conditions `on_bean(dyn Controller)`; `after(SecurityAutoConfiguration)` when `security` is enabled; `try_bean` with `on_missing_bean(WebServer)`, `provides(dyn Service)`, taking `Arc<Config>`, `Vec<Arc<dyn Controller>>`, `Vec<Arc<dyn Filter>>` and, only under `security`, `Option<Arc<SecurityFilterChain>>`. The `App` gets the chain first, then the filters in order, then every controller's routes; a `RouteError` becomes `DiError::creation_from::<WebServer>`. `properties()` lists `server.address` and `server.port` with the defaults that `WebServer::new` uses (read `web.rs` for them).

- [ ] **Step 1: Write the failing tests:**
  - `no_controller_no_server`: the container has no `WebServer`; the report shows `on_bean(dyn Controller)` unmatched; `run()` returns right after runners (with `with_config`, `output(sink)`).
  - `controller_builds_server_and_service`: one `dyn Controller` bean; `container.get::<WebServer>()` resolves and the `Vec<Arc<dyn Service>>` contains it.
  - `chain_comes_before_filters`: a recording filter bean and the default chain; an unauthenticated request is rejected with 401 and the recording filter did not run.
  - `filters_are_ordered`: two `dyn Filter` beans with `order` 2 and 1 run in 1, 2 order.
  - `duplicate_route_fails_startup`: two controllers with `GET /same`; `StartupError` phase `Container`, `find_cause::<RouteError>()` is `Some`.
  - `bad_port_fails_with_the_key`: `server.port = "x"`; the report names `server.port`.
  - `user_server_wins`: a user `WebServer` bean; the auto bean is skipped.
  - `exclude_chain_keeps_server`: `.exclude::<SecurityAutoConfiguration>()` with a controller; the server builds and serves without a chain (a request that the default chain would reject now succeeds); metadata no longer contains `security.jwt.*` keys only when `JwtDecoderAutoConfiguration` is excluded too.
  - `metadata_keys_match_server`: `properties()` equals the set `{server.address, server.port}`, and a config with each key set changes the bound address.
  - `auto_configurations_order_by_feature`: `len()` is 3 with both features; the web one sorts after the security one.
- [ ] **Step 2: Run** `cargo test -p oxbloom-boot --features web,security --test autoconfigure_web` — fails to compile.
- [ ] **Step 3: Implement** `server.rs` and the feature-dependent `auto_configurations()`; handle the `Controller` bound finding.
- [ ] **Step 4: Doc examples** for `WebServerAutoConfiguration` and the feature matrix of `auto_configurations()`.
- [ ] **Step 5: Run** the gate with `--features web`, `--features security` and `--all-features` (all three build and pass), and the boot stack probe (target 241 KB) because the builder code grew; report the number.

---

### Task 4: End-to-end HTTP behavior

**Files:**
- Create: `crates/oxbloom-boot/tests/autoconfigure_http.rs` (features `web`, `security`)
- Modify: none in `src`, unless a defect is found

**Interfaces:**
- Consumes: Tasks 2 and 3.
- Produces: nothing new; this task proves the whole stack over a real socket.

- [ ] **Step 1: Write the tests** using `server.port = 0`, a `RunListener` that reports the bound address, and a `ShutdownHandle` to stop (copy the pattern of `examples/hello/tests/app.rs`, not its code):
  - `default_chain_answers_401`: a protected path without a token gets 401.
  - `valid_jwt_gets_through`: HS256 token signed with the configured secret reaches the handler and `req.authentication()` carries the subject.
  - `invalid_jwt_gets_401`: a token signed with another key is rejected.
  - `public_path_via_customizer`: a `SecurityCustomizer` that permits `/open/**`.
  - `user_chain_overrides_default`: a user chain that permits everything; a plain request succeeds.
  - `conditions_report_shows_skips`: `oxbloom.main.conditions-report=console` output names the skipped defaults when the user supplies replacements.
- [ ] **Step 2: Run** the file; fix any defect in the earlier tasks' source (report it as a finding of this task).
- [ ] **Step 3: Run** the gate with `--all-features`; `tests/signal.rs` and `tests/web.rs` still pass.

---

### Task 5: Migrate `examples/hello`

**Files:**
- Modify: `examples/hello/src/web.rs` (remove `security_chain`, `web_server`; add `PublicPaths`), `examples/hello/src/lib.rs`, `examples/hello/Cargo.toml` if feature flags change, `examples/hello/tests/app.rs` only if a test names removed functions
- Test: existing `examples/hello/tests/*` unchanged in behavior

**Interfaces:**
- Consumes: Tasks 2 and 3 (`SecurityCustomizer`, `auto_configurations()`, `dyn Controller`).
- Produces: nothing for later tasks.

- [ ] **Step 1: Run** the current hello tests to record the baseline pass count.
- [ ] **Step 2: Implement** the migration of the spec: `PublicPaths` (`SecurityCustomizer`, public `/api/hello/**` and `/api/greetings/**`, everything else authenticated; registered with `provides(dyn SecurityCustomizer)`), `HelloController` registered `provides(dyn Controller)`, `.auto_configurations(oxbloom_boot::auto_configurations())` replacing the two `try_bean` lines, doc comments updated ("Feature:" lines), and `hello.custom-greeter` plus `GuestAutoConfig` untouched.
- [ ] **Step 3: Run** `cargo test -p hello` and compare with the baseline: the same tests, all passing, none edited to pass (a test may change only where it called a removed function; say which). Run `make run-hello ARGS="--server.port=0"` for a few seconds and show the startup output.
- [ ] **Step 4: Add** one hello test `console_report_lists_web_auto_configurations`: with `oxbloom.main.conditions-report=console` the report contains the web server group as matched.
- [ ] **Step 5: Run** the gate.

---

### Task 6: Documentation, probes, final gate

**Files:**
- Modify: `README.md` (section "Auto-configured web and security" under "Conditions and auto-configuration"), `docs/API.md`, `AGENT.md`, `docs/ROADMAP-core.md` (C4b done, delivered list, follow-ups from this slice), the spec's Amendments section (append amendments for any change decided during execution, e.g. the `Controller` supertrait)
- Test: workspace

- [ ] **Step 1: Write the docs** in plain prose: how to contribute routes, rules and filters; how to replace a default; the secure-by-default chain; metadata API and JSON dump; exclusion. List these follow-ups in the roadmap: macro option for `properties`, metadata checked against structs, C6 `server.*` extras, registry feature.
- [ ] **Step 2: Run** the stack probes (256-deep build: closures, `#[component]`, `#[bean]`, `.bean`, and the boot probe); report every number; any rise over the Global Constraints minima is a finding to explain or fix. Confirm `build`'s frame is still 0x178.
- [ ] **Step 3: Run** `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features`, `cargo test --workspace --doc --all-features`, `cargo build -p hello`, and `make check-examples`; report counts honestly.
- [ ] **Step 4: Run** `graphify update .`.
