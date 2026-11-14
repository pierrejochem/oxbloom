# Application lifecycle (slice C3a): `oxbloom-boot`

Date: 2026-10-25

Status: implemented (2026-10-25); the Amendments section at the end records where the implementation refines this text

Roadmap item: C3 in `docs/ROADMAP-core.md`, split into C3a (this spec) and C3b (`FailureAnalyzer`, startup failure report, built-in analyzers, CLI template switch).

## Purpose

Today an Oxbloom program wires itself by hand: it builds a `Config`, builds a `Container`, creates an `App`, binds a port and handles Ctrl-C. The new crate `oxbloom-boot` replaces that with one entry point, `OxbloomApplication`, which runs the same steps in a fixed order, fires lifecycle events, runs startup runners, starts and stops long-running services, handles shutdown signals with a deadline, and turns the outcome into an exit status. It is the Oxbloom counterpart of Spring Boot's `SpringApplication`.

Success means that a program using only `oxbloom-config`, `oxbloom-di` and `oxbloom-boot` can start, serve, receive a signal and shut down in a deterministic, testable order, without any hand-written glue and without any global state.

## Scope

In scope:

- The `oxbloom-boot` crate and `OxbloomApplication` with its builder.
- `ApplicationArguments`.
- Lifecycle events through the `RunListener` trait.
- `ApplicationRunner` and `CommandLineRunner`.
- The `Service` trait and ordered start and stop with a shared deadline.
- Signal handling and `ShutdownHandle`.
- `ExitCodeGenerator`, `ExitStatus` and `StartupError`.
- A banner.
- A cargo feature `web` that adds `WebServer`, a `Service` that serves an `App`.
- Documentation and a migration of `examples/hello`.

Out of scope, with the slice that owns it:

- `FailureAnalyzer`, the startup failure report, the built-in analyzers and the `oxbloom new` template switch: C3b.
- Automatic registration of `WebServer` and other services: C4 (auto-configuration).
- Logging and log-based startup information: C5. Until then boot writes the banner and the few diagnostics it needs to a `Write` target that defaults to stderr.
- The full `server.*` property set (limits, compression, forwarded headers): C6.
- Readiness and liveness state: C8.

## Decisions taken in brainstorming

1. C3 is split into C3a and C3b (user choice).
2. The web server is a managed `Service` bean, not a special case in the run loop (user choice).
3. `run()` returns when every started service has stopped: with at least one `Service` it waits for a shutdown signal; with none it returns after the runners, like a command-line tool (user choice).
4. Events go through one `RunListener` trait with a default no-op method per phase (user choice).
5. `run()` returns `Result<ExitStatus, StartupError>`; `run_and_exit()` converts to `std::process::ExitCode`; exit codes come from `ExitCodeGenerator` beans (user choice).
6. Dependency direction: `oxbloom-boot` depends on `oxbloom-config` and `oxbloom-di`. The `web` feature of `oxbloom-boot` depends on `oxbloom-web`. `oxbloom-web` and `oxbloom-config` do not depend on boot (design decision, recommended to and accepted by the user).

## Public API

All public types are in `oxbloom_boot`. Types that may grow are `#[non_exhaustive]`. Every public function has a runnable doc example.

### `OxbloomApplication`

```rust
OxbloomApplication::builder()
    .args(std::env::args().skip(1))
    .config(|b: ConfigBuilder| b.dir("config"))
    .configure(|b: ContainerBuilder| b.component::<MyRepo>())
    .listener(MyListener)
    .banner(Banner::default())
    .shutdown_timeout(Duration::from_secs(30))
    .run()
    .await   // Result<ExitStatus, StartupError>
```

- `builder() -> OxbloomApplicationBuilder`.
- `args(impl IntoIterator<Item = impl Into<String>>)`: the command-line arguments, program name excluded. Default: none. Parsed into `ApplicationArguments`, and also handed to `ConfigBuilder::args` so that `--oxbloom.profiles.active=...` and property overrides work as they do in `oxbloom-config`.
- `config(FnOnce(ConfigBuilder) -> ConfigBuilder)`: adjusts the config builder that boot creates with `Config::builder()`. The arguments are applied by boot before this closure runs, so a closure cannot silently drop them.
- `with_config(Config)`: use a ready `Config` instead (for tests). Mutually exclusive with `config`; the last call wins.
- `configure(FnOnce(ContainerBuilder) -> ContainerBuilder)`: user wiring. May be called more than once; calls apply in call order.
- `listener(impl RunListener + 'static)`: a listener that also receives the phases before the container exists.
- `banner(Banner)`, `shutdown_timeout(Duration)`, `shutdown_handle() -> ShutdownHandle`, `output(impl Write + Send + 'static)` (banner and diagnostics target, default stderr).
- `run(self) -> Result<ExitStatus, StartupError>` (async), `run_and_exit(self) -> std::process::ExitCode` (async).

