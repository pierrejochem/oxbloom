# Remember-me

Oxbloom can keep a browser user logged in across sessions. A user who logs in with the form and ticks "Remember me" gets a second cookie. When the session is gone, the next request that carries only that cookie is logged in again. The crate `oxbloom-security` holds the settings, the token and the auto login in the security chain, and the feature `security` of `oxbloom-boot` turns it on from the `security.remember-me` keys. This is security Slice 7b. It builds on [Form login](21-form-login.md#form-login), [Sessions](20-sessions.md#sessions) and [Session management and security headers](23-session-management.md#session-management-and-security-headers).

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["security"] }
```

```toml
# application.toml
[security.remember-me]
enabled = true
secret = "a-random-secret-of-at-least-32-bytes"
lifetime = "14d"
```

The decisions are in [ADR 0091](../adr/0091-remember-me.md), the public items are in the [API reference](../API.md#47-oxbloom-security), and the example `examples/todos` enables it with a development secret and has `tests/remember_me.rs` (see [Example](14-example.md#example)). The design is a local file under `docs/superpowers`, so this page names it without a link.

## What happens

1. The browser posts the login form with the box ticked. The field is `remember-me` and a value of `on`, `true`, `yes` or `1` counts, compared without regard to ASCII case. After the password is accepted, the answer sets the session cookie and the cookie `OXBLOOM_REMEMBER_ME`.
2. Later the session is gone: the browser was closed, the session timed out or the store was restarted. The browser sends a request with the remember-me cookie and no valid session cookie.
3. The chain finds no login in the request or in the session, reads the cookie and checks it. When the cookie is valid, the chain creates a new session, stores the login in it, and the request goes on as an authenticated one. The answer sets the new session cookie.
4. Logout clears the remember-me cookie, together with the session.

A failed login never sets or clears the cookie. A successful login without the box clears a remember-me cookie that the request carries, so a shared browser does not log the previous user in again when the new session idles out; without a cookie in the request it sets nothing. A login with the box ticked issues a new cookie with a new expiry.

The default login page has the checkbox, and only when remember-me is on. If your application serves its own login page, add `<input type="checkbox" name="remember-me" value="on">` (or the name that you set in `field-name`) to the form that posts to the login processing URL.

## Turning it on

Remember-me needs three things: form login, a secret and a `UserDetailsService` bean, which `security.users.*` gives you through the password auto-configuration. Without form login the startup fails with an error that names `security.form-login.enabled`, without a secret with one that names `security.remember-me.secret`, and without the user service with one that names `security.remember-me.enabled`. The errors never hold a value.

In code, `HttpSecurity::remember_me` takes a closure over the `RememberMe` settings. A second call sees what the first one set, so a customizer can change one setting and keep the rest:

```rust
use std::sync::Arc;
use std::time::Duration;
use oxbloom_config::Secret;
use oxbloom_security::{
    Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity, InMemoryUserDetailsService,
};

let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
let chain = HttpSecurity::new()
    .password_provider(Arc::new(DaoAuthenticationProvider::new(users.clone(), encoder)))
    .form_login(|f| f)
    .remember_me(|r| {
        r.secret(Secret::new("a-remember-me-secret-of-32-bytes".to_owned()))
            .user_details(users)
            .lifetime(Duration::from_secs(7 * 24 * 3600))
    })
    .authorize(|a| a.path("/account/**").fully_authenticated().any_request().authenticated())
    .build();
