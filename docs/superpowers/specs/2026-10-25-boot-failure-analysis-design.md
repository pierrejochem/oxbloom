# Startup failure analysis (slice C3b): `FailureAnalyzer` in `oxbloom-boot`

Date: 2026-10-25

Status: draft for review

Roadmap item: C3 in `docs/ROADMAP-core.md`, second half. The first half (C3a, `docs/superpowers/specs/2026-10-25-boot-design.md`) is implemented.

## Purpose

When an Oxbloom application fails to start, `OxbloomApplication::run` returns a `StartupError` whose text names a phase and the underlying error. That is accurate but not helpful: a developer who forgets a config key, registers a bean twice or has a port clash gets one terse line. Spring Boot answers this with `FailureAnalyzer`: a failure is matched against known causes and printed as a short report with a description and a concrete action. C3b adds the same to Oxbloom.

Success means that the most common startup mistakes (port in use, missing config key, a bean wiring error, and a bad security configuration) each produce a report that names the problem and tells the developer what to do, and that developers can add analyzers for their own errors.

## Scope

In scope:

- The `FailureAnalysis` value, the `FailureAnalyzer` trait, and `StartupError::find_cause` and `StartupError::analysis`.
- Registering analyzers on the builder and as beans, matching order, panic isolation.
- The failure report text and when it is printed (`failure_report` flag, `run` and `run_and_exit`).
- Typed error plumbing so analyzers can reach the real error: a `source` on `DiError::Creation`, `DiError::creation_from`, and `From` impls for `ConfigError` and `SecurityBuildError`.
- Built-in analyzers: port in use, missing config key, bean wiring (cycle, missing, ambiguous, duplicate), security build errors.
- The `oxbloom new` template switched to `OxbloomApplication`.

Out of scope:

- Colors, localization, or machine-readable report formats.
- Analysis of failures after a successful start (runtime errors).
- An analyzer choosing the exit code.
- Automatic registration of analyzers (C4) and logging integration (C5).

## Decisions taken in brainstorming

1. Scope: the four analyzer families plus the security analyzer, plus the CLI template switch (user choice).
2. Registration: `.failure_analyzer(..)` on the builder works for every phase; `Arc<dyn FailureAnalyzer>` beans are also used when a container exists. Order: builder analyzers, bean analyzers, built-ins; the first match wins (user choice).
3. Printing: a builder flag `failure_report(bool)`; default off for `run()`, on for `run_and_exit()`, which prints the report in place of the bare error line (user choice).
4. The security analyzer recognises its error through a typed source on `DiError::Creation` (user choice), not by matching message text.

## Public API

All new public types are in `oxbloom_boot` unless noted. Every public item has a runnable doc example (`AGENT.md`).

### `FailureAnalysis`

`#[non_exhaustive]` struct with `new(description: impl Into<String>, action: impl Into<String>)`, `with_no_action(description)`, `description() -> &str`, `action() -> Option<&str>`. `Display` renders the report block:

```
***************************
APPLICATION FAILED TO START
***************************

Description:

<description>

Action:

<action>
```

The Action section is omitted when there is no action. A failure without a matching analyzer renders the same block with the `StartupError` `Display` as description and no Action section.

### `FailureAnalyzer`

```rust
pub trait FailureAnalyzer: Send + Sync + 'static {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis>;
    fn name(&self) -> &'static str { std::any::type_name::<Self>() }
}
```

Analyzer calls run inside `catch_unwind`. A panicking analyzer is skipped and its name is written to the output target (same rule as `RunListener`). Analyzers must not include configuration values, argument values or secrets in their text; type names, config key names and, for the port analyzer only, a socket address are allowed.

### `StartupError` additions

- `find_cause::<E: Error + 'static>(&self) -> Option<&E>`: walks the `source()` chain starting with the boxed source, downcasting at each level, and returns the first match. The walk stops after 64 levels so that a cyclic chain cannot loop.
- `analysis(&self) -> Option<&FailureAnalysis>`: the analysis attached by boot, if any.
- `Display` and `Debug` are unchanged by this slice, except that `Debug` does not print the analysis text.

### Builder additions

