# ADR 0034: Boot holds no global state

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Application lifecycle (slice C3a): `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-design.md)

## Context

Spring Boot relies on static state in places. Rust tests run in parallel threads inside one process, and a framework with global state forces tests into separate processes or serial execution.

## Decision

Boot holds no global state, so two `OxbloomApplication`s can run in one process; this is required for parallel tests. The same rule was repeated for failure analysis and conditions: two applications analyze and evaluate independently. The unavoidable process-wide exceptions are documented: Tokio's signal handlers stay installed once a service has run, and the `tracing` subscriber is global ([ADR 0051](0051-boot-installs-subscriber-early.md)).

## Consequences

Good:

- Integration tests run many applications in parallel in one binary.
- No hidden coupling between applications.

Bad:

- Some conveniences that would need a global (a static registry, an implicit current application) are not available.
- Process-wide resources (signals, the logging subscriber) still need care in tests.
