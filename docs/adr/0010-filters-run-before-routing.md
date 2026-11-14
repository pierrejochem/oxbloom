# ADR 0010: Filters run before routing and render their own responses

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-security design (Slice 5)](../superpowers/specs/2026-10-24-security-design.md)

## Context

Security needs a layer in front of the router. If filters ran after route matching, an unknown path would answer 404 before security ran, which leaks which routes exist.

## Decision

`Filter` is an async trait, `filter(&self, req, next) -> Response`, and `Next::run` calls the rest of the chain with the router at the end. Filters run before routing, so a deny-by-default chain answers 401 or 403 for unknown paths, never 404. A filter returns a `Response`, not a `Result`: it renders its own error answer. `Request` gains a typed extensions map backed by `http::Extensions`. With no filters registered, behavior is unchanged. In Slice 5 filters ran in registration order; [ADR 0056](0056-filter-order-slots.md) later replaced that with integer order slots.

## Consequences

Good:

- Route existence does not leak through a security chain.
- Filters compose freely and can short-circuit.

Bad:

- Every filter runs for every request, including unknown paths.
- Filters must build complete responses themselves (later unified through problem+json, [ADR 0055](0055-problem-json-errors.md)).
