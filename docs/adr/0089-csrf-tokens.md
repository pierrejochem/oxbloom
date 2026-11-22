# ADR 0089: CSRF protection is a synchronizer token in the session, checked in the security chain for every unsafe request that the session authenticates

- **Status:** Accepted
- **Date:** 2026-11-22
- **Source:** the security Slice 6d design spec `2026-11-22-csrf-design.md` (a local file, not linked) and its plan

## Context

After a form login the session cookie authenticates every route of the application. Slice 6c protected only `POST /login` and `POST /logout`, with a same-origin check that lets a request through when it carries neither `Origin` nor `Referer`. The application's own `POST`, `PUT`, `PATCH` and `DELETE` routes had only the `SameSite=Lax` cookie, which does not stop a same-site attacker such as a sibling subdomain. The default login page could also be framed. How does Oxbloom protect a logged-in browser user against cross-site request forgery? Which requests are checked, where does the check run, how does a page or a script get the token, and what happens to the token at login and logout?

Sessions are [ADR 0087](0087-sessions.md) and form login is [ADR 0088](0088-form-login.md).

## Decision

**A synchronizer token kept in the session.** A token is 32 random bytes from the `aws-lc-rs` provider, encoded as base64url without padding (43 characters), stored under the reserved session key `oxbloom.security.csrf-token`. Both session backends work, because the server never needs its own state: the token lives where the session lives. A stored value that is not a valid token is removed and read as absent. `CsrfToken` has no `Display`, a redacting `Debug` and the accessor `as_str` for templates. The comparison with the submitted value is constant time, also in the length difference, and an empty value fails.

**The token is created lazily and the check never creates one.** The default login page, the token endpoint and `RequestSessionExt::csrf_token()` create the token. `csrf_token()` returns `None` when the request has no session (no session filter ran) and does not create a session; a token created in a session that is new marks it for writing, so the cookie is set. The check only reads the session, so a failed check changes nothing.

**The check runs in the security chain.** It is not a separate filter, because the chain knows whether a request was authenticated by `Basic`, by a bearer token or by the session, and because it runs the login and logout handlers that must verify the token before they authenticate. The chain restores the authentication from the session and then checks the token before authorization. The order is fixed: credentials in a header win, so a request that `Basic` or a bearer token authenticated is exempt even when a stray session cookie is present.

**Which requests are checked.** CSRF protection is on whenever form login is on, with defaults, and every request is checked when its method is unsafe and the session authenticates it, plus `POST` of the login and logout paths. The checked set is every method except `GET`, `HEAD`, `OPTIONS` and `TRACE`, so `CONNECT` and an unknown method token fail closed. The spec text listed four methods; the code checks all unsafe ones, because "unsafe" is what the HTTP specification means and an extension method behind a session must not get through. The exemptions are the safe methods, `Basic` and bearer requests, and anonymous requests: a request without a session, and also one that carries a pre-login session without a login (a session that holds no authentication), because those carry no ambient credential. A public `POST` route therefore keeps working.

**What a failed check does.** It answers a fixed 403 with an empty body and changes nothing. The answer is the same whatever the reason: no session, no stored token, no submitted token, a wrong token, a stale token. It does not use a custom `access_denied` handler, the same as the 6c same-origin 403, so an application cannot restyle it. Inside an `App` the error filter renders the empty 403 as `application/problem+json`. A session that the request did not send (one minted in this request) never passes.

**How the token is submitted.** The header named by `header_name` (default `X-CSRF-TOKEN`) wins. Without that header, and only for `application/x-www-form-urlencoded`, the field named by `field_name` (default `_csrf`) is read from the buffered body, within the 4 KiB limit of the 6c form parser, and the first value wins. A larger body, another content type without the header (a multipart form, JSON), and a token in the query string fail the check. The chain marks the token header sensitive at its start, so `Debug` output of the request shows `Sensitive`. The form field is part of the body and the body is visible in the `Debug` output of the request, the same as the password of the 6c login form; this is documented, not hidden.

**Login and logout.** On `POST /login` and `POST /logout` the 6c same-origin check runs first and the token check second, and either failure gives the same empty 403, before the form is read and before the password provider is asked, so a forged post never reaches it. The origin check stays a second layer on these two posts. It is not extended to the application's routes. A successful login rotates the session id and replaces the token, so a token fetched before the login is dead afterwards and a login CSRF has no token to use. A failed login changes nothing. Logout invalidates the session, which drops the token. When the session filter is missing, a refused login logs the fixed "add a SessionFilter" error and still answers 403, so a misconfiguration is visible.

