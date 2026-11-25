# AGENT.md — Oxbloom coding agent

You are the coding agent for Oxbloom, a modular, Spring-Boot-inspired MVC framework for Rust.
Write the name as **Oxbloom** in prose and **oxbloom** in code, crate names and commands (never "OxBloom"); see `branding/BRAND.md`.
You extend and maintain this repository in small, verified, compilable steps. Developers learn the
framework from your examples, so every public function you add or touch must come with a working,
practical example. This file is read by every agent at the start of a task, subagents included; follow
it unless the user says otherwise.

Read first: `docs/guide/01-status.md` for the current status, `docs/ROADMAP-core.md` for delivered and
planned slices, `docs/API.md` for the example reference, `docs/adr/` for the architecture decisions,
`IDEA.md` for the overall plan, and `docs/superpowers/specs` and `docs/superpowers/plans` for design
history.

## Ground truth

- Only call something done if it is implemented and tested. Planned features stay under "Not
  implemented" in `README.md` and `docs/API.md`.
- Never claim a check passed unless you ran it in this session. Report the real output.
- Inspect the repository before acting: workspace `Cargo.toml`, each crate's `Cargo.toml`,
  `rust-toolchain.toml`, existing tests. Old notes may be wrong.

## Workspace

| Crate | Role |
|---|---|
| `oxbloom-core` | `HttpMethod`, the `Clock` trait and `SystemClock`. No provider or framework dependencies. |
| `oxbloom-macros` | `#[controller]`, `#[get]`..`#[options]`, `#[component]`, `#[configuration]`, `#[bean]` (with the condition options `on_bean`, `on_missing_bean`, `on_property`), `#[auto_configuration]` (generates `::oxbloom_boot` paths; used through `oxbloom_boot::auto_configuration`) |
| `oxbloom-web` | Router, `App` (`handle`, `handle_from`, `serve`), Hyper server, ordered `Filter` chain (`order` module, `Ordered`), `HttpError`/`ErrorRenderer`/`ProblemRenderer` (problem+json), forwarded headers (`ClientInfo`, `ForwardedConfig`, `TrustedProxies`), `RequestLogFilter`. Default feature `json` adds the modules `json` (`Json<T>`, `JsonBody`, `Request::json`) and `negotiation` (the built-in `Accept` check at `order::NEGOTIATION`). Always compiled: the modules `cors` (`CorsConfig`, `CorsFilter`, `CorsError`, `App::cors`, `order::CORS`), `compression` (`CompressionConfig`, `CompressionLevel`, `CompressionFilter`, `codecs`, `App::compression`, `order::COMPRESSION`; the codecs sit behind the features `gzip`, `brotli` and `zstd`, which the default feature `compression` enables together) and `static_files` (`StaticResources`, `ResourceSource`, `DiskResources`, `EmbeddedResources`, `Resource`, `ResourceError`). Feature `di` adds `App::controller`. Feature `tls` (off by default) adds `App::serve_tls` over a `tokio_rustls::TlsAcceptor`; `oxbloom-web` does not depend on `oxbloom-ssl`. |
| `oxbloom-di` | Singleton container: trait/`Option`/`Vec` injection, `primary`/`order`, lifecycle and `shutdown`, lazy beans, `bean` factories, `Configuration`, profiles, conditions (`condition`, `on_bean`, `on_missing_bean`, `on_property*`, `Environment`, `ConditionContext`, `ConditionOutcome`), groups with class-level `Conditions`, the `ConditionReport` and `build_with_report`, `DiError` with a typed `Creation` source (`ErrorSource`, `creation_from`) |
| `oxbloom-config` | TOML + imports + profiles, env/CLI sources, relaxed keys, placeholders, origins, `Secret<T>`. Feature `di` (`ContainerBuilderExt::config`, `From<ConfigError> for DiError`, `impl Environment for Config`). |
| `oxbloom-security` | Filter-chain security, JWT resource server (an optional `Clock` for `exp`/`nbf`), `jwks_ssl_bundle`. Features `jwks` (`ReqwestJwksFetcher`, which runs on `oxbloom-http-client`, with `with_tls` and `with_client`), `di`, `oauth2` (OAuth2 and OpenID Connect login: `HttpSecurity::oauth2_login`, `OAuth2Login`, `OAuth2Settings`, `OAuth2Client` with `AccessToken` and `TokenError` (client credentials and the token cache, `access_token_for` with the refresh of the user's tokens), `ClientCredentialsInterceptor`, `BearerRetryClient` and `UserTokenInterceptor`, module `oauth2/`; it implies `jwks`). |
| `oxbloom-boot` | `OxbloomApplication` entry point: lifecycle events (`RunListener`), `ApplicationArguments`, runners, `Service` start/stop with a shared deadline, signals, `ShutdownHandle`, exit codes, banner, `FailureAnalyzer`s and the startup failure report, auto-configuration (`AutoConfiguration`, `AutoConfigurations`, `ConfigRef`, builder `auto_configuration`/`auto_configurations`/`exclude`, ordering and the cycle analyzer) and the `oxbloom.main.conditions-report` setting, property metadata (`PropertyMetadata`, `boot_properties`, `AutoConfiguration::properties`, builder `property_metadata`). Feature `web` adds `WebServer`, `BindError`, the port-in-use analyzer, `WebServerAutoConfiguration` (module `autoconfigure/server.rs`, built from `dyn Controller` and `dyn Filter` beans; it also reads the CORS, compression and static `server.*` keys) and `RequestLogAutoConfiguration` (`autoconfigure/request_log.rs`); feature `security` enables `oxbloom-security/di` and adds the security analyzer, a `pub(crate)` analyzer for invalid `security.jwt` configuration, `JwtDecoderAutoConfiguration`, `SecurityAutoConfiguration` and `SecurityCustomizer` (modules `autoconfigure/jwt.rs`, `autoconfigure/security.rs`); feature `logging` installs the `oxbloom-logging` subscriber right after the configuration loads (builder `logging(bool)` and `logging_clock`, property `oxbloom.logging.enabled`), registers the `LoggingSystem` bean, emits the startup events at target `oxbloom::boot` (module `events.rs`, `application.name` names the application) and adds a `pub(crate)` analyzer for invalid `logging` configuration; feature `ssl` (implies `web`) adds `SslAutoConfiguration` (`autoconfigure/ssl.rs`), the SSL analyzer (`analyzers/ssl.rs`), `WebServer::with_ssl` and `server.ssl.bundle`, and resolves `security.jwt.jwks-ssl-bundle` in `autoconfigure/jwt.rs`. Feature `http-client` (enables `oxbloom-http-client/config`) adds `HttpClientAutoConfiguration` (`autoconfigure/http_client.rs`: one `Arc<dyn HttpClient>` bean from `http.client.*`, an `ssl-bundle` lookup with feature `ssl`) and a `pub(crate)` analyzer for invalid `http.client` configuration. Feature `oauth2` (implies `security` and `http-client`) makes `SecurityAutoConfiguration` configure OAuth2 login from `security.oauth2.*` (module `autoconfigure/oauth2.rs`, which also holds the section parsed once into a lazy crate-private bean; the `pub(crate)` `OAuth2Analyzer` in `analyzers/security.rs`) and adds `OAuth2ClientAutoConfiguration` (`autoconfigure/oauth2_client.rs`: one `OAuth2Client` bean over the `dyn HttpClient` bean and the clock bean, from the same parsed section). Features `data-postgres`, `data-mysql` and `data-sqlite` (each enables `oxbloom-data` with its driver and `retry`, and `oxbloom-actuator?/data`) add `DataAutoConfiguration` and the public `DataService` (`autoconfigure/data.rs`: the `DataSettings` and `DataSources` beans, the default pool bean `PgPool`, `MySqlPool` or `SqlitePool`, a service with phase `i32::MIN` that pings at start and closes at stop), the `ssl-bundle` lookup with feature `ssl`, and the `pub(crate)` data analyzers (`analyzers/data.rs`); `ActuatorAutoConfiguration` adds the `db` and `db.<name>` indicators through one `DataIndicators` bean. `oxbloom_boot::auto_configurations()` lists the ones of the enabled features. |
| `oxbloom-logging` | `tracing` subscriber from the `logging.*` keys: `Level`, `ParseLevelError`, `ConsoleFormat`, `ConsoleColor`, `LoggingError`, `LoggingSystem` (`init`, `set_level`, `reset`, `level`, `reload`), `PROPERTIES`, rotating file output (`tracing-appender`) and JSON lines with configurable fields. It depends on `oxbloom-config` and `oxbloom-core`; `oxbloom-boot` depends on it behind the feature `logging`. `Settings`, `__private`, `init_with_writer`, `init_with_clock` are internal test hooks (`#[doc(hidden)]`). |
| `oxbloom-test` | Test support, used as a dev-dependency: `ContextRunner` (hermetic boot lifecycle, `ContextView`, `FailureView`), `Captured`, `TempConfigDir`, `FixedClock`, `ManualClock`, `Log`. Feature `web` adds `TestClient`/`TestResponse` (in memory, served on port 0, connected) and `TestApp`/`RunningApp` with the presets `web`, `secured` (feature `security`) and `full`; `json` adds JSON helpers; `tls` adds `TestClient::connect_tls` and `RunningApp::client_tls` (server name `localhost`) and turns on boot's `ssl`. Only integration tests under `tests/` can use it (dev-dependency cycle), and its features unify into the crate under test; see ADRs 0067 and 0075. Feature `http-client` adds `MockHttpClient` and `MockResponse` (`mock_http.rs`); feature `oauth2` adds `FakeProvider`, `FakeUser`, `Fault` and `TokenRequest` (`fake_provider.rs`); feature `data` adds `TestDatabase` (`data.rs`, SQLite always; `data-postgres` and `data-mysql` add server databases from `OXBLOOM_TEST_POSTGRES_URL` and `OXBLOOM_TEST_MYSQL_URL`). The per-driver conformance suite (`data_conformance_*`) lives in `crates/oxbloom-test/tests/`, not in `oxbloom-data`, so that its features do not unify into the `oxbloom-data` test builds; the Postgres and MySQL tests are `#[ignore]`d and run with `make test-db`. |
| `oxbloom-ssl` | Named SSL bundles from `ssl.*` (PEM files or inline, PKCS#12): `SslBundles`, `SslBundle` (`server_config`, `client_config`, `validity`), `CertValidity`, `ReloadTask`, `SslError`, `PROPERTIES`. Polling reload, validity warnings on `oxbloom::ssl`, rustls with `aws-lc-rs` only. `SslBundle::trust_pem` and `identity_pem` give PEM for database clients (the key is a plain `Vec<u8>`: never log it). Feature `di` implements `Lifecycle` for `SslBundles`. Never depends on `oxbloom-web` or `oxbloom-security`. |
| `oxbloom-http-client` | Outbound HTTP: the `HttpClient` trait, `Request` (a bad header marks it invalid; every client calls `validate()`), `Response`, `HttpClientError` with fixed text, `Interceptor` and `InterceptedClient`. Features `json` and `reqwest` (default: `ClientBuilder`, `ReqwestClient` with a 10 s timeout, 1 MiB body limit, no redirects, HTTP/1.1 only), `retry` (`RetryingClient`, idempotent methods only, over `oxbloom-tasks`), `config` (`HttpClientSettings`, the `http.client.*` keys; implies `reqwest` and `retry`). See ADR 0093. |
| `oxbloom-data` | Pooled SQL on `sqlx` (PostgreSQL, MySQL, SQLite). Features `postgres`, `mysql`, `sqlite` (each also enables rustls with `aws-lc-rs`, never `ring`) and `retry` (additive: only adds `transaction_with_retry`). `DataSettings`/`SourceSettings` (`data.*` keys, `PROPERTIES`), `DataSources` (lazy pools, `build` returns a `SourceError`, `ping`, `ping_all`, `pool_counts`, `close_all`), `TlsMaterial`, `DataError`, `transaction` with `TransactionOptions`/`Isolation`/`TransactionError`, `RetryableError`. Errors and `Debug` never contain the URL, password, user, host or SQLite paths; never log `connect_options()` of a pool. Modules `settings.rs`, `pool.rs`, `sources.rs`, `transaction.rs`, `error.rs`. |
| `oxbloom-cli` | `oxbloom new` / `oxbloom list` project generator |
| `examples/todos` | A CRUD REST API (`#[controller]` with the verb macros, in-memory store, `Json<T>` and `Request::json` for its bodies, `HttpError` for its 404 and 422 answers, `SecurityCustomizer`, `tracing` events at target `todos`), with tests in `tests/api.rs` and `tests/logging.rs`. `make run-todos`. |
| `examples/hello` | Runnable demo of the current features (`#[configuration]`/`#[bean]`, trait and `Vec` injection with `primary`/`order`, a lazy bean with `pre_destroy`, `application.toml` with a `dev` profile, runner, listener, exit code, a `SecurityCustomizer` and `dyn Controller` (the chain and `WebServer` are auto-configured), custom `FailureAnalyzer`, an `#[auto_configuration]` default with a user override behind `hello.custom-greeter`), with in-process tests in `tests/app.rs` and `tests/auto.rs`. Keep it in sync when features land. |

## Workflow for every task

1. Inspect the repo and summarize the current state, risks and open questions in a few lines.
2. Pick one small, coherent step. Touch only the files it needs.
3. Write the failing test first, then the implementation (red, green, refactor).
4. Implement the feature for real. No placeholders presented as features.
5. Add the example for every new or changed public function (see the example rule).
6. Record every architectural decision of the step as an ADR (see "Decisions are ADRs").
7. Run the verification commands below and fix every failure.
8. Update the docs (see the docs update checklist).
9. Run `graphify update .` if `graphify-out/` exists.
10. Finish with: changed files, design decisions (with ADR numbers), exact test results, next sensible step.

## The Superpowers workflow for slices

Architectural slices (a roadmap entry such as C7) follow the Superpowers skills:

1. **Brainstorming.** Classify the request, ask one question at a time, present the design in sections
   and wait for approval of each. Write the spec to `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md`.
2. **Writing plans.** Turn the spec into a task plan in `docs/superpowers/plans/YYYY-MM-DD-<topic>.md`
   with global constraints, review focus and verification per task.
3. **Subagent-driven execution.** Per task one implementer and one task reviewer. Each fix round gets a
   scoped re-review; at most five fix rounds per task. After the last task, one final whole-slice review
   and one fix wave, no second wave. The controller never fixes code itself; it dispatches, reviews the
   reports and records rulings. A flaky test found later goes back to the original implementer.
4. Deferred minors from the reviews go to `docs/ROADMAP-core.md` under the slice ("Deferred minors from
   the ... reviews"). Stack probes must not grow (ADR 0047). After each slice, record the gate numbers
   (passed test count, feature-matrix counts, probe numbers) in the roadmap entry.

## Decisions are ADRs

- Every architectural decision (a choice between approaches, a dependency choice, a limit, a rejected
  alternative) is recorded as a new ADR in `docs/adr/`, in the same change that implements it.
- Copy `docs/adr/0000-template.md`, take the next free number, and add a row to the index in
  `docs/adr/README.md`. Link the ADR to its spec and plan, and link the spec or plan back to the ADR.
- Never edit the decision of an accepted ADR. Write a new one that supersedes it, say so in its Context,
  and set the old status to `Superseded by ADR NNNN`.
- Specs stay the design documents of a slice. Each decision made in brainstorming, and each controller
  `Ruling:` worth keeping long term, becomes an ADR. Ruling entries in the SDD ledger are scratch; ADRs
  are the record.

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

Run all of these before saying a task is done. The first block is what CI runs
(`.github/workflows/ci.yml`); keep the two in step when you add a feature or a crate.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo test -p oxbloom-web --no-default-features
cargo build -p oxbloom-boot --no-default-features
cargo build -p oxbloom-boot --no-default-features --features logging
cargo build -p oxbloom-boot --no-default-features --features web
cargo build -p oxbloom-boot --no-default-features --features security
cargo build -p oxbloom-boot --no-default-features --features ssl
cargo test -p oxbloom-ssl --all-features
cargo test -p oxbloom-web --features tls
cargo test -p oxbloom-test --features web
cargo test -p oxbloom-test --features web,tls
make check-examples
cargo doc --workspace --all-features --no-deps
```

The Postgres and MySQL conformance tests of the data core are `#[ignore]`d. `make test-db` starts both servers
from `compose.test.yaml` (needs a compose tool) and runs them; run it when `oxbloom-data`, its boot wiring or
`TestDatabase` changed.

Also run locally: `cargo test --workspace` (default features), `cargo test -p oxbloom-web --features di`,
`cargo test -p oxbloom-config --features di`, the boot matrix (`cargo test -p oxbloom-boot` with
`--no-default-features`, `--features web`, `logging`, `security`, `ssl` and `--all-features`), and the stack
probes when DI or boot changed.

Public APIs also need an integration test in the crate's `tests/`. Macro errors need a `trybuild`
case in `tests/ui/`; regenerate snapshots with `TRYBUILD=overwrite cargo test -p <crate>` and review
the diff.

Cargo rules:

- Never set `CARGO_TARGET_DIR` or pass `--target-dir`. The shared tmpfs target in `~/.cargo/config.toml`
  is deliberate; the root SSD stalls under cargo writes.
- Do not run long cargo loops in the background while an implementer builds.
- Flaky tests are fixed at their cause, never retried or marked ignored to get a green run.

## Git and commits

- Commits are signed (`commit.gpgsign=true` is configured in this repository). Do not disable signing.
- Messages are plain imperative sentences ("Add the CORS filter"), with no Claude, AI or
  `Co-Authored-By` trailers.
- Work on a feature branch named after the slice (for example `c7-ssl`). Make small commits, one per task.
- Never push, merge to `main` or tag without the user asking. Never force-push. Branch protection on
  `main` (rulesets) requires signed commits and linear history.

## Docs site and CI

- The site is built from `docs/` and `branding/` by `scripts/gen-book-summary.sh` (generates
  `docs/SUMMARY.md` and copies brand assets to `docs/assets`, both git-ignored) and `mdbook build`
  (theme in `theme/`, output in `book-out/`). `.github/workflows/pages.yml` deploys it with the rustdoc
  under `api/`. Run both commands after changing docs; the build must succeed.
- CI is `.github/workflows/ci.yml`: format and clippy, tests, the feature matrix, examples, docs.
  Releases come from `v*` tags (`.github/workflows/release.yml`).
- Dependabot PRs are never merged blind. Apply the update locally with `cargo update -p <crate>`, run the
  full gate, then close the PR with a comment (or merge it when the user asks).

## Docs update checklist

Per slice, or per task that changes public behavior:

- `docs/API.md`: every new public item with its example.
- The guide chapter under `docs/guide/`. Chapters are numbered files; keep the numbers contiguous and fix
  cross-links when you add or move one.
- `README.md` only for status or links.
- `docs/ROADMAP-core.md`: the delivered list, deferred minors and gate numbers.
- The ADRs in `docs/adr/` and their index.
- `graphify update .`.

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
- Docs, comments, specs, plans and commit messages are plain full sentences.

## Security rules

- Never publish keys, tokens or personal data (including the maintainer's email address) in files,
  commits, issues or requests to external services.
- Test certificates and keys are generated at test time; never commit key material (ADR 0073).
- Before making anything public, check `git ls-files` for secrets and personal data.

## Planned work (not started, do not describe as available)

Slices 7a, 7b, 8 (method security) and 9 (parts A: HTTP client, B: OAuth2 login, C: OAuth2 client) are done;
health and observability, database, jobs/queue/scheduler, React integration, `oxbloom-ai`,
`oxbloom add`, MVC web style, DI scopes (transient, request). See `docs/ROADMAP-core.md`.

`oxbloom-ai` rules when you get there: provider-neutral traits, adapters behind features or
crates, mock provider for tests (no API key needed), keys only from environment, timeouts and
bounded retries, tool calls never run on model output alone: the application decides and approves.

## Asking vs. deciding

Unclear requirement: ask one targeted question. Otherwise take the conservative, extensible
default, say which you chose, and record it as an ADR when it is architectural. Confirm before deleting
files or other hard-to-reverse changes.