assert!(chain.is_ok());
# Ok::<(), Box<dyn std::error::Error>>(())
```

`build` fails with `SecurityBuildError::RememberMeWithoutFormLogin` without `form_login`, and with `SecurityBuildError::InvalidRememberMe` for a missing or short secret, a missing user service, a lifetime outside the range, a cookie name that is not an HTTP token or a field name that is not made of ASCII letters, digits, `_` and `-`. The text names no value.

## Configuration keys

| Key | Default | Meaning |
|---|---|---|
| `security.remember-me.enabled` | `false` | Turns remember-me on |
| `security.remember-me.secret` | none | The key that signs new cookies; at least 32 bytes; required when enabled |
| `security.remember-me.old-secrets` | `[]` | Earlier secrets whose cookies stay valid; each at least 32 bytes |
| `security.remember-me.lifetime` | `14d` | How long a cookie is valid from its issue; from `1m` to `365d` |
| `security.remember-me.field-name` | `remember-me` | The name of the login form field of the checkbox |
| `security.remember-me.cookie-name` | `OXBLOOM_REMEMBER_ME` | The cookie name; an HTTP token that differs from the session cookie name |

Unknown keys under `security.remember-me` are rejected and named. The values are checked even while `enabled` is `false`, so a bad lifetime or a short secret in a switched-off section still fails the startup. The cookie is `HttpOnly` and `SameSite=Lax`, and its `Path` and `Secure` come from the session settings: the session filter puts a `CookieScope` extension on the request with the path and the `Secure` decision of its own cookie, and the remember-me cookie reads it, so the two cookies cannot disagree. Without a session filter the defaults (`Path=/`, `Secure` on HTTPS) apply, but auto login needs a session filter and logs one fixed error without it. A cookie name equal to the session cookie name fails the startup with an error that names `security.remember-me.cookie-name`. That check runs only on the boot configuration path: a hand-built `HttpSecurity` or a `SessionFilter` that you register with its own cookie name is not checked, and on a clash a session cookie that does not parse as a token would get a clearing cookie with the same name, which destroys anonymous sessions. Keep the two names different.

Boot applies remember-me before the customizers, so a `SecurityCustomizer` can change it with `http.remember_me(|r| ..)`. Boot gives remember-me and form login the same clock bean, and a customizer that calls `http.clock(..)` does not change the clock of remember-me. The keys apply only through `SecurityAutoConfiguration`: an application that defines its own `SecurityFilterChain` bean gets none of them, as for form login and CSRF.

## The secret

The secret is a `Secret` value. No log line, error text, `Debug` output or property metadata shows it, and it is never read from the command line: a `--security.remember-me.secret=...` argument fails the startup, and so does one for `old-secrets`. Put it in a file or in an environment variable that your platform keeps private. Use a random value of at least 32 bytes, and not the session secret, although a key derived from it is different anyway (HKDF with its own info string).

To rotate the secret, put the new one in `secret` and the earlier one in `old-secrets`. New cookies are signed with the new secret, and cookies that were signed with an old one still verify. Remove the old secret when the longest lifetime has passed since the rotation. To end every cookie at once, replace the secret and leave `old-secrets` empty.

## The cookie

The value is `v1.<base64url(username)>.<expiry in seconds>.<base64url(HMAC-SHA256)>`. The MAC covers the user name, the expiry and the stored password hash of the user. Three things follow.

- **The user name is in the clear.** It is only base64url encoded, not encrypted, so anyone who can read the cookie can read the name. The cookie is `HttpOnly`, so scripts cannot read it, but over plain HTTP it is visible on the wire. Use TLS.
- **A changed password ends the cookie at once.** The stored hash is part of the MAC, so after a password change the cookie no longer verifies, and nothing has to be revoked. The same is true for a user that no longer exists, is disabled or is locked: auto login loads the user and refuses all three.
- **The lifetime is fixed.** The `Max-Age` is the lifetime from the moment of issue, and the cookie is never refreshed. After the lifetime the user logs in again.

The token username comes from the `UserDetails` of the user that the user service returned, not from the principal that the login produced. A token over 1 KiB is not issued, and a cookie header over 8 KiB counts as no cookie.

## Auto login

The chain tries the cookie only when all of these hold:

- The request has no `Authorization` header at all, not even one with a scheme that Oxbloom does not know. A header credential wins, and a failed Basic or bearer check never falls back to the cookie. A browser or a proxy that adds an `Authorization` header to every request therefore never gets auto login.
- The session gave no login. A valid session wins, and the cookie is not even read.
- The cookie parses, its expiry is in the future, the user exists in the `UserDetailsService`, is enabled and not locked, and the MAC verifies.

On success the chain does what a password login does: it creates a session or rotates the id of the one that exists, stores the login, writes the reserved keys `oxbloom.security.principal` and `oxbloom.security.revalidated-at`, replaces the CSRF token and applies the session limit of Slice 7a. It does not issue the cookie again, so the expiry stays the one of the first login.

An invalid cookie of any kind (a changed signature, a malformed value, an expired one, an unknown, disabled or locked user, a changed password) gets the same answer: the request goes on as an anonymous one, and the response carries a `Set-Cookie` that clears the cookie. The client cannot tell the reasons apart, but the cost differs: an expired or malformed cookie costs no user service lookup, and an unexpired forged cookie costs one lookup per request. Rate-limit at the edge if lookups are expensive; a forged cookie is still cheaper for an attacker than a password attempt, which costs a lookup plus Argon2. When the `UserDetailsService` fails, the cookie is kept, the request goes on anonymous and one fixed warning is logged, so a short outage does not log the cookie out.

## Remembered and fully authenticated

The stored login of an auto login carries the marker `remembered`. `Authentication::is_remembered()` reads it. A login with the password stores a login without it, also inside a session that was started by a cookie.

`authenticated()` accepts a remembered user. `fully_authenticated()` (`AuthorizationRule::FullyAuthenticated`) accepts only an authenticated request that is not remembered, so the application can ask for the password again before something sensitive:

```rust
use oxbloom_security::HttpSecurity;