- `failure_analyzer(impl FailureAnalyzer)`: repeatable, call order is match order.
- `failure_report(bool)`: sets whether `run()` prints the report. `run_and_exit()` prints it unless the flag was explicitly set to `false`.

### Typed errors through DI (crate `oxbloom-di`)

- `DiError::Creation` gains a field `source: Option<std::sync::Arc<dyn std::error::Error + Send + Sync>>`. The variant is already part of a `#[non_exhaustive]` enum; code that matches `Creation { type_name, message }` with `..` keeps compiling, and code that constructs it literally inside the workspace is updated. `PartialEq`/`Eq` ignore `source`; `Clone` clones the `Arc`; `Debug` prints `source: Some(..)` without its content. `impl Error for DiError` returns the source from `source()`.
- `DiError::creation_from::<T>(source: impl std::error::Error + Send + Sync + 'static) -> DiError` builds a `Creation` for `T` whose message is the error's `Display` and whose `source` is the error.
- `try_bean` and `#[bean(fallible)]` keep an existing `source` when they rewrite the type name to the failing bean's type.

### `From` impls

- Crate `oxbloom-config`, feature `di`: `impl From<ConfigError> for DiError` through `creation_from`, with the type name left for `try_bean` to rewrite.
- Crate `oxbloom-security`, feature `di`: `impl From<SecurityBuildError> for DiError` the same way.

With these, a factory declared with `try_bean(|..| -> Result<T, DiError> { Ok(config.get(..)?) })` keeps the typed error, and `find_cause::<ConfigError>()` finds it.

### `BindError` (crate `oxbloom-boot`, feature `web`)

The private bind error becomes public: `BindError` with `address() -> SocketAddr` and `kind() -> std::io::ErrorKind`. Its `Display` stays the fixed text "the web server cannot bind its listener: <OS error text>" without address or port. `WebServer` start failures box a `BindError` inside the `ServiceError`.

## Matching and attachment

`run` attaches the analysis to the `StartupError` before it calls the `failed` listeners, so listeners can read `analysis()`. Analyzer order: builder analyzers in call order, then, when a container was built, `Arc<dyn FailureAnalyzer>` beans in DI order, then the built-ins (config key, bean wiring, port in use when `web` is on, security when `security` is on). The first `Some` wins. Bean analyzers are collected with `get_all`; a failure to collect them is ignored (the failure being analyzed may be the cause) and the remaining analyzers are used.

## Built-in analyzers

All descriptions are built from type names and key names only.

1. **Port in use** (feature `web`). Matches a `BindError` with kind `AddrInUse`. Description: "The web server could not listen on `<address>` because the port is already in use." Action: "Stop the process that uses the port, or set `server.port` to a free port (0 picks one)." This report is the one place that shows an address. Other bind failures get the generic report.
2. **Missing config key.** Matches `ConfigError::Missing { key }` anywhere in the chain. Description: "The required configuration key `<key>` is not set." Action: "Set it in `application.toml` or the active profile file, as an environment variable (default prefix `APP_`, with `__` between key segments), or as `--<key>=<value>`."
3. **Bean wiring.** Matches `DiError` at any level of the chain:
   - `Cycle { path }`: "The beans form a dependency cycle: `A -> B -> A`." Action: "Remove one of the dependencies, or move the shared logic into a third bean."
   - `Missing { requested, required_by }`: "`<last of required_by>` needs a bean of type `<requested>`, but none is registered." (With an empty `required_by` the first sentence reads "A bean of type `<requested>` was requested, but none is registered.") Action: "Register one with `component`, `instance`, `bean` or `factory`, and check its profile conditions."
   - `Ambiguous { requested, candidates }`: "More than one bean can provide `<requested>`: `<candidates>`." Action: "Mark one of them `primary`, or inject `Vec<Arc<..>>`."
   - `Duplicate { type_name, sources }`: "`<type_name>` is registered more than once: `<sources>`." Action: "Register the type only once."
