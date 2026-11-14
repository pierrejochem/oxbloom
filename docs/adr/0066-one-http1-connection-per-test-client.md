# ADR 0066: Each socket test client uses one HTTP/1.1 connection

- **Status:** Accepted
- **Date:** 2026-11-08
- **Source:** [C10b: Test support for web applications (`oxbloom-test`)](../superpowers/specs/2026-11-08-test-support-app-design.md), [C10b entry in the core roadmap](../ROADMAP-core.md)

## Context

The C10b design sent real HTTP/1.1 through a hyper client with keep-alive, using the legacy `hyper-util` connection pool. A keep-alive test failed 5 times in 30 runs, and the cause was a race in the pool.

## Decision

The socket transports of `TestClient` (`serve` and `connect`) use one HTTP/1.1 connection per client instead of the legacy `hyper-util` pool. A flaky test is fixed at its cause, not retried.

## Consequences

Good:

- Deterministic keep-alive behavior in tests.
- Fewer moving parts than a pool.

Bad:

- Requests from one client are sequential on one connection.
- A stale reused idle connection is not retried automatically (recorded as a deferred minor).