Boot registers these instances in the container before user wiring runs: `Config`, `ApplicationArguments`, `ShutdownHandle`. The `Config` active profiles are passed to `ContainerBuilder::active_profiles`. A user registration of the same type is a `Duplicate` error from `build()`, which boot reports as a `StartupError` in phase `Container`.

### `ApplicationArguments`

- Parsed once from the argument list: `--key=value` is an option with that value, `--flag` is an option with the empty string as value, `--` ends option parsing, every other argument is a non-option argument. A repeated option keeps all values in order.
- `option(&str) -> Option<&str>` (first value), `options(&str) -> &[String]`, `option_names() -> impl Iterator<Item = &str>`, `non_option_args() -> &[String]`, `raw() -> &[String]`.
- `ApplicationArguments::new(impl IntoIterator<Item = impl Into<String>>)` is public so tests can build one.
- `Debug` and `Display` show only the number of options and non-option arguments, never values, because values may be secrets.

### Traits

All are `Send + Sync + 'static` and are meant to be registered as beans with `provides`.

- `Service`: `async fn start(&self) -> Result<(), ServiceError>`, `async fn stop(&self)`, `fn phase(&self) -> i32` (default 0). Async methods use the same boxed-future style as `oxbloom_web::BoxFuture`. `ServiceError` wraps a boxed `std::error::Error + Send + Sync`.
- `ApplicationRunner`: `async fn run(&self, args: &ApplicationArguments) -> Result<(), RunnerError>`.
- `CommandLineRunner`: `async fn run(&self, args: &[String]) -> Result<(), RunnerError>`.
- `ExitCodeGenerator`: `fn exit_code(&self) -> i32`.
- `RunListener`: one method per phase, each with a default empty body and taking a shared reference to a small context type, for example `fn started(&self, ctx: &StartedContext) {}`. The phases are `starting`, `environment_prepared(&Config)`, `context_ready(&Container)`, `started(&Container)`, `ready(&Container)`, `stopping`, `stopped(&ExitStatus)` and `failed(&StartupError)`.

Order of beans of the same trait follows the DI rule: `order`, then registration order. Boot obtains them with `Container::get_all`.

### `ExitStatus` and `StartupError`

- `ExitStatus` holds the numeric code and `fn code(&self) -> i32`.
- `StartupError` is `#[non_exhaustive]`. `phase() -> StartupPhase` (`Config`, `Container`, `Runner`, `Service`, `Shutdown`), `source()` returns the underlying `ConfigError`, `DiError`, `RunnerError` or `ServiceError`. `Display` contains only the phase and the underlying error's own `Display`, which in this codebase is value-free. `StartupError` never contains argument values or config values.

### `ShutdownHandle`

Cloneable. `trigger()` requests shutdown and is idempotent. It is how tests, and runners that want to end the application, stop `run()` without a signal.

### `Banner`

`Banner::default()` (the name Oxbloom and the crate version), `Banner::off()`, `Banner::text(impl Into<String>)`. The config key `oxbloom.main.banner-mode` with the values `console` and `off` overrides the builder setting. Any other value is a `StartupError` in phase `Config`.

### Feature `web`: `WebServer`

- Built with `WebServer::new(app: App, config: &Config) -> Result<WebServer, ConfigError>`; typically registered with `.bean(...)` and `provides(|a: Arc<WebServer>| -> Arc<dyn Service> { a })`.
- Reads the properties `server.address` (default `127.0.0.1`) and `server.port` (default `8080`; `0` picks a free port) through typed binding.
- `phase()` returns `i32::MAX` so the server starts last and stops first.
- `start` binds the listener and spawns `App::serve` with a shutdown future. A bind failure is a `ServiceError`. `stop` signals graceful shutdown and awaits the serving task, so open connections drain.
- `WebServer::local_addr() -> Option<SocketAddr>` is available after `start`, which makes port 0 usable in tests.

## Run order

`run()` performs these steps in this order. After each failure, the failing step's phase is recorded, `RunListener::failed` is called with the error, and cleanup (below) runs.