4. **Security build errors** (feature `security`, which enables `oxbloom-security` with its `di` feature). Matches `SecurityBuildError` with an exhaustive `match`, so adding a variant forces an update:
   - `NoAuthorizeSection`: "The security configuration has no authorize section." Action: "Call `authorize` and add at least one rule."
   - `MatcherAfterAnyRequest`: "A security rule follows `any_request()`, where it can never match." Action: "Put `any_request()` last."
   - `ResourceServerWithoutDecoder`: "`oauth2_resource_server` is configured without a JWT decoder." Action: "Configure a decoder (static keys or JWKS)."
   - `DuplicateAuthorizeSection`: "`authorize` was called more than once." Action: "Put all rules in one `authorize` call."
   - `InvalidPattern`: "A security matcher pattern is not valid. Patterns use Ant syntax, and `*` or `**` is allowed only as a whole path segment; `{id}`, `?` and partial wildcards such as `*.pdf` are not supported." Action: "Use `/users/*` or `/users/**` and match the rest in a handler." The pattern is not shown because the error does not carry it.

## Printing

- The report goes to the output target (stderr by default), once, as the last step before `run` returns `Err`, after `failed` listeners ran and cleanup finished.
- `run()` prints the report only when `failure_report(true)` was set.
- `run_and_exit()` prints the report by default, in place of the bare error line, so nothing is printed twice. With `failure_report(false)` it writes the bare error line as before, so a failure is never silent.

## Error handling and safety rules

- Analyzer text contains no configuration values, argument values or secrets (the port-in-use address excepted, by decision).
- The analysis is computed once per failure; it never changes the error, the phase or the exit code (always 1 for `run_and_exit`).
- Analyzers and `find_cause` allocate nothing proportional to untrusted input beyond the formatted text; candidate and source lists are printed in full (they are developer-controlled type labels).
- No global state. Two applications in one process analyze independently.

## CLI template

`oxbloom-cli` generates a project whose `src/main.rs` uses `OxbloomApplication::builder()` with a `WebServer` and `run_and_exit`, and whose `Cargo.toml` depends on `oxbloom-boot` with the `web` feature. The generated `src/lib.rs` keeps `pub fn app() -> App` because the generated test calls it. The unit tests of `templates.rs` and `cli.rs` are updated. The existing ignored test `generated_project_checks_and_tests` loses its `CARGO_TARGET_DIR` override so that it uses the configured shared target directory instead of creating a new one on tmpfs.

## Testing

- Unit tests per analyzer: exact description and action text for every handled variant, and `None` for non-matching errors.
- Typed error plumbing: `Creation` equality ignores `source`; `creation_from`, `From<ConfigError>` and `From<SecurityBuildError>` keep a downcastable source; `try_bean` and `#[bean(fallible)]` keep it when rewriting the type name.
- `find_cause`: finds the first match, finds nothing for a missing type, stops after 64 levels on a cyclic chain.
- Ordering: builder, then bean, then built-in; a user analyzer overrides a built-in; a bean analyzer is ignored when no container exists; a failing `get_all` for analyzer beans is ignored.
- Panic isolation: a panicking analyzer is skipped and named in the output.
- Printing: `run()` silent by default; `run_and_exit()` prints the report once and not the bare line; `failure_report(false)` on `run_and_exit` prints the bare line; `failed` listeners see `analysis()`.
- Hygiene: a marker in a config value, an argument and a secret never appears in any report text; the port report is pinned to show the address and nothing else configurable.
- Integration, each driving the real application: port taken before `WebServer::start`, missing key through `try_bean`, a cyclic pair, a missing dependency, an ambiguous pair, a duplicate registration, a bad security pattern through `try_bean`.

## Review Focus

1. A failure with no matching analyzer prints the generic block and never panics, including an empty `required_by`, an empty `candidates` list and an empty `sources` list.
2. An analyzer that returns text containing a newline or control characters: the report must not let it forge a second report block (the description and action are printed as given; the test pins that the surrounding block structure is intact for single-line text and documents that analyzers must not emit the block markers).
3. A config failure that happens before any container exists still gets a builder analyzer and a built-in analysis.
4. `DiError::Creation` values cached by the container are cloned many times: cloning must stay cheap (the `Arc` source).
5. `failure_report(true)` on `run()` together with `run_and_exit()` does not print twice.

## Task outline

About eight tasks, each with its own review:

