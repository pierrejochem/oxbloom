# C5b File Output, JSON Output and Request Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an Oxbloom application write rotating log files, emit JSON log lines with configurable fields, and log HTTP requests without leaking credentials.

**Architecture:** `oxbloom-logging` parses the new `logging.file.*` and `logging.json.*` keys into `Settings`, adds a custom JSON `FormatEvent` and a `tracing-appender` file layer next to the console layer, both behind the existing reloadable level filter. `oxbloom-web` gets a `RequestLogFilter`. `oxbloom-boot` registers it as a `dyn Filter` bean through a new auto-configuration.

**Tech Stack:** Rust (pinned by `rust-toolchain.toml`), `tracing`, `tracing-subscriber 0.3.23`, `tracing-appender` (new), `serde_json`, `http`.

**Spec:** `docs/superpowers/specs/2026-11-01-logging-output-design.md`.

## Global Constraints

- The repo is not a git repository. There are no commits; each task ends with a passing gate. The review package is a snapshot diff.
- Never set `CARGO_TARGET_DIR` or `--target-dir`; `~/.cargo/config.toml` already points cargo at `/tmp/cargo-target`.
- Prose says "Oxbloom"; code, crates and commands say `oxbloom`.
- AGENT.md: every public item has a runnable doc example.
- Run `graphify update .` after code changes.
- Task gate: `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo fmt --check`. Baseline: 1563 tests passed, 0 failed, 8 ignored (82 suites); clippy and fmt clean.
- Error messages for invalid logging values name the key and never contain the value. Analyzer text is fixed and never echoes a value.
- Stack probes must not rise: DI 206/277/230/194 KB, boot 241 KB (238 KB without `logging`, 239 KB with), `build()` frame `sub $0x178,%rsp`.
- Key matching is case-insensitive and ignores `-` and `_` (existing rule).
- File output is never colored. `max-history` of 0 is invalid. `oxbloom-logging` gets no HTTP dependency.
- Spec decisions: rotation is `hourly` | `daily` | `never` only; JSON fields are `timestamp`, `level`, `target`, `message`, `span`, `thread` with boolean toggles (`logging.json.fields.<field>`) and renames (`logging.json.names.<field>`).

## Review Focus

- Two JSON fields renamed to the same name, or a rename to an empty string: startup fails naming `logging.json.names.<field>`. Pinned in Task 1.
- `logging.file.enabled = true` with an unwritable or non-directory `logging.file.path`: startup fails with a path-carrying error, not a panic and not a silent no-op. Pinned in Task 3 and Task 5.
- A process that exits right after logging: lines in the non-blocking buffer are on disk after `LoggingSystem` drops. Pinned in Task 3.
- A credential in a custom header (`X-Api-Key`, any case) that is also on the `logging.request.headers` allowlist: it prints `<redacted>`. Pinned in Task 4.
- A request URI with a query string such as `?token=abc`: the logged path has no query. Pinned in Task 4.
- Message text with quotes, newlines and control characters in JSON output: each event is one valid JSON line. Pinned in Task 2.
- A request rejected by the security chain (401/403): the chain runs before `dyn Filter` beans, so the request log does not see it. This is current behavior (filter ordering is C6). Pinned as a documented test in Task 5; report it in the final review.

---

### Task 1: Settings for file, JSON and request keys

**Files:**
- Modify: `crates/oxbloom-logging/Cargo.toml`, `crates/oxbloom-logging/src/settings.rs`, `crates/oxbloom-logging/src/error.rs`, `crates/oxbloom-logging/src/lib.rs`
- Test: `crates/oxbloom-logging/tests/settings.rs`

**Interfaces:**
- Consumes: existing `Settings::from_config(&Config) -> Result<Settings, LoggingError>`, `ConsoleFormat`, `ConfigError::invalid(key, rule)`.
- Produces (public, re-exported from `lib.rs`): `FileRotation { Hourly, Daily, Never }`, `FileFormat { Full, Compact, Json }`, `JsonField { Timestamp, Level, Target, Message, Span, Thread }`, new variant `ConsoleFormat::Json`, new variant `LoggingError::Io { path: PathBuf, source: std::io::Error }` (message names the path, `source()` is the I/O error). Produces (crate-internal): `FileOptions { dir: PathBuf, name: String, rotation: FileRotation, max_history: Option<usize>, format: FileFormat }`, `JsonOptions` with `fn is_on(&self, JsonField) -> bool` and `fn name(&self, JsonField) -> &str`, `Settings::file(&self) -> Option<&FileOptions>` (`None` unless `logging.file.enabled`), `Settings::json(&self) -> &JsonOptions`. `__private::Settings` exposes the same accessors for integration tests. `PROPERTIES` grows by the 10 `logging.file.*` and `logging.json.*` rows from the spec (not `logging.request.*`; those belong to Task 5).

