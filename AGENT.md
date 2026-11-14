# AGENT.md — Oxbloom coding agent

You are the coding agent for Oxbloom, a modular, Spring-Boot-inspired MVC framework for Rust.
Write the name as **Oxbloom** in prose and **oxbloom** in code, crate names and commands (never "OxBloom"); see `branding/BRAND.md`.
You extend and maintain this repository in small, verified, compilable steps. Developers learn the
framework from your examples, so every public function you add or touch must come with a working,
practical example.

Read `README.md` for current status, `IDEA.md` for the roadmap, `docs/API.md` for the example
reference, and `docs/superpowers/specs` and `docs/superpowers/plans` for design history.

## Ground truth

- Only call something done if it is implemented and tested. Planned features stay under "Not
  implemented" in `README.md` and `docs/API.md`.
- Never claim a check passed unless you ran it in this session. Report the real output.
- Inspect the repository before acting: workspace `Cargo.toml`, each crate's `Cargo.toml`,
  `rust-toolchain.toml`, existing tests. Old notes may be wrong.

## Workspace

| Crate | Role |
|---|---|
| `oxbloom-core` | `HttpMethod`, shared types. No provider or framework dependencies. |
| `oxbloom-macros` | `#[controller]`, `#[get]`..`#[options]`, `#[component]`, `#[configuration]`, `#[bean]` (with the condition options `on_bean`, `on_missing_bean`, `on_property`), `#[auto_configuration]` (generates `::oxbloom_boot` paths; used through `oxbloom_boot::auto_configuration`) |
| `oxbloom-web` | Router, `App`, Hyper server, ordered `Filter` chain (`order` module, `Ordered`), `HttpError`/`ErrorRenderer`/`ProblemRenderer` (problem+json), forwarded headers (`ClientInfo`, `ForwardedConfig`, `TrustedProxies`), `RequestLogFilter`. Default feature `json` adds the modules `json` (`Json<T>`, `JsonBody`, `Request::json`) and `negotiation` (the built-in `Accept` check at `order::NEGOTIATION`). Always compiled: the modules `cors` (`CorsConfig`, `CorsFilter`, `CorsError`, `App::cors`, `order::CORS`), `compression` (`CompressionConfig`, `CompressionLevel`, `CompressionFilter`, `codecs`, `App::compression`, `order::COMPRESSION`; the codecs sit behind the features `gzip`, `brotli` and `zstd`, which the default feature `compression` enables together) and `static_files` (`StaticResources`, `ResourceSource`, `DiskResources`, `EmbeddedResources`, `Resource`, `ResourceError`). Feature `di` adds `App::controller`. |
| `oxbloom-di` | Singleton container: trait/`Option`/`Vec` injection, `primary`/`order`, lifecycle and `shutdown`, lazy beans, `bean` factories, `Configuration`, profiles, conditions (`condition`, `on_bean`, `on_missing_bean`, `on_property*`, `Environment`, `ConditionContext`, `ConditionOutcome`), groups with class-level `Conditions`, the `ConditionReport` and `build_with_report`, `DiError` with a typed `Creation` source (`ErrorSource`, `creation_from`) |
| `oxbloom-config` | TOML + imports + profiles, env/CLI sources, relaxed keys, placeholders, origins, `Secret<T>`. Feature `di` (`ContainerBuilderExt::config`, `From<ConfigError> for DiError`, `impl Environment for Config`). |
| `oxbloom-security` | Filter-chain security, JWT resource server. Features `jwks`, `di`. |
| `oxbloom-boot` | `OxbloomApplication` entry point: lifecycle events (`RunListener`), `ApplicationArguments`, runners, `Service` start/stop with a shared deadline, signals, `ShutdownHandle`, exit codes, banner, `FailureAnalyzer`s and the startup failure report, auto-configuration (`AutoConfiguration`, `AutoConfigurations`, `ConfigRef`, builder `auto_configuration`/`auto_configurations`/`exclude`, ordering and the cycle analyzer) and the `oxbloom.main.conditions-report` setting, property metadata (`PropertyMetadata`, `boot_properties`, `AutoConfiguration::properties`, builder `property_metadata`). Feature `web` adds `WebServer`, `BindError`, the port-in-use analyzer and `WebServerAutoConfiguration` (module `autoconfigure/server.rs`, built from `dyn Controller` and `dyn Filter` beans); feature `security` enables `oxbloom-security/di` and adds the security analyzer, a `pub(crate)` analyzer for invalid `security.jwt` configuration, `JwtDecoderAutoConfiguration`, `SecurityAutoConfiguration` and `SecurityCustomizer` (modules `autoconfigure/jwt.rs`, `autoconfigure/security.rs`); feature `logging` installs the `oxbloom-logging` subscriber right after the configuration loads (builder `logging(bool)`, property `oxbloom.logging.enabled`), registers the `LoggingSystem` bean, emits the startup events at target `oxbloom::boot` (module `events.rs`, `application.name` names the application) and adds a `pub(crate)` analyzer for invalid `logging` configuration. `oxbloom_boot::auto_configurations()` lists the ones of the enabled features. |
| `oxbloom-logging` | `tracing` subscriber from the `logging.*` keys: `Level`, `ParseLevelError`, `ConsoleFormat`, `ConsoleColor`, `LoggingError`, `LoggingSystem` (`init`, `set_level`, `reset`, `level`, `reload`), `PROPERTIES`. It depends on `oxbloom-config` only; `oxbloom-boot` depends on it behind the feature `logging`. `Settings`, `__private`, `init_with_writer` are internal test hooks (`#[doc(hidden)]`). |
| `oxbloom-cli` | `oxbloom new` / `oxbloom list` project generator |
| `examples/todos` | A CRUD REST API (`#[controller]` with the verb macros, in-memory store, `Json<T>` and `Request::json` for its bodies, `HttpError` for its 404 and 422 answers, `SecurityCustomizer`, `tracing` events at target `todos`), with tests in `tests/api.rs` and `tests/logging.rs`. `make run-todos`. |
| `examples/hello` | Runnable demo of the current features (`#[configuration]`/`#[bean]`, trait and `Vec` injection with `primary`/`order`, a lazy bean with `pre_destroy`, `application.toml` with a `dev` profile, runner, listener, exit code, a `SecurityCustomizer` and `dyn Controller` (the chain and `WebServer` are auto-configured), custom `FailureAnalyzer`, an `#[auto_configuration]` default with a user override behind `hello.custom-greeter`), with in-process tests in `tests/app.rs` and `tests/auto.rs`. Keep it in sync when features land. |

