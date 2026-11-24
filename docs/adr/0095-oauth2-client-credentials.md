# ADR 0095: The OAuth2 client caches one access token per registration and coalesces the requests that ask for it

- **Status:** Accepted
- **Date:** 2026-11-24
- **Source:** the security Slice 9 design spec `2026-11-23-oauth2-design.md`, part C (a local file, not linked), its plan `2026-11-23-oauth2.md`, Task 11

## Context

An application that calls a protected service as itself, not on behalf of a user, needs an access token from the client credentials grant. Asking the provider on every call is slow and may be rate limited, and caching a token naively has three traps: many callers can find the token stale at the same moment and each ask for a new one, a failed request must not poison the cache, and a provider may send a lifetime of zero or none at all.

## Decision

**`OAuth2Client` holds one token per registration.** `OAuth2Client::client_credentials(registration_id)` returns the cached token until a skew (default 30 seconds) before its expiry and requests a new one after that. The client reads the registrations and providers of `OAuth2Settings` and resolves the provider metadata through a `MetadataCache`, the same type as the login uses but its own instance (ruling R23): `OAuth2Client::new` creates one and `OAuth2Login` another, so an application with both the login and the client discovers each provider twice per hour. Sharing one instance is parked. Discovery keeps the 5 second failure backoff of `MetadataCache`, so within 5 seconds of a failed discovery a call can return `TokenError::Discovery` or `Http` without any request; the token request itself has no backoff. Time comes from `oxbloom_core::Clock`, so tests use `ManualClock`.

**One request at a time per registration.** Each registration has a slot with an async lock that is held while the request is in flight. Callers that arrive meanwhile wait for the lock and take the outcome of that request, a token or a failure, instead of starting their own. A failure is shared with the callers that waited for it but is not kept: the next caller that arrives later makes a new request at once. This avoids a queue of one timed-out request per caller during an outage, and no backoff is needed here because a caller asks only when it needs a token. Slots exist only for configured registration ids, so an unknown id cannot grow the map.

**Lifetimes.** A token without `expires_in` counts as valid for 60 seconds. The expiry is computed with `TokenResponse::expires_at`, which cannot overflow. A token that is within the skew of its expiry when it arrives, including one with `expires_in = 0`, is returned to the callers that waited for that request but is not stored, so one call never loops and the next call asks again.

**`invalidate` and `invalidate_token` (ruling R22).** Both bump a generation counter of the slot, which makes the stored token unusable at once without waiting for an in-flight request; a token whose request started before the bump is not stored. The usable token sits behind a plain mutex that is never held across an await, so both are synchronous and can look at the token while a request is in flight. `invalidate(id)` is the unconditional reset. `invalidate_token(id, refused)` acts only if the stored token equals the refused one (same expiry, constant-time equal value), so a late 401 for an old token cannot discard a fresh one, and N callers refused with the same token cause one refetch between them. The interceptor of Task 12 uses `invalidate_token`.

**A public client cannot use this grant.** A registration with `client-authentication = none` is refused with `TokenError::Invalid` before any request: a public client has no secret, and an unauthenticated client credentials request would hand a token to anyone who knows the client id.

**Errors have fixed text.** `TokenError` is non-exhaustive and carries no URL, body or token. The provider's error code is surfaced only when it is one of the RFC 6749 section 5.2 codes (the same allow-list as the login), and is `unknown` otherwise; the `error_description` is never read. `AccessToken` hides the token in `Debug`.

## Consequences

- Many tasks can call `client_credentials` freely: they share one token and one request per expiry.
- A token that the resource server refuses before its expiry stays in the cache until the application calls `invalidate_token` (or `invalidate`).
- A token lifetime no longer than the skew means a new request on every call; keep the skew below the lifetime the provider issues. Clamping the skew to the lifetime is parked.
- `AccessToken` is `#[non_exhaustive]` (read its fields, do not build it), and `TokenError` is `Copy`, so a variant that carries non-`Copy` data would be a breaking change (parked).
- The cache is per process and in memory; instances behind a load balancer each ask the provider.
- A slot keeps the outcome of its last request in memory until the next request, so an expired token can stay in memory until then.
