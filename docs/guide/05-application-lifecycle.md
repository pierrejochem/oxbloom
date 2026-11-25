# Application lifecycle

`oxbloom-boot` is the single entry point of an Oxbloom application. `OxbloomApplication::builder()` loads the configuration, builds the container, runs the runners, starts the services, waits for a shutdown signal, stops everything in order and returns an exit status. It holds no global state, so two applications can run in one process, for example in parallel tests.

```rust
use std::process::ExitCode;
use std::sync::Arc;
use oxbloom_boot::{OxbloomApplication, Service, WebServer};
use oxbloom_config::Config;
use oxbloom_di::DiError;
use oxbloom_web::App;

fn server(config: Arc<Config>) -> Result<WebServer, DiError> {
    // `?` keeps the `ConfigError` as the source, for the failure analyzers.
    Ok(WebServer::new(App::new(), &config)?)
}

#[tokio::main]
async fn main() -> ExitCode {
    OxbloomApplication::builder()
        .args(std::env::args().skip(1))
        .config(|b| b.dir("config"))
        .configure(|b| b.try_bean(server).provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s }))
        .run_and_exit()
        .await
}
```

## Run order

`run()` performs these steps in a fixed order:

1. It parses the arguments into `ApplicationArguments` and fires `starting`.
2. It builds the `Config` (the arguments are applied to the `ConfigBuilder` after the `config` closure, so `--key=value` overrides and `--oxbloom.profiles.active=dev` always work), fires `environment_prepared`, installs the logging subscriber (only with the feature `logging`, see [Logging](07-logging.md#logging)) and prints the banner.
3. It builds the container. Boot first sets `Config` as the container environment of the conditions, registers `Config`, `ApplicationArguments` and `ShutdownHandle` as instances and passes the active profiles of the configuration to the container, then applies the `configure` closures in call order. It then resolves the order of the auto-configurations and appends each one as a group (an ordering cycle fails here), builds the container and, with `oxbloom.main.conditions-report = "console"`, prints the conditions report. It collects the `RunListener` beans and fires `context_ready`.
4. It runs every `ApplicationRunner` bean, then every `CommandLineRunner` bean, in DI order (`order`, then registration order).
5. It collects the `ExitCodeGenerator` beans. If at least one `Service` bean keeps running (`Service::keeps_running`, `true` by default), it installs its signal handlers. Then it starts the services in ascending `phase`, ties in DI order.
6. It fires `started`, then `ready`.
7. If at least one started service keeps running, it waits for Ctrl-C, SIGTERM (Unix) or a `ShutdownHandle` trigger. With no service, or with services of which none keeps running, it does not wait, which makes a command-line tool return right after its runners.
8. It fires `stopping` and stops the started services in reverse start order under one shared deadline, `shutdown_timeout` (default 30 seconds).
9. It calls `Container::shutdown()`, which runs the `pre_destroy` hooks.
10. It computes the exit code and fires `stopped`.

A failure in any step first asks the failure analyzers (see [Failure analysis](05-application-lifecycle.md#failure-analysis)) and attaches the first analysis to the error, then fires `failed` once, stops the services started so far in reverse order, shuts the container down if it was built, and returns `Err(StartupError)`. `StartupError::phase()` is `Config`, `Container`, `Runner` or `Service`, and `source()` is the underlying `ConfigError`, `DiError`, `RunnerError` or `ServiceError`, so you can downcast it, or use `find_cause` to reach an error further down the chain.

## Services, runners and listeners

- A `Service` is a long-running part of the application, such as a server or a worker. Register a bean that `provides` `dyn Service`. `start` should return once the service runs and spawn its long-running work; `stop` runs in its own task. A `stop` that is still running at the deadline is abandoned (its task is aborted) and its name is written to the output; a service reached after the deadline is not stopped at all. A `stop` that panics is reported, and the remaining services still stop. `phase()` defaults to 0; lower phases start first and stop last.
- An `ApplicationRunner` receives the parsed `ApplicationArguments`, a `CommandLineRunner` the raw argument list. Both run once, before the services start. An error fails the run in the `Runner` phase.
- A `RunListener` observes the run. Builder listeners (`.listener(..)`) receive every event; `RunListener` beans in the container receive `context_ready` onward, after the builder listeners. Every method has an empty default.
- Implement the async traits with `#[oxbloom_boot::async_trait]`, which boot re-exports, so you need no `async-trait` dependency of your own.

## Exit codes and signals

`run()` returns the highest `exit_code()` of the `ExitCodeGenerator` beans, or 0 with no generator. Each code is mapped to a process exit code before the maximum is taken: 0..=255 stays as is, a negative code or one above 255 becomes 255. So a generator returning -1 makes the run exit with 255 even when another returns 0. `run_and_exit()` turns the status into a `std::process::ExitCode`, and into 1 after a startup failure, for which it writes the failure report to the output target first (with `failure_report(false)`, the error's message as one line instead). Use it as the tail of `main`.

Signal handlers are installed from just before the services start (step 5) until the stop sequence ends (step 8), and only when a service keeps running. A service whose `keeps_running()` returns `false`, such as the `DataService` of the data core, only holds resources between start and stop: when no started service keeps running, the run does not wait at step 7 and goes on to stop the services right away, so a run-to-completion application ends after its runners. A Ctrl-C or SIGTERM while a service is still starting is kept: the services finish starting, the wait ends at once and they stop gracefully. A Ctrl-C or SIGTERM that arrives while the services stop forces the exit: the rest of the stop sequence is abandoned, `Container::shutdown()` is skipped, `stopped` fires and the code is 130 for Ctrl-C or 143 for SIGTERM. This also applies to the first signal after a `ShutdownHandle` trigger. `ShutdownHandle::trigger()` is the portable way to end a run without a signal; a trigger before step 7 is remembered, so a runner can end the application before it waits. Inject it as `Arc<ShutdownHandle>` or get it from `OxbloomApplicationBuilder::shutdown_handle()`.

## Banner and output

Boot writes a banner after `environment_prepared`: by default the Oxbloom flower and wordmark in ASCII art, with `:: Oxbloom ::` and `(v{version})` on the last line. When the output is the default stderr and it is a terminal, the default banner is colored (petals and name in oxide, version in grey); `NO_COLOR` turns the colors off, and a redirected `.output(..)` always gets plain text. Change it with `.banner(Banner::text(..))` or turn it off with `.banner(Banner::off())`. The banner, the failure report and the few diagnostics boot writes (a service that failed to start, abandoned or panicking stops, panicking listeners and analyzers, failed `pre_destroy` hooks) go to stderr by default; `.output(..)` redirects them. No message produced by boot contains configuration values, argument values or secrets, except the port-in-use failure report, which shows the socket address; `Debug` and `Display` of `ApplicationArguments` and `Debug` of the builder show counts only.

## Feature `web`

With the cargo feature `web`, `WebServer` is a `Service` that serves an `oxbloom_web::App`. Its `phase()` is `i32::MAX`, so it starts after every other service and stops before them. `stop` stops accepting connections and waits until the open ones have drained, at most `server.shutdown-timeout` (default 30 s); then the remaining connections are dropped. `WebServer` is a cheap `Clone` handle, and `local_addr()` returns the bound address after `start`, which makes `server.port = 0` usable in tests. A failed bind fails the run in the `Service` phase with a `ServiceError` whose source is a `BindError`; `BindError::address()` and `kind()` return the address it tried and the `std::io::ErrorKind`, while its `Display` and `Debug` show neither the address nor the port.

## Failure analysis

When startup fails, boot asks a list of `FailureAnalyzer`s to explain the failure. The first `FailureAnalysis` it gets is attached to the `StartupError` (`error.analysis()`) and printed as a report with a description and, when the analyzer knows one, an action:

```
***************************
APPLICATION FAILED TO START
***************************

Description:

The web server could not listen on `127.0.0.1:8080` because the port is already in use.

Action:

Stop the process that uses the port, or set `server.port` to a free port (0 picks one).
```

A failure that no analyzer recognises gets the same block with the error's message as the description and no Action section.

Analyzers are asked in this order, and the first `Some` wins: the builder analyzers (`.failure_analyzer(..)`, in call order, for failures in every phase), then the beans that provide `dyn FailureAnalyzer` (in DI order, only when the container was built; a failure to create them is ignored), then the built-in analyzers in the order of the table below. Each call is panic-isolated: a panicking analyzer is skipped and named in the output as `analyzer {name} panicked`. Eager bean creation and wiring failures happen inside `build()`, before any container exists, so register analyzers meant for them on the builder with `failure_analyzer`. If creating the analyzer beans fails, all bean analyzers are dropped silently; if it panics, boot writes the line `failure analyzer beans panicked` and also goes on without them.

| Built-in analyzer | Matches | Description |
|---|---|---|
| Missing configuration key | `ConfigError::Missing { key }` | "The required configuration key `<key>` is not set." The action names `application.toml`, the profile file, the environment variable and `--<key>=<value>`. |
| Bean wiring | `DiError::Cycle`, `Missing`, `Ambiguous`, `Duplicate` | The cycle path; the bean that needs a missing type; the candidates of an ambiguous type; the sources of a duplicate. Each with an action, such as "Mark one of them `primary`, or inject `Vec<Arc<..>>`." |
| Port in use (feature `web`) | a `BindError` of kind `AddrInUse` | "The web server could not listen on `<address>` because the port is already in use." Other bind errors get the generic report. |
| Security (feature `security`) | every `SecurityBuildError` variant | For example "`authorize` was called more than once." with "Put all rules in one `authorize` call." |

The features are those of `oxbloom-boot`: `web` adds `WebServer` and the port analyzer, and `security` enables `oxbloom-security` with its `di` feature and adds the security analyzer. The exact texts are listed in the design, `docs/superpowers/specs/2026-10-25-boot-failure-analysis-design.md`, section "Built-in analyzers".

A custom analyzer recognises its own error with `StartupError::find_cause::<E>()`, which walks the `source()` chain (at most 64 levels) and returns the first error of type `E`:

```rust
struct QuotaAnalyzer;

impl FailureAnalyzer for QuotaAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        failure.find_cause::<QuotaExceeded>()?;
        Some(FailureAnalysis::new("The API quota is used up.", "Raise the quota."))
    }
}

OxbloomApplication::builder().failure_analyzer(QuotaAnalyzer)
```

For `find_cause` to reach an error raised inside a bean, the error must travel as a typed source: return `DiError::creation_from::<T>(error)` from the factory, or use `?` on a `ConfigError` or `SecurityBuildError` (see [Errors](03-dependency-injection.md#errors)). A factory that turns the error into a string with `DiError::creation::<T>(e.to_string())` loses it.

Printing: `run()` prints the report only with `.failure_report(true)`. `run_and_exit()` prints it by default, and with `.failure_report(false)` writes the bare error line instead; it never writes both. The report is written once, after the `failed` listeners ran and cleanup finished, as the last step before the error is returned.

Limits:

- Only `try_bean` and `#[bean(fallible)]` rewrite the type name of a `Creation` error to the failing bean's type. A `factory` or `Component::create` that uses `?` on a `ConfigError` is not rewritten: its error is labelled with `ConfigError`'s own type name. Use `try_bean` there, or `map_err(DiError::creation_from::<T>)` inside a `factory`.
- Analyzers only see startup failures. A failure after the application started (a crashed serving task, an error in a handler) is not analyzed.
- The port-in-use report shows the socket address. It is the one report that contains a configuration value, by design; `BindError`'s `Display` and `Debug` do not show it.
- The bad-pattern report cannot show the offending pattern, because `SecurityBuildError::InvalidPattern` does not carry it.
- The description and action are printed as given. An analyzer must not put the block markers (the line of asterisks or `APPLICATION FAILED TO START`) into them, and must not include configuration values, argument values or secrets. This is documented, not enforced.
- `find_cause::<DiError>()` returns the first `DiError` in the chain, which for a bean factory is the `Creation` error; a wiring error nested below a `Creation` error would not be analyzed by the bean-wiring analyzer. Nothing in Oxbloom produces that shape today.
- Boot still writes its diagnostic line `service X failed to start` before the report when a service fails to start.
- The default panic hook still prints the payload of a panicking analyzer to stderr; Oxbloom itself never prints it.

## Configuration keys

| Key | Meaning |
|---|---|
| `oxbloom.main.conditions-report` | `console` prints the conditions evaluation report right after the container is built, `off` (the default) prints nothing. Any other value fails the run in the `Config` phase; the error names the key, not the value. |
| `oxbloom.main.banner-mode` | `console` prints the banner (the default banner when the builder turned it off), `off` prints nothing. Any other value fails the run in the `Config` phase. |
| `application.name` | The application name in the startup events ("Starting demo"). Default `application`. Read only with the feature `logging`. |
| `oxbloom.logging.enabled` | `false` skips installing the logging subscriber. Default `true`. A value that is not a boolean fails the run in the `Config` phase. Read only with the feature `logging`. |
| `server.address` | The IP address `WebServer` binds to. Default `127.0.0.1`. A host name is a `ConfigError`. |
| `server.port` | The port `WebServer` binds to. Default `8080`; `0` picks a free port. |
| `server.shutdown-timeout`, `server.max-body-bytes`, `server.header-read-timeout`, `server.forward-headers.strategy`, `server.forward-headers.trusted-proxies`, `server.error.include-message` | Limits, timeouts, forwarded-header handling and the error switch. Defaults and rules are in [Server properties](09-errors-and-filter-order.md#server-properties). Read only with the feature `web`. |

Set them like any other key, for example `--server.port=9000` on the command line or `APP_SERVER__PORT=9000` in the environment.

## Limits

- A panic in a runner or in `Service::start` is not caught. It propagates out of `run()`, and the services started before it are not stopped. Only a panic in `Service::stop` is caught.
- Listener methods are synchronous and must not panic. A panic is caught and reported as `listener {name} panicked`, but the process-wide panic hook still prints the panic to stderr, and the remaining callbacks of that listener for the event are skipped.
- A Ctrl-C or SIGTERM while services stop forces exit code 130 or 143 and skips `Container::shutdown()`, so no `pre_destroy` hook runs.
- Once a service has run, Tokio's process-wide signal handlers stay installed for the rest of the process, so a later Ctrl-C or SIGTERM no longer terminates the process by default.
- Whenever a service is registered, boot needs a Tokio runtime with the time and IO drivers, such as `#[tokio::main]` or a runtime builder with `enable_all()`: boot bounds each `stop` with a timer, and when a service keeps running it installs its signal handlers just before the first service starts.
- Arguments are `String`. `std::env::args()` panics on arguments that are not valid UTF-8, so a program that must survive them passes `std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned())` instead.
- `ExitCodeGenerator` beans are collected before the services start, so a lazy generator bean is created at that point, not at exit time.
- A `stop` that exceeds `shutdown_timeout` is abandoned and its task is aborted. A service can therefore not hang `run()` as long as `stop` does not block the thread, but it may not have released its resources.
- The deadline is enforced by the Tokio timer, so it cannot interrupt a `stop` that blocks the thread: a blocking `std::thread::sleep` in `stop` on a current-thread runtime delays `run()` for as long as it blocks.
- The `pre_destroy` hooks of step 9 are not bounded by the deadline, and `Service::start` has no timeout.
- Dropping the `run()` future, or losing it in a `select!`, leaves the started services running and the container is never shut down. End a run with `ShutdownHandle::trigger()` instead.
- The Windows signal branch (Ctrl-C only, there is no SIGTERM) is compiled only on Windows and has not been verified.