**The default login page and the framing limit.** With CSRF on, the default page creates the pre-login session, puts the token in a hidden field named `field_name`, and adds `frame-ancestors 'none'` to its content security policy and the header `X-Frame-Options: DENY`. This closes the framing limit of 6c, for this page only. The page gets its token only when the fallback renders it: an application page at the login path creates no session. Because the page writes a session for an anonymous browser, it is a second anonymous session writer beside the saved request of 6c, bounded by the same sampled eviction.

**The token endpoint.** When `token_endpoint` is set, a `GET` of that exact path answers 200 with `Content-Type: application/json`, `Cache-Control: no-store` and the body `{"headerName":"..","parameterName":"..","token":".."}`. It creates the session and the token when missing, so it is also an anonymous session writer. The chain answers it before the application route, so on a collision the endpoint wins. A cross-origin page cannot read it unless the application's CORS filter allows it. Without a session filter it logs the fixed "add a SessionFilter" error and passes the request on to the application. It is off by default. A path that equals a form login path is refused at build time.

**Settings and configuration.** `HttpSecurity::csrf(|c| ..)` takes a `CsrfConfig` with `enabled` (true), `field_name` (`_csrf`), `header_name` (`X-CSRF-TOKEN`) and `token_endpoint` (none). Calling `csrf` without `form_login` makes `build` fail with `SecurityBuildError::CsrfWithoutFormLogin`, also for `enabled(false)`. A bad field or header name (empty, or a character outside ASCII letters, digits, `-` and `_`) or a token endpoint that is not a safe local path or equals a form login path makes `build` fail with `SecurityBuildError::InvalidCsrfConfig`. The error texts are fixed and contain no value. `CsrfProperties::from_config` reads `security.csrf.{enabled,field-name,header-name,token-endpoint}`, rejects unknown keys and names the key, not the value, in an error. With form login on, `enabled` defaults to true. With form login off the chain never calls `csrf`, and any `security.csrf` key except `enabled = false` fails the startup naming the key, because the table would otherwise be ignored without notice. The property metadata lists the four keys and a test pins that the list is exactly what the reader reads. The boot stack probes did not grow.

**The split.** Slice 6d is the token, the check, the login and logout changes, the login page and the endpoint. A general security-headers filter, a signed double-submit cookie, a script-readable token cookie, multipart parsing, per-request tokens and extending the origin check to application routes stay out.

## Consequences

Good:

- A logged-in browser user is protected on every unsafe route of the application, not only on login and logout, and an application that only turns on form login is protected with no further code.
- A login CSRF has no token to use, and the token of the login page is dead after the login.
- A forged login or logout never reaches the password provider, and all refusals look the same.
- The token never reaches a log line, an error text, a redirect, a URL or `Debug` output of a request header; both session backends work.
- The default login page can no longer be framed.

Bad:

- A script on the same origin (XSS) can read the token from the page or from the endpoint. CSRF tokens do not protect against XSS.
- A multipart form cannot carry the field, because the reader parses only urlencoded bodies; it must send the header.
- With the cookie backend the server keeps nothing, so an attacker who can plant a sealed cookie of their own session (for example from a sibling subdomain) also knows that session's token and passes the check. The synchronizer token cannot prevent that; see [ADR 0087](0087-sessions.md).
- The login page and the token endpoint are anonymous session writers, and an anonymous flood can fill the cap of the in-memory store and evict real sessions. Sampled eviction only bounds the cost.
- Only the login page is protected against framing; the application's own pages and every other response are not.
- The same-origin check still covers only login and logout, and allows a request with neither `Origin` nor `Referer`.
- A failed check skips a custom `access_denied` handler, so its 403 cannot be restyled.
- The `_csrf` form field is visible in the `Debug` output of the request.
- An application that rotates or invalidates the session while it answers a `404` for the login page gets a page with a token that is no longer valid (a corner case).

## Alternatives considered

- A signed double-submit cookie: rejected, because it needs a second cookie and a key and adds no protection that a session-bound token lacks.
- A script-readable `XSRF-TOKEN` cookie: rejected for now; the header and the endpoint serve scripts without a cookie that every request carries.
- A separate CSRF filter: rejected, because it would have to learn from the chain which credentials authenticated the request and could not verify the token inside the login handler before the password check.
- Checking only `POST`, `PUT`, `PATCH` and `DELETE`: rejected for the fail-closed set of every unsafe method.
- Checking anonymous requests that have no session: rejected, because they carry no ambient credential and a public `POST` route must keep working.
- Extending the origin check to every application route: rejected for this slice; the token covers them, and a request without `Origin` and `Referer` stays allowed for non-browser clients.
- One token per request: rejected, because it breaks back buttons, several tabs and forms that are rendered ahead of time.
