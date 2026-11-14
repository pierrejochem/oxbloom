# C5b: File output, JSON output and request logging

Date: 2026-11-01. Builds on C5a (`docs/superpowers/specs/2026-10-25-logging-design.md`).

## Goal

A deployed Oxbloom application can write rotating log files, emit structured JSON, and log HTTP requests without leaking credentials. All three are driven by `logging.*` keys, in the same style as the C5a keys.

## Decisions

- Rotation is time-based only (`hourly`, `daily`, `never`). Size-based rotation is out of scope.
- JSON output has a fixed set of fields. Each field can be switched off and renamed. There is no user-defined layout.
- The request logging filter lives in `oxbloom-web` and is enabled through `oxbloom-boot` auto-configuration. `oxbloom-logging` gets no HTTP dependency.
- The slice must not grow the boot stack probe beyond the 241 KB baseline (239 KB at the end of C5a with `logging`).

## Out of scope

SIGHUP or file-watcher reload, MDC-style context fields, per-appender level filters, size-based rotation, applying `logging.console.*` or `logging.file.*` changes at runtime (the output layers stay fixed at install time, as in C5a), an HTTP endpoint for levels (C8).

## Properties

The `logging.file.*` and `logging.json.*` keys are added to `oxbloom_logging::PROPERTIES` (8 rows, next to the existing 5). The `logging.request.*` keys are listed by `RequestLogAutoConfiguration::properties()` in `oxbloom-boot`, not by `oxbloom_logging::PROPERTIES`. Key matching follows the existing rule (case-insensitive, ignoring `-` and `_`).

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `logging.file.enabled` | bool | `false` | Write a log file. |
| `logging.file.path` | string | `logs` | Directory for log files. Created if missing. |
| `logging.file.name` | string | `application.name`, else `oxbloom` | File name prefix. |
| `logging.file.rotation` | `hourly` \| `daily` \| `never` | `daily` | Rotation period. |
| `logging.file.max-history` | integer >= 1 | unset (keep all) | Number of rotated files to keep. |
| `logging.file.format` | `full` \| `compact` \| `json` | `full` | Line layout. Never colored. |
| `logging.console.format` | adds `json` | unchanged | Console may also use the JSON layout. |
| `logging.json.fields.<field>` | bool | `true` | Include the field. |
| `logging.json.names.<field>` | string | the field name | Rename the field in the output. |
| `logging.request.enabled` | bool | `false` | Log one event per HTTP request. |
| `logging.request.headers` | list of strings | empty | Request headers to include (allowlist). |
| `logging.request.level` | level | `info` | Level of the request event. |

`<field>` is one of `timestamp`, `level`, `target`, `message`, `span`, `thread`. An unknown field name, an empty or duplicate rename target, or a `max-history` of 0 is a `ConfigError::Invalid` naming the key.

## `oxbloom-logging`

- `Settings` parses the new keys into `FileOptions` and `JsonOptions`, next to `ConsoleOptions`.
- File output uses `tracing-appender`: a `RollingFileAppender` built with the configured prefix, rotation and `max_log_files`, wrapped in the non-blocking writer.
- `LoggingSystem` owns the non-blocking guard. Dropping the system, or boot shutdown, flushes buffered lines.
- JSON output uses a small custom `FormatEvent` implementation that writes each line with `serde_json`. It applies the field toggles and renames, so names do not depend on library defaults. Console and file share it.
- The file layer and the console layer both sit behind the existing reloadable filter, so `set_level`, `reset` and `reload` apply to both.
- A directory that cannot be created, or a file that cannot be opened, returns a new `LoggingError` variant that carries the path and the I/O error.

## `oxbloom-web`: `RequestLogFilter`

- Implements `Filter`. It emits one event per request at target `oxbloom::web::request` and at `logging.request.level`, with the fields `method`, `path`, `status`, `latency_ms` and the allowlisted headers.
- The query string is not logged. Only the path is.
- Redaction. A header value prints as `<redacted>` when its value has `is_sensitive()` set, or when its name is on the built-in deny list: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`. The deny list takes priority over the allowlist. The `App::handle` path does not mark values sensitive (only `serve` does), so name-based redaction is the guarantee. The check is on the lowercase header name.
- The filter takes its settings (enabled level, allowlist) in its constructor and reads no `Config` itself.

## `oxbloom-boot`

- An auto-configuration registers `RequestLogFilter` on the `App` when `logging.request.enabled` is true. It is skipped otherwise, and the conditions report shows why.
- It is described by `PropertyMetadata`, like the other auto-configurations.
- Invalid `logging.file.*`, `logging.json.*` and `logging.request.*` values flow through the existing logging analyzer. The new I/O error variant gets a suggested action (check the directory and its permissions).
- Boot keeps the new options inside the existing boxed settings, so the builder and the run plan do not grow.

## Testing

- Settings: defaults, each key, invalid values, key-spelling variants.
- File output (tempdir): file name and prefix, rotation names for each period, `max-history` cleanup, flush on drop, no ANSI in files, unwritable directory error.
- JSON: field toggles, renames, rename collisions, console `json`.
- Runtime levels still apply to the file and JSON layers.
- Request filter: event fields, status and latency, query string omitted, allowlist, redaction by sensitive flag and by name (every case and spelling of the deny-list names, plus a custom header on the allowlist that is also on the deny list).
- Boot: auto-configuration on and off, conditions report entry, analyzer output for a bad key and for an unwritable directory.
- Gate: `cargo test --workspace --all-features`, clippy `-D warnings`, `cargo fmt --check`, stack probes (zero growth over 241 KB, `build()` frame `0x178`).

## Documentation

README section "Logging" gains file, JSON and request logging. `docs/API.md` lists the new public items. `docs/ROADMAP-core.md` marks C5b done and records follow-ups. `examples/todos` gets a commented sample of the new keys.

## Dependencies

`oxbloom-logging`: `tracing-appender` (new) and `serde_json` (moves from dev-dependencies to dependencies, because the custom formatter writes the JSON). The `tracing-subscriber` `json` feature is not enabled. `oxbloom-web`: `tracing` only.
