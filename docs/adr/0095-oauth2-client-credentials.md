# ADR 0095: The OAuth2 client caches one access token per registration and coalesces the requests that ask for it

- **Status:** Accepted
- **Date:** 2026-11-24
- **Source:** the security Slice 9 design spec `2026-11-23-oauth2-design.md`, part C (a local file, not linked), its plan `2026-11-23-oauth2.md`, Task 11

## Context

An application that calls a protected service as itself, not on behalf of a user, needs an access token from the client credentials grant. Asking the provider on every call is slow and may be rate limited, and caching a token naively has three traps: many callers can find the token stale at the same moment and each ask for a new one, a failed request must not poison the cache, and a provider may send a lifetime of zero or none at all.

## Decision

**`OAuth2Client` holds one token per registration.** `OAuth2Client::client_credentials(registration_id)` returns the cached token until a skew (default 30 seconds) before its expiry and requests a new one after that. The client reads the registrations and providers of `OAuth2Settings` and resolves the provider metadata through the same `MetadataCache` as the login, so discovery is shared, cached and bounded the same way. Time comes from `oxbloom_core::Clock`, so tests use `ManualClock`.

**One request at a time per registration.** Each registration has a slot with an async lock that is held while the request is in flight. Callers that arrive meanwhile wait for the lock and take the outcome of that request, a token or a failure, instead of starting their own. A failure is shared with the callers that waited for it but is not kept: the next caller that arrives later makes a new request at once. This avoids a queue of one timed-out request per caller during an outage, and no backoff is needed here because a caller asks only when it needs a token. Slots exist only for configured registration ids, so an unknown id cannot grow the map.

**Lifetimes.** A token without `expires_in` counts as valid for 60 seconds. The expiry is computed with `TokenResponse::expires_at`, which cannot overflow. A token that is within the skew of its expiry when it arrives, including one with `expires_in = 0`, is returned to the callers that waited for that request but is not stored, so one call never loops and the next call asks again.

**`invalidate`.** It bumps a generation counter of the slot, which makes the stored token unusable at once without waiting for an in-flight request, and a token whose request started before the bump is not stored. It is a synchronous method, so callers can use it from any context.

**A public client cannot use this grant.** A registration with `client-authentication = none` is refused with `TokenError::Invalid` before any request: a public client has no secret, and an unauthenticated client credentials request would hand a token to anyone who knows the client id.

**Errors have fixed text.** `TokenError` is non-exhaustive and carries no URL, body or token. The provider's error code is surfaced only when it is one of the RFC 6749 section 5.2 codes (the same allow-list as the login), and is `unknown` otherwise; the `error_description` is never read. `AccessToken` hides the token in `Debug`.

## Consequences

- Many tasks can call `client_credentials` freely: they share one token and one request per expiry.
- A token that the resource server refuses before its expiry stays in the cache until the application calls `invalidate`.
- The cache is per process and in memory; instances behind a load balancer each ask the provider.
- A slot keeps the outcome of its last request in memory until the next request, so an expired token can stay in memory until then.
