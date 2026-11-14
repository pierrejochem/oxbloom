# ADR 0053: Log files rotate by time only, and JSON output has a fixed, configurable field set

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C5b: File output, JSON output and request logging](../superpowers/specs/2026-11-01-logging-output-design.md)

## Context

Deployed applications need log files and structured output. Rotation can be by time or size; JSON layouts can be fixed or user-defined.

## Decision

Rotation is time-based only (`hourly`, `daily`, `never`), through `tracing-appender`'s `RollingFileAppender` with the non-blocking writer; `LoggingSystem` owns the guard, so dropping it or boot shutdown flushes buffered lines. Size-based rotation is out of scope. JSON output has a fixed set of fields (`timestamp`, `level`, `target`, `message`, `span`, `thread`), each of which can be switched off and renamed; there is no user-defined layout. JSON is written by a small custom `FormatEvent` with `serde_json`, not the `tracing-subscriber` `json` feature, so field names do not depend on library defaults. Console and file share the formatter and sit behind the same reloadable filter.

## Consequences

Good:

- Field names in the output are stable and controlled by Oxbloom.
- Runtime level changes apply to every output.

Bad:

- No size-based rotation; high-volume services must rely on external rotation or short periods.
- No custom layouts beyond toggles and renames.
