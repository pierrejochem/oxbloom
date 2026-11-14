# ADR 0037: All services stop under one shared shutdown deadline

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Application lifecycle (slice C3a): `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-design.md), [implementation plan](../superpowers/plans/2026-10-25-boot.md)

## Context

A service whose `stop` hangs must not hang the process, and a deployment platform sends SIGKILL after its own grace period. Per-service timeouts add up unpredictably.

## Decision

The whole stop sequence shares one deadline, `shutdown_timeout` (default 30 seconds). A service that has not stopped at the deadline is abandoned and named on the output target, and boot continues with `Container::shutdown()`. A second Ctrl-C or SIGTERM while services stop returns at once with code 130 or 143, fires `stopped` and skips `Container::shutdown()`. Signal listeners are installed just before the first service starts, so a signal during a slow `start` is kept. Panics in runners and `Service::start` propagate; only a panic in `Service::stop` is caught (each `stop` runs in its own task), so the remaining services still stop.

## Consequences

Good:

- `run()` always returns within a known bound.
- A double Ctrl-C forces an exit, as users expect.

Bad:

- One slow service eats the budget of the services stopped after it.
- Tokio's signal handlers stay installed for the rest of the process once a service has run.
