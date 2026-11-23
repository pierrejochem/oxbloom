# Core feature roadmap (from spring-boot/core)

Date: 2026-10-24

Source: the module and package names under `spring-projects/spring-boot/core` (`spring-boot`, `spring-boot-autoconfigure`, `spring-boot-autoconfigure-processor`, `spring-boot-test`, `spring-boot-test-autoconfigure`, `spring-boot-docker-compose`, `spring-boot-testcontainers`, `spring-boot-properties-migrator`). The GitHub listing shows names only. What each package does is taken from knowledge of Spring Boot, so each slice's design step must re-check Spring Boot's reference documentation before the spec is written.

Each item below becomes its own slice with its own spec, plan and review, as in Slices 1 to 5. Slices marked done are implemented; the rest are not. Existing crates: `oxbloom-core`, `-macros`, `-web`, `-di`, `-cli`, `-config`, `-security`, `-boot`, `-logging`, `-test`, `-ssl`.

## What Spring Boot core has, and what we have

| Spring Boot area (package) | What it provides | Oxbloom today |
|---|---|---|
| `SpringApplication`, runners, exit codes, banner, startup logging, shutdown hooks, run listeners, `ApplicationArguments` | One entry point that builds the environment, wires the context, runs `CommandLineRunner`s, reports startup, handles shutdown and exit codes | `oxbloom-boot` (C3 done): `OxbloomApplication`, runners, services, listeners, signals, exit codes, banner, `ApplicationArguments`, failure analyzers and the startup failure report; no startup logging |
| `context`, `env`, `convert`, `origin`, `validation`, `bootstrap` | Property sources, profiles, `@ConfigurationProperties` binding, relaxed binding, `Duration`/`DataSize` conversion, value origins in errors, validation, `${placeholder}` resolution, `EnvironmentPostProcessor` | `oxbloom-config`: TOML files, profiles, env overrides, serde binding, `Secret` |
| `autoconfigure.condition`, `autoconfigure` (all areas), `autoconfigure-processor` | `@ConditionalOnProperty/Bean/MissingBean/Class`, ordered auto-configurations, metadata generation | C4a done: conditions on registrations and groups (`on_bean`, `on_missing_bean`, `on_property*`, custom), `Environment`, the conditions report, `#[auto_configuration]` with `after`/`before`, explicit `AutoConfigurations` lists and `exclude` in `oxbloom-boot`. The web server, security chain and JWT decoder are auto-configured (C4b) and auto-configurations describe their keys as `PropertyMetadata` (hand-written, no generation from structs) |
| Bean container features used by Boot (lazy init, lifecycle, `ApplicationContextFactory`) | Lazy beans, init and destroy callbacks, primary/qualifier, collection injection, factory methods, profiles on beans | `oxbloom-di` (C1 done): lazy beans, `post_construct`/`pre_destroy` and ordered shutdown, `primary`/`order`, trait, `Option` and `Vec` injection, `bean` factories and `#[configuration]`/`#[bean]`, profile conditions, and (C4a) conditions on registrations and groups; no name qualifiers |
| `logging` | Logging system abstraction, levels and groups from properties, file and structured (JSON) output | `oxbloom-logging` (C5a and C5b done): `tracing` subscriber from `logging.*` keys, levels, groups, console format and color, rotating file output, JSON lines with configurable fields, runtime level change through `LoggingSystem`, startup events in `oxbloom-boot` (feature `logging`); `RequestLogFilter` in `oxbloom-web` and `RequestLogAutoConfiguration` in `oxbloom-boot` (feature `web`). Size-based rotation and runtime changes of the output layers are open |
| `availability`, `info`, `admin`, `cloud` | Liveness and readiness state, build and git info, platform detection (Kubernetes) | `oxbloom-actuator` (C8 done): liveness and readiness state flipped by boot at `ready` and `stopping`; `/health`, `/health/liveness`, `/health/readiness`, `/info` and `/loggers` under a base path (default `/actuator`); `HealthIndicator` and `InfoContributor` beans with a timeout per indicator; a built-in SSL certificate indicator; build and git info through the application's `build.rs` and the `build_info!` macro; Kubernetes detection. No separate management port, no roles per endpoint, no metrics |
| `web` (server properties, error handling, forwarded headers, compression, static resources, JSON) | `server.*` properties, error attributes and problem details, content negotiation, JSON conversion | `oxbloom-web` (C6a, C6b-1 and C6b-2 done): router, Hyper server, ordered `Filter` chain (`order` module), `HttpError` and `ErrorRenderer` with problem+json, `server.*` limits, shutdown timeout and forwarded headers. It also has `Json<T>`, `Request::json` and `Accept` negotiation (feature `json`). It also has a CORS filter, a compression filter (gzip, brotli, zstd) and static resources (disk and embedded). |
| `ssl` | SSL bundles from PEM or keystores, reloadable | `oxbloom-ssl` (C7 done): named bundles from PEM and PKCS#12, polling reload, validity warnings; HTTPS for the web server (`server.ssl.bundle`), a bundle for the JWKS fetcher (`security.jwt.jwks-ssl-bundle`) and `TestClient::connect_tls`. No mutual TLS, no JKS, no client reload |
| `task`, `thread`, `retry` | Task executor and scheduler properties, retry policy | `oxbloom-tasks` (C9a and C9b done): `#[scheduling]` and `#[scheduled]` with cron (six fields, Spring day-of-week numbers, IANA zones, daylight saving rules), fixed rate and fixed delay; a `Scheduler` with one clock-driven loop per task, skip on overlap, a concurrency bound and a bounded shutdown; the `tasks.scheduling.*` keys; `TasksAutoConfiguration` in `oxbloom-boot` (feature `tasks`). `RetryPolicy`, `#[retryable]` and `RetryAutoConfiguration` with the `tasks.retry.*` keys (C9b done); persistence, distributed locking and run-time changes of the task list are open |
| `json` | Mapper configuration, JSON parser abstraction | `serde_json` used directly |
| `diagnostics` (`FailureAnalyzer`) | Human-readable startup failure reports with a suggested action | `oxbloom-boot` (C3b, C4b and C5a done): `FailureAnalyzer`, `FailureAnalysis`, built-in analyzers for a missing config key, bean wiring, port in use, security build errors, invalid `security.jwt` configuration and invalid `logging` configuration (the last two are internal analyzers, not public items) |
| `spring-boot-test` and `-test-autoconfigure` | `@SpringBootTest`, `ApplicationContextRunner`, output capture, test property sources, sliced tests (web, JSON), mock web client | C10a and C10b done: `oxbloom-test` has `ContextRunner`, captured output, temporary config directories, `Clock`s, `TestClient` and `TestApp` with slice presets |
| `spring-boot-testcontainers`, `-docker-compose`, `autoconfigure.service.connection` | Service connections from containers or compose files into config | None |
| `spring-boot-properties-migrator` | Warns about renamed or removed config keys | None |

Not applicable to Rust, so deliberately skipped: `aop`, `jmx`, `admin` (JMX), classpath scanning, virtual-thread properties, `template` engines (revisit only if an MVC view layer is built).

## Proposed slices, in dependency order

Numbering continues the security roadmap (Slice 6 to 9 stay as planned). Core slices carry the letter C and are interleaved.

### C1: DI v2 (`oxbloom-di`) — done
Done on 2026-10-24; design in `docs/superpowers/specs/2026-10-24-di-v2-design.md`, plan in `docs/superpowers/plans/2026-10-24-di-v2.md`, usage in `README.md` and `docs/API.md`.

Delivered:
- Trait and collection injection: `Arc<dyn Trait>`, `Option<Arc<..>>` and `Vec<Arc<..>>`, with `provides`, `primary` and `order`.
- Lifecycle: `post_construct` and `pre_destroy` hooks (also through the `Lifecycle` trait), `Container::shutdown` in reverse creation order with a `ShutdownReport`.
- Lazy beans, serialized by one reentrant container lock.
- Typed bean factories: `.bean` and `.try_bean` with 0 to 12 injected parameters, and `#[configuration]`/`#[bean]`.
- Profile conditions (`profile`, `profile_not`, `!name`, any-of), with the active names passed through `ContainerBuilder::active_profiles`.
- A dependency depth limit of 256, a `#[non_exhaustive]` `DiError` with source labels, and per-bean error caching (except `Cycle` and `TooDeep`).

Follow-ups, not yet done:
- A per-trait `primary`: today `primary` applies to every key of the registration.
- Extending `Inject` with more shapes; the trait is sealed for now.
- A `cfg_attr` nested inside another `cfg_attr` on a `#[bean]` method is not copied to its registration and fails to compile (E0599) when the method is compiled out.
- A `#[bean(lifecycle)]` option (still open after C3). Today `#[bean]` methods cannot take lifecycle hooks; register the resource as an `instance` or `factory` with `.lifecycle::<T>()`, or add `.pre_destroy` after `.bean`.
- Boxing the `DiError` payloads to shrink stack frames, so deep graphs need less stack (a 256-deep debug build needs about 206 to 322 KB today).
- `ContainerBuilderExt::config` in `oxbloom-config` does not forward the active profiles; applications call `.active_profiles(config.active_profiles().iter().cloned())` themselves.

### C2: Configuration v2 (`oxbloom-config`)
Fixes known gaps (list values from env, duplicate-spelling errors) and adds Boot's binding features.
- `#[config_properties(prefix = "server")]` macro (already planned): binds, validates, registers as a bean.
- Validation hook (a `Validate` trait, optional `validator` crate feature) with errors that name the key.
- Relaxed binding: `kebab-case`, `snake_case` and `camelCase` map to the same field in files and env.
- Typed units: `Duration` (`30s`, `5m`), `DataSize` (`10MB`), booleans and enums from strings.
- List and map overrides from env (`APP_X__0=`, comma-separated values).
- `${placeholder}` resolution with defaults, and `oxbloom.config.import` of extra files.
- Command-line property source (`--server.port=9000`) and profile groups.
- Value origin tracking: errors say "from application-dev.toml line 4" or "from env APP_X" (never the value).
- Deprecated and renamed key migration warnings (the `properties-migrator` equivalent).
- Config metadata generation by a derive macro, for IDE and CLI use (the `autoconfigure-processor` equivalent for properties).

### C3: Application lifecycle (`oxbloom-boot`, new crate)
The `SpringApplication` equivalent that ties config, DI, web and security together. It was split into C3a and C3b; both are done.

#### C3a: `oxbloom-boot` — done
Done on 2026-10-25; design in `docs/superpowers/specs/2026-10-25-boot-design.md` (with its Amendments section), plan in `docs/superpowers/plans/2026-10-25-boot.md`, usage in `README.md` (section "Application lifecycle") and `docs/API.md`.

Delivered:
- `OxbloomApplication::builder()...run()` and `run_and_exit()`: load config and profiles, build the container, run `ApplicationRunner` and `CommandLineRunner` beans, start and stop `Service` beans in phase order under one shutdown deadline, handle Ctrl-C and SIGTERM, shut the container down.
- Run listeners and lifecycle events (`starting`, `environment_prepared`, `context_ready`, `started`, `ready`, `stopping`, `stopped`, `failed`).
- Exit codes (`ExitCodeGenerator`, `ExitStatus`), `StartupError` with `StartupPhase`, `ShutdownHandle`, a banner with `oxbloom.main.banner-mode`.
- Command-line arguments (`ApplicationArguments`).
- Cargo feature `web`: `WebServer`, a `Service` that serves an `oxbloom_web::App` from `server.address` and `server.port`. `examples/hello` uses it.

#### C3b: Failure analysis — done
Done on 2026-10-25; design in `docs/superpowers/specs/2026-10-25-boot-failure-analysis-design.md` (with its Amendments section), plan in `docs/superpowers/plans/2026-10-25-boot-failure-analysis.md`, usage in `README.md` (section "Application lifecycle", subsection "Failure analysis") and `docs/API.md`.

Delivered:
- `FailureAnalyzer` and `FailureAnalysis`, registered on the builder or as beans, panic-isolated, with a startup failure report (description and action) printed by `run_and_exit()` and, with `failure_report(true)`, by `run()`.
- `StartupError::find_cause` and `StartupError::analysis`.
- A typed source on `DiError::Creation` (`ErrorSource`, `DiError::creation_from`), and `From<ConfigError>` and `From<SecurityBuildError>` for `DiError` under the `di` features, so `?` in `try_bean` keeps the typed error.
- Built-in analyzers: missing config key, bean wiring (cycle, missing, ambiguous, duplicate), port in use (feature `web`, with the public `BindError`), security build errors (feature `security`).
- The CLI template (`oxbloom new`) starts the project through `OxbloomApplication`, and `--oxbloom-path` must contain the four crates the template depends on.
- `examples/hello` reworked into a small application that shows the features available today.

Follow-ups from C3b, not yet scheduled:
- Boot writes its diagnostic line `service X failed to start` before the failure report; with the report in place that line is redundant and could be dropped or folded into the report.
- `find_cause::<DiError>()` returns the first `DiError` in the chain, which is the `Creation` error of a bean factory, so a wiring error nested below a `Creation` would not be analyzed. Nothing produces that shape today; a walk that skips `Creation` errors would cover it.
- Analysis of failures after startup (a crashed serving task, a failing service at runtime) is not covered; analyzers see startup failures only.
- The bad-pattern report cannot name the pattern, because `SecurityBuildError::InvalidPattern` does not carry it.
- Analyzer text is printed as given; a check that rejects or escapes the block markers is not implemented.
- Adding `source` to `DiError::Creation` was a breaking change for external code that builds or destructures the variant without `..`; marking the variants' fields `#[non_exhaustive]` would make future additions non-breaking.

Follow-ups from C3a, owned by later slices or not yet scheduled:
- Automatic registration of `WebServer` and other services: C4b (done).
- Logging and log-based startup information: C5a (done). Boot still writes its banner and the reports to a `Write` target that defaults to stderr; the startup events go to `tracing`.
- The full `server.*` property set (limits, compression, forwarded headers): C6.
- Readiness and liveness state: C8 (done).
- `WebServer` minor issues: a panic or IO error of the serving task is dropped silently in `stop`; a failed bind consumes the `App`, so the same `WebServer` cannot retry; a `stop` that is abandoned at the deadline leaves the serving task detached instead of aborting it.
- `WebServer`: if the serve task ends by itself (IO error) nothing triggers shutdown, so `run()` keeps waiting with no server.

### C4: Conditions and auto-configuration
Depends on C1, C2, C3. Split into C4a (the mechanism) and C4b (using it).

#### C4a: Conditions and the auto-configuration mechanism — done
Done on 2026-10-25; design in `docs/superpowers/specs/2026-10-25-conditions-autoconfig-design.md` (with its Amendments section), plan in `docs/superpowers/plans/2026-10-25-conditions-autoconfig.md`, usage in `README.md` (section "Conditions and auto-configuration") and `docs/API.md`.

Delivered:
- Conditions in `oxbloom-di` as modifiers on any registration: `condition`, `on_bean`, `on_missing_bean`, `on_property`, `on_property_value`, `on_property_value_or_missing`, evaluated once in registration order against the registrations kept before, combined with AND and stopping at the first one that does not match.
- Groups with class-level `Conditions`, decided once at their first kept registration; nested groups AND their conditions.
- The `Environment` trait and `impl Environment for Config` (feature `di` of `oxbloom-config`).
- The `ConditionReport` (`build_with_report`, `Container::condition_report`) and the `oxbloom.main.conditions-report` setting (`off` or `console`).
- `#[bean(on_bean(T), on_missing_bean, on_missing_bean(T), on_property(..))]` options.
- `#[auto_configuration(after(..), before(..), on_..)]`, the `AutoConfiguration` trait, `ConfigRef`, `AutoConfigurations` lists, and the builder methods `auto_configuration`, `auto_configurations` and `exclude`. Auto-configurations are registered after the user's wiring, ordered by a stable topological sort; a cycle is a `Container`-phase error with its own failure analyzer.
- `examples/hello` shows an auto-configured default and a user override behind a configuration flag.

#### C4b: Auto-configure the existing wiring — done
Done on 2026-10-25; design in `docs/superpowers/specs/2026-10-25-web-security-autoconfig-design.md` (with its Amendments section), plan in `docs/superpowers/plans/2026-10-25-web-security-autoconfig.md`, usage in `README.md` (section "Auto-configured web and security") and `docs/API.md`.

Delivered:
- In `oxbloom-boot`, behind the `security` and `web` features, three hand-written auto-configurations: `JwtDecoderAutoConfiguration` (applies when the `security.jwt` section exists), `SecurityAutoConfiguration` (a closed chain that authenticates every request unless customized) and `WebServerAutoConfiguration` (applies when a `dyn Controller` bean exists), each backing off to a user bean of the same type. `auto_configurations()` returns the list for the enabled features.
- Contributor beans: `dyn Controller` for routes (`Controller` now has `Send + Sync` supertraits), the new `SecurityCustomizer` trait for rules, and `dyn Filter` for extra filters.
- Property metadata: `PropertyMetadata` with `to_json`, `boot_properties()`, `AutoConfiguration::properties()` and `OxbloomApplicationBuilder::property_metadata()`, which lists the keys of the registered, not excluded auto-configurations.
- `Environment::contains`, so `on_property("security.jwt")` matches a section, with an override for `Config`.
- A built-in analyzer for invalid `security.jwt` configuration (feature `security`, not a public item): a failure report that names the key and never its value.
- `examples/hello` drops its own chain and server wiring and contributes a `SecurityCustomizer` and a `dyn Controller` instead.
- Stack probes unchanged: `build`'s frame is 0x178 and the 256-deep minima are 206 KB (closures), 277 KB (`#[component]`), 230 KB (`#[bean]`), 194 KB (`.bean`) and 241 KB (boot).