1. Parse arguments. Fire `starting` on builder listeners.
2. Build the `Config`. Fire `environment_prepared` on builder listeners. Print the banner.
3. Build the `Container` with boot's instances, `active_profiles`, then the `configure` closures. Collect `RunListener` beans (`get_all`) and use builder listeners followed by bean listeners from now on. Fire `context_ready`.
4. Run all `ApplicationRunner` beans in order, then all `CommandLineRunner` beans in order. A runner error stops the sequence.
5. Collect `Service` beans and start them in ascending `phase`, ties by DI order. A start error stops the services started so far (reverse order), then fails.
6. Fire `started`, then `ready`.
7. If no service was started, go to step 8 without waiting: it fires `stopping` and has nothing to stop. Otherwise wait for the first of: Ctrl-C, SIGTERM on Unix, `ShutdownHandle::trigger`.
8. Fire `stopping`. Stop services in reverse start order. The whole stop sequence shares one deadline, `shutdown_timeout` (default 30 s). A service whose `stop` has not finished at the deadline is abandoned: boot writes its type name to the output target and continues.
9. Call `Container::shutdown()`. Failures in its `ShutdownReport` are not fatal and are written to the output target by type name.
10. Compute the exit status: the maximum `exit_code` of all `ExitCodeGenerator` beans, or 0 if there are none. Fire `stopped`. Return `Ok(ExitStatus)`.

Cleanup after a failure: stop already-started services in reverse order under the same deadline rule, then call `Container::shutdown()` if a container was built, then return `Err(StartupError)`. `run_and_exit` maps `Err` to exit code 1.

A second signal during step 8 returns immediately without waiting further, with code 130 for Ctrl-C or 143 for SIGTERM.

## Error handling and safety rules

- Listener methods are called inside `catch_unwind`. A listener panic is ignored (type name written to the output target) and never changes the outcome. Listener methods are synchronous and must not block for long; this is documented.
- Runner and service panics are not caught: they propagate like any panic in user code. Documented.
- No message produced by boot contains configuration values, argument values, header or secret values. Only type names, phase names, counts and the underlying error's value-free `Display` are used.
- The shutdown deadline is a hard bound on the stop sequence. A service that ignores it cannot hang `run()`.
- Boot holds no global state. Two `OxbloomApplication`s can run in one process, which is required for parallel tests. Signal handlers are installed only for the duration of step 7 and step 8.
- `ShutdownHandle::trigger` before step 7 is remembered, so a runner that triggers shutdown makes `run()` skip the wait.

## Testing

Unit tests: argument parsing (`--k=v`, `--flag`, `--`, repeated options, empty and unusual input), phase ordering of services, tie-breaking by `order`, deadline abandonment with a service that never stops, abort on runner error with no services started, abort on service start error with reverse stop of earlier services, listener panic isolation, exit code maximum, banner modes including the invalid `banner-mode` value.

Integration tests, each driving the complete lifecycle in-process with `ShutdownHandle` and `ApplicationArguments::new`, no real signals:

- An ordered event log shows exactly: `starting`, `environment_prepared`, `context_ready`, runner, service start, `started`, `ready`, `stopping`, service stop, container `pre_destroy`, `stopped`.
- An application with no `Service` bean returns right after the runners, with `started` and `ready` fired and no wait.
- A failure at each of the five phases returns the right `StartupError::phase()` and still calls `failed` once.
- With the `web` feature: bind port 0, read `local_addr`, make a real HTTP request, trigger shutdown, and observe that an in-flight request completes before `run()` returns.
- Hygiene test: a marker string placed in config values, argument values and a secret never appears in `Display` or `Debug` of `StartupError`, in the banner output, or in the diagnostics output.
- Compile-fail (trybuild): registering a `Service` that is not `Send + Sync` is rejected with the existing DI diagnostics.

## Review Focus

These inputs are not covered by the tests above by default and each gets a test in the owning task:

1. A runner that calls `ShutdownHandle::trigger` and a service that is already running: shutdown must still stop services in reverse order exactly once.
2. A `stop` that panics: the remaining services must still be stopped and the container shut down.
3. Two `Service` beans with equal phase and equal `order`: start order must be registration order and stop order its reverse.
4. A `RunListener` bean that depends on a lazy bean: `get_all` creates it during step 3; its failure is a `Container`-phase `StartupError`, not a listener panic.
5. A zero `shutdown_timeout`: services get no grace period, are abandoned immediately, and `run()` still returns.
6. Arguments with non-UTF-8 content are out of scope because the API takes `String`; `std::env::args()` panics on them, and the documentation recommends `args_os().map(lossy)` for programs that need to survive them.

## Task outline

About nine tasks, each with its own review:

1. Crate scaffold, workspace wiring and `ApplicationArguments`.
2. `RunListener`, event contexts and listener dispatch with panic isolation.
3. Runner traits, `OxbloomApplication` builder, steps 1 to 4 of the run order.
4. `Service`, ordered start and stop, shared deadline, cleanup after failure.
5. Signals and `ShutdownHandle`.
6. `ExitCodeGenerator`, `ExitStatus`, `StartupError`, `run_and_exit`.
7. Banner and the `banner-mode` property.
8. Feature `web` and `WebServer`.
9. Documentation (README, `docs/API.md`, `AGENT.md`, ROADMAP), migration of `examples/hello`, final gate.

