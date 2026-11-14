# ADR 0029: Lazy bean creation is serialized by one reentrant container lock

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-di v2 design (slice C1: traits, collections, lifecycle, beans, profiles)](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

Lazy beans are created on first use, possibly from several threads at once, and a lazy bean can depend on other lazy beans. Per-bean locks risk deadlocks when two threads walk crossing dependency chains.

## Decision

Lazy creation is serialized by one container-wide lock that is reentrant for the creating thread; the creation chain runs on that thread with its own resolver stack. Concurrent first access creates the bean once and the others wait. Two threads can never deadlock on each other's lazy chains, and cycles through lazy beans are reported as `Cycle`. `shutdown` takes the same lock. Errors in a lazy bean's creation appear at first use, not at `build()`.

## Consequences

Good:

- No deadlocks between lazy chains, by construction.
- Each lazy bean is created exactly once.

Bad:

- All lazy creation in a container is serialized, even for unrelated beans.
- A thread that needs a bean being created lazily on another thread waits on the lock; no timeout is planned.