Follow-ups from C4b, not yet scheduled:
- A macro option for `properties` on `#[auto_configuration]`; today the method is written by hand.
- Metadata checked against the structs that read the keys; today tests pin the lists by hand.
- The `server.*` extras (limits, compression, forwarded headers) stay with C6; only `server.address` and `server.port` are described.
- The opt-in `registry` feature and exclusion through configuration keys (see the C4a follow-ups) are still open.
- `SecurityCustomizer`s cannot each add rules, because `HttpSecurity::authorize` may be called once; a way to merge rule sections would allow several rule-contributing beans.
- The two moved trybuild cases (`tests/ui/autoconfig_no_impls`) run only without the `security` and `web` features, since rustc then lists the built-in `AutoConfiguration` implementors in its output.
- `on_property` now sees tables and lists as present for `Config`; a custom `Environment` must override `contains` to do the same.
- The metadata type name of `server.address` is `IpAddr`; a host name is rejected until C6.

Follow-ups from C4a, not yet scheduled:
- An opt-in `registry` feature that collects auto-configurations at link time; the explicit lists stay the only mechanism until then.
- Exclusion of auto-configurations through configuration keys.
- The missing-bean failure report could name the registrations that a condition skipped.
- Condition options on `#[component(..)]`; the builder modifiers cover it today.
- Boot clones `Config` to use it as the container environment; a shared adapter behind an `Arc` would avoid the second copy.
- Two texts for modifier misuse coexist: condition modifiers say "needs a registration before it", the older modifiers say "must follow a registration". One text would be clearer.
- The conditions report and the failure report are printed one after another without a blank line between them.

### C5: Logging (`oxbloom-logging`)
Split into C5a and C5b. Both are done.

#### C5a: Logging system, levels, groups and startup logging — done
Done on 2026-10-25; design in `docs/superpowers/specs/2026-10-25-logging-design.md` (with its Amendments section), plan in `docs/superpowers/plans/2026-10-25-logging.md`, usage in `README.md` (section "Logging") and `docs/API.md`.

Delivered:
- The new crate `oxbloom-logging`: a `tracing` subscriber built from the `logging.*` keys, with `Level`, groups (`logging.group.<name>`), console format (`full`, `compact`, `pretty`) and color (`auto`, `always`, `never`), the `log` facade bridged through `tracing-log`, and `LoggingSystem` with `init`, `set_level`, `reset`, `level` and `reload` for runtime change. `PROPERTIES` lists the keys.
- In `oxbloom-boot`, behind the new feature `logging`: the subscriber is installed right after the configuration loads, the `LoggingSystem` is a bean, `.logging(false)` and `oxbloom.logging.enabled = false` skip the install, a built-in analyzer explains invalid `logging` configuration, and boot logs its startup and shutdown events with the target `oxbloom::boot`. `application.name` names the application in those events and is listed in `boot_properties()`.
- The new example `examples/todos`, a CRUD API that uses the web, security and logging stack (`make run-todos`). `examples/hello` enables the feature.
- Stack probes: the DI minima (206, 277, 230 and 194 KB) and the frame of `build` (0x178) are unchanged. The boot probe was 241 KB before C5a. At the end of C5a it is 238 KB without the feature `logging` and 239 KB with it (also with all features). Boot keeps the `Config` given to `with_config` and the logging settings boxed, so the feature adds one pointer to the builder and to the run plan.

#### C5b: File output, JSON output and request logging — done
Done on 2026-11-01; design in `docs/superpowers/specs/2026-11-01-logging-output-design.md`, plan in `docs/superpowers/plans/2026-11-01-logging-output.md`, usage in `README.md` (section "Logging") and `docs/API.md`.

Delivered:
- File output in `oxbloom-logging` through `tracing-appender`: `logging.file.enabled|path|name|rotation|max-history|format`, rotation `hourly`, `daily` or `never` (`FileRotation`), line layout `full`, `compact` or `json` (`FileFormat`), never colored. A directory or file that cannot be created or opened fails startup with the new `LoggingError::Io`, which carries the path.
- JSON output with a custom formatter shared by console and file: `ConsoleFormat::Json`, `logging.json.fields.<field>` toggles and `logging.json.names.<field>` renames for `timestamp`, `level`, `target`, `message`, `span` and `thread` (`JsonField`). Duplicate or empty names, unknown fields and a `max-history` of 0 are `ConfigError::Invalid` naming the key. `PROPERTIES` now lists thirteen keys.
- `RequestLogFilter` in `oxbloom-web` and `RequestLogAutoConfiguration` in `oxbloom-boot` (feature `web`), with `logging.request.enabled|headers|level`. The logging failure analyzer in boot covers the new keys and the I/O error.
- The `README.md` section "Logging", `docs/API.md` and the example `examples/todos` (commented sample of the new keys) describe all of it.
- Stack probes unchanged, re-measured at the end of C5b with the manual probe tests (`OXBLOOM_DI_STACK_KB` in `crates/oxbloom-di/tests/depth.rs` and `OXBLOOM_BOOT_STACK_KB` in `crates/oxbloom-boot/tests/conditions_report.rs`): the DI minima are 206, 277, 230 and 194 KB, the frame of `build` is `sub $0x178,%rsp`, and boot needs 238 KB without the feature `logging` and 239 KB with it (also with all features).

Limits that users must know (also in the README):
- `tracing-appender` prunes by file-name prefix. With `max-history`, every file in the log directory whose name starts with the configured prefix is a candidate, including unrelated ones (prefix `app` matches `application.log`). With `rotation = "never"` and `max-history = 1` the existing file is deleted at start. Use a dedicated directory and a distinctive prefix.
- The file writer is non-blocking and lossy: when its buffer fills, events are dropped rather than blocking the caller. Buffered lines are flushed when the last `LoggingSystem` clone drops, which boot does when `run()` returns; events logged after that are not in the file.
- `LoggingSystem` creates the log directory and file and starts the writer before installing the subscriber, so an `AlreadyInstalled` init leaves an empty log file. `logging.file.enabled = true` is ignored when logging is disabled (`oxbloom.logging.enabled = false` or `.logging(false)`).
- Two `LoggingSystem` builds in one process on the same directory run two writers on one file, and both prune it (only matters in tests).
- The request log header allowlist is the operator's responsibility. Always redacted: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key` and `x-auth-token`, in any case and any `-` or `_` spelling (not a `.` spelling), and any value flagged sensitive. Values print as `name="value"`, redacted ones as `name=<redacted>`. The query string is never logged.
- A `logging.request.level` below the root or target level logs nothing unless `logging.level.oxbloom::web::request` (or the root) is lowered to match.
- (Fixed in C6a.) The request log used to run after the security chain, so 401, 403 and 413 answers were not logged. It now takes the outermost slot and records them.
- The output layers (console, file, JSON options) are fixed when the subscriber is installed; `set_level`, `reset` and `reload` change levels only.
- JSON line shape: keys are the configured names of the enabled fields; event fields other than `message` are extra keys; a built-in key wins over an event field of the same name; `span` is the innermost span as a flat object with its fields plus `name` (the span name wins over a span field called `name`); `timestamp` is RFC 3339 UTC with millisecond precision; `level` is lowercase. Key order in a line is alphabetical and not guaranteed.

Follow-ups from C5b, not yet scheduled:
- Size-based rotation (rotation is time-based only).
- Filter ordering of the request log relative to the security chain (done in C6a).
- Applying changes of `logging.console.*`, `logging.file.*` and `logging.json.*` at runtime.
- Logging the requests that the security chain rejects (done in C6a).
- Moving the `PROPERTIES` rows of `logging.request.*` next to the filter (today they are described by `RequestLogAutoConfiguration::properties()` in `oxbloom-boot`, not by `oxbloom_logging::PROPERTIES`).
- `logging.request.level` accepts only `error` to `trace`; `off` is rejected on purpose, so the way to silence the request log is `logging.request.enabled = false`.

Deferred minors from the C5b reviews:
- JSON: a built-in key silently overwrites an event field of the same name and a span field called `name` is overwritten by the span name (both documented, untested); events bridged from `log` carry `log.*` fields; only the innermost span is emitted.
- File output: the max-history test asserts at most 3 files rather than exactly 3; `LoggingSystem` holds the appender guard behind `#[expect(dead_code)]` (could be a `_guard` field); empty `logging.file.path` and `logging.file.name` are rejected only when file output is enabled, and so is `max-history = 0`.
- Settings: a non-string `application.name` is ignored silently when choosing the default file name.
- Request log: work is done per request even when the event level is disabled (no `tracing::enabled!` check); invalid allowlist entries match nothing and are dropped without a warning; there is no event when the inner handler panics; names with a `.` instead of `-`/`_` are not normalized; the outer `headers` and `path` fields are printed unquoted by the subscriber, which can confuse strict logfmt parsers.
- Tests: the `bogus = false` assertion in `oxbloom-logging/tests/settings.rs` passes the whole TOML line as the forbidden string; a comment in `oxbloom-boot/tests/request_log.rs` about raising the target through the root is stale; the invalid-level test lacks a non-`true` `enabled` case; `RequestLogAutoConfiguration` metadata is described as pinned by a test but only the doctest checks the keys.

Follow-ups from C5a, not yet scheduled:
- A configuration watcher or a SIGHUP handler that calls `LoggingSystem::reload`; today the call is explicit.
- An HTTP endpoint to read and change levels at runtime (C8, done: `/loggers` in `oxbloom-actuator`).
- MDC-style context fields on every event.
- A macro option for property metadata, shared with the C4b follow-up.
- `reload` validates `logging.console.*` but cannot apply it, because the output layer is fixed at install time (still open after C5b, now also for `logging.file.*` and `logging.json.*`).
- Per-appender level filters.

### C6: Web server properties, errors and JSON (`oxbloom-web`)
Split into C6a, C6b-1 and C6b-2. All three are done.

#### C6a: Filter order, uniform errors and `server.*` properties — done
Done on 2026-11-01; design in `docs/superpowers/specs/2026-11-01-web-errors-ordering-design.md`, plan in `docs/superpowers/plans/2026-11-01-web-errors-ordering.md`, usage in `README.md` (section "Errors and filter order") and `docs/API.md`.

Delivered:
- Filter order: the `oxbloom_web::order` constants (`REQUEST_LOG` -400, `ERROR_HANDLING` -300, `BODY_LIMIT` -200, `SECURITY` -100, `DEFAULT` 0; lower runs first, ties keep registration order), the provided method `Filter::order()`, `Ordered` and `App::filter_ordered`. Boot registers the security chain and the `dyn Filter` beans with plain `App::filter`, and each declares its own slot. The request log is now outermost and records 401, 403, 404, 405, 413 and 500; an oversized body gets 413 before security runs.
- Errors: `HttpError` (status, title, detail, type URI, string, integer and boolean extensions), `ErrorValue`, `ErrorAttributes`, `IntoResponse for Result<T, E>`, `ErrorRenderer`, `RequestInfo` and the default `ProblemRenderer` (RFC 9457 `application/problem+json`). The built-in error filter renders `HttpError`, the framework answers (404, 405, 413, a panic as 500) and the 401, 403 and 503 of the security chain, keeping headers such as `WWW-Authenticate` and `Allow`. A renderer is replaced by a `dyn ErrorRenderer` bean or `App::error_renderer`. An author `detail` is always shown; `server.error.include-message` (`never` default, `always`) only controls framework-supplied messages (the panic message). A response with a body is never rewritten, and `HEAD` error answers carry no body.
- `server.*` in `oxbloom-boot`: `shutdown-timeout` (30s), `max-body-bytes` (1 MiB), `header-read-timeout` (10s, above 0 and at most one day), `forward-headers.strategy` (`none`), `forward-headers.trusted-proxies` (loopback), `error.include-message` (`never`), with `App::shutdown_timeout`, `App::forwarded` and the existing `max_body_bytes` and `header_read_timeout` as the code-level equivalents. An invalid value fails startup naming the key and never the value.
- Forwarded headers: `ClientInfo`, `Scheme`, `ForwardedConfig`, `ForwardStrategy`, `TrustedProxies`, `ProxyParseError` and `Request::client_addr`, `scheme` and `host`. `Forwarded` is read first, else `X-Forwarded-*`; the hops are walked from right to left and the first untrusted hop is the client; an invalid element makes the header unusable. The proxy must set or strip these headers; the README and `docs/API.md` say so prominently.
- Shutdown: `server.shutdown-timeout` drops connections that stay open and logs a `warn` event on target `oxbloom::web`.
- The example `examples/todos` answers 404 and 422 with `HttpError` and a `detail`, and its configuration has a commented `server.*` sample.
- Stack probes unchanged, re-measured at the end of C6a with the manual probe tests (`OXBLOOM_DI_STACK_KB` and `OXBLOOM_BOOT_STACK_KB`): the DI minima are 206, 277, 230 and 194 KB, the frame of `ContainerBuilder::build` is `sub $0x178,%rsp`, and boot needs 238 KB without the feature `logging` and 239 KB with it (also with all features).

Limits that users must know (also in the README):
- A panic in a filter ordered before `ERROR_HANDLING` is caught by the outer catch and answered as 500, but it unwinds through every filter outside it: the request log event is lost when the request log itself panics or when any filter ordered between `REQUEST_LOG` and `ERROR_HANDLING` (for example at -350) panics. A filter ordered before `REQUEST_LOG` loses the event only when it panics before it calls `next.run`. Panics in handlers and in filters after `ERROR_HANDLING` are answered by the error filter and logged with status 500.
- The panic message is in the `error` log event `request panicked: ...` on target `oxbloom::web`, and Rust's default panic hook also prints it to standard error. It never reaches the response with `include-message = never`.
- `HttpError` extension values are strings, integers and booleans only, and duplicate keys can produce duplicate members.
- Statuses without a reason phrase (499, 599) render an empty `title`.
- A rendered response drops the extensions of the original response.
- A renderer that panics escapes the outer catch.
- `ErrorAttributes` on a 2xx or 3xx status is rendered as problem+json.
- Forwarded headers are only safe behind a proxy that sets or strips `Forwarded`, overwrites or strips `X-Forwarded-Proto` and `-Host`, and appends to `X-Forwarded-For`. The framework cannot detect a proxy that does not. An empty `trusted-proxies` list trusts nobody, and the default list is loopback only. Nothing in security uses the client address yet. `Request::client_addr()` is `None` under `App::handle`.

Follow-ups from C6a, not yet scheduled:
- A forwarded-header strategy that names which header family to read (`Forwarded` only, or `X-Forwarded-*` only).
- Using the client address in security rules.
- Per-route error renderers.
- Exception-type handlers (Spring `@ExceptionHandler`).

Deferred minors from the C6a reviews:
- Errors: the `error_value_conversions` test is weak; `instance_never_contains_a_query` in the render tests cannot fail (the real test is in `tests/errors.rs`); rendering of statuses without a reason phrase is untested; duplicate extension keys are not deduplicated.
- Pipeline: `Ordered` derives nothing (no `Debug` or `Clone`); the panic catch costs two `Box::pin` allocations per request; no test checks that bodiless 204 and 304 answers pass through; the 404 plus panic request-log scenario asserts a strict event count.
- Forwarded: `is_authority` accepts trailing-dot, consecutive-dot and leading-dash host names and a literal `%`; `::/0` excludes IPv4-mapped peers (they are canonicalized before matching); `ClientInfo.addr` keeps the IPv4-mapped form; there are no tests for `::ffff:10.0.0.0/104` and a /31 boundary.
- Boot: non-table parents of `server.*` keys are ignored and the default applies; the config failure analyzer handles only missing keys, so invalid `server.*` values get the generic container report; the header-read-timeout error text says "1 millisecond" but the parser also accepts `ns` and `us`; `defaults_for_every_new_key` is weak (the real defaults are asserted in `web.rs` and in an HTTP test).

#### C6b-1: JSON bodies and content negotiation — done
Done on 2026-11-01; design in `docs/superpowers/specs/2026-11-01-web-json-design.md`, usage in `README.md` (section "JSON and content negotiation") and `docs/API.md`.

Delivered:
- Cargo feature `json` of `oxbloom-web`, in `default`. Without it `Json`, `JsonBody` and `Request::json` are not compiled and `serde_json` is not a dependency.
- `Json<T>` response (200, `application/json`, tagged with a `JsonBody` extension; `(StatusCode, Json<T>)` for another status; a serialization failure is a 500 without `detail`, logged at `error` on `oxbloom::web`).
- `Request::json::<T>()` returning `Result<T, HttpError>`: 415 with `accepted = "application/json"` for a missing or non-JSON `Content-Type` (`application/json` or `+json`, parameters and case allowed), 400 with `invalid JSON body: <class> error at line L column C` (class `syntax`, `data` or `eof`).
- The built-in negotiation filter at the new slot `order::NEGOTIATION` (-250, between `ERROR_HANDLING` and `BODY_LIMIT`; the constant exists without the feature). It answers 406 with `supported = "application/json"` for a `Json<T>` response that `Accept` excludes, also for `HEAD`. Ranges are `*/*`, `type/*` and `type/subtype`; `q` follows RFC 9110; the most specific range decides; no header or no valid range means acceptable.
- `examples/todos` drops its hand-written JSON helpers and uses `Json<T>` and `Request::json`.
- Stack probes unchanged, re-measured at the end of C6b-1 with the manual probe tests: the DI minima are 206, 277, 230 and 194 KB, the frame of `ContainerBuilder::build` is `sub $0x178,%rsp`, and boot needs 238 KB without the feature `logging` and 239 KB with it. The colored startup banner first grew the boot probe by 1 KB because `Banner` stored two strings; it now stores a small enum and builds the art when it prints.

Follow-ups from C6b-1, not yet scheduled:
- Typed extractors (a `Json<T>` parameter in handler signatures).
- Pluggable converters (other media types, a converter registry).
- `Accept` parameters beyond `q`, and negotiation for answers other than `Json<T>`.
- Request body decompression (`Content-Encoding`).
- A `Content-Type` default for clients that omit it.

Deferred minors from the C6b-1 reviews:
- Json: the `Io` class arm of the error classification is unreachable for a slice and is outside the three classes of the spec (`syntax`, `data`, `eof`).
- Tests: the 400 test asserts the line but not the column; the two todos 415 tests do not assert `accepted` or `detail`.
- Negotiation: internal whitespace such as `text /html` is not rejected by the range check.
- Negotiation: equal-specificity ties in `Accept` go to the first range (RFC 9110 leaves it open); `application/json;q=0, application/json` gives 406, the reverse order 200.
- Negotiation: `Accept` is parsed on every request, also for answers that are not JSON (linear, one allocation per range).
- Dependencies: `oxbloom-security` still depends on `oxbloom-web` with default features.