- [ ] **Step 1: Write failing tests** in `tests/settings.rs`: `file_defaults_when_enabled` (`[logging.file]\nenabled = true` gives dir `logs`, name `oxbloom`, `Daily`, `None` history, `Full`); `file_absent_when_not_enabled`; `file_name_defaults_to_application_name` (`application.name = "shop"` gives `shop`); `each_file_key_parses` (hourly/never, `max-history = 7`, format `json`); `max_history_zero_is_invalid` (error key `logging.file.max-history`); `bad_rotation_names_key_not_value` (value `hunter2`, key `logging.file.rotation`, message lacks `hunter2`); `json_fields_default_on_with_own_names`; `json_toggle_and_rename` (`message` renamed to `msg`, `thread` off); `json_unknown_field_is_invalid` (key `logging.json.fields.bogus`); `json_rename_collision_is_invalid` and `json_rename_empty_is_invalid` (key `logging.json.names.<field>`); `console_format_json_parses`; `key_spellings_match` (`max_history`, `Max-History`).
- [ ] **Step 2:** Run `cargo test -p oxbloom-logging --test settings`. Expect: compile or assertion failures.
- [ ] **Step 3:** Add `RawFile`, `RawJson` (`fields: BTreeMap<String, bool>`, `names: BTreeMap<String, String>`) to the `Raw` struct and the parsing in `Settings::from_config`; validate with `ConfigError::invalid(key, fixed rule text)`. Add the enums with the existing `ConsoleFormat` derive and doc-example pattern, and the `Io` variant. Add `tracing-appender` and move `serde_json` to `[dependencies]` in `Cargo.toml`. Add the `PROPERTIES` rows and update the `PROPERTIES.len()` doc assertion in `lib.rs`.
- [ ] **Step 4:** Run `cargo test -p oxbloom-logging`. Expect: all pass, including doc tests.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 2: JSON event formatter

**Files:**
- Create: `crates/oxbloom-logging/src/json.rs`
- Modify: `crates/oxbloom-logging/src/system.rs` (console layer accepts `ConsoleFormat::Json`), `crates/oxbloom-logging/src/lib.rs` (`mod json;`)
- Test: `crates/oxbloom-logging/tests/system_scoped.rs` (extend)

**Interfaces:**
- Consumes: `JsonOptions` from Task 1, `__private::scoped(config, writer) -> (Dispatch, LoggingSystem)`.
- Produces: `pub(crate) struct JsonFormat` implementing `tracing_subscriber::fmt::FormatEvent<S, N>` and constructed by `JsonFormat::new(JsonOptions)`. One line per event: a JSON object whose keys are the configured names for the enabled fields (event fields other than `message` are included as extra keys, never dropped). `timestamp` is RFC 3339 UTC, `level` is lowercase.

- [ ] **Step 1: Write failing tests** (use `scoped` with a shared buffer writer, `serde_json::from_str` on each line): `json_line_has_default_fields` (timestamp, level, target, message present); `toggle_off_removes_field` (`thread` off, `timestamp` off); `rename_changes_key` (`message` to `msg`); `event_fields_are_kept` (`tracing::info!(user = 7, "hi")` yields `"user": 7`); `awkward_message_is_one_valid_line` (quotes, `\n`, `\u{1}`, non-ASCII: exactly one line, parses, round-trips the text); `span_fields_when_enabled` (inside a span with a field, key `span` holds its name and fields); `level_filter_still_applies` (`set_level` hides and shows events in JSON output).
- [ ] **Step 2:** Run the new tests. Expect: FAIL, no JSON console format.
- [ ] **Step 3:** Implement `JsonFormat` in `json.rs` building a `serde_json::Map` per event with a field visitor, written with `writeln!`. Wire `ConsoleFormat::Json` in `console_layer` (signature gains `&JsonOptions`).
- [ ] **Step 4:** Run `cargo test -p oxbloom-logging`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 3: File layer, rotation, flush

