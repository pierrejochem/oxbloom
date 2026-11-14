# ADR 0043: Conditions are evaluated in registration order against the registrations kept so far

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Conditions and auto-configuration (slice C4a)](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md)

## Context

Conditional beans (`on_bean`, `on_missing_bean`, `on_property`) must be decided before the provider index is built. Spring evaluates them with complex phase rules; Oxbloom needed a rule that is simple and predictable.

## Decision

A condition is a modifier on the most recent registration, like `profile` or `primary`, and applies to any registration. `build()` evaluates conditions right after profile filtering, walking registrations in registration order and asking each condition in the context of the registrations kept so far. Several conditions on one registration are combined with AND (profile conditions use OR, which is documented), and evaluation stops at the first condition that does not match. A skipped registration is invisible, exactly like a profile-skipped one. A registration never sees itself or later registrations. The condition report records each evaluated condition with a value-free reason; boot prints it when `oxbloom.main.conditions-report = console` (default `off`).

## Consequences

Good:

- The outcome follows from reading the registrations top to bottom.
- `has_bean` uses an index built while walking, so evaluation stays linear.

Bad:

- A condition on a type registered later never matches, which is the documented order rule.
- Condition closures that panic abort `build()`.
