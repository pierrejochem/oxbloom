# ADR 0013: JWTs are verified with `jsonwebtoken` and an algorithm allow-list fixed at build time

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-security design (Slice 5)](../superpowers/specs/2026-10-24-security-design.md)

## Context

A JWT resource server must verify signatures and claims. Writing cryptographic code is out of the question, and JWT libraries have a history of algorithm-confusion attacks where the token's `alg` header picks the verification key type.

## Decision

Oxbloom writes no cryptographic code; signature work uses the `jsonwebtoken` crate. The decoder's algorithm allow-list is fixed when it is built. The token's `alg` header is checked against the list and never selects the key type; `alg: none` is always rejected, and a decoder built for symmetric algorithms never accepts asymmetric ones or the reverse. `exp` is required by default (with an explicit opt-out), `nbf` is checked, `iss` and `aud` are optional, and clock skew defaults to 60 seconds. Failures map to `Expired` or `InvalidToken`; the client sees only `error="invalid_token"`, and an `on_failure` hook gives the application a token-free `FailureReason`. Tokens are read only from the `Authorization: Bearer` header, never from the query string or body.

## Consequences

Good:

- Algorithm confusion is ruled out by construction.
- No own cryptography to audit.

Bad:

- Behavior depends on `jsonwebtoken`'s validation semantics, which must be checked against its documentation on upgrades.
