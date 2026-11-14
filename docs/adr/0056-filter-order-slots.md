# ADR 0056: Filters are ordered by integer slots

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6a: Filter ordering, uniform errors and `server.*` properties](../superpowers/specs/2026-11-01-web-errors-ordering-design.md)

## Context

With request logging, error rendering, a body limit and security all being filters, registration order ([ADR 0010](0010-filters-run-before-routing.md)) was too fragile: the request log could not see security rejections or body-limit answers, and the security chain was placed first by special-case code.

## Decision

Filters form one ordered chain. Public constants in `oxbloom_web::order` define slots; a lower number runs first (outer), and equal values keep registration order. The table grew to `REQUEST_LOG -400`, `CORS -350`, `ERROR_HANDLING -300`, `NEGOTIATION -250`, `BODY_LIMIT -200`, `COMPRESSION -150`, `SECURITY -100`, `DEFAULT 0`. `Filter` has a provided `order()` returning `DEFAULT`; `App::filter` inserts at `f.order()` and `App::filter_ordered` overrides it. The security chain becomes an ordinary `dyn Filter` that returns `order::SECURITY`. The 413 answer is produced inside the pipeline: `dispatch` marks an oversized body and `BodyLimitFilter` answers before security runs.

## Consequences

Good:

- The request log records every answer, including 401, 403, 404, 405 and 413.
- A filter carries its own place, whether it comes from code or from a bean; new built-ins slot in between existing ones.

Bad:

- Slot numbers are a public contract; moving one is a breaking change.
- Users must pick a number to place a filter relative to the built-ins.
