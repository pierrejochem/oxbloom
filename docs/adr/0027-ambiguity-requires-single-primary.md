# ADR 0027: Several providers for one single dependency are an error unless exactly one is `primary`

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-di v2 design (slice C1: traits, collections, lifecycle, beans, profiles)](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

With trait injection, several beans can provide the same `dyn Trait`. A container that picks one silently (first or last registered) makes behavior depend on registration order in ways nobody sees.

## Decision

When a single `Arc<dyn T>` (or `Arc<T>`) is requested and several kept entries provide it, the one marked `primary` wins. With none or several `primary` entries the result is `DiError::Ambiguous { requested, candidates }`, naming the candidates by source label. The container never picks silently. Collections get all providers, ordered by `order` and then registration order.

## Consequences

Good:

- Ambiguity is visible at startup with the candidate list.
- Collection order is deterministic and controllable.

Bad:

- Adding a second implementation of a trait can break existing single injections until one is marked `primary`.
