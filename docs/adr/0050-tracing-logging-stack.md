# ADR 0050: Logging uses `tracing` with `tracing-subscriber` and the `tracing-log` bridge

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [C5a: Logging system, levels, groups and startup logging](../superpowers/specs/2026-10-25-logging-design.md)

## Context

Oxbloom needed a configured logging system driven by `logging.*` properties, with levels per target, groups and runtime changes. The Rust ecosystem has the `log` facade and the `tracing` ecosystem.

## Decision

A new crate `oxbloom-logging` builds the subscriber on `tracing` and `tracing-subscriber` (default features off; `std`, `fmt`, `ansi`, `env-filter`, `tracing-log`, `smallvec`) and initializes `tracing-log`, so events from crates that use the `log` facade reach the same subscriber and honor the same levels. It depends on `oxbloom-config` only. Target and group names may contain only letters, digits, `_`, `-` and `:`, because they become part of a filter directive and another character could inject a second directive. Its property keys are a plain static list that boot converts to `PropertyMetadata`.

## Consequences

Good:

- One subscriber for both ecosystems.
- Structured events and spans are available to the framework and to applications.

Bad:

- `tracing-subscriber` features and APIs shift between versions (0.3.23 has no `reload` feature; the reload module comes with `std`).

## Alternatives considered

- The `log` facade alone: rejected.
- A hand-written logging stack: rejected.
