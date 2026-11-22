# Session management and security headers

Oxbloom adds a set of security headers to the responses of the security chain, limits how many sessions one user can hold, lets an application end the sessions of a user and checks the stored login of a session again on an interval. The crate `oxbloom-security` holds the settings, the index of the sessions of a user and the `SessionRegistry`, and the feature `security` of `oxbloom-boot` reads the keys and wires them. The headers are on by default. The limit and the registry need the in-memory session store.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["security"] }
```

```toml
# application.toml
[security.session]
store = "memory"
max-sessions-per-user = 2
revalidate-interval = "5m"
```

An application can end the other sessions of a user, for example behind a "log out everywhere else" button, with the registry. Inject it as `Arc<SessionRegistry>`:

```rust
use std::sync::Arc;
use oxbloom_security::{RequestSecurityExt, RequestSessionExt, SessionRegistry};
use oxbloom_web::{Request, Response};

struct Account {
    registry: Arc<SessionRegistry>,
}

impl Account {
    /// Ends every other session of the logged-in user and keeps this one.
    async fn log_out_elsewhere(&self, req: Request) -> Response {
        let (Some(session), Some(auth)) = (req.session(), req.authentication()) else {
            return Response::new(Default::default());
        };
        let _ended = self
            .registry
            .invalidate_others(auth.principal(), &session.id())
            .await;
        Response::new(Default::default())
    }
}
```

The decisions are in [ADR 0090](../adr/0090-security-headers-and-session-management.md), the public items are in the [API reference](../API.md#47-oxbloom-security), and the example `examples/todos` sets a limit of two sessions and has `POST /me/logout-everywhere`. The design is a local file under `docs/superpowers`, so this page names it without a link. It builds on [Sessions](20-sessions.md#sessions), [Form login](21-form-login.md#form-login) and [CSRF protection](22-csrf.md#csrf-protection).

## Security headers

The chain adds the headers to every response it returns or passes on: the 401, the 403, redirects, the default login page, the token endpoint and the answers of your routes. A header that the response already has is never replaced, and names are compared without regard to case, so a handler or an application filter that sets its own value wins. All of them are on by default.

| Header | Default | When |
|---|---|---|
| `X-Content-Type-Options` | `nosniff` | every response |
| `Referrer-Policy` | `no-referrer` | every response |
| `X-Frame-Options` | `DENY` | every response |
| `Cache-Control` | `no-store` | the request was authenticated by the session, by `Basic` or by a bearer token, and the response has no `Cache-Control` |
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` | `Request::scheme()` is `Https` |

`Request::scheme()` is `Https` for a TLS listener, or behind a proxy that you trust with `server.forward-headers`, so a forwarded header from an untrusted client does not turn HSTS on. There is no default content security policy and no `Permissions-Policy`, because a useful policy depends on your pages. Set them with the header of your own response.

In code, `HttpSecurity::headers` takes a closure. Each call changes only what it touches, and a new `HttpSecurity` starts from the defaults, so a customizer can add one value and keep the rest that the configuration set:

```rust
use std::time::Duration;
use oxbloom_security::HttpSecurity;

let http = HttpSecurity::new()
    .authorize(|a| a.any_request().permit_all())
    .headers(|h| {
        h.frame_options(Some("SAMEORIGIN"))
            .referrer_policy(None) // no Referrer-Policy header at all
            .hsts(|s| s.max_age(Duration::from_secs(86_400)).preload(true))
    });
assert!(http.build().is_ok());
```

The keys are in the table at the end. The string `off`, in any ASCII case, removes a header, and `enabled = false` removes all of them. A header name or value must be a valid header token (visible ASCII and no control character), and a bad value fails `build` with `SecurityBuildError::InvalidHeadersConfig`, or the startup with an error that names the key. The text never holds the value. The check also runs when the headers are disabled.

## The sessions of a user

Form login stores the user name of the login under the reserved session key `oxbloom.security.principal`. The in-memory store finds the sessions of a user by scanning its sessions for that value, so there is no second index that could disagree with the store. A scan costs time in proportion to the number of stored sessions. It runs at a login only when a limit is set, and at each call of the registry.

