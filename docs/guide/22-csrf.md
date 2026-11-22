# CSRF protection

Oxbloom protects a browser user who is logged in with form login against cross-site request forgery. The crate `oxbloom-security` keeps a random token in the session, and the security chain refuses every unsafe request that a logged-in session makes unless it sends that token back. The feature `security` of `oxbloom-boot` turns it on together with form login and reads the `security.csrf` keys. This is security Slice 6d. It builds on [Form login](21-form-login.md#form-login) and [Sessions](20-sessions.md#sessions), and [Limits](#limits) lists what it does not stop.

An application that only turns on form login is already protected. A form that posts to a route of the application has to carry the token:

```rust
use oxbloom_security::RequestSessionExt;
use oxbloom_web::{Request, Response};

/// Escapes a value for HTML text and for a double-quoted attribute.
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn page(req: Request) -> Response {
    // `None` when no session filter ran.
    let hidden = req.csrf_token().map_or_else(String::new, |token| {
        format!(
            "<input type=\"hidden\" name=\"_csrf\" value=\"{}\">",
            escape_html(token.as_str())
        )
    });
    Response::new(format!("<form method=\"post\" action=\"/save\">{hidden}...</form>").into())
}
```

The decisions are in [ADR 0089](../adr/0089-csrf-tokens.md), the public items are in the [API reference](../API.md#47-oxbloom-security), and the example `examples/todos` posts the token from its `/me` page. The design is a local file under `docs/superpowers`, so this page names it without a link.

## What happens

1. The browser asks for the login page. Oxbloom's default page creates the session and the token, and puts the token in a hidden field named `_csrf`.
2. The browser posts the login form with that field. The chain checks the origin and then the token, and only then asks the password provider. On success the session id is rotated and the token is replaced, so the token of the login page is dead afterwards.
3. A page of the logged-in user gets the new token with `req.csrf_token()` and writes it into its forms, or a script gets it from a meta tag or from the [token endpoint](#the-token-endpoint).
4. Every unsafe request that a logged-in session makes must send the token in the header `X-CSRF-TOKEN` or in the form field `_csrf`. Without a matching token the answer is 403.
5. The logout post needs the token too. Logout invalidates the session, which drops the token.

## Which requests are checked

A request is checked when its method is unsafe and the session authenticates it. The checked set is every method except `GET`, `HEAD`, `OPTIONS` and `TRACE`, so `CONNECT` and an unknown method token are checked as well. The login and logout posts are checked whether a session is logged in or not.

These requests are not checked:

- The safe methods `GET`, `HEAD`, `OPTIONS` and `TRACE`. They must not change state, so do not change state in a `GET` handler.
- A request authenticated by `Basic` or a bearer token. Credentials in a header win, so a stray session cookie on such a request does not matter. A bearer token is not sent by a browser on its own, so there is nothing to forge. `Basic` is different: a browser that cached Basic credentials sends them automatically on a forged cross-site request, and such a request is not checked. Keep the exemption in mind and use `Basic` for non-browser clients only.
- An anonymous request that has no session. A public `POST` route keeps working.
- An anonymous request that carries a pre-login session but is not logged in (for example a browser that has only fetched the login page). It has no ambient credential. If such a route does something that a logged-in session could also do, the check does not apply to the anonymous call.

A failed check answers a fixed 403 with an empty body, whatever the reason: no session, no token in the session, no submitted token, a wrong or a stale token. Inside an application the error filter renders it as `application/problem+json`. A custom `access_denied` handler is not used for it, the same as for the same-origin 403 of form login, so it cannot be restyled. A failed check changes nothing in the session.

## Sending the token

- **Header.** `X-CSRF-TOKEN: <token>` (the name is `header-name`). The header wins when both are present. Use it for `fetch` and for JSON requests.
- **Form field.** The field `_csrf` (the name is `field-name`) of an `application/x-www-form-urlencoded` body of at most 4 KiB. The first value wins when the field is repeated.
- **Not accepted.** A token in the query string is never read. A multipart form cannot carry the field, because the body is not parsed: send the header. A JSON body without the header fails the check.

The chain marks the token header sensitive, so the `Debug` output of a request shows `Sensitive` for it. The form field is part of the body, and the body is visible in the `Debug` output of the request, the same as the password of the login form. Do not print a whole request.

Do not allowlist the token header for [request logging](07-logging.md): the built-in deny list redacts `x-csrf-token` and `x-xsrf-token` by name, but a custom `security.csrf.header-name` is not on it and would be logged in full. The token is 43 characters of base64url and is compared in constant time. It never reaches a log line, an error text, a redirect or a URL. `CsrfToken` has no `Display` and prints as `CsrfToken(<redacted>)`; `as_str` gives the value for a template, and you must not log it.

## Configuration keys

| Key | Default | Meaning |
|---|---|---|
| `security.csrf.enabled` | `true` with form login | Turns the check off with `false` |
| `security.csrf.field-name` | `_csrf` | The name of the hidden form field |
| `security.csrf.header-name` | `X-CSRF-TOKEN` | The name of the request header |
| `security.csrf.token-endpoint` | none | A path whose `GET` answers the token as JSON |

```toml
# application.toml (every key is optional)
[security.csrf]
field-name = "_csrf"
header-name = "X-CSRF-TOKEN"
token-endpoint = "/csrf"
```

A name must be non-empty and use letters, digits, `_` and `-`. The endpoint must be a local path (the rule of [The redirect rule](21-form-login.md#the-redirect-rule)) and must differ from every form login path. Unknown keys are rejected, and a bad value fails the startup with the key named and never echoes the value. Boot calls `csrf` after every `SecurityCustomizer`, as it does for form login, so the configuration wins over a customizer that calls `.csrf(..)`.

CSRF protection belongs to form login. With `security.form-login.enabled` off, nothing is applied: `security.csrf.enabled = false` alone is accepted, and `enabled = true` or any other `security.csrf` key fails the startup and names the key. Do not leave a `security.csrf` table in a configuration that has no form login.

In code, `HttpSecurity::csrf` takes a closure over `CsrfConfig`:

```rust
use oxbloom_security::HttpSecurity;

let http = HttpSecurity::new()
    .csrf(|c| c.field_name("token").header_name("X-Token").token_endpoint("/csrf"));
```

`build` fails with `SecurityBuildError::CsrfWithoutFormLogin` when `csrf` was called without `form_login`, also with `enabled(false)`, and with `SecurityBuildError::InvalidCsrfConfig` when a name or the endpoint is not valid. A page that writes the field by hand must use the configured field name, because the default `_csrf` is rejected when another name is configured.

## The default login page

With CSRF on, the default login page creates the session and the token and puts the token in the hidden field. The page always adds `frame-ancestors 'none'` to its content security policy and the header `X-Frame-Options: DENY`, so another site cannot frame it, also with CSRF off; only the hidden field depends on CSRF. The other headers and texts are as in [The default login page](21-form-login.md#the-default-login-page). The page gets its token only when Oxbloom renders it: a page that your application serves at the login path creates no session, so it has to call `req.csrf_token()` itself, and it sees an anonymous user. Without a session filter the page has no field and the error "form login has no session: add a SessionFilter to the application" is logged.

## The token endpoint

With `security.csrf.token-endpoint = "/csrf"` a `GET /csrf` answers 200 with `Content-Type: application/json`, `Cache-Control: no-store` and this body:

```json
{"headerName":"X-CSRF-TOKEN","parameterName":"_csrf","token":"..."}
```

A script, for example a single-page application, reads it and sends the token in the header. The endpoint creates the session and the token when they are missing. The chain answers it before the application's routes, so when your application maps the same path, the endpoint wins. A cross-origin page cannot read the answer unless your CORS configuration allows that origin. Without a session filter the endpoint logs the same fixed error as the page and the request goes on to the application. The endpoint is off by default.

## What the session holds

The token is stored under the session key `oxbloom.security.csrf-token`. The prefix `oxbloom.security.` is reserved for Oxbloom, and `Session::set` returns `SessionError::ReservedKey` for such a key since Slice 7b. A stored value that is not a valid token is removed and read as absent. The token is created only when the login page, the endpoint or `csrf_token()` asks for it, never by the check. A token created in a session that is new marks it for writing, so the cookie is set. `csrf_token()` returns `None` when no session filter ran and never creates a session.

## Limits

- **Cross-site scripting.** A script on the same origin can read the token from the page or from the endpoint. CSRF tokens do not protect against XSS; escape every value you write into a page.
- **Multipart forms and large bodies.** The reader parses only urlencoded bodies of at most 4 KiB (the cap of the form login parser). A multipart form, a larger urlencoded form, and a body with one pair that does not decode always get 403, so such forms must send the header, for example from a script.
- **Planted cookies.** The session cookie `OXBLOOM_SESSION` has no `__Host-` prefix, so an attacker who controls a sibling subdomain can log in with their own account, take their own session id and token, and plant that cookie in the victim's browser; the victim's requests then run in the attacker's session, whose token the attacker knows. This holds with either session backend, and a CSRF token cannot prevent it. See [Sessions](20-sessions.md#sessions).
- **Anonymous session writers.** The login page and the token endpoint write a session for an anonymous browser, and so does any page of your application that calls `req.csrf_token()` for an anonymous visitor, next to the saved request of form login. An anonymous flood can fill the cap of the in-memory store and evict real sessions; sampled eviction bounds the cost, not the effect.
- **Framing.** Slice 7a adds `X-Frame-Options: DENY` to every response of the chain (see [Session management and security headers](23-session-management.md#security-headers)); `Content-Security-Policy: frame-ancestors` is still set only on the default login page. Set `frame-ancestors` yourself if you need to allow or refuse specific origins.
- **The origin check.** The same-origin check of form login still covers only `POST` of the login and logout URLs, and it allows a request with neither `Origin` nor `Referer`. The token is what protects your own routes.
- **Anonymous posts.** An anonymous request with no session, and one with a pre-login session that is not logged in, are not checked.
- **Expired sessions.** A login page that stays open past the idle timeout, or a logout after the session expired, gets a 403 (`application/problem+json`) instead of a redirect, because the stored token is gone. The user has to load the login page again.
- **Remember-me.** An auto login from the remember-me cookie creates a session with a new token, so an unsafe request that auto logs in is a 403, and the next `GET` works (see [Remember-me](24-remember-me.md#limits)).
- **Basic in a browser.** A browser that cached `Basic` credentials sends them on forged requests, which are not checked. Use `Basic` for non-browser clients only.
- **A dead token on a page.** An application that rotates or invalidates the session inside its handler while it answers a `404` for the login path gets the default page with a token that is no longer valid. This is a corner case.
- **No custom 403.** The 403 of a failed check does not use your `access_denied` handler.
- **Out of scope.** A signed double-submit cookie, a script-readable token cookie and one token per request are not built.
