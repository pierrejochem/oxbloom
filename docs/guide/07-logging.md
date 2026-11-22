# Logging

The crate `oxbloom-logging` builds a [`tracing`](https://docs.rs/tracing) subscriber from the `logging.*` configuration keys. With the cargo feature `logging` of `oxbloom-boot`, boot installs it for you, so an application needs no setup code. Events from crates that use the `log` facade reach the same subscriber and obey the same levels. The feature is off by default; without it boot is unchanged.

```toml
# application.toml
[logging.level]
root = "info"                 # the default level
"oxbloom_web::router" = "debug"   # a module path; a key with `::` needs quotes
web = "warn"                  # the level of the group `web` (see below)

[logging.group]
web = ["oxbloom_web", "oxbloom_security"]

[logging.console]
format = "compact"            # full (default), compact or pretty
color = "auto"                # auto (default), always or never
```

| Key | Meaning |
|---|---|
| `logging.level.root` | The default level for every target. Default `info`. |
| `logging.level.<target>` | The level of a module path such as `oxbloom_web`, or of a group name. |
| `logging.group.<name>` | A list of targets that share the level `logging.level.<name>`. |
| `logging.console.format` | `full`, `compact`, `pretty` or `json`. Default `full`. |
| `logging.console.color` | `auto` colors only when standard output is a terminal, `always`, `never`. Default `auto`. |

Levels are `off`, `error`, `warn`, `info`, `debug` and `trace`, matched case-insensitively. On the command line a level is `--logging.level.oxbloom_web=debug`, and a target with `::` is written the same way: `--logging.level.oxbloom_web::router=debug`. An invalid value is a `ConfigError::Invalid` that names its key and never contains the value, and with boot it fails the run in the `Config` phase with a report that explains the key. The console output goes to standard output.

The rules for targets and groups:

- A level for a target applies to that module path and everything below it. The longest matching target wins.
- A target name may contain only letters, digits, `_`, `-` and `:`. Anything else is rejected, because the name becomes part of a filter directive and a comma or `=` could add a second one.
- A target with its own level keeps it when its group has a level: the own target beats the group. When a group name equals a target name, the group wins. If two groups list the same target, the group that sorts last by name wins. A group cannot be named `root`.
- Group and `root` names are matched like configuration keys: case, `-` and `_` do not matter.

## Startup events

With the feature on, boot emits `tracing` events at the target `oxbloom::boot`:

- `info`: `Starting {app}` with the active profiles, and `Started {app} in {secs}s`. The name `{app}` is `application.name`, or `application` when the key is absent.
- `info`: each service started and stopped, and the address the web server is listening on.
- `warn`: configuration warnings, such as a deprecated key.
- `debug`: one event per condition decision. They are emitted together after the container is built.
- `error`: the one-line failure summary, in addition to the failure report.

The banner, the conditions report and the failure report are reports, not log lines, and keep going to the output target. A failure that happens before the subscriber exists, such as an invalid `logging.*` value, logs no `error` event; its failure report is the only record.

Boot skips the installation with `.logging(false)` on the builder or `oxbloom.logging.enabled = false` in the configuration. The property is checked for a valid boolean even when the builder turned logging off. A process has one global subscriber. If one is already installed, for example by an earlier application in the same test binary, boot keeps it, registers no `LoggingSystem` bean and emits one `warn` event through the existing subscriber; a bad `logging.*` value is still a failure in that case.

## File output

A file log is written in addition to the console, never instead of it.

```toml
[logging.file]
enabled = true
path = "var/log/myapp"        # a directory, created when missing; default "logs"
name = "myapp"                # file name prefix; default application.name, else "oxbloom"
rotation = "daily"            # hourly, daily (default) or never
max-history = 14              # keep this many files; unset keeps all; 0 is invalid
format = "full"               # full (default), compact or json; files are never colored
```

| Key | Meaning |
|---|---|
| `logging.file.enabled` | `true` writes a log file. Default `false`. |
| `logging.file.path` | The directory for the files. Default `logs`. |
| `logging.file.name` | The file name prefix. Default `application.name`, else `oxbloom`. |
| `logging.file.rotation` | `hourly`, `daily` or `never`. Default `daily`. Size-based rotation is not supported. |
| `logging.file.max-history` | The number of files to keep, at least 1. Unset keeps all of them. |
| `logging.file.format` | `full`, `compact` or `json`. Default `full`. |

A directory that cannot be created or a file that cannot be opened fails startup with `LoggingError::Io`, which carries the path; with boot the failure report suggests checking the directory and its permissions.

- Pruning is by file-name prefix. With `max-history`, every file in the log directory whose name starts with the configured prefix is a candidate for deletion, including unrelated ones (the prefix `app` also matches `application.log`). With `rotation = "never"` and `max-history = 1` the existing file is deleted at start. Use a dedicated directory and a distinctive prefix.
- The file writer is non-blocking and lossy: when its buffer fills, events are dropped and the caller is not blocked. Lines that are buffered are flushed when the last `LoggingSystem` clone drops, which boot does when `run()` returns, after the `failed` or `stopped` listeners. Events logged after `run()` returns are not written to the file.
- `LoggingSystem` creates the log directory and file and starts the writer before it installs the subscriber, so an init that fails with `AlreadyInstalled` leaves an empty log file. `logging.file.enabled = true` is ignored when logging is disabled (`oxbloom.logging.enabled = false` or `.logging(false)`).
- Two `LoggingSystem` builds in one process that point at the same directory run two writers on one file, and both prune it. This only happens in tests.

## JSON output

`logging.console.format = "json"` and `logging.file.format = "json"` write one JSON object per line. Console and file share the field options.

```toml
[logging.console]
format = "json"

[logging.json.fields]
thread = false                # leave a field out (all six default to true)

[logging.json.names]
timestamp = "ts"              # rename a field in the output
message = "msg"
```

`<field>` in `logging.json.fields.<field>` and `logging.json.names.<field>` is one of `timestamp`, `level`, `target`, `message`, `span` and `thread`. An unknown field, an empty name, or two included fields with the same output name is a `ConfigError::Invalid` that names the key.

```json
{"id":3,"level":"info","msg":"created todo","span":{"id":7,"name":"request"},"target":"todos","ts":"2026-11-01T09:30:12.481Z"}
```

- The keys are the configured names of the enabled fields. The order of the keys in a line is alphabetical and not guaranteed, so parse the line instead of relying on a position. Event fields other than `message` are extra keys, for example `id` above. A built-in key wins over an event field with the same name.
- `span` is the innermost span as a flat object: its fields plus `name`. A span field called `name` is replaced by the span name.
- `timestamp` is RFC 3339 in UTC with millisecond precision; `level` is lowercase.
- Message text is escaped, so each event is one line.

## Request logging

With the feature `web` of `oxbloom-boot`, `RequestLogAutoConfiguration` registers a `RequestLogFilter` as a `dyn Filter` bean when `logging.request.enabled` is `true`. It logs one event per request, after the response exists, with the target `oxbloom::web::request` and the fields `method`, `path`, `status`, `latency_ms` and `headers`.

```toml
[logging.request]
enabled = true
level = "info"                # error, warn, info (default), debug or trace
headers = ["user-agent", "x-request-id"]   # the allowlist; default empty

[logging.level]
"oxbloom::web::request" = "debug"          # needed only when level is below the root level
```

| Key | Meaning |
|---|---|
| `logging.request.enabled` | `true` registers the filter. Default `false`. |
| `logging.request.headers` | The request header names to print. Default empty. |
| `logging.request.level` | The level of the request event. Default `info`. |

- The query string is never logged, only the path. A path that still carries a query (a hand-built `Request`) is cut at the first `?`.
- The header allowlist is the operator's responsibility. Redacted regardless of the allowlist: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`, `x-csrf-token` and `x-xsrf-token`, in any case and with `-` or `_` as the separator (not with `.`), and any value flagged sensitive. A header value prints as `name="value"`; a redacted one prints as `name=<redacted>`. A name that only contains a credential word, such as `x-my-token`, is not redacted; this includes a custom `security.csrf.header-name`, which must not be allowlisted.
- A `logging.request.level` below the root level, or below `logging.level.oxbloom::web::request`, logs nothing unless that level is lowered to match.
- The filter takes the outermost slot, `order::REQUEST_LOG`, so it records the answers of the error handling, the body limit and the security chain too: 401, 403, 404, 405, 413 and 500 are in the request log.
- An application that registers its own `RequestLogFilter` bean replaces the auto-configured one.

## Changing levels at runtime

Boot registers the `LoggingSystem` as a bean, so any component can inject it:

```rust
use std::sync::Arc;
use oxbloom_logging::{Level, LoggingSystem};

#[component]
struct Diagnostics {
    logging: Arc<LoggingSystem>,
}

impl Diagnostics {
    fn verbose(&self) -> Result<(), oxbloom_logging::LoggingError> {
        self.logging.set_level("my_app::db", Level::Trace)
    }
}
```

- `set_level(target, level)` changes one target; `set_level("root", ..)` changes the default. For a group name it sets the level of the group; members that have their own level keep it.
- `reset(target)` returns the target to the level of the last `init` or `reload`. For a group it restores the group level and each member's own entry.
- `level(target)` returns the level in force.
- `reload(&config)` re-applies all `logging.level.*` and `logging.group.*` keys from a `Config` you pass, removes keys that are gone and drops earlier `set_level` changes. It validates `logging.console.*`, `logging.file.*` and `logging.json.*` but does not apply them, because the output layers are fixed at install time. There is no file watcher and no SIGHUP handler in this slice; calling `reload` is up to the application.
- Replacing the filter is not atomic with respect to events on other threads: an event may see the old level or the new one.
- Calling `set_level` from inside a logging callback does not deadlock.

A library crate must never call `LoggingSystem::init`. Only boot or the application does. A library emits events with `tracing` and leaves the subscriber to the application.

Without boot, call `LoggingSystem::init(&config)` once at the start of `main`; a second call, or any other installed subscriber, gives `LoggingError::AlreadyInstalled`.

## Invalid configuration

A built-in analyzer for invalid `logging` configuration (feature `logging`) turns a `ConfigError::Invalid` under the key `logging` into a failure report that names the key and lists the accepted values. It is not a public item.

## Limits

- Output layers (console, file and the JSON options) are fixed when the subscriber is installed. `set_level`, `reset` and `reload` change levels only; they apply to the console and the file alike.
- File rotation is time-based only; see [File output](07-logging.md#file-output) for the pruning and buffering limits.
- `RequestLogAutoConfiguration` needs the feature `web`. A panic in a filter ordered before the request log loses that request's log event; see [Errors and filter order](09-errors-and-filter-order.md#errors-and-filter-order). The events only appear when a subscriber is installed, for example with the feature `logging`.
- No HTTP endpoint for levels (C8), no MDC-style context fields and no per-appender level filters.
- One global subscriber per process; `LoggingSystem::init` fails with `AlreadyInstalled` the second time.
- A user-registered `LoggingSystem` bean is a duplicate-bean error, because boot registers its own.
