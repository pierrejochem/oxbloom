# ADR 0058: JSON is offered as `Json<T>` and `Request::json`, negotiated after the handler

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6b-1: JSON bodies and content negotiation](../superpowers/specs/2026-11-01-web-json-design.md)

## Context

Applications hand-wrote JSON helpers. Spring offers message converters and typed handler arguments; Oxbloom handlers take a `Request` and return `IntoResponse`, and JSON is the only format needed.

## Decision

`Json<T>` is a response type and `Request::json::<T>()` a parsing method; handler signatures do not change and there are no typed extractors. Serde support is behind the `json` feature of `oxbloom-web`, on by default. `Request::json` requires an `application/json` or `+json` content type (else 415) and answers 400 with a `detail` that names the error class and position but never body text. Negotiation is an `Accept` check only on responses marked `JsonBody`, done by `JsonNegotiationFilter` at `order::NEGOTIATION` after the handler has run, as in Spring; problem+json errors are never turned into 406. A serialization failure answers 500 and logs the error text instead of sending it. There is no pluggable format registry.

## Consequences

Good:

- Small API that fits the existing handler model.
- Error answers stay problem+json and never loop into 406.

Bad:

- The handler runs before a 406 is decided, so its side effects happen even when the client cannot accept the answer.
- Other formats would need a new design.

## Alternatives considered

- Typed extractors (`Json<T>` as a handler argument): out of scope.
- A pluggable message-converter registry: rejected while JSON is the only format.
