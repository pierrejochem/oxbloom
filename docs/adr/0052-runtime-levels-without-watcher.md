# ADR 0052: Log levels change at runtime through a handle and an explicit `reload`, without a file watcher

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [C5a: Logging system, levels, groups and startup logging](../superpowers/specs/2026-10-25-logging-design.md), [C5b logging output design](../superpowers/specs/2026-11-01-logging-output-design.md)

## Context

Operators want to raise a log level on a running application. Options range from a handle API to SIGHUP handling, a file watcher or a full configuration refresh subsystem.

## Decision

Runtime change means `LoggingSystem::set_level`, `reset` and `level` plus an explicit `reload(&Config)` that re-applies all `logging.level.*` and `logging.group.*` keys. There is no file watcher, no SIGHUP handling and no configuration refresh subsystem. Level changes go through the `tracing_subscriber` reload handle; the swap is not atomic with respect to events on other threads, which is accepted. The output layers (`logging.console.*`, later `logging.file.*`) are fixed at install time; `reload` validates but does not apply them.

## Consequences

Good:

- Small, testable API; an HTTP endpoint (C8) can build on it later.
- No background watcher thread or signal handling in a library.

Bad:

- Changing the configuration file has no effect until something calls `reload`.
- Output format changes need a restart.
