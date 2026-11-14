# ADR 0054: Request logging is a filter in `oxbloom-web` that redacts credential headers

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C5b: File output, JSON output and request logging](../superpowers/specs/2026-11-01-logging-output-design.md)

## Context

HTTP request logging needs the request and response, which live in the web layer, while `oxbloom-logging` must not gain an HTTP dependency. Request logs are a classic place to leak credentials.

## Decision

`RequestLogFilter` lives in `oxbloom-web` and is enabled through a boot auto-configuration when `logging.request.enabled` is true. It emits one event per request at target `oxbloom::web::request` with method, path, status, latency and allowlisted headers. The query string is never logged. A header value prints as `<redacted>` when it is marked sensitive or when its lowercase name is on a built-in deny list (`authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`), and the deny list beats the allowlist. The filter takes its settings in its constructor and reads no `Config`.

## Consequences

Good:

- `oxbloom-logging` stays free of HTTP.
- Credentials cannot reach the request log even if a user allowlists them.

Bad:

- Useful data in query strings is never logged.
- Custom credential headers outside the deny list must be kept off the allowlist by the user.
