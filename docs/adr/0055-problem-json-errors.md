# ADR 0055: Every error answer is rendered as problem+json through one replaceable `ErrorRenderer`

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6a: Filter ordering, uniform errors and `server.*` properties](../superpowers/specs/2026-11-01-web-errors-ordering-design.md)

## Context

Before C6a, handlers, security and the framework produced error answers in different shapes (empty bodies, ad hoc JSON). Clients need one shape, and panics in handlers needed a defined answer.

## Decision

Handlers report errors with `HttpError` and `Result`; `IntoResponse` is implemented for `Result<T, E>`. Framework answers (404, 405 with `Allow`, 413, and 500 for a panic caught in `dispatch`) use the same path. The built-in `ErrorHandlingFilter` renders every response that carries `ErrorAttributes` and every bodiless 4xx or 5xx response through one `ErrorRenderer`; the default `ProblemRenderer` writes RFC 9457 `application/problem+json` with `instance` set to the path, never the query. A response that already has a body is left alone. An author's `detail` is always shown; the internal panic message is shown only with `server.error.include-message = always`. Boot registers `ProblemRenderer` with `on_missing_bean`, so an application bean replaces it. As a consequence, the 401 and 403 answers of the security chain ([ADR 0015](0015-authentication-failure-responses.md)) now carry problem+json bodies with unchanged headers.

## Consequences

Good:

- One error shape for clients, including framework and security answers.
- Panics answer 500 instead of dropping the connection, and their text stays internal by default.

Bad:

- Existing tests that asserted empty bodies had to change.
- The JSON is written by hand with escaping, so the renderer needs its own tests.