`SessionRegistry` is the application's view of it:

- `sessions_of(name)` lists the live sessions as `SessionSummary` values (the id, the created time and the touched time, the one touched longest ago first; the last use of a session is recorded at most once a minute). The id of a session is a credential, so `Debug` of a summary prints none of it.
- `invalidate_all(name)` ends every session of the user and returns how many there were.
- `invalidate_others(name, current_id)` ends every session except the one with `current_id`, which you get from `req.session()`.

With the in-memory store and a `security.session` section, boot registers a `SessionRegistry` bean, unless your application has one. The bean is lazy. It exists only for the in-memory store: with the cookie store there is no bean, and a store bean of your own without the index makes the injection fail with `RegistryWithoutIndex`. A consumer that asks for `Arc<SessionRegistry>` or for `Option<Arc<SessionRegistry>>` then fails the startup. The registry adds no HTTP endpoint. Your route has to be a state-changing request, so the CSRF token applies to it like to any other post.

## Limiting the sessions of a user

`security.session.max-sessions-per-user` (a positive integer, no default) or `HttpSecurity::session_limit(sessions, max)` limits a user to `max` sessions. At a successful login the sessions of that user that were touched longest ago are ended until `max - 1` others remain, so the new session fits. The last use of a session is recorded at most once a minute, so "touched longest ago" is accurate to about a minute. The session that the login replaces is never counted. A failed login ends nothing. A store error is logged with a fixed text and never fails the login. A new login is never refused because of the limit.

The key `security.session.max-sessions` is a different setting: it caps the whole in-memory store. The two do not interact.

The limit needs the in-memory store and form login. With the cookie store, or a store bean that has no index, the startup fails with an error that names `security.session.max-sessions-per-user`. Without form login, `HttpSecurity::build` fails with `SecurityBuildError::SessionLimitWithoutFormLogin`, and a limit of zero fails with `InvalidSessionLimit`.

## Revalidating the user

A session keeps the authentication of the login, so a user that was disabled or locked after the login would stay logged in until the session expired. With `security.session.revalidate-interval` (default `5m`, `0` turns it off, any other value at least one second) or `HttpSecurity::revalidate(interval, users)`, the chain loads the user again from the `UserDetailsService` when the last check is missing, older than the interval or in the future. Form login stores the time of the last check under `oxbloom.security.revalidated-at`, in seconds since the epoch, at the login and at every check. The user is loaded by the principal of the stored authentication, never by the principal key, so a key that application code wrote cannot select the roles of another user.

- A user that is gone, disabled or locked ends the session, and the request goes on as an anonymous one (the usual 401 or the redirect to the login page).
- Changed authorities are stored in the session and used for this request and the next ones.
- With no change, only the time of the check advances.
- When the user service fails, the session stays and the time does not advance, so the next request tries again. A fixed warning is logged.

`UserDetails` has `enabled` and `locked`, and no flag for expired accounts, so these are the two checks. With form login configured and a `UserDetailsService` bean, the default interval applies without a key. The `UserDetailsService` bean must know every user that form login authenticates. If form login authenticates through another provider (LDAP, a custom database provider) while a `security.users` bean exists, every session ends at the first check after the interval. Set `security.session.revalidate-interval = "0s"` or provide a service that knows those users. An explicit interval without that bean fails the startup with an error that names `security.session.revalidate-interval`, and an explicit interval without form login fails `build` with `SecurityBuildError::RevalidateWithoutFormLogin`. `HttpSecurity::clock` sets the clock for the check (default the system clock), and boot passes the `Clock` bean, so tests use `ManualClock` and move time without sleeping.

## Settings in the configuration

Boot applies the headers, the limit, the revalidation and the clock before the customizers, so a `SecurityCustomizer` can override or compose them. It applies form login and CSRF after the customizers, so there the configuration wins. The keys apply only through `SecurityAutoConfiguration`: an application that defines its own `SecurityFilterChain` bean gets none of them, as for form login and CSRF, even though the keys are listed in the property metadata.