1. `oxbloom-di`: `Creation.source`, `creation_from`, `Error::source`, kept sources in `try_bean` and `#[bean(fallible)]`.
2. `From<ConfigError>` and `From<SecurityBuildError>` for `DiError` under the `di` features.
3. Boot core: `FailureAnalysis`, `FailureAnalyzer`, `find_cause`, `analysis()`, rendering, registration, flag, printing.
4. Config-key and bean-wiring analyzers.
5. `BindError` and the port-in-use analyzer (feature `web`).
6. The security analyzer (feature `security`).
7. CLI template and its tests.
8. Documentation, `hello`, spec amendments and the final gate.

## Amendments

These decisions were taken while planning and implementing the slice. Where they differ from the text above, they win.

1. The new field of `DiError::Creation` is `source: ErrorSource`, not an `Option` of a trait object. `ErrorSource` is a public newtype around an optional shared error, with the constructors `ErrorSource::none()`, `ErrorSource::new(err)` and `Default` (empty), and the accessors `is_some()` and `as_error() -> Option<&(dyn Error + 'static)>`. Its `Debug` prints `ErrorSource(..)` in both states and never the error's content, it is `Clone`, and its `PartialEq` is always true. As a result, neither the content nor the presence of a source affects the equality of a `DiError`: a `Creation` with a source equals the same `Creation` without one. This keeps the derives of `DiError` (`Debug`, `Clone`, `PartialEq`, `Eq`) instead of hand-written impls. `DiError::creation_from` takes any `Error + Send + Sync + 'static`, and `impl Error for DiError` returns the held error from `source()`.
2. Inside `ErrorSource` the error is stored as `Option<Arc<Box<dyn Error + Send + Sync>>>`. The extra `Box` makes the `Arc` a thin pointer, so `DiError` stays at 56 bytes. A first version with a fat `Arc<dyn Error>` grew `DiError` to 64 bytes and raised the debug-build stack needed for a 256-deep dependency chain (closures from 206 to 222 KB, `#[component]` from 277 to 302 KB, `#[bean]` from 230 to 246 KB). With the thin pointer the minima are unchanged from before the slice. A size guard test pins the size of `DiError`.
3. The `From<ConfigError>` and `From<SecurityBuildError>` impls call `DiError::creation_from::<ConfigError>` and `DiError::creation_from::<SecurityBuildError>`. `try_bean` and `#[bean(fallible)]` then rewrite the type name to the failing bean's type and keep the source.
4. The built-in analyzer structs are crate-private. Users see only the `FailureAnalyzer` trait and `FailureAnalysis`; the built-ins are always asked last.
5. `oxbloom-boot` enables `oxbloom-config/di` unconditionally, so `?` on a `ConfigError` works inside `try_bean` for every boot user without a feature of their own.
6. The analysis is attached to the `StartupError` before the `failed` listeners run, so a listener can read `analysis()`. The report itself is still printed last, after the listeners and cleanup.
7. The collection of the `Arc<dyn FailureAnalyzer>` beans runs inside `catch_unwind`. A panicking lazy analyzer bean factory would otherwise escape `get_all` and skip the `failed` listeners and the cleanup. A panic there is treated like a failed `get_all`: boot writes the value-free line `failure analyzer beans panicked` to the output target and goes on with the remaining analyzers.
8. When an analyzer panics, boot reads its `name()` for the line `analyzer {name} panicked` inside a second `catch_unwind`, falling back to `<unknown>`, so a panicking `name()` cannot escape either.
9. `oxbloom new` checks the framework root more strictly. The `--oxbloom-path` (or the default workspace) must contain the `Cargo.toml` of `oxbloom-web`, `oxbloom-boot`, `oxbloom-config` and `oxbloom-di`, the four crates the template depends on. A missing manifest is the usage error `not an oxbloom workspace: missing <path>` with exit code 2.
10. Boot keeps its diagnostic line `service X failed to start`, written before the report when a service fails to start. With `run_and_exit()` and `failure_report(false)` the output is therefore that line followed by the bare error line.
11. Adding the `source` field is a breaking change for code outside the workspace that builds `DiError::Creation` literally or destructures it without `..`, because the enum-level `#[non_exhaustive]` does not cover the fields of a variant. This was accepted for the slice and is listed as a follow-up in `docs/ROADMAP-core.md`.
