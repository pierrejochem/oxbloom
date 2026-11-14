# ADR 0028: Bean lifecycle hooks run through a trait and an explicit `shutdown`

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-di v2 design (slice C1: traits, collections, lifecycle, beans, profiles)](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

Beans such as connection pools need initialization and cleanup. Spring uses method annotations; Rust has `Drop`, but `Drop` cannot report errors, cannot be ordered across beans and does not run reliably when `Arc`s are still held elsewhere.

## Decision

Lifecycle uses a `Lifecycle` trait (`post_construct`, `pre_destroy`) plus builder closures, not method attributes. `post_construct` runs once per bean after its dependencies' hooks; a failing hook aborts `build()` and runs `pre_destroy` of already initialized beans in reverse creation order. `Container::shutdown()` runs `pre_destroy` for every created bean in reverse creation order, catches a panicking hook as a `DestroyFailure`, and is idempotent; afterwards every `get*` returns `ShutDown`. There is no implicit shutdown in `Drop` and no async hooks; boot calls `shutdown` explicitly.

## Consequences

Good:

- Cleanup order mirrors creation order and is testable.
- Shutdown failures are reported instead of lost.

Bad:

- An application that does not call `shutdown` (or use boot) never runs `pre_destroy`.
- Hooks are synchronous; async cleanup belongs in a boot `Service`.

## Alternatives considered

- Method attributes (Spring style): rejected in favor of a trait plus closures.
- Shutdown in `Drop`: rejected.
