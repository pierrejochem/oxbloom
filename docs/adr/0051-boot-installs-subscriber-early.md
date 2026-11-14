# ADR 0051: Boot installs the logging subscriber right after the configuration loads

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [C5a: Logging system, levels, groups and startup logging](../superpowers/specs/2026-10-25-logging-design.md)

## Context

The logging subscriber is global per process. If the application installed it explicitly, boot's own startup events would go unlogged; an auto-configuration would run too late, after the container is built.

## Decision

With the `logging` feature, `run()` calls `LoggingSystem::init(&config)` right after the configuration is loaded and before the container is built, and registers the `LoggingSystem` as a bean. `.logging(false)` or `oxbloom.logging.enabled=false` skips the installation. `AlreadyInstalled` is not fatal: boot emits one `warn` event through the existing subscriber and writes nothing to its output target, so a test binary that runs many applications does not pollute their output. Other logging errors fail the startup in the `Config` phase. A library crate must never call `init`; only boot or the application does. Banner, conditions report and failure report stay reports on the output target, not log lines.

## Consequences

Good:

- Boot's own events (starting, services, warnings, the failure summary) are logged.
- Zero setup code for applications.

Bad:

- Tests that install a subscriber, or run an application with `logging`, must live in their own test binary or turn logging off.
- A failure before logging is installed (an invalid `logging.*` value) is only visible in the failure report.

## Alternatives considered

- An explicit user call: leaves boot's events unlogged.
- An auto-configuration: too late for anything before the container.
