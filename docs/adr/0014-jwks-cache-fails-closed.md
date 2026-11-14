# ADR 0014: JWKS keys are cached with a rate-limited refresh and fail closed after a stale window

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-security design (Slice 5)](../superpowers/specs/2026-10-24-security-design.md)

## Context

A resource server that fetches signing keys from an issuer's JWKS endpoint must handle key rotation, issuer outages and attackers who send tokens with random `kid` values to make the service hammer the issuer.

## Decision

`JwksSource` caches keys for 5 minutes by default and looks them up by `kid`. An unknown `kid` triggers one refresh, rate-limited to one per 30 seconds, and concurrent lookups share a single in-flight fetch. If a refresh fails, the cached set is served until it is older than twice the TTL; after that the decoder fails closed with `ProviderUnavailable`, answered as 503. Only keys whose `kty`, `alg` and `use` fit the allow-list are used. The JWKS URI must use `https` unless `allow_insecure_http()` is called for loopback tests; the body is capped at 1 MiB and the timeout is 5 seconds. The default fetcher, `ReqwestJwksFetcher`, uses `reqwest` with rustls and sits behind the cargo feature `jwks`, so `oxbloom-security` without it pulls in no HTTP client.

## Consequences

Good:

- Key rotation works without restarts, and random `kid` values cannot amplify load on the issuer.
- A short issuer outage does not take the service down; a long one fails closed instead of accepting unverifiable tokens.
- The large `reqwest` dependency tree is opt-in.

Bad:

- The cache has concurrency subtleties (shared fetch, rate limit, stale serve) that need many tests.
- A key revoked at the issuer can stay usable for up to the TTL.
