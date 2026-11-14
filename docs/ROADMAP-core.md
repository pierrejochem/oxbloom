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
| `task`, `thread`, `retry` | Task executor and scheduler properties, retry policy | `oxbloom-tasks` (C9a done): `#[scheduling]` and `#[scheduled]` with cron (six fields, Spring day-of-week numbers, IANA zones, daylight saving rules), fixed rate and fixed delay; a `Scheduler` with one clock-driven loop per task, skip on overlap, a concurrency bound and a bounded shutdown; the `tasks.scheduling.*` keys; `TasksAutoConfiguration` in `oxbloom-boot` (feature `tasks`). Retry, backoff and jitter (C9b) are open, and so are persistence, distributed locking and run-time changes of the task list |
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

### C9b: Retry (open)
- `RetryPolicy` and `#[retryable]` with backoff and jitter (the Boot `retry` package), built on the task types of `oxbloom-tasks`.
- Feeds the `oxbloom-jobs`/`queue`/`worker` crates in `IDEA.md`.

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
3. **Security Slice 6** (`PasswordEncoder`, `UserDetailsService`, Basic, form login and sessions). It needs beans with lifecycle and typed config, so it comes after C1 and C2.
4. **C3 Application lifecycle** with `FailureAnalyzer` (done: C3a and C3b).
5. **C5 Logging** (C5a and C5b done), then **C6 Web server properties, errors and JSON** (C6a, C6b-1 and C6b-2 done).
6. **C4 Conditions and auto-configuration** (C4a, the mechanism, and C4b, auto-configuring the existing wiring with property metadata, done).
7. **C10 Test support** (C10a, the context runner and clock, and C10b, `TestApp`, `TestClient` and slicing, done).
8. **C7 SSL bundles** (done) and **C8 Availability, info and health** (done), **C9a Tasks and scheduling** (done), then **C9b Retry.**
9. Security Slices 7 to 9, then `oxbloom-ai`, database, jobs and the rest of `IDEA.md`.
10. **C11 Service connections** after the database and queue crates exist.

## Open questions for the design steps

- C4: decided in C4a: explicit per-crate lists (`auto_configurations()`); link-time registration may come later as an opt-in feature.
- C1/C4: keep the container compile-time checked as far as possible, or move to a runtime graph with better diagnostics. The current container is runtime, with compile-time macro help.
- C2: use `validator` or define our own `Validate` trait.
- C5: decided in C5a: `tracing` with `tracing-subscriber` and `tracing-log`; levels change at runtime through `LoggingSystem`, with an explicit `reload` call and no watcher.
- Naming: whether `oxbloom-boot` should re-export the other crates behind feature flags, as `spring-boot-starter-*` do.
