# ADR 0038: Lifecycle events go through one `RunListener` trait with no-op defaults

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Application lifecycle (slice C3a): `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-design.md)

## Context

Applications want to observe startup phases (environment prepared, context ready, started, failed). Spring uses an event bus with typed events.

## Decision

Events go through one `RunListener` trait with one method per phase and an empty default body: `starting`, `environment_prepared(&Config)`, `context_ready(&Container)`, `started`, `ready`, `stopping`, `stopped(&ExitStatus)` and `failed(&StartupError)`. Methods take their arguments directly, without context structs. Listeners come from the builder (they also see phases before the container exists) and from beans. Listener methods are synchronous and run inside `catch_unwind`; a panic is ignored, named on the output target, and never changes the outcome.

## Consequences

Good:

- Implementing a listener means overriding only the phases of interest.
- A buggy listener cannot break the startup.

Bad:

- Listeners must not block, because they are synchronous.
- A new phase means a new trait method (with a default, so it is not breaking).