#### C6b-2: CORS, compression and static resources — done
Done on 2026-11-01; design in `docs/superpowers/specs/2026-11-01-web-cors-compression-static-design.md`, usage in `README.md` (sections "CORS", "Compression" and "Static resources") and `docs/API.md`.

Delivered:
- `CorsFilter` at the new slot `order::CORS` (-350), with `CorsConfig`, `CorsError` and `App::cors`. An allowed origin gets `Access-Control-Allow-Origin` and `Vary: Origin` (also on error answers); an allowed preflight is answered 204 before error handling and security; other origins pass without CORS headers. With a list of origins (not `*`) every answer that is not a preflight answer gets `Vary: Origin`, also without or from a disallowed origin. Origins are normalized (scheme and host in lower case), default ports are not stripped, methods are compared case-sensitively, an empty origin list is invalid, and `*` mixed with other entries or combined with credentials is invalid. Two `Origin` headers count as not allowed.
- `CompressionFilter` at the new slot `order::COMPRESSION` (-150), with `CompressionConfig`, `CompressionLevel`, `codecs()` and `App::compression`. gzip, brotli and zstd sit behind the features `gzip`, `brotli` and `zstd`; `compression` enables all three and is in `default`. `HEAD` gets the headers of `GET`, `Accept-Encoding` is parsed with `q` values (first entry of a repeated name wins, an unlisted codec takes the `q` of `*`), `identity;q=0` gives 406 only for a response that would be compressed, and a strong `ETag` becomes weak. Responses with `Content-Encoding`, `Content-Range`, `no-transform`, an empty body or a 1xx, 204 or 304 status pass unchanged.
- Static resources: `StaticResources` (a `Controller`), `ResourceSource`, `DiskResources`, `EmbeddedResources`, `Resource` and `ResourceError`, with `ETag`, `Last-Modified`, `Cache-Control` and 304. Paths must be in canonical raw form so that a path rule in `oxbloom-security` cannot be bypassed by encoding: every byte has one spelling, either raw (printable ASCII except space, `"`, `#`, `<`, `>`, `?`, a backtick, `{` and `}`) or as an upper-case hex escape (non-ASCII bytes, those characters and `%`), and the other spelling answers 404. Empty and `.` segments, lower-case hex escapes, dotfiles and symlinks leaving the root also answer 404. Index files are the one exception: `/dir`, `/dir/` and `/dir/index.html` serve the same file, so a directory is protected with `/dir/**`. On unix the file is opened with `O_NOFOLLOW | O_NONBLOCK` on Linux (including Android), Apple systems, FreeBSD, OpenBSD and NetBSD.
- The `server.cors.*`, `server.compression.*` and `server.static.*` keys in `WebServer::new`, listed in `boot_properties()`. A present `server.cors.allowed-origins` adds a CORS filter next to one set in code. `server.compression.enabled = true` without a codec, a missing static directory and a `server.static.index` that is not a plain file name fail startup naming the key.
- `examples/todos` has a commented sample of the new keys.
- Stack probes unchanged, re-measured at the end of C6b-2 with the manual probe tests: the DI minima are 206, 277, 230 and 194 KB, the frame of `build` is `sub $0x178,%rsp`, and boot needs 238 KB without the feature `logging` and 239 KB with it.

Limits that users must know (also in the README):
- Security note: on a case-insensitive file system `/ADMIN/x` can serve `admin/x`, which a case-sensitive raw rule does not match.
- Listing `null` as a CORS origin together with credentials is accepted and is a foot-gun.
- Compression runs synchronously; level `best` on a large body can take seconds and tens of MB per encode, with no limit on concurrent encodes. Error answers rendered by the error filter are not compressed. A `mime-types` entry `*/*` matches nothing.
- Static files are read whole (no streaming) and there are no range requests. Mounted at `/`, the catch-all route turns a non-`GET` request to an unknown path into 405. The `server.*` sub-keys have no effect without their switch key. A `StaticResources` index that is unclean is ignored by the web API and rejected at startup by `server.static.index`.

Follow-ups from C6b-2, not yet scheduled:
- Streaming bodies (static files and compression).
- Range requests.
- SPA fallback to the index file.
- Pre-compressed sibling files (`app.js.gz`, `app.js.br`).
- Per-path CORS rules.

Deferred minors from the C6b-2 reviews:
- CORS: `null` with credentials is accepted.
- CORS: default ports are not stripped.
- CORS: methods are case-sensitive.
- CORS: an empty method list is accepted.
- CORS: the integration tests lack trailing-dot, userinfo, comma-joined and non-UTF-8 `Origin` cases.
- Compression: the doc comment on `candidate_size` is stale.
- Compression: the output is used even when it is not smaller than the input.
- Compression: the `HeadBodyStripped` marker stays in the response extensions.
- Static: the `# Security` section sits mid-way in the `DiskResources` doc.
- Static: the Android open flags are not cross-compiled.
- Static: a symlink loop gives 500.
- Static: a 304 carries the strong `ETag` after a compressed weak 200.
- Boot: the 255-byte `server.static.index` boundary is untested.
- Boot: the error texts of `RouteError::Duplicate` and `RouteError::Path` for `server.static.path` are untested.
- Boot: the compression-level test depends on the output size of the codecs.
- Boot: `StaticResources::index()` silently drops an invalid name.
- Cargo: brotli is pinned to 9.0.0 in `Cargo.lock` because 9.1.1 was under two weeks old; `Cargo.toml` keeps a caret requirement.

### C7: SSL bundles (`oxbloom-ssl`) — done
Done on 2026-11-14; design in `docs/superpowers/specs/2026-11-14-ssl-bundles-design.md`, plan in `docs/superpowers/plans/2026-11-14-ssl-bundles.md`, usage in the guide chapter `docs/guide/16-ssl-bundles.md` and `docs/API.md` (section 4.11), decisions in ADRs 0068 to 0082.

Delivered:
- The crate `oxbloom-ssl`: `SslBundles` (`from_config`, `with_clock`, `get`, `names`, `reload_interval`, `expiry_warning`, `reload`, `start_reload`), `SslBundle` (`name`, `has_identity`, `has_trust`, `validity`, `server_config`, `client_config`), `CertValidity`, `ReloadTask`, `SslError` and `PROPERTIES`. Bundles come from `ssl.bundle.pem.<name>` (files or inline PEM, `certificate`, `private-key` as a `Secret`, `trust`) and `ssl.bundle.pkcs12.<name>` (`path`, `password` as a `Secret`), with rustls 0.23 and the `aws-lc-rs` provider only (no `ring` in the tree). Errors name the bundle and the key, never a key, a password, a path or file contents. Feature `di` implements `Lifecycle` for `SslBundles`.
- Reload by polling file metadata every `ssl.reload-interval` (default `30s`, `0s` off): size, modification time and, on Unix, device and inode, with symlinks followed; metadata compared before and after the read; the whole bundle rebuilt and swapped atomically behind an `ArcSwap` that the certificate resolver reads at every handshake; a failed reload keeps the old material and warns once per file change on `oxbloom::ssl`. `SslBundles::reload()` does the same on demand.
- Validity warnings (expired, not yet valid, expiring within `ssl.expiry-warning`, default `14d`) against the `Clock`, at load, after each successful reload and every 24 hours while the condition holds. They never stop the startup.
- `App::serve_tls(listener, acceptor, shutdown)` in `oxbloom-web` behind the feature `tls` (`tokio-rustls` without a provider; `oxbloom-web` does not depend on `oxbloom-ssl`): handshake per connection task with a 10 second timeout, pending handshakes dropped at shutdown, handshake failures at `debug` on `oxbloom::web` with the peer and the error kind, scheme `https`, any negotiated protocol other than `http/1.1` closed.
- Boot feature `ssl` (turns `web` on, enables `oxbloom-ssl/di` and `oxbloom-web/tls`, and `oxbloom-security/jwks` when `security` is on): `SslAutoConfiguration` (condition `on_property("ssl.bundle")`, optional `dyn Clock` bean, `on_missing_bean::<SslBundles>()`), the built-in SSL analyzer, `WebServer::with_ssl` and `server.ssl.bundle` (the port then speaks only HTTPS). `WebServer::new` refuses a set `server.ssl.bundle`.
- `ReqwestJwksFetcher::with_tls(client_config)` and `jwks_ssl_bundle(&config)` in `oxbloom-security`; boot resolves `security.jwt.jwks-ssl-bundle` against the `SslBundles` bean (ADR 0074). An unknown name, the key without `ssl`, and the key together with a `JwksFetcher` bean are `ConfigError::Invalid` on the key.
- `TestClient::connect_tls(addr, ca_pem)` and `RunningApp::client_tls(ca_pem)` in `oxbloom-test` behind the feature `tls`; the server name is always `localhost`.
- Property metadata for every new key, the conditions report entry, and test certificates generated at test time with `rcgen` (no key material committed).
- Final fix wave: a reload can no longer change the parts of a bundle (`NoIdentity`, `EmptyBundle`, the new `SslError::ShapeChanged`; ADR 0070), the transport scheme is the default for a missing forwarded `proto` (ADR 0081), unknown keys directly under `ssl` are rejected, an unusable PKCS#12 trust certificate is `SslError::Pkcs12`, new PEM error tests, the PKCS#12 format claims cut to what is tested, and ADR 0082 records the C7 dependencies.

Gate numbers at the end of C7 (all measured in Task 8):
- `cargo test --workspace --all-features`: 2356 passed, 0 failed, 8 ignored (2224 before C7).
- Boot feature matrix (`cargo test -p oxbloom-boot`), re-baselined because boot's `oxbloom-test` dev-dependency now enables `tls` and with it boot's `ssl` (ADR 0075): `--no-default-features` 420 -> 447, `--features web` 420 -> 447, `--features logging` 440 -> 467, `--features security` 420 -> 447, `--all-features` 440 -> 467, and the new `--features ssl` 447; each with 0 failed and 1 ignored (the probe). No `cargo test -p oxbloom-boot` run builds boot without `ssl` any more; the compile-only builds (now also `--no-default-features --features ssl`) compile those paths, and `cargo test -p oxbloom-test --features web` is the only test run that executes boot without `ssl`.
- `cargo test -p oxbloom-web --no-default-features` 357 (356 before C7, one new unit test), `cargo test -p oxbloom-web --features tls` 422, `cargo test -p oxbloom-ssl --all-features` 83, `cargo test -p oxbloom-test --features web` 155, `cargo test -p oxbloom-test --features web,tls` 161, `make check-examples` 41.
- After the final fix wave (re-measured): `cargo test --workspace --all-features` 2364 passed, 0 failed, 8 ignored; `cargo test -p oxbloom-ssl --all-features` 91; `cargo test --workspace` 2354; the boot matrix (447/447/467/447/467/447), `oxbloom-web` (357, 422 with `tls`), `oxbloom-test` (155, 161) and `make check-examples` (41) unchanged; stack probes unchanged.
- Stack probes unchanged: the DI minima are 206, 277, 230 and 194 KB, and boot needs 238 KB without the feature `logging` (also with `ssl`) and 239 KB with it (also with all features); each value passes and one KB less overflows.

Limits that users must know (also in the guide):
- No mutual TLS and no `client-auth`, no HTTP to HTTPS redirect, no ALPN `h2`.
- The JWKS client does not hot reload; a client configuration fixes its trust store when it is built.
- No OCSP stapling and no revocation checks. JKS keystores and encrypted PEM keys are not supported.
- Reload polls, so a change applies within `ssl.reload-interval`; with `0s` there is no 24 hour repeat of validity warnings either (ADR 0080).
- Expired certificates only warn.
- No limit on concurrent connections or half-open handshakes beyond the 10 second handshake timeout.

Follow-ups from C7, not yet scheduled:
- Bundles for database and HTTP clients.
- Mutual TLS on the server, an HTTP to HTTPS redirect, HTTP/2.
- A reloading JWKS client.
- Expose `SslBundle::validity()` through a health or info endpoint (C8, done: the `ssl` health indicator).

