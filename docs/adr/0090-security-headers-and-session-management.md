# ADR 0090: Security headers are added by the security chain, and session management is a per-user scan of the in-memory store with a limit at login and a revalidation on an interval

- **Status:** Accepted
- **Date:** 2026-11-22
- **Source:** the security Slice 7a design spec `2026-11-22-session-management-design.md` (a local file, not linked) and its plan

## Context

After Slice 6d a logged-in browser user is protected against forged requests, but three things were still open. Responses carried no general security headers, only the framing headers of the default login page. A user could hold any number of sessions, and an application had no way to list or end the sessions of one user. And the authentication in the session was never checked again, so a user who was disabled or locked, or whose roles changed, kept the old session until it expired. How does Oxbloom add the headers, limit the sessions of a user, offer "log out everywhere" and keep a stored authentication from going stale, without a second store and without growing the boot stack?

Sessions are [ADR 0087](0087-sessions.md), form login is [ADR 0088](0088-form-login.md) and CSRF is [ADR 0089](0089-csrf-tokens.md).

## Decision

**The headers are added by the security chain.** `HttpSecurity::headers(|h| ..)` takes a `HeadersConfig`. The chain adds the headers to every response it returns or passes on, including the 401, the 403, the redirects, the default login page and the token endpoint. A header that the response already has is never replaced, and names are compared without regard to case, so an application or a handler wins. The defaults are all on: `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, `Cache-Control: no-store` (only when the request was authenticated by the session, by `Basic` or by a bearer token, and the response has no `Cache-Control`) and `Strict-Transport-Security: max-age=31536000; includeSubDomains` (only when `Request::scheme()` is `Https`, which holds for a TLS listener or a trusted proxy, so a forwarded header of an untrusted client has no effect). `max_age`, `include_subdomains` and `preload` are settable. There is no default content security policy and no `Permissions-Policy`. A header name or value must be a valid header token (visible ASCII, no control character), and `build` fails with `SecurityBuildError::InvalidHeadersConfig`, whose text names no value. `headers` composes: a new `HttpSecurity` starts from the defaults and each call changes only what its closure touches, so a customizer keeps the headers that the configuration set.

**The per-user index is a scan, not a second map.** The trait `PrincipalSessions` (separate from `SessionStore`, so existing stores keep compiling) has `sessions_of`, `delete_of` and `evict_oldest`. `InMemorySessionStore` implements it by scanning its shards under their locks and reading the value of the reserved session key `oxbloom.security.principal` of each live entry. A scan is exact by construction, because there is no index that could disagree with the store, and it costs time in proportion to the stored sessions, but only when a limit or the registry is used. The cookie backend has no server-side state, so it has no index. `SessionStore` gets a defaulted method `principal_sessions` that returns the index (`None` by default), and `SessionFilter::principal_sessions` exposes it. `SessionSummary` carries the id, the created time and the touched time, and its `Debug` prints none of them.

**Form login writes two reserved keys.** At every successful login it stores the username under `oxbloom.security.principal` (a JSON string) and the time of the last check under `oxbloom.security.revalidated-at` (seconds since the epoch). The index reads the first key. Revalidation never reads the first key to find a user: it reloads the user by the principal of the stored authentication, because a key that application code can write must not select whose authorities a session gets.

**The limit runs at login.** `security.session.max-sessions-per-user` (a positive integer, no default) and `HttpSecurity::session_limit`. The key `security.session.max-sessions` stays the cap of the whole store. After a successful login the handler evicts the sessions of that user that were touched longest ago, until `max - 1` others remain. The last use of a session is recorded at most once a minute, so "touched longest ago" is accurate to about a minute. The session that the login replaces is passed as `except`, because the session filter saves the new session only after the handler returns, and the old id disappears by the rotation. A failed login evicts nothing. A store error is logged with a fixed text and never fails the login. The policy is "evict the oldest"; refusing the new login is not built.

**The registry.** `SessionRegistry` wraps the index: `sessions_of`, `invalidate_all` and `invalidate_others(name, current_id)`. Boot registers it as a lazy bean when there is a `SessionFilter` and the store is `memory`, unless the application has one. This slice adds no HTTP endpoint for it.

**Revalidation runs in the chain, on an interval.** `security.session.revalidate-interval` (default 5 minutes, `0` is off, any other value at least one second) and `HttpSecurity::revalidate(interval, users)`. When a request carries a session login and the stored time is missing, older than the interval or in the future, the chain reloads the user from the `UserDetailsService`. A user that is gone, disabled or locked ends the session, and the request goes on anonymous. Changed authorities are stored and used from then on. With no change only the time advances. The `UserDetailsService` bean must know every user that form login authenticates: if form login authenticates through another provider (LDAP, a custom database provider) while a `security.users` bean exists, every session ends at the first check after the interval, so set `security.session.revalidate-interval = "0s"` or provide a service that knows those users. An error of the user service keeps the session and does not advance the time, so the next request tries again. `UserDetails` has `enabled` and `locked` and no "expired" flag, so these are the two checks. `HttpSecurity::clock` feeds the check, so tests use the manual clock, and boot passes the `Clock` bean.

**Settings and configuration.** The keys are `security.headers.{enabled, content-type-options, frame-options, referrer-policy, cache-control}` and `security.headers.hsts.{enabled, max-age, include-subdomains, preload}` (the exact string `off` removes a header), plus the two session keys above. The readers reject unknown keys and wrong types and name the key, never the value. A limit with the cookie backend, or with a store bean that has no index, fails the startup with an error that names the key. An explicit revalidation interval without a `UserDetailsService` bean fails the startup the same way. Without form login, `HttpSecurity::build` fails for a limit or a revalidation (`SessionLimitWithoutFormLogin`, `RevalidateWithoutFormLogin`).

**Which settings win.** Boot applies the headers, the limit, the revalidation and the clock before the customizers, so a `SecurityCustomizer` can override or compose them. It applies form login and CSRF after the customizers, so configuration wins there (ADR 0088 and 0089). The session keys are applied only by `SecurityAutoConfiguration`: an application that defines its own `SecurityFilterChain` bean gets none of them, as for form login and CSRF.

**The split.** Slice 7a is the headers, the index, the limit, the registry and the revalidation. Remember-me is Slice 7b. A default content security policy, `Permissions-Policy`, a refuse-new-login policy, ending the sessions of a user when the password changes, an admin endpoint for sessions and a Redis or SQL index stay out.

## Consequences

Good:

- Every response of the chain carries the headers with no code in the application, and an application can still set its own value.
- A user can be held to a number of sessions, and an application can end the others with one call.
- A user that is disabled or locked loses the session within one interval, and changed roles take effect within one interval.
- The index needs no write path: nothing can fall out of step with the store.
- The new code writes no session id, user name or header value to a log line, an error text or `Debug` output.
- The boot stack probes did not grow (238 KB and 239 KB).

Bad:

- Concurrent logins of one user can overshoot the limit. Each login lists the sessions before the other one is saved, so with racing logins up to `max + 1` sessions can exist until the next login evicts the extra; with N logins at the same moment up to N - 1 extra sessions can exist. A re-login from an authenticated session X can also race another login of the same user that evicts X as the oldest. Then the rotation of X finds nothing, no cookie is set and the redirect still goes to the success URL. The result is fail-closed and rare: the user is anonymous and logs in again.
- The scan costs time in proportion to the number of stored sessions at every login, when a limit is set, and at every registry call.
- The cookie backend has no limit, no registry and no "log out everywhere", because it keeps no state on the server. Setting the limit with it fails the startup.
- With the cookie backend a revalidation can end a session only for the cookie that was sent. A cookie that was captured after the last check stays valid until the next check, up to one interval after the session ended, and a replayed old cookie works again if the user is enabled again before the replay.
- A changed password does not end any session. Revalidation compares only the user's presence, `enabled`, `locked` and the authorities.
- Revalidation is bounded by the interval: a disabled user keeps the session for up to one interval, and a stale role is used for up to one interval.
- A user service outage keeps the sessions alive. The sessions are not ended, and the next request tries again, so there is no backoff and a failing service is called once per due request.
- Form login that is turned on by a customizer gets no default revalidation, because the default applies only when form login comes from configuration. An explicit interval works.
- A `SessionRegistry` injected on a store without the index fails the injection. The bean is lazy, but a consumer that asks for `Arc<SessionRegistry>` or `Option<Arc<SessionRegistry>>` fails at startup, and with the cookie store there is no bean at all.
- The session keys of `security.session` and the `security.headers` keys apply only through `SecurityAutoConfiguration`. They are listed in the property metadata, but an application with its own `SecurityFilterChain` bean ignores them.
- The reserved key prefix `oxbloom.security.` is not enforced by `Session::set`, so application code can write the authentication and principal keys. Revalidation reloads the user by the principal of the stored authentication to limit what a forged principal key can do, but the per-user index trusts that key.
- Session-authenticated login and logout posts get no `Cache-Control: no-store`, because the chain does not count them as authenticated by the session in that path.
- A header validation error shows up even when the headers are disabled.
- `delete_of` and `invalidate_all` do not end a session that is saved while they run.

## Alternatives considered

- A second map from user name to session ids, kept up to date on save, rotate, delete and eviction: rejected for the scan, which cannot disagree with the store.
- A separate filter for the headers: rejected, because the chain knows how the request was authenticated (for `Cache-Control`) and owns the 401, 403 and redirect responses.
- Refusing the new login when the limit is reached: rejected for evicting the oldest session, so a user is never locked out by forgotten browsers. It may come later as a policy.
- Revalidating on every request: rejected for an interval, because it would call the user service on every request.
- Ending the session when the user service fails: rejected, because an outage would log everyone out.
- Reading the user to reload from the principal key: rejected for the principal of the stored authentication, because the key is not protected from application code.
- A default content security policy: rejected for now, because a useful policy depends on the application's pages.
