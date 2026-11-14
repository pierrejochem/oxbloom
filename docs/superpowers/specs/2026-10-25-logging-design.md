# C5a: Logging system, levels, groups and startup logging

Date: 2026-10-25. Slice C5a of `docs/ROADMAP-core.md`. Depends on C2 (configuration), C3 (boot) and C4b (property metadata). C5b (file output, JSON output, request logging filter) follows in its own spec.

## Goal

An Oxbloom application gets a configured `tracing` subscriber without writing any setup code. Log levels come from `logging.level.*` properties, can be grouped, and can be changed while the application runs. Boot logs its own startup and shutdown events. The console output format is configurable.

## Decisions already taken (brainstorm, 2026-10-25)

- C5 is split. C5a is this spec. C5b holds file output with rotation, JSON output with configurable fields, and the request logging filter that never prints credential headers.
- The stack is `tracing` with `tracing-subscriber` and the `tracing-log` bridge. The `log` facade alone and a hand-written stack were rejected.
- Boot installs the subscriber early, right after the configuration is loaded and before the container is built. An explicit user call and an auto-configuration were rejected: the first leaves boot's own events unlogged, the second is too late for anything before the container.
- Runtime change means a handle API plus an explicit `reload(&Config)` call. There is no file watcher, no SIGHUP handling and no config refresh subsystem in this slice.

## Non-goals

File output and rotation, JSON output, the request logging filter (all C5b). An HTTP endpoint for levels (C8). A configuration watcher. MDC-style context fields. Per-appender level filters.

## Crate `oxbloom-logging`

A new workspace member, `crates/oxbloom-logging`. It depends on `oxbloom-config`, `tracing`, `tracing-subscriber` (features `std`, `fmt`, `ansi`, `env-filter` and `tracing-log`; see the Amendments) and `tracing-log`. It has no dependency on `oxbloom-di` or `oxbloom-boot`; boot depends on it.

### Properties

| Key | Type | Default | Meaning |
|---|---|---|---|
| `logging.level.root` | level | `info` | Default level for every target. |
| `logging.level.<target>` | level | none | Level for a module path, such as `oxbloom_web`, or for a group name. |
| `logging.group.<name>` | list of strings | none | Targets that share a level. `logging.level.<name>` applies to all of them. |
| `logging.console.format` | `full`, `compact`, `pretty` | `full` | Console line layout. |
| `logging.console.color` | `auto`, `always`, `never` | `auto` | ANSI colors. `auto` colors only when the output is a terminal. |

Levels are `off`, `error`, `warn`, `info`, `debug` and `trace`, matched case-insensitively. A group name takes precedence over a plain target with the same name. An invalid value is a `ConfigError::Invalid` that names its key and never contains the value.

### API

```rust
pub enum Level { Off, Error, Warn, Info, Debug, Trace }

pub struct LoggingSystem { /* opaque, cheap to clone */ }

impl LoggingSystem {
    pub fn init(config: &Config) -> Result<LoggingSystem, LoggingError>;
    pub fn set_level(&self, target: &str, level: Level) -> Result<(), LoggingError>;
    pub fn reset(&self, target: &str) -> Result<(), LoggingError>;
    pub fn level(&self, target: &str) -> Level;
    pub fn reload(&self, config: &Config) -> Result<(), LoggingError>;
}

#[non_exhaustive]
pub enum LoggingError {
    AlreadyInstalled,
    Config(ConfigError),
    Filter(String),
}
```

- `init` installs the global subscriber once. A second call in the same process, or any other installed subscriber, gives `AlreadyInstalled`.
- `LoggingSystem` stays an opaque struct, so C5b can add a worker guard for file output without an API change.
- `set_level` and `reset` change one target at runtime. `reset` returns the target to the level from the last `init` or `reload`. `set_level("root", ..)` changes the default.
- `reload` re-applies all `logging.level.*` and `logging.group.*` from the `Config` the caller passes. A key that is no longer present is removed. It does not change `logging.console.*`, because the output layer is fixed at install time.
- Level changes go through the `tracing_subscriber` reload handle on the filter layer. Replacing the filter is not atomic with respect to events on other threads; an event may see the old or the new filter. This is accepted.
- `tracing-log` is initialized by `init`, so events from crates that use the `log` facade reach the same subscriber and honor the same levels.
- A library crate must never call `init`. Only boot or the application does.

### Property metadata

`oxbloom-logging` cannot depend on `oxbloom-boot`, which owns `PropertyMetadata`. It therefore exposes the keys as a plain static list of `(key, type_name, default, description)` tuples, and `oxbloom-boot` converts them to `PropertyMetadata` for `property_metadata()`. A test checks that every declared key is read by `init`.

## Boot integration

`oxbloom-boot` gets a feature `logging`, off by default, which adds the `oxbloom-logging` dependency. Without the feature boot is unchanged.

- `run()` loads the configuration, then calls `LoggingSystem::init(&config)`, before the container is built.
- On success the `LoggingSystem` is registered as a bean, so user code can inject it.
- `AlreadyInstalled` is not fatal. Boot skips the bean and emits one `warn` event (target `oxbloom::boot`) through the subscriber that is already installed. It writes nothing to the output target, so a test binary that runs many applications in one process does not pollute their output.
- Any other `LoggingError` is a startup failure. A new built-in `LoggingConfigAnalyzer` (feature `logging`) explains a `ConfigError::Invalid` whose key is under `logging`, naming the key and a fixed message, never the value.
- The builder method `.logging(false)` and the property `oxbloom.logging.enabled=false` skip the installation. The property is listed in `boot_properties()`, with default `true`.