## Amendments

These refinements were decided while planning and implementing the slice (plan `docs/superpowers/plans/2026-10-25-boot.md`, section "Spec amendments decided while planning"). Where they differ from the text above, they take precedence.

1. Only a panic in `Service::stop` is caught. Each `stop` runs in its own spawned task, so a panic arrives as a `JoinError` and is reported by name while the remaining services still stop. Panics in runners and in `Service::start` still propagate, as the safety rules say. This resolves Review Focus item 2 against the safety-rules paragraph.
2. `StartupPhase` has the variants `Config`, `Container`, `Runner` and `Service`. There is no `Shutdown` variant, because nothing produces it. The enum is `#[non_exhaustive]`, so a variant can be added later without a breaking change.
3. `RunListener` methods take their arguments directly (`&Config`, `&Container`, `&ExitStatus`, `&StartupError`). There are no context structs.
4. `RunListener` and `Service` have a provided method `fn name(&self) -> &'static str`, which defaults to `std::any::type_name::<Self>()`, so that diagnostics can name a `dyn` value.
5. A second signal while the services stop returns at once with code 130 for Ctrl-C or 143 for SIGTERM. It fires `stopped` and skips `Container::shutdown()`, so no `pre_destroy` hook runs.
6. Exit codes (superseded in part by amendment 16): `ExitCodeGenerator` beans are collected before `Container::shutdown()`, because a shut-down container refuses `get_all`, and they are evaluated after the services have stopped.
7. `WebServer` is a cheap `Clone` handle with an inner `Arc`, so a test or a listener can keep a clone and read `local_addr()`. The `ServerInfo` bean mentioned in early drafts is dropped.
8. Boot applies its arguments to the `ConfigBuilder` after the user's `config` closure, not before it as the `config` bullet above says. `ConfigBuilder::args` replaces earlier arguments, so applying them first would let a closure drop them. `stopping` fires on every successful run, also when no service ran; step 7 then goes to step 8 with nothing to stop. `oxbloom_boot` re-exports `async_trait` for `Service` and runner implementations. `ServiceError::source()` and `RunnerError::source()` return the wrapped error itself, so callers can downcast it to their own type.
9. One set of signal listeners serves steps 7 and 8 (superseded in part by amendment 15: the set is now installed just before the first service starts), so that a quick double Ctrl-C does not lose the second press. Any Ctrl-C or SIGTERM while the services stop forces 130 or 143, including the first signal after a `ShutdownHandle` trigger. Tokio's signal handlers stay installed for the rest of the process once a service has run; this is inherent to Tokio and documented. A runtime used with services needs the time and IO drivers.
10. The task order differs from the task outline above. The error types move to Task 2, because `RunListener::failed` needs `StartupError`, and `ShutdownHandle` moves to Task 3, because boot registers it as an instance.

Further amendments from the implementation:

11. The internal function that waits for a signal during step 8 (`second_signal`) is an `async fn` that takes ownership of the signal listeners from step 7, instead of a hand-written future. This is not public API; it is recorded because the review of Task 5 accepted it in this form.
12. `ExitCodeGenerator` beans are collected at the start of step 5, before any service starts, so a failure to create one fails the run early in the `Container` phase. A lazy generator bean is therefore created at that point, not at exit time. The codes are still evaluated after the services have stopped.
13. The banner is printed by default on every run: `Banner::default()` writes `Oxbloom v{version}` to the output target unless the builder sets `Banner::off()` or `oxbloom.main.banner-mode` is `off`.
14. Step 7 is corrected in the run order above: with no started service the run goes to step 8, fires `stopping` and has nothing to stop, instead of going straight to step 9.
15. Signal handlers are installed from just before the services start (step 5), not only in steps 7 and 8; this replaces the rule "Signal handlers are installed only for the duration of step 7 and step 8" in the safety rules. The one listener set is created right before the first `Service::start`, used by the step-7 wait and moved into step 8, so a Ctrl-C or SIGTERM during a slow `start` is kept: the services finish starting, the wait ends at once and they stop gracefully. With no `Service` bean no handler is installed.
16. Each `ExitCodeGenerator` code is mapped to a process exit code before the maximum is taken: 0..=255 stays, a negative code or one above 255 becomes 255. `ExitStatus` carries the mapped value, so a lone -1, or -1 next to 0, gives 255. `run_and_exit` then only converts the code; it still clamps to 0..=255 for an `ExitStatus` built by hand with `ExitStatus::new`. This replaces the `clamp(0, 255)` description that amendment 6 had.
17. `ApplicationArguments` implements `Display` with counts only, for example `ApplicationArguments(2 options, 1 non-option arguments)`, like its `Debug`.
