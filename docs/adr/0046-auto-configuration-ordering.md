# ADR 0046: Auto-configurations are ordered by a stable topological sort

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Conditions and auto-configuration (slice C4a)](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md)

## Context

Auto-configurations depend on each other (the security chain uses the JWT decoder), so they declare `after` and `before`. The order must be deterministic and must keep the list order where there is no constraint.

## Decision

`A::after() = [B]` and `B::before() = [A]` both mean B is processed before A. The order is Kahn's algorithm that always takes the ready configuration earliest in the list. For `[P, Q, R, S]` with S before Q the order is P, R, S, Q; Spring Boot's sorter would give P, S, Q, R, and Oxbloom documents the difference. References to configurations not in the list and self-references are ignored. A cycle is a `Container`-phase `StartupError` naming the types on the cycle, with its own analyzer.

## Consequences

Good:

- Deterministic order that respects the list where it can.
- Cycles fail with a readable report.

Bad:

- The order can differ from Spring Boot's in tie situations.