**Files:**
- Create: `crates/oxbloom-logging/src/file.rs`
- Modify: `crates/oxbloom-logging/src/system.rs` (`build` adds the file layer and keeps the guard in `Inner`), `crates/oxbloom-logging/src/lib.rs`
- Test: `crates/oxbloom-logging/tests/system_file.rs` (new)

**Interfaces:**
- Consumes: `FileOptions`, `JsonFormat` (Tasks 1, 2), `LoggingError::Io`.
- Produces: `pub(crate) fn file_layer(opts: &FileOptions, json: &JsonOptions) -> Result<(Box<dyn Layer<Filtered> + Send + Sync>, WorkerGuard), LoggingError>`. `LoggingSystem::build` stays `Result<(Dispatch, Self), LoggingError>`; `Inner` gains a `guard: Option<WorkerGuard>` so the last clone dropped flushes.

- [ ] **Step 1: Write failing tests** in `tests/system_file.rs` using `tempfile::tempdir()` and `scoped`: `writes_to_file_with_prefix` (file name starts with the prefix); `no_ansi_in_file` (even with `console.color = always`); `daily_and_hourly_and_never_names` (daily name ends with a `YYYY-MM-DD` date, hourly with date and hour, never has no suffix; read the directory listing); `max_history_removes_old_files` (create more rotated files than `max-history` by pre-seeding the directory with old-dated names, log once, expect the count to be at most `max-history`); `flush_on_drop` (log, drop every clone of the system, read the file, line present); `json_format_in_file` (`logging.file.format = "json"` yields parseable lines); `creates_missing_directory`; `path_is_a_file_gives_io_error_with_path`; `file_and_console_both_receive_events`; `set_level_applies_to_file`.
- [ ] **Step 2:** Run `cargo test -p oxbloom-logging --test system_file`. Expect: FAIL.
- [ ] **Step 3:** Implement `file_layer` with `tracing_appender::rolling::Builder` (`filename_prefix`, `rotation`, `max_log_files` when set) and `tracing_appender::non_blocking`. Create the directory first and map failures to `LoggingError::Io`. Box one `fmt::layer().with_ansi(false)` per `FileFormat`. Compose it in `build` with the console layer.
- [ ] **Step 4:** Run `cargo test -p oxbloom-logging`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green. Re-check that `LoggingSystem` is still `Send + Sync + Clone`.

### Task 4: `RequestLogFilter` in `oxbloom-web`

**Files:**
- Create: `crates/oxbloom-web/src/request_log.rs`
- Modify: `crates/oxbloom-web/Cargo.toml` (add `tracing = "0.1.44"`; dev-dependency `tracing-subscriber` with `fmt`), `crates/oxbloom-web/src/lib.rs` (`pub use request_log::RequestLogFilter;`)
- Test: unit tests in `request_log.rs`

**Interfaces:**
- Consumes: `Filter`, `Next`, `Request`, `Response`; `Request` exposes `method`, `path`, `query`, `headers` (see `request.rs`).
- Produces: `pub struct RequestLogFilter` with `pub fn new(level: tracing::Level, headers: Vec<String>) -> Self` implementing `Filter`. It emits one event at target `oxbloom::web::request` after the response with fields `method`, `path`, `status`, `latency_ms` and `headers` (a string of `name=value` pairs for allowlisted headers that are present). A value prints as `<redacted>` when `HeaderValue::is_sensitive()` is true or the lowercase name is one of `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`; the deny list wins over the allowlist. Allowlist matching is case-insensitive. The query string is never logged.

- [ ] **Step 1: Write failing tests** (capture events with a `tracing_subscriber` fmt layer writing to a shared buffer, via `with_default`): `logs_method_path_status_latency`; `query_string_is_not_logged` (`/a?token=abc`, buffer lacks `abc`); `allowlisted_header_is_logged`; `non_allowlisted_header_is_not_logged`; `authorization_is_redacted_even_if_allowlisted`; `custom_credential_names_redacted_in_any_case` (`X-Api-Key`, `x-AUTH-token`, `Set-Cookie`); `sensitive_flagged_value_is_redacted` (custom header marked `set_sensitive(true)`); `level_is_configurable` (a `debug` filter logs nothing under an `info` subscriber); `response_is_passed_through_unchanged` (status and body).
- [ ] **Step 2:** Run `cargo test -p oxbloom-web request_log`. Expect: FAIL.
- [ ] **Step 3:** Implement the filter: record `Instant::now()` before `next.run(req)`, read method/path/headers before moving the request, emit the event with a level-dispatching macro (`tracing::event!` needs a const level, so match on the level and call each).
- [ ] **Step 4:** Run `cargo test -p oxbloom-web`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 5: Boot auto-configuration, metadata, analyzer