| Key | Default | Meaning |
|---|---|---|
| `security.headers.enabled` | `true` | `false` adds no header at all |
| `security.headers.content-type-options` | `nosniff` | `off` removes it |
| `security.headers.frame-options` | `DENY` | `off` removes it |
| `security.headers.referrer-policy` | `no-referrer` | `off` removes it |
| `security.headers.cache-control` | `no-store` | for authenticated requests; `off` removes it |
| `security.headers.hsts.enabled` | `true` | only on `https` requests |
| `security.headers.hsts.max-age` | `365d` | whole seconds, at least one second |
| `security.headers.hsts.include-subdomains` | `true` | adds `includeSubDomains` |
| `security.headers.hsts.preload` | `false` | adds `preload` |
| `security.session.max-sessions-per-user` | none | at least 1; needs the in-memory store |
| `security.session.revalidate-interval` | `5m` | `0` is off; otherwise at least `1s` |

## Limits

- **Racing logins.** The limit is checked at a login, before the session of a concurrent login is saved. Two logins of one user at the same moment can leave `max + 1` sessions until the next login ends the extra one. With N logins at the same moment, up to N - 1 extra sessions can exist. A re-login from an authenticated session X can also race another login of the same user that evicts X as the oldest. Then the rotation of X finds nothing, no cookie is set and the redirect still goes to the success URL. The result is fail-closed and rare: the user is anonymous and logs in again.
- **The cookie backend.** It keeps no state on the server, so it has no limit, no registry and no "log out everywhere". Setting the limit with it fails the startup. Revalidation works, but the cookie is the session: a copy of the cookie that was taken after the last check stays valid for up to one interval after the session ended, and a replayed old cookie works again if the user was enabled again before the replay.
- **A changed password.** It does not end any session. Revalidation compares only whether the user exists, `enabled`, `locked` and the authorities. Call `invalidate_all` yourself when a password changes. With remember-me (Slice 7b) a changed password does end every remember-me cookie of that user at once, because the cookie is bound to the stored password hash, but a session that exists stays until its next revalidation, up to one interval (see [Remember-me](24-remember-me.md#limits)).
- **Remember-me.** Log out everywhere, `invalidate_all`, `invalidate_others` and the limit end sessions and do not revoke the remember-me cookie, so an ended or evicted session logs straight back in from the cookie (see [Remember-me](24-remember-me.md#limits)). An auto login counts against the limit like a password login.
- **The interval.** Revalidation is bounded by the interval: a user that is disabled keeps the session for up to one interval, and a changed role takes effect after up to one interval. A shorter interval calls the user service more often.
- **A user service outage.** The session stays while the service fails, so an outage does not log everybody out, and it also does not end the session of a user who was disabled in the meantime. There is no backoff: each due request calls the service and logs one warning.
- **A customizer and the default.** The default interval applies only when form login comes from configuration. Form login that a customizer turns on gets no default revalidation, but an explicit interval works.
- **The reserved keys.** The prefix `oxbloom.security.` is reserved. Since Slice 7b `Session::set` returns `SessionError::ReservedKey` for such a key and `Session::remove` ignores it, so application code can no longer write the authentication and principal keys. The user to reload still comes from the stored authentication, and the per-user index reads the principal key that form login wrote.
- **The registry.** It exists only with the in-memory store (see above). A consumer on a store without the index fails the injection.
- **The answers of login and logout.** The `POST` answers of the login and logout paths get no `Cache-Control: no-store` when the session already authenticates them.
- **Scope of the keys.** The session and header keys are applied only by `SecurityAutoConfiguration`, and not with your own `SecurityFilterChain` bean.
- **Concurrent ends.** `invalidate_all` and `invalidate_others` do not end a session that is saved while they run.
- **Out of scope.** A default content security policy, `Permissions-Policy`, a policy that refuses the new login, ending sessions on a password change, an admin endpoint for sessions and Redis or SQL indexes are not built.