let http = HttpSecurity::new().authorize(|a| {
    a.path("/account/password").fully_authenticated()
        .path("/account/**").authenticated()
        .any_request().permit_all()
});
```

A remembered user who meets a `fully_authenticated()` rule is an authenticated user that a rule refuses, so the answer is the 403 of the access denied handler, as for any rule that refuses a logged-in user. Your application can send such a user to the login page, where a login with the password replaces the remembered login. Basic and bearer count as fully authenticated.

A JWT whose claims contain `oxbloom.remembered: true` is read as remembered, because the marker is read from the details of the authentication. That can only make `fully_authenticated()` refuse the token, so it is deny-only. If your tokens carry that claim for another reason, rename it. Oxbloom does not change the converter.

## Limits

- **No per-device revocation and no theft detection.** The cookie is stateless: there is no list of issued cookies. A user cannot see or end the cookie of one device, and a thief who copies the cookie is not noticed. There is no admin endpoint to revoke cookies either.
- **A stolen cookie works until it expires or the password changes.** The lifetime is fixed and short by default (14 days). Lower it for sensitive applications, ask for `fully_authenticated()` on sensitive paths, and change the secret to end every cookie. A lower `lifetime` also applies to cookies already issued: a cookie whose expiry is further away than the configured lifetime from now is refused and cleared.
- **The lifetime is fixed.** It is not refreshed on use. Logging in again with the box ticked issues a new cookie.
- **An auto login on an unsafe request gets 403.** The new session has a new CSRF token that the request cannot carry, so a `POST`, `PUT`, `PATCH` or `DELETE` that auto logs in is refused. The answer still sets the new session cookie, so the next `GET`, and any request after it, works. For the same reason a logout `POST` that arrives after the session expired, with CSRF on, is a 403 and keeps the cookie, until a `GET` has logged the browser in again.
- **The cookie holds the user name in the clear.** See above.
- **The user service must know every form-login user.** Remember-me loads the user again through the `UserDetailsService` bean, at issue and at auto login. When form login authenticates through another provider, the user is not found: no cookie is issued, and an existing one is cleared.
- **Any `Authorization` header blocks auto login.**
- **Log out everywhere and the session limit do not revoke remember-me.** `SessionRegistry::invalidate_all`, `invalidate_others` and the eviction of the limit end sessions only. A session that was ended or evicted logs straight back in from the cookie as soon as the browser sends it, so the user is not logged out by it. A changed password does end the cookie, but not the sessions that exist (see below).
- **A burst of parallel requests after the session expired.** Each request that carries only the cookie can log in on its own, each creates a session, and each counts against the limit of Slice 7a, until the limit evicts the extra ones.
- **A password change and sessions.** A changed password ends every remember-me cookie of that user at once. Existing sessions are not ended, because revalidation does not check the password; call `SessionRegistry::invalidate_all` (7a) when a password changes. Revalidation only checks that the user exists, is enabled and not locked, and the authorities.
- **A custom login page.** It must post the field itself. Only the default page has the checkbox.
- **Scope of the keys.** They are applied only by `SecurityAutoConfiguration`, and not with your own `SecurityFilterChain` bean.
- **JWT claim.** See above.

## Changes of the API

Two additions break code that matches the enums exhaustively, and one rule tightens a convention:

- `AuthorizationRule::FullyAuthenticated` is a new variant. A `match` over `AuthorizationRule` without a wildcard arm no longer compiles.
- `SessionError::ReservedKey` is a new variant, with the same effect on an exhaustive `match`.
- The prefix `oxbloom.security.` of the session keys is now enforced: `Session::set` returns `SessionError::ReservedKey` for such a key and writes nothing, and `Session::remove` ignores it. Reading with `get` is still allowed. Application code that wrote these keys, for example to fake a login in a test, must log in through the chain, or use a test of its own. The methods `Session::set_reserved` and `Session::remove_reserved` are `#[doc(hidden)]` and meant for Oxbloom's own code and tests.
- `TestResponse::header_all(name)` in `oxbloom-test` returns every value of a header, for example two `Set-Cookie` lines. `Authentication::is_remembered()` and `RuleBuilder::fully_authenticated()` are new methods.

## Testing

`TestClient` has no cookie jar: a test keeps the cookies by hand, as `examples/todos/tests/remember_me.rs` does. It logs in with `remember-me=on` in the form, picks the `OXBLOOM_REMEMBER_ME` pair out of the `Set-Cookie` lines with `header_all`, drops the session cookie and shows that a request with only the remember-me cookie is logged in again. A changed signature is refused and cleared, an `Authorization` header of any scheme blocks the auto login, and logout clears the cookie. The example's users come from the configuration, so the test cannot change a password. The crate tests in `crates/oxbloom-security/tests/remember_me_autologin.rs` cover a changed password, a disabled user and a locked user, with a user service that they change between the requests.
