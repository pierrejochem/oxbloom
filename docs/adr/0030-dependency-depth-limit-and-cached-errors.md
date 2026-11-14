# ADR 0030: Dependency depth is capped at 256 and creation errors are cached per entry

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-di v2 design (slice C1: traits, collections, lifecycle, beans, profiles)](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

Resolution is depth-first and recursive, so a very deep dependency chain can overflow the stack. Slice 3 also had a minor issue: a provider whose error was swallowed (for example by an optional injection) ran again on the next request.

## Decision

The dependency depth along the resolver stack is capped at 256; deeper is `DiError::TooDeep` with the path truncated to its first 8 entries. A creation that failed returns the same `DiError` for every later request of that entry, and the provider never runs again. `Cycle` and `TooDeep` describe the requester's stack rather than the entry, so they are never cached: an entry first reached deep or through a cycle can still be built from another stack. Tests build a 256-deep chain and check that a 300-deep chain fails on a thread with a small stack; the stack needed for 256 levels is measured by probes ([ADR 0047](0047-stack-probes-must-not-grow.md)).

## Consequences

Good:

- A pathological graph fails with a clear error instead of a stack overflow.
- Providers with side effects run at most once.

Bad:

- Graphs deeper than 256 are rejected even when the stack would suffice.
- A cached `Missing` or `Creation` error describes its first requester's path.