### Startup events

Boot emits `tracing` events with the target `oxbloom::boot`:

- `info`: "Starting {app}" with the active profiles, and "Started {app} in {secs}s".
- `info`: each service start and stop. The web server also logs its bound address.
- `warn`: configuration warnings, such as deprecated keys.
- `debug`: one event per condition decision.
- `error`: the failure summary, one line.

The banner, the conditions report and the failure report are reports, not log lines. They keep going to the `Write` output target. The failure summary is additionally logged as one `error` event, so a log file shows that startup failed.

## Testing

- `oxbloom-logging`: level parsing (every level, case-insensitive, an invalid value names its key and not its value); groups (expansion, a group name beats a target name); `set_level`, `reset`, `level` and `reload` against a capturing layer, including removal of a key on `reload`; console format and color through an injectable writer; `log` facade events captured through the bridge. The `AlreadyInstalled` test and every test that calls `init` live in their own integration-test binaries, because the global subscriber is per process. Other tests use `tracing::subscriber::with_default`.
- Boot: the startup events appear with the right levels and target; `.logging(false)` and `oxbloom.logging.enabled=false` turn installation off; an invalid `logging.level.*` value gives a startup failure whose report names the key and not the value; the `LoggingSystem` bean can be injected; `oxbloom.logging.enabled` has a consumption test like the other boot keys.
- Stack probes are re-run after the change. The numbers must not rise above the C4b baseline: 206, 277, 230 and 194 KB, boot 241 KB, and the `build()` frame at 0x178.
- `examples/hello` enables the feature. Its existing tests pass unchanged.

## Documentation

A runnable doc example for every public item (AGENT.md). `README.md` gets a logging section. `docs/API.md`, `AGENT.md` and `docs/ROADMAP-core.md` are updated: C5a done, C5b listed with its scope.

## Open risks

- A process has one global subscriber. Tests that install one must run in separate binaries, or they fail with `AlreadyInstalled`.
- `tracing-subscriber`'s reload handle holds the filter behind a lock. `set_level` from inside a logging call on the same thread must not deadlock; a test covers a `set_level` call from within an event handler context.
- Adding `tracing` to boot adds a dependency to the stack probe path. The probes decide whether this is acceptable.

## Amendments

These rulings were made while implementing C5a. Where they differ from the text above, they win.

- `tracing-subscriber` 0.3.23 has no `reload` feature. The reload module comes with `std`. The crate depends on `tracing-subscriber` with default features off and the features `std`, `fmt`, `ansi`, `env-filter`, `tracing-log` and `smallvec`; `Cargo.toml` is the source of truth.
- `AlreadyInstalled` is not an output line. Boot emits one `warn` event with target `oxbloom::boot` through the subscriber that is already installed, and writes nothing to the output target. With the feature on, a test binary that runs many applications would otherwise add a line to the output of every application after the first.
- Logging initialisation failures are reported in the `Config` phase of `StartupError`, because logging is installed before the container is built and no other phase fits.
- Group semantics. `set_level(group, level)` sets the level of the group itself. A member that has a level of its own keeps it, because an own target level beats its group. When two groups contain the same target, the group that sorts last by name wins. A group named `root` is invalid. `reset(group)` restores the group level and also the entry of each member to the values of the last `init` or `reload`. `level(group)` gives the group's level, or the root level when the group has none.
- Names are matched like `Config` keys: case, `-` and `_` are ignored when a group or `root` is looked up.
- A target name may contain only letters, digits, `_`, `-` and `:`. The names become part of a filter directive, so any other character, such as `,` or `=`, could inject a second directive. A name with another character is a `ConfigError::Invalid` naming the key (or, for `set_level` and `reset`, `logging.level.<target>`). The value is never part of the message.
- `Settings`, the parsed form of the `logging.*` keys, is a `pub` struct that is reachable only through the module `oxbloom_logging::__private`. That module is `#[doc(hidden)]` and not a public API; the integration tests use it. `LoggingSystem::init_with_writer` and boot's `logging_writer` are `#[doc(hidden)]` test hooks that send the console output to a writer other than standard output.
- `oxbloom.logging.enabled` is validated even with `.logging(false)`: a value that is not a boolean is a `Config` phase failure either way. A bad `logging.*` value is also fatal when a subscriber is already installed, because the configuration is parsed before the install is attempted.
- A failure before logging is installed, such as an invalid `logging.*` value, logs no `error` event, because there is no subscriber yet. The failure report on the output target is the only record.
- The application name in "Starting {app}" and "Started {app} in {secs}s" comes from the key `application.name`, with the default `application`. The key is listed in `boot_properties()`.
- `reload` validates `logging.console.*` but does not apply it, because the output layer is fixed at install time. An invalid console value therefore still fails a `reload`.
- The condition decisions (`debug` events) are emitted as one batch after the container is built, because the decisions are known only then. They are not interleaved with other startup events.
- `LoggingConfigAnalyzer` is `pub(crate)`, a built-in analyzer for invalid `logging` configuration. It looks inside `LoggingError::Config` for a `ConfigError::Invalid` whose key is `logging` or lies under it.
- A `LoggingSystem` registered by the user is a duplicate-bean error: boot registers its own `LoggingSystem` bean.
- The CRUD example `examples/todos` was added to this slice at the request of the user. It builds its JSON by hand, because `Json<T>` belongs to C6.