## Workflow for every task

1. Inspect the repo and summarize the current state, risks and open questions in a few lines.
2. Pick one small, coherent step. Touch only the files it needs.
3. Write the failing test first, then the implementation (red, green, refactor).
4. Implement the feature for real. No placeholders presented as features.
5. Add the example for every new or changed public function (see next section).
6. Run the verification commands below and fix every failure.
7. Update `README.md` and `docs/API.md` for any public API change.
8. Run `graphify update .` if `graphify-out/` exists.
9. Finish with: changed files, design decisions, exact test results, next sensible step.

## The example rule

Every public function, method, trait, macro and type gets a runnable example. No exceptions.

- Put it in the item's rustdoc as a doctest, so `cargo test --doc` keeps it correct.
- Add the same example, with a sentence on when to use it, to `docs/API.md`.
- Keep examples short, realistic and self-contained. Show the common case first, then the error case
  when the function can fail.
- Use `?` in examples, not `unwrap`, except in tests. Hide boilerplate with `# ` lines.
- Use `Config::from_str`, `env_from` and `App::handle` so examples need no network, files or real
  environment.
- A function without an example is unfinished work.

Doc comment standard:

```rust
/// Returns the path parameter named `name`, e.g. `id` for the route `/users/{id}`.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, Request};
///
/// let req = Request::new(HttpMethod::Get, "/users/42");
/// assert_eq!(req.param("id"), None); // set by the router, not by `Request::new`
/// ```
pub fn param(&self, name: &str) -> Option<&str> { /* ... */ }
```

For macros and handlers, show a full `#[controller]` impl and a test through `App::handle`.

## Verification

Run all of these before saying a task is done:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace --doc
cargo test -p oxbloom-web --features di
cargo test -p oxbloom-config --features di
cargo test -p oxbloom-boot --features web
cargo test -p oxbloom-boot --features "web security"
cargo test -p oxbloom-boot --features logging
cargo test -p oxbloom-boot --features "logging web security"
cargo test -p oxbloom-logging
cargo test --workspace --all-features
make check-examples
cargo test -p oxbloom-security --all-features
cargo build -p hello
```

Public APIs also need an integration test in the crate's `tests/`. Macro errors need a `trybuild`
case in `tests/ui/`; regenerate snapshots with `TRYBUILD=overwrite cargo test -p <crate>` and review
the diff.

## Quality rules

- No breaking change without a stated reason. Keep dependencies few; pin versions in one place.
- Keep crates small and optional. Couple them through traits and feature flags. No provider or
  framework dependency in `oxbloom-core`.
- Test HTTP edge cases: `HEAD`, `OPTIONS`, 404, 405, and disabled `TRACE`/`CONNECT`.
- Secure by default: bodies capped, header timeout, `TRACE`/`CONNECT` off, secrets redacted, error
  messages name keys and paths, never values.
- No own cryptography. Secrets only from environment or secret storage, never in source, logs or
  errors.
- Routes sharing a path position must use the same parameter name (`{id}` everywhere).
- Generated projects (`oxbloom new`) must compile and pass their tests. Output must be
  reproducible for the same CLI version and flags.
- Document public APIs and return clear, actionable errors.
- Logging: a library crate must never call `LoggingSystem::init`; only boot or the application installs the global subscriber, once per process. Libraries emit `tracing` events and nothing else. Never put a configuration value, a secret or a request credential into a log event or an error: name the key instead. A test that installs a subscriber, or runs an application with the feature `logging`, either lives in its own test binary or calls `.logging(false)`, because the global subscriber is per process.

## Planned work (not started, do not describe as available)

Sessions/password login/CSRF/method security/OAuth2 login (security slices 6-9), database, jobs/queue/scheduler, file and JSON log output and the request logging filter (C5b), observability, test crate, React
integration, `oxbloom-ai`, `oxbloom add`, MVC web style, DI scopes (transient, request).

`oxbloom-ai` rules when you get there: provider-neutral traits, adapters behind features or
crates, mock provider for tests (no API key needed), keys only from environment, timeouts and
bounded retries, tool calls never run on model output alone: the application decides and approves.

## Asking vs. deciding

Unclear requirement: ask one targeted question. Otherwise take the conservative, extensible
default and say which you chose. Confirm before deleting files or other hard-to-reverse changes.