Deferred minors from the C7 reviews:
- Task 1: `p12-keystore` 0.4.1 pulls in the pre-release crates `cms 0.3.0-rc.2` and `pkcs12 0.2.0-rc.0` through caret ranges on pre-releases (recorded in ADR 0082 in the final fix wave). The `pbes1` feature is enabled but no test reads a PBES1 file; the docs now say so, and a test or dropping the feature is still open.
- Task 1: a failure to set up the platform verifier is reported as `SslError::Io` with kind `Other` on the trust key, which is misleading; add a dedicated variant (and amend the spec's variant list).
- Task 1: several private keys in one PEM value or PKCS#12 file silently use the first; reject it or document it, and add a test.
- Task 1: missing tests for a legacy OpenSSL encrypted key (`Proc-Type`/`DEK-Info`) and for a `certificate` value that holds only a key (fixed in the final fix wave).
- Task 1: every non-leaf certificate of a PKCS#12 file becomes a trust anchor; document it on `has_trust` and `client_config`.
- Task 1: `client_config` fixes its verifier at call time, so later trust changes are not seen; out of scope per the spec (the JWKS client does not reload).
- Task 1: the test `missing_file_is_an_io_error...` in `tests/bundles.rs` keeps a useless `ca()` setup.
- Task 1 (unverified): bundle names that differ only by case or `-`/`_` under the same kind (`pem.web` and `pem.Web`); `DuplicateName` compares only across kinds. Closed by the final review: `oxbloom-config` rejects such keys within one source and merges them across sources, so two entries never exist.
- Task 2: `stop()` and `Drop` can block on a hung read; document or bound it.
- Task 2: the bundle state mutex is held across file I/O, so `reload()` blocks; document it.
- Task 2: separate replacement of the certificate and key files can warn once with `key-mismatch`; document the atomic swap.
- Task 2 (unverified): no explicit test that a handshake in progress is unaffected by a reload; it relies on the per-handshake resolver reading the `ArcSwap`.
- Task 2: `reload_global.rs` has `mod common;` but uses only `ca` and `write`, which can give `dead_code` warnings in that test binary.
- Task 3: `from_config` warns with the system clock before `with_clock`, so with a far-off injected clock both clocks warn; add an internal `from_config_with_clock` path.
- Task 3: a certificate that does not parse is silently left out of `validity()`; add a `warn` or `debug` event naming the bundle.
- Task 3: the certificate subject is logged raw; strip control characters (log-line injection under plain-text formatters).
- Task 3: the validity tests lack `ExpiresSoon` to `Expired` as the clock advances, a condition that clears and then warns again, a file-change reload through a tick that warns at once, and they use 200 ms sleeps in negative assertions.
- Task 3: a repeat warning can run up to one reload interval late (documented).
- Task 4: the `serve_tls` doc example used `rustls::` (a dev-dependency); it now uses `tokio_rustls::rustls::` so it copies as is (fixed in Task 8).
- Task 4: a drop for an ALPN mismatch is logged as "TLS handshake failed" with kind `Unsupported` and is untested; give it its own message and a test with a configuration offering `[h2, http/1.1]`.
- Task 4: on TLS, a trusted proxy sending `X-Forwarded-For` without `X-Forwarded-Proto` yields `Scheme::Http` (documented and tested); Spring keeps the request's own scheme. Fixed in the final fix wave: the transport scheme is the default (ADR 0081).
- Task 4: no cap on concurrent connections or half-open handshakes (the same exposure as plain `serve`); recorded as a known limit.
- Task 4: `tests/tls.rs` asserts that a connect fails after shutdown, which risks ephemeral port reuse.
- Task 5: `tls_acceptor` maps every `server_config` error to "has no certificate and private key"; match `NoIdentity` explicitly.
- Task 5: the unknown-bundle message lacks the requested name.
- Task 5: `write_leaf` in `autoconfigure_ssl.rs` renames the key before the certificate, which gives a window for one spurious `KeyMismatch` warning; comment it.
- Task 5: `clock_bean_reaches_the_ssl_bundles` uses `reload-interval = 0s`, so it cannot show that the clock is set before the task starts.
- Task 5: spec line 87 said `WebServer::new` serves HTTPS (aligned with the `with_ssl` ruling in Task 8); the "Web server listening" event shows no scheme (still open).
- Task 6: an overlong line and an ambiguous sentence in `docs/API.md` about which run covers boot without `ssl` (fixed in Task 8: only `cargo test -p oxbloom-test --features web`); an optional `connect_stream` helper; the `wrong_ca` test ties to the word "certificate" in the rustls text.
- Task 7: `map_err(|_| unusable())` in `jwt.rs` drops the `SslError` or reqwest cause; keep it as the source or name its kind.
- Task 7: the bundle is resolved before the decoder's "only valid with `jwk-set-uri`" check; the error order is cosmetic.
- Task 7: the tests use `localhost` and reqwest honours `HTTPS_PROXY`, so they break on runners with a proxy and no `NO_PROXY`; use `127.0.0.1` with an IP SAN or document `NO_PROXY`.
- Task 7: spec line ~125 still said `jwt_decoder_from_config` takes an optional `SslBundles`, contradicting the "As built" bullet below it (struck in Task 8).
- Final fix wave: a reload that drops the trust store fails with `SslError::EmptyBundle`, whose text ("has neither an identity nor a trust store") does not fit a bundle that keeps its identity; a dedicated text or variant would be clearer.
- Task 7: the client-certificate path (an identity in `client_config`) is not tested end to end.
- Task 7: the trust replacement of a bundle with a trust store (rejecting a public-CA certificate) is documented but untested.

### C8: Availability, info and health (`oxbloom-actuator`) — done
Done on 2026-11-14; design in `docs/superpowers/specs/2026-11-14-actuator-design.md`, plan in `docs/superpowers/plans/2026-11-14-actuator.md`, usage in the guide chapter `docs/guide/17-actuator.md` and `docs/API.md` (section 4.12), decisions in ADR 0083.

Delivered:
- The crate `oxbloom-actuator`: `Availability` (`Liveness`, `Readiness`, `break_liveness`, `set_readiness`), `Status`, `Health`, `HealthIndicator`, `HealthRunner`, `BuildInfo`, the `build_info!` macro, `build::emit`, `Platform`, `Info`, `InfoContributor` and `ActuatorSettings`; with the feature `web` the `ActuatorController`, with `logging` the `/loggers` endpoint, and with `ssl` the `SslHealthIndicator`.
- The endpoints `GET /health`, `/health/liveness`, `/health/readiness`, `/info`, `GET /loggers`, `GET /loggers/{target}` and `POST /loggers/{target}` under `management.endpoints.base-path` (default `/actuator`). Health answers 200 for `UP` and 503 otherwise, and shows components only with `management.health.show-details = "always"`. Liveness never runs an indicator, and readiness runs the indicators named in `management.health.readiness.include`.
- Health indicators run at the same time, each in its own task under `management.health.timeout` (default `5s`); a timeout or a panic is `DOWN` with an `error` detail. Duplicate names get the keys `name#2`, `name#3`. The built-in `ssl` indicator reports expired certificates as `DOWN` and lists the bundles with a certificate that is not valid yet or expires soon.
- Build info through the application's `build.rs` (`oxbloom_actuator::build::emit()`) and the `build_info!()` macro; `Platform` detection of Kubernetes; `InfoContributor` beans.
- `/loggers` over `LoggingSystem::levels`, `level` and `set_level`, with `::` separated targets and problem+json 400 answers.
- Boot feature `actuator` (turns `web` on, enables `oxbloom-actuator/web`, and `logging` and `ssl` forward to the crate): `ActuatorAutoConfiguration` with nine property metadata entries, an `AvailabilityListener` that sets readiness at `ready` and `stopping`, validation of `management.health.readiness.include` at startup, `management.endpoints.health.public` for the security chain, and a skip when the application registers no `dyn Controller`.
- `LoggingSystem::levels` in `oxbloom-logging` and the actuator in the `examples/todos` application, with a `build.rs`, its README and the CI matrix.
- ADR 0083 and the guide chapter 17 (the roadmap chapter is now 18).

Gate numbers at the end of C8 (as measured by the implementer of Task 6; the controller re-measures at the end):
- `cargo test --workspace --all-features`: 2467 passed, 0 failed, 8 ignored (2364 before C8).
- Boot feature matrix (`cargo test -p oxbloom-boot`): `--no-default-features` 447, `--features web` 447, `--features logging` 467, `--features security` 447, `--features ssl` 447, `--features actuator` 460, `--all-features` 481; each with 0 failed and 1 ignored (the probe).
- `cargo test -p oxbloom-actuator --all-features` 84 (re-run in Task 7), `make check-examples` 44 (41 before).
- Stack probes: boot needs 238 KB without the feature `logging` and 239 KB with it, and `frame sub $0x178,%rsp` is unchanged; each value passes and one KB less overflows.

Limits that users must know (also in the guide):
- The endpoints share the application's port; a separate management port is not available.
- All endpoints have the same authorization: with the default chain every endpoint needs authentication, and `management.endpoints.health.public = true` opens only `<base-path>/health/**`. That option adds its own `authorize` section, so an application customizer that also calls `authorize` fails with `DuplicateAuthorizeSection`; open the health path in your own customizer instead, as `examples/todos` does.
- The readiness 503 during shutdown shows only on open connections: the state flips at `stopping`, right before the server stops, and a new connection is usually refused during the drain.
- `management.endpoints.enabled` switches the group on only with the value `true` or a missing key; any other value skips the group silently (visible in the conditions report).
- The group is skipped when the application registers no `dyn Controller`.
- A health indicator must not block the thread; the timeout cannot interrupt a blocking call.
- `/loggers` targets use `::`; a dotted target is a 400 on `GET` and `POST`, and `configuredLevel: null` is a 400, not a reset. `POST` requires `Content-Type: application/json` (415 otherwise).
- The build script of the application reruns when `build.rs`, the git `HEAD`, the current branch ref or `packed-refs` changes, so a new commit refreshes the git sha of `/info`; a change that touches none of these files (such as a fetch that only updates a remote ref) does not.

Follow-ups from C8, not yet scheduled:
- A separate management port.
- Roles per endpoint.
- `/metrics`, `/env`, `/beans` and `/threaddump` endpoints, and a `diskSpace` indicator.
- Resetting a log level through `configuredLevel: null`.
- A bound on the loggers directives: an authenticated caller can `POST` any number of distinct targets, so the directive set of the logging system grows without bound. This is acceptable until roles per endpoint exist.
- An `oxbloom-test` signal that fires after the bean listeners, so that `TestApp::start` returns after readiness flips on a multi-thread runtime.

Deferred minors from the C8 reviews:
- Task 1: the crates `serde` and `tokio` were unused until later tasks (they are used now), `Status` has no `Serialize` or `Hash` derives (add them when a task needs them), and `availability.rs` shares the constants 0 and 1 across the two enums.
- Task 2: a blocking indicator cannot be interrupted by the timeout (documented: indicators must not block); the spawned tasks are not aborted when the caller drops `run()`; `build::emit` tracked `.git/HEAD` relative to the application's package directory, which did not follow commits and reran the script on every build in a workspace member, and `emit` had no unit test (both resolved in the final fix wave: `emit` now watches the absolute git `HEAD`, the current branch ref and `packed-refs`, and its output formatting has unit tests); coverage gaps remain for concurrency timing, an `Unknown`-only worst status, an `only` filter that matches nothing, and the `build_info!` macro outside its doctest; `Info::render` lets a contributor overwrite the `build` and `platform` keys (documented); the build time is a string of epoch seconds.
- Task 2: a generated key `name#2` can collide with an indicator that is literally named `name#2`, which loses a map entry although the status stays right; there are no tests for `name#3` or for the `cancelled` branch.
- Task 3: a base path such as `/a//b` kept an empty segment (resolved in the final fix wave: each segment must now be non-empty and hold only ASCII letters, digits, `-`, `_`, `.` and `~`, with unit tests for whitespace, `//a` and route pattern characters); test gaps remain for a bad `show-details`, the parsing of `enabled`, `public` and `loggers`, `show-details = "always"` with a failing indicator, and a disabled health endpoint answering 404; the `status_body` test helper is duplicated.
- Task 4: `ssl_indicator` duplicates the private `validity::condition` classification in `oxbloom-ssl` (expose a public helper there); the indicator skips a bundle silently when `bundles.get(&name)` fails (it should become a warning); the 1 KiB body limit and the marker check in `loggers.rs` are undocumented in the code; `GET` reports `configuredLevel` by an exact match against `levels()`, so the spellings `Root`, a group name or a dashed form report `null` while a `POST` with them succeeds; percent-encoded or very long targets are untested (it is unverified whether `param()` decodes); there are no tests for concurrent `POST`s, case-insensitive levels, `POST` of the root or a group, or the `body_too_large` marker.
- Task 5: a value of `management.endpoints.enabled` other than `true` skips the group silently; there is no HTTP test of the 503 during the drain; `TestApp::start` can return before the availability listener fires on a multi-thread runtime (see the follow-up on an `oxbloom-test` signal); the metadata test checks 2 of the 9 defaults and has no explicit `enabled = true` case; the doc wrapping in `autoconfigure/mod.rs` is ragged and its module header omits the actuator contributor.
- Task 6: the doc of `PublicPaths` in `examples/todos` should say that the `/actuator` prefix is hard-coded (a changed base path makes health answer 401, which fails closed); the info test of the example checks `todos::build_info()` and not the served bean, so a wiring regression would pass; the `build.rs` of the example had the same relative `rerun-if-changed=.git/HEAD` limit as Task 2 (resolved with it in the final fix wave); the Makefile has no rows for the features `ssl` or `actuator`, so only `ci.yml` changed.
- Final review: `GET <base-path>/loggers/{target}` accepted a dotted target although the docs said it was a 400; it now checks the target with `LoggingSystem::validate_target` (resolved in the final fix wave). The startup warning that boot logs for the loggers endpoint without the feature `security` has no test, because boot's dev-dependency on `oxbloom-test` turns `security` on for every test build of boot; the guide and API.md document it.

Rulings made during C8:
- `BuildInfo::from_env` was dropped in favour of the exported macro `build_info!`, which expands in the application's crate, because a library cannot see the application's build environment; the cost if wrong is a small API rename.
- `BuildInfo` derives `Default` (all fields absent), because the auto-configuration needs it when no bean exists.
- `ActuatorSettings`, `Info` and `HealthRunner` are not gated on the feature `web`; only `ActuatorController` is, because the auto-configuration and the tests use them without it.
- The work happens on the branch `c8-actuator`, with commits by the implementers as the plan says.
- `management.endpoints.enabled` and the readiness include validation belong to the auto-configuration (Task 5): the first skips the whole group, the second fails the startup with the key and the unknown name, because the reviewer found both fail-open.
- The auto-configuration is skipped when the application registers no `dyn Controller`, because no `WebServer` would start; the cost if wrong is that an application with only the actuator gets no server and must add a controller.
- The metadata lists nine keys, not seven, because `management.endpoint.<id>.enabled` is one key per endpoint (the plan miscounted); the test pins the real list.
- A value of `management.endpoints.enabled` other than `true` skips the group silently, as documented on `conditions()`, because DI has no "not equal" condition and a skip fails closed; the cost if wrong is that an operator typo disables the actuator without an error, visible in the conditions report.
- The `examples/todos` application opens `/actuator/health/**` in its own `PublicPaths` instead of using `health.public = true`, because two `authorize` sections fail the startup; this keeps `/info` and `/loggers` at 401, and a changed base path makes health answer 401 (fail closed).

### C9a: Tasks and scheduling (`oxbloom-tasks`) — done
Done on 2026-11-14; the design is the C9a spec `2026-11-14-tasks-scheduling-design.md` and the plan is `2026-11-14-tasks-scheduling.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/18-tasks.md` and `docs/API.md` (section 4.13), and decisions are in ADR 0084.

Delivered:
- The crate `oxbloom-tasks`: `Trigger` (`FixedRate`, `FixedDelay`, `Cron`), `CronExpr`, `Zone`, `TriggerError`, `parse_duration`, `Task`, `Scheduling`, `Outcome`, `TaskError`, `Scheduler`, `SchedulerConfig`, `TaskStatus` and `TasksSettings`.
- The macros `#[scheduling]` and `#[scheduled(..)]` in `oxbloom-macros`, re-exported by `oxbloom-tasks`, without a feature flag. They generate `impl Scheduling` for an inherent impl block, with compile errors that name the method, and the application registers the bean with `.provides(.. -> Arc<dyn Scheduling>)`.
- Cron expressions with six fields, seconds first, Spring numbering and names of the day of the week (mixed freely and translated before the `cron` crate), IANA zones, and daylight saving rules: a skipped local time does not fire and a repeated one fires once.
- A scheduler with one loop per task, driven only by the injected `Clock` in chunks of at most one second; skip on overlap, logged at `warn` and counted; failures and panics logged at `error` with the schedule continuing; a global concurrency bound; `initial_delay`; and a shutdown that waits up to `tasks.scheduling.shutdown-timeout` and then aborts the runs, also when the `stop` future is dropped.
- Startup errors that name the task and the key (`cron`, `zone`, `fixed_rate`, `fixed_delay`, `initial_delay` or `name`) and never the value, and a rejection of duplicate task names.
- `Scheduler::tasks()` as a snapshot for inspection and `Scheduler::is_running()`.
- Boot feature `tasks`: `TasksAutoConfiguration` with five property metadata entries and `SchedulerService`, a boot service in the phase `i32::MAX` with the DI order `i32::MAX`, so it starts after the web server and before readiness flips, and stops before the server.
- The `TodoCleanup` task in `examples/todos` (a fixed delay of one minute that removes done todos older than `todos.cleanup.max-age`), its tests and its README, and the CI matrix.
- ADR 0084 and the guide chapter 18 (the roadmap chapter is now 19).

Gate numbers at the end of C9a (re-measured at the final gate on the final tree):
- `cargo test --workspace --all-features`: 2619 passed, 0 failed, 8 ignored (2481 before C9a).
- Boot feature matrix (`cargo test -p oxbloom-boot`): `--no-default-features` 447, `--features web` 447, `--features logging` 467, `--features security` 447, `--features ssl` 447, `--features actuator` 460, `--features tasks` 462, `--all-features` 496; each with 0 failed and 1 ignored (the probe).
- `cargo test -p oxbloom-tasks --all-features` 113, `cargo test -p oxbloom-macros` 21, `make check-examples` 50 (44 before).
- Stack probes: boot needs 238 KB without the feature `logging` and 239 KB with it (237 and 238 overflow), unchanged from C8.

Limits that users must know (also in the guide):
- A trigger that finds the previous run still active is skipped and not made up later.
- A forward clock jump gives one run, never a burst. A backward jump delays a fixed-rate or fixed-delay task by at most one period. A cron task has no backward clamp, so an NTP step back stalls a per-second cron task for that span.
- Sub-hourly cron expressions have a gap of about 75 minutes on a fall-back day.
- Cron evaluation ends at the year 2100.
- A plain `fn` runs in `spawn_blocking` and cannot be aborted: `stop` returns at the deadline, the thread is leaked until the work finishes, and the tokio runtime shutdown still waits for it.
- Unknown keys under `tasks` are rejected only when the group runs. With `tasks.scheduling.enabled = false`, or with no `dyn Scheduling` bean, `tasks.typo` is accepted silently, and any value of `enabled` other than `true` switches the group off silently.
- The configuration duration grammar (`1.5s`) is richer than the one of the attribute literals and `parse_duration` (a whole number with `ms`, `s`, `m`, `h` or `d`).
- The scheduler shares the phase and the order `i32::MAX` with the web server, so another service with both values would tie with it.
- A method named `clone`, `to_string` or `scheduled` can collide with `Arc` or trait methods; a `#[cfg]`-gated `#[scheduled]` method and `unsafe fn` are not supported; the return type is checked by shape.
- `Trigger::cron_in_default_zone(x)` and `Trigger::cron(x, Zone::Utc)` are not equal under `PartialEq`.

Follow-ups from C9a, not yet scheduled:
- An actuator `/scheduledtasks` endpoint over `Scheduler::tasks()`.
- Persistence and catch-up of missed runs, and distributed locking for several instances.
- Adding and removing tasks at run time.
- Per-task concurrency limits.
- A backward clamp for cron tasks.
- One duration grammar for configuration and attributes.
- `#[diagnostic::on_unimplemented]` on `RunResult`, for a clearer error when a task returns an error type that is not `Display`.
- Fully qualified calls (`Self::method(&*this)`) in the macro output, so a method named `clone`, `to_string` or `scheduled` works.
- Re-emitting the impl block when the macro reports an error, so the compiler does not add "no method found" errors.

Deferred minors from the C9a reviews:
- Task 1: the branch `_ => cursor = now` of the loop-stall guard in the cron evaluation should return `None` (it cannot be reached with the current `cron` crate). Coverage gaps remain for zones with a half-hour offset, a `last_end` earlier than `last_start`, and the `if let Some(far)` assertion. The crates `tokio`, `tracing` and `oxbloom-core` were unused until Task 2 (they are used now). Duplicate expansion in a day-of-week field (`0-7`, `1,1`) is untested.
- Task 2: the log recorder helper in the tests captures events only on the test thread, so there is no multi-thread capture. A cancelled run is recorded as `Failed` without a log line. The skip event has no field with the count of skipped triggers. There is no test of `stop` called twice, of `start` after `stop`, or of a `spawn_blocking` call inside a user future, where the runtime shutdown waits for the blocking thread. `SchedulerConfig::zone` is read by `Scheduler::try_new` only to fill cron triggers without a zone.
- Task 3: the error for a repeated trigger key reads "two triggers `cron` and `cron`". There is no custom message for an error type that is not `Display`.
- Task 4: a `fixed_delay` of `0s` is untested at the boot level.
- Task 5: the jump of the workspace test count from 2481 to 2611 at that point is not attributed in the implementer's report (Task 4 added 15 under `--all-features` and Task 5 about 6), so the final gate re-verified it (2619 passed). The test `the_max_age_comes_from_configuration` stores its result in a field called `removed_with`, which is misleading. The wording of the `done_at` doc is cosmetic. There is no scheduler-level run test of the example, because the attribute period is the literal `1m`. Some lines in the example's README, `cleanup.rs` and `application.toml` are long. The new `delete` test is sequential, so it is a regression guard and not a race test.

Rulings made during C9a:
- The macro generates `impl Scheduling` and the application registers the bean as a `dyn Scheduling`, where the spec said the macro registers a `dyn ScheduledTask` bean per method, because an impl macro cannot register beans (`#[controller]` works the same way); the user-visible behavior is the same.
- Work happens on the branch `c9a-tasks`, with commits by the implementers; the specs and plans under `docs/superpowers` and the `.superpowers` directory stay local and git-ignored.
- The Task 2 tests use a small local test clock instead of the `ManualClock` of `oxbloom-test`, because `oxbloom-test` depends on `oxbloom-boot`, which optionally depends on `oxbloom-tasks`, so a dev-dependency would make a cycle.
- Spring day-of-week numbering (`0` and `7` are Sunday, `1` is Monday) is translated in the cron parser instead of rejecting numeric days; names are translated to numbers too, so `SUN-TUE` means Sunday to Tuesday.
- `Task::invalid` takes the key (`name`, `key: &'static str`, `error`) and `TaskError` names it, because startup errors must name the task and the key.
- `Scheduler::stop` has an abort-on-drop guard around the runs in flight, because boot abandons a service's `stop` at its own deadline and a dropped future would otherwise leave the runs detached.
- `tasks.scheduling.zone` applies to cron tasks without a `zone`: the cron trigger keeps an optional zone and `Scheduler::try_new` fills the configured one.
- `oxbloom-macros` stays free of feature flags, like `#[controller]`, and the spec's "behind a feature" is amended in ADR 0084.
- `Scheduler::try_new` rejects duplicate task names, because names identify tasks in `tasks()` and in logs across several `#[scheduling]` impls.
- The scheduler uses the phase `i32::MAX` of the web server and the DI order `i32::MAX`, because no higher phase exists; this was verified by reading the stable sort by phase, the stable DI order and the reverse stop.

### C9b: Retry (`oxbloom-tasks`) — done
Done on 2026-11-14; the design is the C9b spec `2026-11-14-retry-design.md` and the plan is `2026-11-14-retry.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/18-tasks.md` (section Retry) and `docs/API.md` (section 4.13, the retry subsection and `RetryAutoConfiguration`), and decisions are in ADR 0085.

Delivered:
- `RetryPolicy`, `Backoff`, `Jitter`, `RetryDefaults` and `RetryError` in `oxbloom-tasks`: `run(label, op, retry_if)` with fixed or exponential backoff capped at `max_delay`, three jitter modes, a pure `delay(retry_number, unit_random)`, sleeping with `tokio::time::sleep` (paused-time tests work), and events on target `oxbloom::tasks` (`warn` per retry, one `error` when the attempts run out). `fastrand` draws the jitter.
- The process-wide defaults `set_defaults`, `defaults` and `reset_defaults`, in a `static RwLock`, and `RetryDefaults::from_config` for the `tasks.retry.*` keys (`max-attempts`, `backoff`, `initial`, `multiplier`, `max-delay`, `jitter`) with errors that name the key and never the value.
- The macro `#[retryable(..)]` in `oxbloom-macros`, re-exported by `oxbloom-tasks`, without a feature flag: `max_attempts`, `backoff`, `initial`, `multiplier`, `max_delay`, `jitter` and `retry_if`; a key that is omitted is read from `defaults()` at call time; compile errors that name the method. It combines with `#[scheduled]` in either order, and one scheduled run then includes all its attempts.
- Boot feature `tasks`: `RetryAutoConfiguration` with six property metadata entries (eleven `tasks.*` keys with the scheduler's five), without conditions, so the retry keys apply with or without a scheduler.
- `TodoCleanup::purge_at` in `examples/todos` is `#[retryable(max_attempts = 3, initial = "50ms")]`, with `TodoStore::fail_next`, tests, a README section and a `[tasks.retry]` block in `application.toml`.
- ADR 0085 and the retry chapter of the guide.

Gate numbers at the end of C9b: `cargo test --workspace --all-features` 2700 passed, 0 failed, 8 ignored (2619 before C9b); `oxbloom-tasks` 178, `oxbloom-macros` 25, `oxbloom-boot` 505 (all features); `make check-examples` 54; clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB (237 overflows) and 239 KB with all features (238 overflows); the five `tasks` feature builds of `oxbloom-boot` and the `--no-default-features --features tasks` doc tests (112) pass.

Limits that users must know (also in the guide):
- The defaults are process-wide state. Each context's `RetryAutoConfiguration` overwrites them, a context without it keeps what the process set before, they stay after the run ends, and `oxbloom-test` has no reset (call `reset_defaults()` in a test).
- Only an `async fn` that returns `Result<T, E>` with `E: Display` can be retried. The receiver is `&self` or absent, and `&mut self`, `self` and `mut` arguments are compile errors. The body runs again for every attempt, so it may borrow its arguments but not consume them.
- The default jitter is `full`; Spring's default is none.
- The macro keeps its own copy of the duration grammar (a whole number with `ms`, `s`, `m`, `h` or `d`), while the configuration binder accepts more forms.
- With `max_attempts = 1` the first failure logs the `error` event; an error that `retry_if` rejects logs nothing.
- `RetryAutoConfiguration` registers `RetryDefaults` without `on_missing_bean`, so a user-registered `RetryDefaults` bean collides, and choosing back-off would skip `set_defaults`.
- The log label is the bare method name, without the type.
- A scheduled task that exhausts its attempts logs the retry's `error` event and the scheduler's failed-run event.

Follow-ups from C9b, not yet scheduled:
- A circuit breaker and rate limiting.
- Retry of plain `fn` methods, retry on a result value, and `&mut self` receivers.
- Per-error backoff and named policies in configuration.
- Metrics and an actuator view of retries.
- A `recover` fallback method like Spring's `@Recover`.
- A reset of the process-wide defaults in `oxbloom-test`.

Deferred minors from the C9b reviews:
- Task 1: the doc says `unit_random` is in `[0, 1)` while the clamp is `[0, 1]`. A NaN multiplier silently becomes `max_delay` (the configuration reader and the macro validate it). The `delay_ms` field truncates sub-millisecond delays. The panic test prints stderr noise. There are no event tests for `max_attempts = 1` and for a `retry_if` that returns `false`.
- Task 2: no test sets every key of both sections through both readers, as a drift guard for the duplicated key lists in `retry_settings.rs` and `settings.rs`. The `retry_settings.rs` doc says the scheduling section is "ignored" although its keys are validated, and its `# Errors` omits it. There is no test that a wrong-typed value under `tasks.retry` stays hidden, some assertions for `0`, NaN and infinity are weak, and the missing upper bound of `max-attempts` is undocumented.
- Task 3: lifetime-only generics and `where` clauses are rejected but untested, and only the first bad attribute key is reported. An `impl Trait` return type, a nested `mut` pattern and a trait impl are untested and the docs are silent on an alias without arguments (`fmt::Result`). A body that is a single `return Ok(..);` statement is untested.
- Task 4: the end-to-end test with `max-attempts = 3` equals the default, so it cannot tell a configured value from the default by behavior. One assertion (`!contains('0')`) is brittle. A failure after `set_defaults` leaves the new defaults, which follows the process-wide ruling.
- Task 5: the exhaustion test cannot tell three attempts from one. `purge_at` duplicates the cutoff of `cleanup_at`. A second error log, at the target `todos`, is not in the README. The `application.toml` comment "overrides max-attempts (3)" is misleading, and the table row in `lib.rs` omits the retry block.

Rulings made during C9b:
- The defaults sit in a `static RwLock<RetryDefaults>`, where the spec said a `OnceLock`, so that `set_defaults` can run again; `reset_defaults()` exists, each context's `RetryAutoConfiguration` overwrites the values, and a context without it keeps the previous ones.
- `oxbloom-test` does not reset the defaults between contexts, where the spec said it does; a test calls `reset_defaults()` itself.
- `RetryPolicy::run_with_random` is `#[doc(hidden)] pub`, so tests in other crates can replace the random source; the spec kept that function private.
- Only `async fn` with `Result<T, E>` and `E: Display` is accepted; the receiver is `&self` or absent; a free `async fn` is accepted because an attribute cannot see its context.
- The default jitter is `full`.
- A separate `RetryAutoConfiguration` (not folded into `TasksAutoConfiguration`), so the retry settings apply without any scheduler.
- The macro keeps its own copy of the duration grammar, and a test in `oxbloom-macros` checks it against `parse_duration`.
- The retry API is documented as a subsection of `docs/API.md` section 4.13, not as a new section 4.14, and `RetryAutoConfiguration` sits beside `TasksAutoConfiguration` in the boot section.
- Work happens on the branch `c9b-retry`, which started from `c9a-tasks` because C9a was not merged to `main` yet.

### Security Slice 6a: Password encoding, user details and HTTP Basic (`oxbloom-security`) — done
Done on 2026-11-14; the design is the Slice 6a spec `2026-11-14-password-basic-design.md` and the plan is `2026-11-14-password-basic.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/19-password-login.md` and `docs/API.md` (section 4.7, the password login subsection, and `PasswordEncoderAutoConfiguration`), and decisions are in ADR 0086. Sessions are done in Slice 6b, form login and logout are Slice 6c, and CSRF tokens are Slice 6d.

Delivered:
- In `oxbloom-security`: `PasswordEncoder`, `Argon2PasswordEncoder` (Argon2id, OWASP minimum by default, PHC strings) and `DelegatingPasswordEncoder` (`{id}hash`, only `argon2` today); `UserDetails`, `UserDetailsService`, `InMemoryUserDetailsService`; `DaoAuthenticationProvider` with a dummy-hash verification for unknown users and hashing in `spawn_blocking`; `HttpSecurity::http_basic` and `password_provider`; the settings readers `PasswordSettings`, `Argon2Settings`, `HttpBasicSettings` and `users_from_config`.
- `AuthenticationRequest::UsernamePassword`, `AuthenticationError::{AccountDisabled, AccountLocked}` and `SecurityBuildError::{HttpBasicWithoutProvider, InvalidRealm}`.
- In `oxbloom-boot` (feature `security`): `PasswordEncoderAutoConfiguration` (encoder, in-memory user store from `security.users`, DAO provider; ten property metadata entries including the two `security.http-basic.*` keys), HTTP Basic in `SecurityAutoConfiguration` when `security.http-basic.enabled = true`, and analyzer texts for the two new build errors.
- `examples/todos`: `GET /api/admin/stats` behind HTTP Basic with two demo users, tests and README notes.
- ADR 0086, the guide chapter 19 (the roadmap chapter is now 21) and the API section.

Gate numbers at the end of Slice 6a: `cargo test --workspace --all-features` 2821 passed, 0 failed, 8 ignored (2700 before Slice 6a); `cargo test -p oxbloom-boot` per feature: none 460, `security` 460, `web` 460, `logging` 480, `ssl` 460, `actuator` 473, `tasks` 484, all features 518, each with 0 failed and 1 ignored (the probe); `make check-examples` 59 (54 before); clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB (237 overflows) and 239 KB with all features (238 overflows); the `cargo build -p oxbloom-boot` builds for `tasks`, `tasks,logging`, `tasks,ssl`, `tasks,security`, `tasks,actuator`, `security`, `security,logging`, `security,web` and `web`, each with default features off, pass, and the `--no-default-features --features tasks` doc tests (113) pass; `cargo tree -i ring --workspace --all-features` finds no `ring`.

Limits that users must know (also in the guide):
- The enums `AuthenticationRequest`, `AuthenticationError` and `SecurityBuildError` are not `#[non_exhaustive]`, so the added variants are a semver-breaking change.
- `security.http-basic.enabled` defaults to `false`; adding users does not open Basic.
- Keys under `security.users.<name>` are matched ignoring case, `-` and `_`: two names that differ only in these fail to load from one file (`DuplicateKey`) and merge across sources. The written spelling is kept and compared exactly at login.
- With Basic on, the chain adds every `dyn AuthenticationProvider` bean as a password provider.
- No rate limiting and no lockout counters; no database store; no re-encoding of a stale hash on login.
- `matches` verifies with the cost stored in the hash, so each concurrent login uses that memory (19 MiB by default); the dummy hash of an unknown user uses the configured cost, so equal timing holds only while stored hashes use the configured parameters. Anonymous requests are bounded only by the blocking pool of tokio (512 threads by default), so the worst case is about 512 x 19 MiB, roughly 9.5 GiB at the default cost. A login limit or a semaphore around the hash is deferred.
- A malformed `Authorization` header bypasses a custom `entry_point` and gets the built-in Basic challenge.
- The timing of an unknown user is equalised with a dummy hash, but a user with a malformed stored hash answers faster (a narrow difference).

Follow-ups from Slice 6a, not yet scheduled:
- Form login and logout are done in Slice 6c; sessions are done in Slice 6b.
- Login rate limiting, a limit on concurrent logins and lockout counters.
- A database-backed `UserDetailsService`.
- Re-encoding a stale hash after a successful login.
- bcrypt and other algorithms in the `DelegatingPasswordEncoder`, and account-expiry flags and password policy.
- `#[non_exhaustive]` on the security enums.

Deferred minors from the Slice 6a reviews:
- `matches` of the encoder itself (not the configured users, whose hashes are now range-checked at startup) takes its cost from the stored hash, so a hostile hash that bypassed the config reader can still hang or exhaust a login.
- A stale doc comment in `chain.rs` (`basic` versus `scheme`) remains.
- `upgrade_encoding` ignores the algorithm variant and the version, so an `argon2i` or `argon2d` hash or a version other than 19 reports no upgrade.
- `Argon2PasswordEncoder::new` does not cap `memory_kib` upward beyond what the crate accepts: `new(u32::MAX, 1, 1)` passes validation and `encode` then panics on out-of-memory. The `expect` message in `encode` is inaccurate and there is no `# Panics` note.
- `DelegatingPasswordEncoder::new` accepts empty ids and ids that contain braces.
- Encoder tests are missing for `{}`, an empty rest after `{argon2}`, nested braces, hostile parameters and a registered id that is not the default.
- `DaoAuthenticationProvider::new` runs `encode` synchronously for the dummy hash, which blocks an async worker when it is called from async code; a rustdoc note is missing.
- A user with a malformed stored hash answers faster than an unknown user, because nothing is hashed: a narrow timing difference.
- The raw password is copied into a plain `String` for `spawn_blocking` and is not zeroized.
- No test captures the tracing output of a failed login, and the test for disabled and locked accounts checks the status but not the equality of `WWW-Authenticate`.
- The Basic `HeaderValue` is formatted for every 401 with an `.ok()` fallback instead of once in `build()`.
- A long Basic token is fully decoded before the 1024-byte caps of the provider apply; a cheap length check before decoding is missing.
- Untested cases: `ProviderUnavailable` giving 503 in a Basic chain, the answer for a malformed header on a mixed chain, and the non-UTF-8 and non-text header cases, which assert the status only.
- There is no failure-analyzer text for the new keys (the analyzer covers only `security.jwt`), and `TestApp::secured()` now also adds `PasswordEncoderAutoConfiguration`.
- The `security.http-basic.*` keys sit in the metadata of `PasswordEncoderAutoConfiguration` although `SecurityAutoConfiguration` reads them, so excluding the first hides keys that the chain applies.
- The "not echoed" test covers only the above-bound value, and every Argon2 parameter rejection is reported on `memory-kib`.
- In the example, the `stats()` test helper returns an opaque tuple and `tests/basic.rs` hand-rolls Base64.
- The semaphore around `DaoAuthenticationProvider::verify` (a limit on concurrent hashes) stays deferred.
- The check that rejects a realm with control characters is written twice, in `password_config.rs` and in `HttpSecurity::http_basic`, so the two can drift apart.
- `docs/guide/06-conditions-and-auto-configuration.md` (line 131) still says `oxbloom-boot` ships three auto-configurations; it leaves out `RequestLog`, `Ssl`, `Actuator`, `Tasks`, `Retry` and `Password`, and "without either feature" ignores `tasks`.
- The guide chapter 19 and `docs/API.md` say only "`{argon2}` plus a PHC string" for a configured password and do not mention the startup range check on the cost parameters.
- The example README says any authenticated user can read `/actuator/info` and `/actuator/loggers`; loggers answers 404 in that configuration, and if it were mounted, the `viewer` could change log levels because the rule asks for no role. A comment in `examples/todos/tests/api.rs` (around line 382) is stale, and no test checks that `viewer` gets 200 from `/actuator/info`.

Rulings made during Slice 6a:
- Argon2 comes from the RustCrypto `argon2` crate, which is pure Rust, so the dependency tree stays free of `ring`.
- Basic is opt-in through `security.http-basic.enabled`, because configured users must not open a route to passwords on their own.
- Every failed login gets the same 401 and the same challenge, also for a disabled or locked account; the reason is only in the `debug` log.
- Work happens on the branch `s6a-password-basic`, started from `main` after the local merge of C9a and C9b.

### Security Slice 6b: Sessions (`oxbloom-security`) — done
Done on 2026-11-15; the design is the Slice 6b spec `2026-11-15-sessions-design.md` and the plan is `2026-11-15-sessions.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/20-sessions.md` and `docs/API.md` (section 4.7, the sessions subsection, and `SessionAutoConfiguration`), and decisions are in ADR 0087. Login and logout endpoints are Slice 6c and CSRF protection is Slice 6d, so Slice 6b is only the session layer.

Delivered:
- In `oxbloom-security`: `Session` (`get`, `set`, `remove`, `invalidate`, `rotate_id`, `is_new`, `id`), `RequestSessionExt`, the `SessionStore` trait (`load`, `save`, `update`, `rotate`, `delete`, `maintain`) with `SessionData` and the sharded `InMemorySessionStore` (cap, oldest-touched eviction), `CookieSessionBackend` (AES-256-GCM through `aws-lc-rs`, key from HKDF-SHA256 over the whole secret with an empty salt and the info `oxbloom session cookie v1`, cookie name as associated data, random 96-bit nonce per write, `old-secrets` for rotation), `SessionBackend`, `SessionFilter` at `SESSION_ORDER` (-101), `SessionSettings`, `SameSite`, `SecureMode`, `SessionError` and `SessionConfig`.
- The trait gained `update` (write only when the id still exists) and `maintain`, and `rotate` returns `Result<bool>`, so a request that was already running cannot bring a logged-out session back. A request that loses that race stores nothing and leaves the cookie alone.
- The filter enforces the absolute timeout at load for every backend (stored data past its creation time plus the absolute timeout counts as unknown) and gives the store the remaining lifetime of the session as its TTL, so a store that honours only the TTL never serves a session past its absolute deadline.
- In `oxbloom-boot` (feature `security`, which now also enables `dep:oxbloom-web`): `SessionAutoConfiguration`, which runs after the password encoder and before the security chain, only when the table `security.session` exists, with ten property metadata entries. A user `dyn SessionStore` bean replaces the in-memory store with `store = "memory"`, a user `SessionFilter` bean steps the configured one aside (it must also be provided as `dyn Filter`), and one optional `Arc<dyn Clock>` bean feeds the filter, the store and the cookie backend.
- `examples/todos`: `GET /api/visits` counts visits in a session (`store = "memory"`), open to anonymous users, with a test and README notes.
- ADR 0087, the guide chapter 20 (the roadmap chapter is now 22) and the API section.

Gate numbers at the end of Slice 6b: `cargo test --workspace --all-features --no-fail-fast` 2993 passed, 0 failed, 8 ignored (2821 before Slice 6b); `cargo test -p oxbloom-boot` per feature: none 475, `security` 475, `web` 475, `logging` 495, `ssl` 475, `actuator` 488, `tasks` 499, all features 533, each with 0 failed and 1 ignored (the probe); `make check-examples` 61 (59 before); clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB (237 overflows) and 239 KB with all features (238 overflows); the `cargo build -p oxbloom-boot` builds for `tasks`, `tasks,logging`, `tasks,ssl`, `tasks,security`, `tasks,actuator`, `security`, `security,logging`, `security,web` and `web`, each with default features off, pass, and the `--no-default-features --features tasks` doc tests (114) pass; `cargo tree -i ring --workspace --all-features` finds no `ring`. One flaky test showed up in the gate and is not part of this slice: `repeat_warning_comes_after_24_hours_not_before` in `crates/oxbloom-ssl/tests/validity.rs` (from C7, commit fa7d072) failed in one of two full runs and in about 2 of 9 runs on its own, and passed in the other runs; the slice does not touch `oxbloom-ssl`.

Limits that users must know (also in the guide):
- (Closed by Slice 6d, which added CSRF tokens.) There was no CSRF protection in Slice 6b: `SameSite=Lax` was the only mitigation. `HttpOnly` keeps scripts from reading the cookie and does nothing against cross-site requests. Login and logout came with Slice 6c.
- A stateless cookie cannot be revoked on the server: a copy stays valid until its expiry. With the stateless backend `Session::id()` is random on every request.
- Random nonces cap one key at about 2^32 writes, so the secret should be rotated periodically.
- The in-memory store is per process: sessions are lost at a restart and not shared between instances. The global cap evicts the oldest-touched session; Slice 6c replaced the O(n) scan per new save at the cap with sampled eviction (constant cost per write), and the cap can be exceeded by the number of racing saves. An anonymous flood that writes sessions, such as `GET /api/visits` in the todos example, can still fill the cap and then evict real users' sessions.
- Concurrent requests on one session are last-writer-wins. A logout racing another request cannot bring the session back. If a rotate commits before a concurrent logout of the old id, the session lives on under the new id. A `Session` clone used after the filter ran loses its writes silently.
- A store error or a `TooLarge` session answers 503 (the client sees the server's problem document and no session cookie), and the side effects of the handler stand. A failed `rotate` or save after the handler ran leaves the client without a session.
- At most 8 same-name cookie values are tried per request, and a `Cookie` header over 8 KiB counts as none.
- `security.session.secret` and `old-secrets` are refused on the command line, but a placeholder can evade this (the same gap as `security.jwt.secret`). `old-secrets` in one environment variable is split on commas, so use the indexed form for a secret that contains a comma.

Follow-ups from Slice 6b, not yet scheduled:
- Login and logout endpoints and form login: done in Slice 6c.
- Sampled eviction in the in-memory store: done in Slice 6c.
- CSRF protection: done in Slice 6d.
- Redis and SQL session stores (they must implement `update` and `rotate` as conditional writes).

Notes for Slice 6c:
- Logout should invalidate by the current session id, and an application should call `rotate_id` only at login.
- `SessionSettings::validate` now rejects an idle timeout below one second (such a value used to pass and made every write answer `Clear`).
- Browser-side ordering of `Max-Age=0` against a `Set-Cookie` of a concurrent rotate needs session lineage (Slice 6c or 6d).

Deferred minors from the Slice 6b reviews:
- The test `repeat_warning_comes_after_24_hours_not_before` in `crates/oxbloom-ssl/tests/validity.rs` is flaky (a timing wait on the reload task; it failed in about 2 of 9 solo runs): fix it in its own change, because it can fail a full-workspace gate at random.
- `cookie.rs` returns an empty `Set-Cookie` value as a fallback when the builder is misused, and a `debug_assert` for that case is missing.
- An over-long `Cookie` header is skipped on its own and no test pins it, and `SID=` returning an empty value is untested.
- Tests are missing for cookie names with a double quote, a comma, a tab or non-ASCII characters, and for paths with other control or non-ASCII characters.
- There is no test for `Duration::MAX` or for a `max_age` of `None`.
- The eviction at the cap scanned every shard on each new save; Slice 6c replaced it with sampled eviction at constant cost per write. A cookieless flood of anonymous session writers can still fill the cap and evict real users' sessions.
- Racing saves can exceed the cap; this is documented.
- Tests are missing for expired entries freeing room under the cap, for `rotate` of an expired old id and for `rotate` onto an existing new id, and no test captures the eviction warning.
- A failure of the random number generator panics in the request path (the filter does not catch it; the error filter answers 500).
- The docs should advise rotating the secret periodically because of random 96-bit nonces (about 2^32 writes per key); the guide and ADR 0087 now do.
- The format version is not part of the associated data; it is already authenticated inside the sealed payload, so this is fine.
- `TooLarge` counts only the cookie value, not the name and the attributes.
- `open` parses the JSON payload twice.
- Tests are missing for wrong types inside `d`, a negative or float `c`, `touched` before `created`, and a 29-byte value.
- The 8-value cookie cap and the 43-character shape filter are untested.
- After a backwards clock jump the once-a-minute `maintain` stops until the clock passes the last run plus 60 seconds.
- The once-a-minute `maintain` sweep runs inline on a request.
- The test `filter_order_is_before_security` does not prove the order (`session_filter.rs` pins it); it should be renamed or assert from the security chain.
- The doc of `SessionAutoConfiguration` said the filter "backs off" for a user `SessionFilter`, but a bare `SessionFilter` bean is not a `dyn Filter`, so the application must also provide it as `dyn Filter`; the docs now say so.
- A placeholder can evade the command-line refusal of the secrets (the same gap as the JWT reader); the guide notes it.
- `old-secrets` in a single environment variable is split on commas; the guide points to the indexed form.
- The test that the secret is not echoed uses a weak `= 0` needle.
- The 503 test does not check that the body is empty, and the application's problem renderer may render a generic problem document; the guide says the filter's own body is empty and the client sees the problem document.
- `cargo doc -D warnings` fails on a private intra-doc link in `jwks.rs:427`, which is older than this slice.
- Optional minors from the final review: `max-cookie-bytes` has no upper bound, although browsers drop a cookie above about 4096 bytes and the filter ignores a `Cookie` header over 8 KiB; the server backend looks up any 43-byte cookie value, not only the base64url alphabet; there is no guidance on the `__Host-` cookie name prefix against a sibling subdomain planting a same-name cookie (belongs with Slice 6c); two doc lines are over 100 characters (`crates/oxbloom-boot/src/autoconfigure/mod.rs:75`, `crates/oxbloom-test/src/app.rs:52`); and `SessionSettings::validate` has no unit test for a sub-second idle timeout.
- Of the notes for Slice 6c above, logout by the current session id and `rotate_id` only at login are done in Slice 6c; session lineage for the browser-side ordering of `Max-Age=0` is still open.

Rulings made during Slice 6b:
- Plan and subagent execution were approved together; the `SessionStore` trait got a default no-op `maintain` in the filter task; the branch `s6b-sessions` started from `s6a-password-basic`.
- At most 8 session-cookie values are tried per request (the server backend only values of the 43-character id shape); a real cookie after 8 bogus same-name cookies is ignored.
- A `TooLarge` error from the seal at commit answers 503 like every `SessionError`.
- A loaded session is written back with the conditional `update`, and moved with `rotate`. This closes the logout race in the trait itself. The ruling first said that a missing id answers `Clear`; the final review reversed that part, because an unknown id is never adopted, so a clear protects nothing and can delete a newer cookie (a double-submitted login, or a logout followed by a new login during a slow request). A missing id now stores nothing and leaves the cookie alone.
- The final review moved the absolute timeout into the filter: stored data past its absolute deadline counts as unknown at load, and the store TTL is the remaining lifetime of the session.
- The final review moved the gate for sampled eviction to before Slice 6c's saved request; the code change was made in Slice 6c.
- `SessionConfig::from_config` refuses the secrets on the command line by checking the value origin, as the JWT reader does; `security.jwt.secret` is not registered through `ConfigBuilder::secret_key` anywhere.
- The `security` feature of `oxbloom-boot` also enables `dep:oxbloom-web`, so the gate covers `cargo build -p oxbloom-boot --no-default-features --features security`.
- Work happens on the branch `s6b-sessions`, started from `s6a-password-basic`.

### Security Slice 6c: Form login (`oxbloom-security`) — done
Done on 2026-11-15; the design is the Slice 6c spec `2026-11-15-form-login-design.md` and the plan is `2026-11-15-form-login.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/21-form-login.md` and `docs/API.md` (section 4.7, the form login subsection, and `SecurityAutoConfiguration`), and decisions are in ADR 0088. CSRF tokens came with Slice 6d.

Delivered:
- In `oxbloom-security`: `FormLogin` (the settings and their builder), `HttpSecurity::form_login`, `FormLoginConfig` (the reader of `security.form-login`), `is_safe_redirect_target`, the new `SecurityBuildError` variants `FormLoginWithoutProvider` and `InvalidFormLoginUrl`, and `Serialize` and `Deserialize` for `Authentication`. The chain restores the `Authentication` from the session, handles `POST /login` and `POST /logout` after a same-origin check, saves the request of an anonymous browser (`GET` or `HEAD` with `Accept: text/html`) and answers 302 to the login page, redirects back after the login, and serves a default login page after the router answered 404 for it.
- The session keys `oxbloom.security.authentication` and `oxbloom.security.saved-request`; the prefix `oxbloom.security.` is reserved.
- Sampled eviction in `InMemorySessionStore`: the oldest of a sample of at most 16 entries, starting at a random shard and a random position within the first 64 entries of it; the method `eviction_probes` is hidden from the docs and exists for the tests.
- In `oxbloom-boot` (feature `security`): `SecurityAutoConfiguration` applies `form_login` when `security.form-login.enabled` is true, with ten property metadata entries (`security.form-login.*`); it needs the table `security.session` and a password provider bean.
- `examples/todos`: the HTML page `GET /me` behind the login, `[security.form-login]` in `application.toml`, a test `tests/login.rs` and README notes.
- ADR 0088, the guide chapter 21 (the roadmap chapter is now 25 since Slice 7b) and the API section.

Gate numbers at the end of Slice 6c: `cargo test --workspace --all-features --no-fail-fast` 3097 passed, 0 failed, 8 ignored (2993 before Slice 6c); `cargo test -p oxbloom-boot` per feature: none 484, `security` 484, `web` 484, `logging` 504, `ssl` 484, `actuator` 497, `tasks` 508, all features 542, each with 0 failed and 1 ignored (the probe); `make check-examples` 63 (61 before); clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB (237 overflows) and 239 KB with all features (238 overflows); the nine `cargo build -p oxbloom-boot` builds for `tasks`, `tasks,logging`, `tasks,ssl`, `tasks,security`, `tasks,actuator`, `security`, `security,logging`, `security,web` and `web`, each with default features off, pass; `cargo tree -i ring --workspace --all-features` finds no `ring`. The flaky `oxbloom-ssl` validity test from Slice 6b did not fail in this gate run.

Limits that users must know (also in the guide):
- (Closed by Slice 6d.) Slice 6c had no CSRF tokens: the same-origin check and `SameSite=Lax` were the interim measure. A request with neither `Origin` nor `Referer` is still allowed by that check; Slice 6d added tokens on top of it.
- The stored authentication is not refreshed: a role change or a lock takes effect at the next login or at session expiry. (Closed by Slice 7a, which revalidates on an interval.)
- Login always rotates the session id. API clients get 401 and no session. Basic and bearer never create a session, and Basic wins over a session.
- The default login page is served only after the router answered 404 for a `GET` or `HEAD` of the login path, so any 404 at that path, even one that an application handler chose deliberately, is replaced by the page. It has a fixed CSP and fixed texts. In Slice 6c it had no `frame-ancestors` or `X-Frame-Options`; Slice 6d added both. On `HEAD` the page keeps its body when seen through `App::handle`.
- The success and failure URLs are not exempt from the authorization rules; only `POST` login, `POST` logout and `GET` or `HEAD` of the login page are. A custom `failure_url` or `logout_success_url` on a path other than the login page needs a `permit_all` rule, otherwise the user is redirected to the login page again. The defaults work.
- The application's own `GET /login` handler always sees an anonymous user. An outage of the password provider (`ProviderUnavailable`) answers the same 302 as any failed login.
- Sampled eviction: the in-memory store evicts the oldest of a sample of at most 16 entries, starting at a random shard and a random position within the first 64 entries of it, at constant cost per write. An anonymous flood of `Accept: text/html` requests to protected paths can still fill the cap and evict real sessions, and overshoot from racing saves persists until expiry or a sweep.
- The redirect-target rule: a single leading `/`, no `//` or `/\` start, no control character or backslash, no `://` before the first `?`, at most 2048 bytes, and rechecked at use.

Follow-ups from Slice 6c, not yet scheduled:
- CSRF protection and the framing of the default login page: done in Slice 6d.
- Refreshing or revalidating the stored authentication (done in Slice 7a).
- Remember-me (done in Slice 7b), concurrent-session limits (done in Slice 7a), OAuth2 login, registration, password reset and per-user rate limiting.
- A themeable login page.

Deferred minors from the 6c reviews:
- The origin tests lack `http://example.com@evil.com` and the uppercase scheme and IPv6 cases.
- A trailing-dot host never matches the origin, so it fails closed.
- A losing duplicate password in the login form is not zeroised.
- U+2028 and bidirectional control characters are not rejected in redirect targets.
- The collision check of the logout path against the login paths compares paths exactly.
- The test of a tampered saved request does not assert that the value is removed.
- There is a misplaced comment in `form_login/mod.rs`.
- The stored `login` time is validated but not used.
- `Deserialize` of `Authentication` bypasses `Authentication::new`.
- The cookie-backend half of the fixation test is not a guard for `rotate_id`.
- There are no `HEAD` tests.
- `rotate_id` runs before `store_authentication`, so the error path of the latter is unreachable.
- `path_of` is duplicated in three files.
- The login handler has two small logging nits: it formats the error with `format!("{error:?}")`, and `username_len` is 0 for a malformed form.
- The test `wrong_types_are_rejected` only checks `is_err`.
- A bad URL while `enabled = false` is silently accepted, and an unknown key while disabled is untested.
- There is no boot startup test for a bad or equal field name.
- Sampled eviction: the lower bound of the probe test is weak, and the race test bound `< 60` is only an upper bound.

Rulings made during Slice 6c:
- The branch is `s6c-form-login`, started from `s6b-sessions`; nothing is pushed.
- The stored authentication is not refreshed (closed by Slice 7a), and a request with neither `Origin` nor `Referer` passes the same-origin check, because non-browser clients send neither.
- The default login page is the fallback for a 404 of the router, and it stayed frameable until Slice 6d.

### Security Slice 6d: CSRF tokens (`oxbloom-security`) — done
Done on 2026-11-22; the design is the Slice 6d spec `2026-11-22-csrf-design.md` and the plan is `2026-11-22-csrf.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/22-csrf.md` and `docs/API.md` (section 4.7, the CSRF subsection, and `SecurityAutoConfiguration`), and decisions are in ADR 0089.

Delivered:
- In `oxbloom-security`: `CsrfConfig` (the settings and their builder, with `validate`), `CsrfToken` (no `Display`, a redacting `Debug`, `as_str`), `CsrfProperties` (the reader of `security.csrf`), `HttpSecurity::csrf`, `RequestSessionExt::csrf_token`, and the new `SecurityBuildError` variants `CsrfWithoutFormLogin` and `InvalidCsrfConfig`.
- A synchronizer token of 32 random bytes (43 characters of base64url) in the session under the reserved key `oxbloom.security.csrf-token`, created lazily and never by the check. The chain checks it for every method except `GET`, `HEAD`, `OPTIONS` and `TRACE` when the session authenticates the request, and on the login and logout posts. The header `X-CSRF-TOKEN` or the urlencoded field `_csrf` carries it. A failed check is a fixed empty 403 that changes nothing.
- Login replaces the token, logout drops it. The default login page creates the pre-login session, carries the token in a hidden field and adds `frame-ancestors 'none'` and `X-Frame-Options: DENY`. An optional token endpoint (`security.csrf.token-endpoint`) answers `{"headerName":..,"parameterName":..,"token":..}`.
- In `oxbloom-boot` (feature `security`): `SecurityAutoConfiguration` calls `csrf` with the `security.csrf` settings when form login is on, with four property metadata entries (`security.csrf.*`); the boot stack probes did not grow.
- `examples/todos`: the `/me` page has two forms that post the token, `POST /me/reset-visits` needs it, the tests `tests/csrf.rs` and `tests/login.rs` use the token flow, and the README and `application.toml` describe it.
- ADR 0089, the guide chapter 22 (the roadmap chapter is now 25 since Slice 7b) and the API section.

Gate numbers at the end of Slice 6d: `cargo test --workspace --all-features --no-fail-fast` 3203 passed, 0 failed, 8 ignored (3097 before Slice 6d); `cargo test -p oxbloom-boot` per feature: none 496, `security` 496, `web` 496, `logging` 516, `ssl` 496, `actuator` 509, `tasks` 520, all features 554, each with 0 failed and 1 ignored (the probe); `make check-examples` 66 (63 before); clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB (237 overflows) and 239 KB with all features (238 overflows); the nine `cargo build -p oxbloom-boot` builds for `tasks`, `tasks,logging`, `tasks,ssl`, `tasks,security`, `tasks,actuator`, `security`, `security,logging`, `security,web` and `web`, each with default features off, pass; `cargo tree -i ring --workspace --all-features` finds no `ring`.

Limits that users must know (also in the guide):
- The checked set is every method except `GET`, `HEAD`, `OPTIONS` and `TRACE`, so `CONNECT` and an unknown method fail closed. The spec text listed four methods; the code is stricter.
- Anonymous requests with no session are exempt, and so is an anonymous `POST` that carries a pre-login session without a login. `Basic` and bearer requests are exempt, also with a stray session cookie.
- A failed check answers a fixed empty 403 and skips a custom `access_denied` handler, the same as the same-origin 403 of 6c.
- A script on the same origin (XSS) can read the token. A multipart form needs the header, because the reader parses only urlencoded bodies. The `_csrf` field is part of the body and is visible in the `Debug` output of the request, like the 6c password; the header is marked sensitive.
- With either session backend, an attacker who controls a sibling subdomain can plant the cookie `OXBLOOM_SESSION` (it has no `__Host-` prefix) of a session they logged in to and so know its token, so the token cannot stop that attack.
- `Basic` requests are exempt, but a browser that cached Basic credentials sends them on forged cross-site requests, which are not checked; `Basic` is for non-browser clients only.
- A form-field token is read only from urlencoded bodies of at most 4 KiB; a larger urlencoded form, or a body with one pair that does not decode, always gets 403 and must send the header.
- A login page left open past the idle timeout, or a logout after the session expired, gets a 403 instead of a redirect.
- Any application page that calls `req.csrf_token()` for an anonymous visitor writes a session. The framing headers of the default login page are always added, also with CSRF off.
- The login page and the token endpoint are anonymous session writers, bounded by sampled eviction but not prevented. Only the login page is protected against framing. (Closed by Slice 7a for `X-Frame-Options`.)
- The same-origin check still covers only the login and logout posts, and it allows a request with neither `Origin` nor `Referer`.
- The chain answers the token endpoint before the application route, so the endpoint wins on a collision. An application that invalidates or rotates the session inside its handler while it answers a `404` for the login page gets a page with a token that is no longer valid (a corner case).
- `HttpSecurity::csrf` without `form_login` fails the build, also with `enabled(false)`. With form login off, every `security.csrf` key except `enabled = false` fails the startup and names the key. The reserved session keys now are `oxbloom.security.authentication`, `oxbloom.security.saved-request` and `oxbloom.security.csrf-token`.

Follow-ups from Slice 6d, not yet scheduled:
- A general security-headers filter (done in Slice 7a), a signed double-submit cookie, a script-readable token cookie, multipart parsing, per-request tokens and extending the origin check to the application's routes.

Deferred minors from the 6d reviews:
- There are no tests for the case of the content type or for a header value that is not UTF-8.
- `CsrfToken::from_stored` accepts a non-canonical base64 value.
- One bad unrelated pair in a form body fails the whole body read for the token. This is inherited from 6c and fails closed.
- The rustdoc of `csrf_token` should say that a created token marks a new session for writing, which sets the cookie.
- The corrupt-token test does not check that the replacement is persisted, and an optional `debug_assert` on the result of the session `set` could be added.
- The test of the safe methods `OPTIONS` and `TRACE` asserts only that the answer is not 403.
- A rustdoc line of `HttpSecurity::csrf` in `http_security.rs` is too long.
- The `unwrap_or_else` fallback of the JSON helper in `form_login/handlers.rs` is unreachable.
- The test names in `csrf_page.rs` overclaim: `off_by_default` asserts only that the answer is not 200, and `only_anonymous_write` checks one `GET`. One comment there is garbled.
- The token endpoint is handled in two separate chain calls, one for the match and one for the answer.
- Two no-echo probes in the tests of `properties.rs` are weak.
- There is no test of `enabled = false` together with a bad name or endpoint, in the reader or in boot.
- The test of the 403 body checks only its wording.
- There is no test that the default `_csrf` field is rejected when a custom field name is configured.
- The boot collision test covers only `/login`.
- `CsrfProperties` is a unit struct that only serves as a namespace.
- A configured token endpoint is silently ignored when `enabled = false`.
- `CsrfConfig::validate` does not reject a `field_name` that equals the username or password field of form login.
- The form-field token is limited to urlencoded bodies of 4 KiB; reading it up to the web body limit would lift this.
- The request-log deny list is a list of names, so a custom CSRF header name must not be allowlisted for request logging.
- `RequestSessionExt::csrf_token` has a default body, so an outside implementer of the trait is not broken by the new method.

Rulings made during Slice 6d:
- The branch is `s6d-csrf`, started from `main`; nothing is pushed.
- The check is in the chain and not in a separate filter, because the chain knows which credentials authenticated the request.
- A failed check answers a fixed empty 403 and does not use a custom `access_denied` handler.
- The checked set is every unsafe method, and `HttpSecurity::csrf` without `form_login` fails the build even with `enabled(false)`.
- With form login off, every `security.csrf` key except `enabled = false` fails the startup, which is stricter than the first plan.
- The token endpoint without a session filter logs the fixed error and passes the request on to the application. The login page gets its token only when Oxbloom renders it.
- A refused login without a session filter logs the "add a SessionFilter" error and still answers 403.

### Security Slice 7a: Security headers and session management (`oxbloom-security`) — done
Done on 2026-11-22; the design is the Slice 7a spec `2026-11-22-session-management-design.md` and the plan is `2026-11-22-session-management.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/23-session-management.md` and `docs/API.md` (section 4.7, the slice 7a subsection, and `SecurityAutoConfiguration` in section 4.8), the decisions are in ADR 0090. Remember-me is Slice 7b, which is done (see below).

Delivered:
- Headers: `HttpSecurity::headers`, `HeadersConfig`, `HstsConfig`, `HeadersProperties` and the new `SecurityBuildError::InvalidHeadersConfig`. The chain adds `X-Content-Type-Options`, `Referrer-Policy`, `X-Frame-Options`, `Cache-Control: no-store` (authenticated requests only) and HSTS (`https` requests only) to every response, never over a header the response has. `headers` composes on the current settings.
- Per-user sessions: `PrincipalSessions` (`sessions_of`, `delete_of`, `evict_oldest`), `SessionSummary`, `SessionRegistry` (`sessions_of`, `invalidate_all`, `invalidate_others`), `SessionStore::principal_sessions` and `SessionFilter::principal_sessions`. The in-memory store implements the trait by scanning its shards for the reserved key `oxbloom.security.principal`; the cookie backend has no index.
- Limit: `HttpSecurity::session_limit` and `security.session.max-sessions-per-user` (the key `security.session.max-sessions` stays the cap of the whole store). Form login evicts the oldest sessions of the user at login and writes `oxbloom.security.principal` and `oxbloom.security.revalidated-at`.
- Revalidation: `HttpSecurity::revalidate`, `HttpSecurity::clock` and `security.session.revalidate-interval` (default 5 minutes, `0` is off, at least one second otherwise). The user is reloaded by the principal of the stored authentication; a missing, disabled or locked user ends the session.
- In `oxbloom-boot` (feature `security`): `SecurityAutoConfiguration` reads the keys and applies them before the customizers, a lazy `SessionRegistry` bean, the `SessionManagementAnalyzer`, nine `security.headers.*` metadata entries and two `security.session.*` entries. The boot stack probes did not grow.
- `examples/todos`: `security.session.max-sessions-per-user = 2`, `POST /me/logout-everywhere` (`LogoutEverywhereController`, which injects `SessionRegistry` and calls `invalidate_others`), the `/me` page has a third form, and `tests/session_management.rs` (3 tests with cookie jars). `tests/csrf.rs` now expects three `_csrf` fields. `oxbloom-core` became a dev-dependency of the example for the doc example.
- ADR 0090, the guide chapter 23 (the roadmap chapter was renamed to 24 with `git mv`, and to 25 in Slice 7b) and the API section.

Gate numbers at the end of Slice 7a: `cargo test --workspace --all-features --no-fail-fast` 3330 passed, 0 failed, 8 ignored (3203 before Slice 7a; counted by summing the `test result` lines, doctests included); `cargo test -p oxbloom-boot` per feature, summed the same way: none 522, `web` 522, `logging` 542, `security` 522, `ssl` 522, `actuator` 535, `tasks` 546, all features 580, each with 0 failed and 1 ignored (the probe); `make check-examples` 70 passed (66 before), 0 failed; clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB (237 overflows, exit 101) and 239 KB with all features (238 overflows, exit 101); the fourteen `cargo build -p oxbloom-boot --no-default-features` builds of CI (none, `logging`, `web`, `security`, `ssl`, `actuator`, `actuator,logging`, `actuator,ssl`, `actuator,security`, `tasks`, `tasks,logging`, `tasks,ssl`, `tasks,security` and `tasks,actuator`) and the four `oxbloom-security` builds (no features, `di`, `jwks`, `di,jwks`) pass; `cargo tree -i ring --workspace --all-features` finds no `ring`.

Limits that users must know (also in the guide):
- Concurrent logins of one user can overshoot the limit by the number of racing logins until the next login evicts the extra.
- The cookie backend has no index, so no limit, no registry and no "log out everywhere"; with it a copy of the cookie that was taken after the last check stays valid for up to one interval after the session ended, and a replayed old cookie works again if the user is enabled again before the replay.
- A changed password does not end any session. Revalidation is bounded by the interval, and a user service outage keeps the sessions (no backoff, one warning per due request).
- A `SessionRegistry` consumer on a store without the index fails the injection (also `Option<Arc<SessionRegistry>>`, at startup); with the cookie store there is no bean.
- The `security.headers.*` and the two session keys are applied only by `SecurityAutoConfiguration`, and are ignored with a user `SecurityFilterChain` bean, as for form login and CSRF.
- (Closed by Slice 7b: `Session::set` now enforces the reserved prefix `oxbloom.security.`.) `Session::set` did not enforce the reserved prefix `oxbloom.security.`, so application code could write the authentication and principal keys. Revalidation reloads the user by the principal of the stored authentication for that reason; the per-user index trusts the principal key.
- Boot applies the headers, the limit, the revalidation and the clock before the customizers (a customizer can override or compose), and form login and CSRF after them (configuration wins).
- Form login that a customizer turns on gets no default revalidation; an explicit interval works.

Follow-ups from Slice 7a, not yet scheduled:
- Remember-me (done in Slice 7b). A default content security policy, `Permissions-Policy`, a refuse-new-login policy, ending the sessions of a user on a password change, an admin endpoint for sessions and Redis or SQL indexes.

Deferred minors from the 7a reviews:
- Task 1: validate runs when headers disabled (document or leave).
- Task 1: session-authenticated login/logout POST responses get no no-store (authenticated=false in handle_post path).
- Task 1: headers tests lack assertions for token endpoint, CSRF 403, authenticated 403 no-store, malformed-credentials early return.
- Task 1: debug_assert on unreachable HeaderValue::from_str failure in apply.rs.
- Task 2: M1 remove_ids does not re-check principal ownership under the shard lock.
- Task 2: M2 concurrency test should assert store.len() equals loadable sessions.
- Task 2: M3 wrong comment in tests/session_principal.rs ("b and c go" -> "b goes").
- Task 2: M4 PrincipalSessions methods and SessionSummary accessors lack own doc examples.
- Task 2: M5 docs: delete_of/invalidate_all do not end a session saved while they run.
- Task 3: partial write if store_principal fails after store_authentication (unreachable today); write principal first or roll back.
- Task 3: handler takes plain usize max, relies on build rejecting zero; NonZeroUsize would make it structural.
- Task 3: log-leak test on normal path checks only "\"alice\"" and username=alice; use the stricter plain-alice check.
- Task 3: concurrent logins can overshoot max+1; document in ADR/guide (already in plan docs list).
- Task 4: no check fresh.principal()==stored principal (defence in depth; end session on mismatch).
- Task 4: cookie backend replay window: a cookie captured after the last check stays authenticated for up to one interval after the session ended; one sentence in the revalidate doc/guide/ADR.
- Task 4: derived Debug on Revalidated::Updated(Authentication) would print the principal (never formatted today).
- Task 4: interval.as_secs() rounds down; sub-second interval checks every request; Task 5 reader should require interval >= 1s (or document).
- Task 4: no backoff during user-service outage, one warn per due request.
- Task 5: default revalidation keys on form login from config only (form login enabled by a customizer gets no default revalidation; explicit interval works).
- Task 5: limit/interval without form login fail via SecurityBuildError that does not name the key (CSRF reader uses ConfigError).
- Task 5: SessionRegistry bean lazy; store without index -> injecting fails with RegistryWithoutIndex, Option<Arc<SessionRegistry>> consumers fail startup too; precedence split (headers/limit/revalidation/clock before customizers, form login and CSRF after).
- Task 5: session keys advertised in SESSION metadata but applied only by SecurityAutoConfiguration (ignored with a user SecurityFilterChain bean; same for form login and CSRF).
- Task 5: hsts.max-age accepts fractional seconds (1500ms truncates).
- Task 5: boot header test checks only a 401; no no-store / configured cache-control on an authenticated boot response.
- Task 5: over-long doc line at autoconfigure/security.rs:295.
- Task 5: boot compose test covers referrer-policy/content-type-options but not hsts/cache-control keys.

Rulings made during Slice 7a:
- The branch is `s7a-session-management`, started from `main`; nothing is pushed.
- The per-user limit key is `security.session.max-sessions-per-user`, because `security.session.max-sessions` already caps the whole store.
- There is no "expired" check, because `UserDetails` has only `enabled` and `locked`.
- The index is a scan of the in-memory store, not a second map, and `PrincipalSessions` has a third method `evict_oldest`.
- Revalidation reloads the user by the principal of the stored authentication, never by the principal key.
- A revalidate interval above zero and under one second is refused.

### Security Slice 7b: Remember-me (`oxbloom-security`) — done
Done on 2026-11-22; the design is the Slice 7b spec `2026-11-22-remember-me-design.md` and the plan is `2026-11-22-remember-me.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/24-remember-me.md` and `docs/API.md` (section 4.7, the slice 7b subsection, and `SecurityAutoConfiguration` in section 4.8), the decisions are in ADR 0091. Method security was Slice 8 (done on 2026-11-23, see below) and OAuth2 login and client is Slice 9, the next one.

Delivered:
- Reserved prefix: `Session::set` returns the new `SessionError::ReservedKey` for a key that starts with `oxbloom.security.` and writes nothing, `Session::remove` ignores such a key (it stays infallible), and `get` is unchanged. The `#[doc(hidden)]` methods `set_reserved` and `remove_reserved` serve Oxbloom's own writers and this repository's integration tests.
- Token and settings: a stateless signed token `v1.<base64url(username)>.<expiry>.<base64url(HMAC-SHA256)>` bound to the stored password hash, keyed by HKDF-SHA256 over the remember-me secret with its own info string, constant-time verification with old secrets for rotation, a limit of 1 KiB. `RememberMe`, `RememberMeSettings`, `RememberMeProperties`, `HttpSecurity::remember_me` (composes), and the new `SecurityBuildError::RememberMeWithoutFormLogin` and `InvalidRememberMe`.
- Marker and rule: `Authentication::remembered` and `is_remembered`, `AuthorizationRule::FullyAuthenticated` and `RuleBuilder::fully_authenticated()`. The new enum variants break exhaustive matches on `AuthorizationRule`, `SessionError` and `SecurityBuildError`.
- Issue, clear and auto login: the cookie `OXBLOOM_REMEMBER_ME` (`HttpOnly`, `SameSite=Lax`, `Path` and `Secure` from the session settings through the `CookieScope` request extension of the session filter, a fixed `Max-Age`) after a password login with the checkbox, cleared at logout and at a successful login without the box that carries the cookie, and an auto login in the chain only without a header credential and without a session login. The default login page shows the checkbox when remember-me is on.
- In `oxbloom-boot` (feature `security`): `SecurityAutoConfiguration` reads the six `security.remember-me.*` keys (checked even while disabled), applies remember-me before the customizers with the one clock bean, rejects a cookie name equal to the session cookie name, and lists the keys in its metadata. The boot stack probes did not grow.
- In `oxbloom-test`: `TestResponse::header_all`.
- `examples/todos`: `[security.remember-me]` with a public development secret, `tests/remember_me.rs` (6 tests with cookie jars: the checkbox and the round trip, no checkbox, the cookie attributes, a changed signature and a forged user name, a header credential, logout). The example's users come from the configuration, so the test cannot change a password or disable a user; the crate tests of `oxbloom-security` cover those. `tests/csrf.rs` needed no change: its count of three `_csrf` fields is for the `/me` page, not the login page.
- ADR 0091, the guide chapter 24 (the roadmap chapter is now 25, renamed with `git mv`) and the API section.

Gate numbers at the end of Slice 7b: `cargo test --workspace --all-features --no-fail-fast` 3465 passed, 0 failed, 8 ignored (3330 before Slice 7b; counted by summing the `test result` lines, doctests included); `cargo test -p oxbloom-boot --no-default-features` per feature, summed the same way: none 542, `web` 542, `logging` 562, `security` 542, `ssl` 542, `actuator` 555, `tasks` 566, and `--all-features` 600, each with 0 failed and 1 ignored (the probe); `make check-examples` 76 passed (70 before), 0 failed; clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB passing and 237 KB overflowing (exit 101) with no features, and 239 KB passing and 238 KB overflowing (exit 101) with all features; the fourteen `cargo build -p oxbloom-boot --no-default-features` builds of CI (none, `logging`, `web`, `security`, `ssl`, `actuator`, `actuator,logging`, `actuator,ssl`, `actuator,security`, `tasks`, `tasks,logging`, `tasks,ssl`, `tasks,security` and `tasks,actuator`) and the four `oxbloom-security` builds with `--no-default-features` (no features, `di`, `jwks`, `di,jwks`) pass; `cargo tree -i ring --workspace --all-features` finds no `ring` (cargo prints "nothing to print").

Limits that users must know (also in the guide):
- No per-device revocation and no theft detection: a stolen cookie works until it expires or the password changes, and the lifetime is fixed.
- An auto login on an unsafe request gets 403 until a `GET`; a logout `POST` after the session expired, with CSRF on, is 403 and keeps the cookie.
- The cookie holds the user name in the clear (base64url, not encrypted).
- The `UserDetailsService` must know every form-login user; any `Authorization` header blocks auto login.
- Log out everywhere (the 7a registry) and the session limit do not revoke remember-me: an evicted or ended session logs straight back in, and a burst of parallel requests after the session expired can each auto log in and count against the limit.
- A changed password ends every remember-me cookie of the user at once; existing sessions are not ended, because revalidation does not check the password. Call `SessionRegistry::invalidate_all` (7a) when a password changes.
- A JWT claim `oxbloom.remembered: true` counts as remembered (deny-only for `fully_authenticated()`); it is documented and the converter is not changed.
- The `CookieScope` request extension carries the session cookie's `Path` and `Secure` decision to the remember-me cookie; without a session filter there is no auto login.
- A customizer's `http.clock(..)` does not change the clock of remember-me; the keys apply only through `SecurityAutoConfiguration`.
- Secrets are never read from the command line and never echoed; rotate with `old-secrets`.

Follow-ups from Slice 7b, not yet scheduled:
- A persistent token store with per-device revocation and theft detection, a refreshing cookie, an admin endpoint to revoke remember-me cookies, and the 7a deferred minors.

Known minors after the 7b reviews (none changes behavior; the ones the guide already states as limits are left out):
- The tests do not check that a rejected `Session::set` or an ignored `Session::remove` leaves a fresh session without a `Set-Cookie`.
- The expiry boundary test of the token assumes the Linux `SystemTime` range instead of computing the bound with `checked_add`.
- `TokenCodec::new` returns a `Result` and its comment gives the wrong reason; one doc line of `parse` and one of `autoconfigure/security.rs` are over 100 columns, and a comment above the remember-me read in `security.rs` is misplaced.
- `AutoLogin` derives `Debug` and its `LoggedIn` variant holds an `Authentication`, which would print the principal if anyone formatted them (nothing does today).
- A session that revalidation ended, together with a valid cookie, costs a second user service lookup in the same request.
- The tests that look for one fixed log text read a log buffer shared by the whole process, so they do not prove the text appears exactly once.
- There is no test that a valid session with a cookie never reads the cookie, nor that a planted anonymous session id is rotated at auto login.
- The `remember_cookie` path returns without a log when the user is missing, which `validate` makes unreachable.
- The check that the remember-me cookie name differs from the session cookie name reads the configuration, so a `SessionFilter` bean with its own cookie name is not compared.
- The short-secret boot test does not pin whether `secret` or `old-secrets[0]` is the short one, nor that the failure report names the key; the bad disabled section is tested through `read()` and not through the startup report.
- The `header_all` doc example has no single-value case and no mixed-case name, and the lifetime metadata default is checked as a literal.

Rulings made during Slice 7b:
- The branch is `s7b-remember-me`, started from `main`; nothing is pushed.
- `Session::remove` stays infallible and ignores reserved keys; `set` returns `SessionError::ReservedKey`.
- Auto login is a free `check_token` function and a method of the form login handlers, because the handlers hold the limit, the CSRF settings and the clock.
- Any `Authorization` header, even of an unknown scheme, blocks auto login (fail-closed; header credentials win).
- The missing-session check runs after a cookie is found and before the token check.
- `AuthorizationRule` gets the variant `FullyAuthenticated`, a source-compatible change only for code that does not match the enum exhaustively.

### Security Slice 8: Method security (`oxbloom-security`, `oxbloom-macros`) — done
Done on 2026-11-23; the design is the Slice 8 spec `2026-11-23-method-security-design.md` and the plan is `2026-11-23-method-security.md` (both local files under `docs/superpowers`, named here without a link), usage is in the guide chapter `docs/guide/25-method-security.md` and `docs/API.md` (section 4.7, the slice 8 subsection, and `with_authentication` in section 4.10), the decisions are in ADR 0092. The next slice is Slice 9 (OAuth2 login and client).

Delivered:
- Attributes: `#[method_security]` on an inherent `impl` block, with `#[pre_authorize]`, `#[post_authorize]`, `#[pre_filter]` and `#[post_filter]` on its methods (crate `oxbloom-macros`, re-exported by `oxbloom-security`). The rules are typed and checked at compile time: `role`, `any_role`, `authority`, `any_authority`, `authenticated`, `fully_authenticated`, `permit_all`, `deny_all`, `all`, `any`, `not` and `custom = check(arg, ..)` with a synchronous function. The order is fixed: `pre_filter`, `pre_authorize`, the body, `post_authorize`, `post_filter`. It combines with `#[controller]` and with DI beans.
- Runtime in `oxbloom-security`: `AccessDenied` (`Unauthenticated` gives 401 and `Forbidden` gives 403 through `From<AccessDenied> for HttpError`, the same answers as the chain), `SecurityContext::current` and `SecurityContext::scope` over a Tokio task-local, `oxbloom_security::spawn` that carries the context, and the shared role and authority functions that the path rules and the method rules both use. A bare `tokio::spawn` loses the context and fails closed.
- The chain runs the rest of every request inside `SecurityContext::scope`.
- Weaving: an async method with a post attribute has its own `return`s rewritten into labeled `break`s so the declared return type keeps its coercions; a `return` inside a macro call that needs a coercion fails with E0308 (documented). Compile errors for a trait impl, a repeated attribute, an unknown rule, a name that is not a parameter, `returned` outside `post_authorize`, a non-`Vec` filter argument and a non-`Result` return type, with `trybuild` cases.
- The check runs at the first poll of an async method. A denial logs the method and the principal at debug level on the target `oxbloom::security` and never an argument or a returned value. There are no configuration keys.
- In `oxbloom-test` (feature `security`, which now also enables the optional `oxbloom-security` dependency): `with_authentication`.
- `examples/todos`: `TodoAdmin::purge_done`, a service method secured with `#[pre_authorize(role = "ADMIN")]`, behind `DELETE /api/admin/done`, and `tests/method_security.rs` (a service test with `with_authentication` and a test of 401, 403 and 200 with the demo users).
- ADR 0092, the guide chapter 25 (the roadmap chapter is now 26, renamed with `git mv`) and the API section.

Gate numbers at the end of Slice 8: `cargo test --workspace --all-features --no-fail-fast` 3562 passed, 0 failed, 8 ignored (3471 before Slice 8; counted by summing the `test result` lines, doctests included); `make check-examples` 79 passed (76 before), 0 failed; clippy `-D warnings` and `cargo fmt --check` clean; the boot stack probes are unchanged at 238 KB passing and 237 KB overflowing (exit 101) with no features, and 239 KB passing and 238 KB overflowing (exit 101) with all features; the fourteen `cargo build -p oxbloom-boot --no-default-features` builds of CI (none, `logging`, `web`, `security`, `ssl`, `actuator`, `actuator,logging`, `actuator,ssl`, `actuator,security`, `tasks`, `tasks,logging`, `tasks,ssl`, `tasks,security` and `tasks,actuator`) and the four `oxbloom-security` builds with `--no-default-features` (no features, `di`, `jwks`, `di,jwks`) pass; `cargo tree -i ring --workspace --all-features` finds no `ring` (cargo prints "nothing to print").

Limits that users must know (also in the guide):
- The caller is a task-local context: a bare `tokio::spawn` loses it and is denied; use `oxbloom_security::spawn`.
- The check of an async method runs at the first poll of its future, not at the call.
- A post check cannot name a parameter that the body moved, a pattern parameter cannot be named in `custom`, and `returned` is reserved in `post_authorize`.
- In an async method with a post attribute, a `return` inside a macro call that needs a coercion fails with E0308; a trailing `return Ok(x);` becomes a `break`, so `clippy::needless_return` no longer reports it.
- A method with `pre_authorize`, `post_authorize` or `post_filter` must return `Result<T, E>` with `E: From<AccessDenied>`; `pre_filter` alone allows any return type.
- `custom` checks are synchronous.

Parked, not yet scheduled:
- Asynchronous `custom` checks (for example a lookup in a service).
- An expression language.
- Caching of decisions.
- The per-call clone of the `Authentication` (through the `SecurityContext` clone).
- The `Vec<&str>` that the chain allocates in `AuthorizationRule::grants` for `HasAnyRole` and `HasAnyAuthority` on each request (fix: an iterator-generic core).

Known minors after the Slice 8 reviews (none changes behavior):
- Task 1: the `deny` log has no capture test of its own (a later test covers it).
- Task 2: the login page branch in `chain.rs` runs `next.run` unscoped, which equals anonymous (a one-line comment would help), and there is no test that a session or remember-me restored context reaches the scope (same code path).
- Task 3: the context and the `__auth` borrow are held across every `.await` of the body, although for a method with only pre checks the check could sit in an inner block; a pattern parameter gives the message "not a parameter"; type errors from the generated `Err(..)` have weak spans (use the span of the return type); the composition with `#[configuration]` has no test in this repository.
- Task 4: `#[allow(clippy::diverging_sub_expression)]` on the generated `let` also covers the user's async body, so it hides only `f(return x)` style code.
- Task 5: the boot test of 401 and 403 asserts only the status and the content type prefix, not the problem+json body or `WWW-Authenticate`; the allowed case of `/api/close` has no status assertion; the doc example in `security_context.rs` has a doubled `Ok::<..>`; the boot test uses the full path of `GrantedAuthority`.

Rulings made during Slice 8:
- The branch is `s8-method-security`, started from `main`; nothing is pushed.
- `pre_filter` alone leaves the body unwrapped and allows any return type, `impl Trait` included, because only post steps need the wrapper.
- `oxbloom-test` feature `security` enables an optional `oxbloom-security` dependency, because `with_authentication` names its types.
- The `.stderr` files of the UI cases were generated and read by hand.

### C10: Test support (`oxbloom-test`)

#### C10a: Context runner and clock — done
Done on 2026-11-08; design in `docs/superpowers/specs/2026-11-08-test-support-context-design.md`, plan in `docs/superpowers/plans/2026-11-08-test-support-context.md`, usage in `README.md` and `docs/API.md` (section 4.10).

Delivered:
- `Clock` and `SystemClock` in `oxbloom-core`: one method, `now() -> SystemTime`. The `Clock` landed in C10a, not as a separate slice. It reaches the JWT decoder (`JwtDecoderBuilder::clock`, `jwt_decoder_from_config_with_clock`), the JSON log timestamp (`LoggingSystem::init_with_clock`, `OxbloomApplicationBuilder::logging_clock`, feature `logging`) and boot (`JwtDecoderAutoConfiguration` takes an optional `Arc<dyn Clock>` bean). With a clock set, the JWT decoder turns off the `exp` and `nbf` checks of `jsonwebtoken` and makes them itself, after the issuer and audience checks. Without a clock it behaves as before.
- The crate `oxbloom-test`: `ContextRunner` (real boot lifecycle to the ready event, a synchronous check, shutdown; hermetic: no environment variables, banner and logging off, captured output, `server.port = 0`), `ContextView`, `FailureView` with `run_failing`, `Captured`, `Log`, `TempConfigDir`, `FixedClock` and `ManualClock`.
- Boot and example integration tests moved onto the crate.
- Deviations from the spec, all recorded there: the port default is the first command-line argument; the signal handlers cannot be disabled; shutdown has boot's 5 second timeout and a text-match guard for shutdown failures; `run_failing` does not check shutdown lines; the JSON log timestamp clock test lives in `oxbloom-logging` and `oxbloom-boot`, not in the runner; `property` refuses `security.jwt.secret`.
- Stack probes unchanged (boot 238 KB, 239 KB with `logging`; DI 206, 277, 230 and 194 KB).

Deferred minors from the C10a reviews:
- Clock: the `Arc<dyn Clock>` test uses `SystemClock` rather than a fixed clock, and the `Arc` impl is undocumented.
- JWT: `check_time` relies on `reject_tokens_expiring_in_less_than` being 0; add a comment.
- Logging: the `logging_clock` doctest has a comment instead of an assertion, there is no test of the clock in file output, and `clock_wiring.rs` must stay the only test that installs a global subscriber.
- JWT: a clock before the Unix epoch makes every token `Expired`, including tokens without `exp`.
- Test crate: `Log` has no `Debug` derive, and the test module in `clock.rs` has redundant imports.
- Spec: line 116 says the JSON log timestamp is tested through the runner; it is covered by `oxbloom-logging` and `oxbloom-boot` tests, so the text needs an update.
- Runner: the shutdown-failure guard is a text match (a user service that prints the same wording gives a false positive), the hung-stop test adds about 5 s, and `run_failing` does not check shutdown lines.
- Tests: a request-log test comment says "exactly one" but the check also passes with a duplicate, and the import order in `tests/web.rs` is off.

#### C10b: App and client, slicing — done
Done on 2026-11-08; design in `docs/superpowers/specs/2026-11-08-test-support-app-design.md`, plan in `docs/superpowers/plans/2026-11-08-test-support-app.md`, usage in `README.md` and `docs/API.md` (section 4.10).

Delivered:
- `App::handle_from(req, peer)` in `oxbloom-web`, added in C10b: it runs the server's dispatch steps (body limit, client resolution with the forwarded headers, filters, router) on a buffered `Request`, so an in-memory call gives the same answer as a served one. `handle` is unchanged and both share code with the server path.
- `TestClient`, `RequestBuilder` and `TestResponse` in `oxbloom-test` (feature `web`; `json` adds the JSON helpers): `in_memory`, `serve`, `connect`, `peer`, `addr`, `shutdown`, one request builder for all three modes, every `send` and `shutdown` bounded at 10 seconds. The socket transports use one HTTP/1.1 connection per client. That replaced the first design, the legacy `hyper-util` connection pool, after a keep-alive flake (5 failures in 30 runs of the keep-alive test) that came from a pool race.
- `TestApp`, `RunningApp` and `Stopped` (feature `web`): a whole boot application with a real web server on port 0, sharing one lifecycle module with `ContextRunner`; presets `new`, `web`, `secured` (feature `security`) and `full`; `listener`, `start` and `start_failing`.
- Migration: the real-HTTP tests of `oxbloom-boot` (`autoconfigure_web.rs`, `autoconfigure_http.rs`, 15 tests) moved onto `TestApp` and `TestClient`, and the request helpers of the `hello` and `todos` examples moved onto `TestClient::connect`. Stayed raw or on their own harness: `oxbloom-web`'s own tests, tests that need a raw socket, in-`src` unit tests (dev-dependency cycle), and the examples' own `Running`/`start` harness, because `TestApp` cannot take an `OxbloomApplicationBuilder`.
- Boot feature matrix (`cargo test -p oxbloom-boot`), re-baselined because the `oxbloom-test` dev-dependency turns boot's `web` and `security` features on for test builds: `--no-default-features` 298 -> 420, `--features web` 370 -> 420, `--features logging` 317 -> 440, `--features security` 329 -> 420, `--all-features` 440 -> 440. No `cargo test -p oxbloom-boot` run builds `web` without `security` any more, so the `#[cfg(not(feature = "security"))]` paths of boot (for example in `autoconfigure/server.rs`) lost that run. Feature-off coverage is now (a) the compile-only builds `cargo build -p oxbloom-boot --no-default-features`, `--no-default-features --features logging`, `--no-default-features --features web` and `--no-default-features --features security`, which do not see dev-dependencies, and (b) `cargo test -p oxbloom-test --features web`, which builds boot with `web` and without `security` and runs `TestApp::web()`, `TestApp::full()` and the stop and drop tests against it.
- Stack probes unchanged (boot 238 KB, 239 KB with `logging`; DI 206, 277, 230 and 194 KB).

Deferred minors from the C10b reviews:
- Task 1 (`handle_from`): `agree_app` duplicates `echo_app` in `tests/handle_from.rs`.
- Task 1: the `served()` helper parses the status with `text[9..12]`, which panics on an empty reply.
- Task 1: no test pins that `handle_from` marks credentials as sensitive (it is the same code path as the server).
- Task 2 (`TestClient`): an optional retry on a stale reused idle connection (`try_send_request`) is not done.
- Task 2: the expected text of a `should_panic` test should be tighter.
- Task 4 (`TestApp`): the `expect` on the captured `Config` has no captured output in its message.
- Task 6 (examples): `to_str().unwrap_or_default()` blanks non-UTF-8 header values in the `Reply` of the `todos` example.
- Docs: the `handle_from`, `serve` and `connect` snippets in `docs/API.md` reuse `app` and `Ping` without showing their setup.
- Feature coverage (recorded in the final fix wave): boot's `web`-without-`security` code runs only through `cargo test -p oxbloom-test --features web`; the four `cargo build -p oxbloom-boot` feature-off builds are compile-only.

### C11: Service connections and dev services
- `ConnectionDetails` abstraction: a config-independent description of a service (database, queue, JWKS issuer).
- Testcontainers integration (via `testcontainers-rs`) feeding connection details into the context; docker-compose detection for local development.
- Needs the database and queue crates to exist; schedule after them.

## Recommended order

1. **C2 Configuration v2.** Smallest dependencies, fixes open defects, and every other slice reads config.
2. **C1 DI v2** (done). Required by C3, C4, C8 and C10.
3. **Security Slice 6** (`PasswordEncoder`, `UserDetailsService`, Basic, form login and sessions): 6a (password encoding, user details, HTTP Basic), 6b (sessions), 6c (form login and logout) and 6d (CSRF tokens) are done, and so are Slices 7a (security headers, session management), 7b (remember-me) and 8 (method security). It needs beans with lifecycle and typed config, so it comes after C1 and C2.
4. **C3 Application lifecycle** with `FailureAnalyzer` (done: C3a and C3b).
5. **C5 Logging** (C5a and C5b done), then **C6 Web server properties, errors and JSON** (C6a, C6b-1 and C6b-2 done).
6. **C4 Conditions and auto-configuration** (C4a, the mechanism, and C4b, auto-configuring the existing wiring with property metadata, done).
7. **C10 Test support** (C10a, the context runner and clock, and C10b, `TestApp`, `TestClient` and slicing, done).
8. **C7 SSL bundles** (done) and **C8 Availability, info and health** (done), **C9a Tasks and scheduling** (done) and **C9b Retry** (done).
9. Security Slice 8 (method security, done) and 9 (OAuth2 login and client), then `oxbloom-ai`, database, jobs and the rest of `IDEA.md`.
10. **C11 Service connections** after the database and queue crates exist.

## Open questions for the design steps

- C4: decided in C4a: explicit per-crate lists (`auto_configurations()`); link-time registration may come later as an opt-in feature.
- C1/C4: keep the container compile-time checked as far as possible, or move to a runtime graph with better diagnostics. The current container is runtime, with compile-time macro help.
- C2: use `validator` or define our own `Validate` trait.
- C5: decided in C5a: `tracing` with `tracing-subscriber` and `tracing-log`; levels change at runtime through `LoggingSystem`, with an explicit `reload` call and no watcher.
- Naming: whether `oxbloom-boot` should re-export the other crates behind feature flags, as `spring-boot-starter-*` do.