**Files:**
- Create: `crates/oxbloom-boot/src/autoconfigure/request_log.rs`
- Modify: `crates/oxbloom-boot/src/autoconfigure/mod.rs`, `crates/oxbloom-boot/src/analyzers/logging.rs`, `crates/oxbloom-boot/src/lib.rs` (re-export `RequestLogAutoConfiguration`)
- Test: `crates/oxbloom-boot/tests/logging_output.rs` (new), `crates/oxbloom-boot/tests/autoconfigure_http.rs` (extend)

**Interfaces:**
- Consumes: `RequestLogFilter::new(Level, Vec<String>)` (Task 4), `AutoConfiguration` trait (`conditions()`, `properties()`), pattern of `JwtDecoderAutoConfiguration`, `Conditions::on_property_value("logging.request.enabled", "true")`, `Config::get`.
- Produces: `pub struct RequestLogAutoConfiguration` (feature `web`), registering the filter as `Arc<dyn Filter>` with `.order(i32::MIN)` (first among `dyn Filter` beans), added to `auto_configurations()`. It reads `logging.request.headers` (default empty) and `logging.request.level` (default `info`); invalid level is `ConfigError::invalid("logging.request.level", ..)`. Its `PropertyMetadata` lists `logging.request.enabled`, `logging.request.headers`, `logging.request.level`. `LoggingConfigAnalyzer` also handles `LoggingError::Io` with fixed text naming the path and the action "check that the directory exists or can be created and that the process may write to it", and its invalid-key action text mentions `logging.file.*` and `logging.json.*`.

- [ ] **Step 1: Write failing tests.** In `logging_output.rs` (feature `logging`): `file_logging_from_config` (app with `logging.file.enabled`, a tempdir path, run to completion, file contains the `oxbloom::boot` startup event); `unwritable_path_fails_startup_with_analysis` (path is a regular file: `StartupError`, analysis mentions the path and "check", no value echoed); `bad_file_key_is_analyzed` (`logging.file.rotation = "weekly"`: description names the key). In `autoconfigure_http.rs` (features `web`, `logging`): `request_log_off_by_default` (no event for a request, conditions report shows the skip); `request_log_on` (event with status 200); `request_log_redacts_authorization_header_on_allowlist`; `request_log_misses_security_rejections` (a 401 from the chain produces no request event; the test documents current ordering). Unit tests in `analyzers/logging.rs` for the `Io` variant text.
- [ ] **Step 2:** Run `cargo test -p oxbloom-boot --all-features`. Expect: FAIL.
- [ ] **Step 3:** Implement the auto-configuration, metadata, `mod.rs` wiring and analyzer changes as described in Interfaces.
- [ ] **Step 4:** Run `cargo test -p oxbloom-boot --all-features`. Expect: PASS.
- [ ] **Step 5:** Run the task gate. Expect: green.

### Task 6: Docs, example, roadmap, final gate

**Files:**
- Modify: `README.md` (section "Logging"), `docs/API.md`, `docs/ROADMAP-core.md` (C5b done, follow-ups), `examples/todos` (commented sample of the new keys in its `application.toml`), `crates/oxbloom-logging/src/lib.rs` crate docs
- Test: doc tests, `make check-examples`

**Interfaces:**
- Consumes: everything above.
- Produces: user-facing docs only.

- [ ] **Step 1:** Update README and `docs/API.md` with the new keys, the redaction rule, the flush-on-drop behavior and the filter-ordering limitation. Mark C5b done in the roadmap (delivered list, probe numbers, follow-ups: size-based rotation, filter ordering with security in C6, applying output changes at runtime).
- [ ] **Step 2:** Run `make check-examples`. Expect: builds and tests pass.
- [ ] **Step 3:** Run the full gate: `cargo test --workspace --all-features`, clippy `-D warnings`, `cargo fmt --check`. Record the totals.
- [ ] **Step 4:** Re-measure the stack probes with the C5a method (DI minima, boot without `logging` and with it, `build()` frame via disassembly). Expect: zero growth over the C5a numbers. If the method is not recorded in the repo, take it from the C5a ledger or ask the user before measuring differently.
- [ ] **Step 5:** Run `graphify update .`.
