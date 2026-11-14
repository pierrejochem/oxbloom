# ADR 0015: Authentication failures answer 401, 403 or 503 with token-free challenges

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-security design (Slice 5)](../superpowers/specs/2026-10-24-security-design.md)

## Context

A security layer must tell clients why a request failed without revealing token contents or giving attackers an oracle, and must distinguish a bad credential from an outage of the key provider.

## Decision

An anonymous request or any invalid token gets 401 with `WWW-Authenticate: Bearer`; when a supplied token failed, the header carries `error="invalid_token"` (RFC 6750) and no description derived from the token. A malformed or duplicated `Authorization` header is `error="invalid_request"`. An authenticated but unauthorized request gets 403. An outage that prevents validation gets 503 without a challenge. `AuthenticationError`, `FetchError` and `SecurityBuildError` have fixed `Display` texts. In Slice 5 the bodies were empty; since C6a ([ADR 0055](0055-problem-json-errors.md)) the same statuses and headers carry a problem+json body.

## Consequences

Good:

- Clients and proxies can react correctly to each case, including retrying on 503.
- No token text reaches a response or an error message.

Bad:

- Clients get little detail about why a token was rejected; the application must log the `FailureReason` itself.
