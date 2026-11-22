# Form login

Oxbloom can log a browser user in with an HTML form. The crate `oxbloom-security` holds the `FormLogin` settings and the handlers that the security chain runs, and the feature `security` of `oxbloom-boot` turns it on from the `security.form-login` keys. This is security Slice 6c. It builds on [Password login](19-password-login.md#password-login) for the user check and on [Sessions](20-sessions.md#sessions) for the state between requests. There are no CSRF tokens yet (Slice 6d), so read [Limits](#limits) before you use it.

```toml
# application.toml
[security.session]
store = "memory"

[security.form-login]
enabled = true

[security.users.alice]
password = "{argon2}$argon2id$..."
roles = ["USER"]
```

The decisions are in [ADR 0088](../adr/0088-form-login.md), the public items are in the [API reference](../API.md#47-oxbloom-security), and the example `examples/todos` has a protected page `GET /me` behind the login (see [Example](14-example.md#example)). The design is a local file under `docs/superpowers`, so this page names it without a link.

## What happens

1. An anonymous browser asks for a protected page, for example `GET /orders?page=2` with `Accept: text/html`. The chain answers 302 to `/login` and saves `/orders?page=2` in the session.
2. The browser asks for `/login`. Oxbloom serves its default login page, unless the application maps `GET /login` itself.
3. The browser posts the form to `/login`. On success the session id is rotated, the authentication is stored in the session, and the answer is 302 to the saved page, or to `/` when nothing was saved. On failure the answer is 302 to `/login?error`.
4. Later requests carry the session cookie. The chain restores the authentication from the session, so `req.authentication()` works as it does for Basic or a token.
5. The browser posts to `/logout`. The session is invalidated and the answer is 302 to `/login?logout`.

An API client does not get this. A request that does not prefer HTML, or that is not a `GET` or `HEAD`, keeps the 401 and its challenge, and no session is created for it. Basic and bearer requests never create a session either, and when a request has both a session and a `Basic` header, Basic wins.

## Turning it on

Form login needs two things: the table `security.session` (the authentication is kept in the session) and at least one password provider bean, which `security.users.*` gives you through the password auto-configuration. Without them the startup fails, and the message names the key.

In code, `HttpSecurity::form_login` takes a closure over the `FormLogin` settings:

```rust
use oxbloom_security::HttpSecurity;

let http = HttpSecurity::new()
    .form_login(|f| f.default_success_url("/home").login_page_enabled(false));
```

`build` fails with `SecurityBuildError::FormLoginWithoutProvider` when there is no password provider, and with `SecurityBuildError::InvalidFormLoginUrl` when a URL or a field name is not valid.

## Configuration keys

| Key | Default | Meaning |
|---|---|---|
| `security.form-login.enabled` | `false` | Turns form login on |
| `security.form-login.login-page` | `/login` | Where the login page is and where browsers are sent |
| `security.form-login.login-processing-url` | `/login` | Where the form is posted |
| `security.form-login.logout-url` | `/logout` | Where the logout form is posted |
| `security.form-login.default-success-url` | `/` | Where a login goes when nothing was saved |
| `security.form-login.failure-url` | `/login?error` | Where a failed login goes |
| `security.form-login.logout-success-url` | `/login?logout` | Where a logout goes |
| `security.form-login.username-field` | `username` | The name of the username field |
| `security.form-login.password-field` | `password` | The name of the password field |
| `security.form-login.login-page-enabled` | `true` | Whether Oxbloom serves the default page |

Unknown keys under `security.form-login` are rejected and named. A URL must be a local path (see [The redirect rule](#the-redirect-rule)); a field name must be non-empty and use letters, digits, `_` and `-`. The two field names must differ, and the logout path must differ from the login page and from the processing path. A bad value fails the startup with the key named and never echoes the value.

## The default login page

The chain serves a small HTML form at the login page when the router answered 404 for a `GET` or `HEAD` of that path and `login-page-enabled` is true. Three consequences follow.

- If the application maps `GET /login` itself, its handler answers and the default page is not used. That handler always sees an anonymous user.
- Any 404 at that path is replaced by the page, also a 404 that an application handler chose deliberately.
- Through `App::handle` a `HEAD` answer of the page keeps its body.

The page has fixed texts for `?error` ("Invalid username or password.") and `?logout` ("You have been logged out."), and no other query value is shown. Its headers are `Cache-Control: no-store`, `X-Content-Type-Options: nosniff` and `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'`. The CSP is fixed and there is no inline script. The page has no `frame-ancestors` and no `X-Frame-Options`, so another site can frame it. That is a clickjacking limit, to be addressed with Slice 6d. To use your own page, set `login-page-enabled = false` or map the path, and post a form with the two field names to the processing URL.

## The authorization rules

Only three requests skip the authorization rules: `POST` of the login processing URL, `POST` of the logout URL, and `GET` or `HEAD` of the login page. Every other path follows your rules, including the success and failure URLs. The defaults work. A custom `failure-url` or `logout-success-url` on a path other than the login page needs a `permit_all` rule, otherwise the user is redirected to the login page again.

## The redirect rule

The saved request and every configured URL follow one rule, which `is_safe_redirect_target` implements. A target must start with a single `/`, must not start with `//` or `/\`, must not contain a control character or a backslash, must not contain `://` before the first `?`, and is at most 2048 bytes long. The saved request is checked when it is saved and again when it is used, so a tampered session cannot cause an open redirect. A configured URL that fails the rule fails the startup. A later anonymous request replaces the saved one only when its path differs, and the paths of form login itself are never saved.

## What the session holds

The authentication is stored as JSON under the session key `oxbloom.security.authentication`, and the saved request under `oxbloom.security.saved-request`. The prefix `oxbloom.security.` is reserved for Oxbloom: application code must not set a key with it. A stored value that cannot be read is removed and the request is anonymous, never a 500. `Authentication` implements `Serialize` and `Deserialize` for this; deserializing a value does not go through `Authentication::new`.

The stored authentication is not refreshed. A role change or a locked account takes effect at the next login or when the session expires, not on the next request.

## Login and logout details

- The login form is `application/x-www-form-urlencoded` and at most 4 KiB. Another content type answers 400. A larger body, a missing field and invalid UTF-8 are a failed login.
- Login always rotates the session id before it stores the authentication, so a planted session id never carries a login. A logged-in user who posts again gets a new id and a replaced authentication.
- Every failed login gives the same 302 to the failure URL: an unknown user, a wrong password, a locked or disabled account, and an outage of the password provider (`ProviderUnavailable`) alike. The session is not changed. The username is never put in the redirect or in a log line; the `debug` log has the error variant and the length of the username.
- Logout invalidates the session and answers 302 to the logout success URL. `GET /logout` never changes state.
- `rotate_id` and `invalidate` follow the rules of the session filter: see [Sessions](20-sessions.md#sessions).

## Limits

- There are no CSRF tokens yet. The same-origin check on the two posts and the `SameSite=Lax` cookie are the interim measure; Slice 6d adds tokens. The check compares the origin of `Origin` (or of `Referer` without it) with the request's own scheme, host and port, honouring the trusted forwarded headers, and answers 403 for a mismatch or `Origin: null`. A request with neither `Origin` nor `Referer` is allowed, because non-browser clients send neither.
- The stored authentication is not refreshed (see above).
- The default login page can be framed and is served for any 404 at the login path.
- The in-memory store evicts, at its cap, the oldest-touched entry of a sample of at most 16 entries, which starts at a random shard and at a random position within the first 64 entries of it. The cost per write is constant. An anonymous flood of `Accept: text/html` requests to protected paths can still fill the cap, because each one saves a request, and evict real sessions. Racing saves can exceed the cap, and the overshoot persists until expiry or a sweep.
- A session lives in one process with the in-memory store, so a restart logs everybody out and a second instance does not know the sessions.
- Remember-me, concurrent-session limits, OAuth2 login, registration, password reset and per-user rate limiting are not built.
