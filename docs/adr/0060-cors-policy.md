# ADR 0060: CORS is off until origins are configured and never turns a disallowed origin into 403

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6b-2: CORS, compression and static resources](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md)

## Context

Browser cross-origin requests need CORS headers. A permissive default is a security risk, and CORS is a browser mechanism, not an authorization layer. Security slice 8 also listed CORS; this slice took it over.

## Decision

CORS is off until `server.cors.allowed-origins` is set. `CorsFilter` sits at `order::CORS`, outside error handling and security, so error answers carry CORS headers and a preflight never reaches security. A request from a disallowed origin passes without CORS headers instead of being turned into 403, and a disallowed preflight passes on to the chain. With a list of origins, every answer that is not a preflight answer gets `Vary: Origin`, so shared caches stay correct. `Access-Control-Allow-Origin` echoes the origin and is `*` only for an exact `*` without credentials. Validation rejects `*` mixed with origins, `*` with credentials, and origins that are not `scheme://host[:port]`.

## Consequences

Good:

- Secure default, and no accidental `*` with credentials.
- Security rules are unaffected by CORS.

Bad:

- A per-path CORS rule set is not supported.
- Disallowed origins get no explicit rejection, which can confuse developers debugging a browser.
