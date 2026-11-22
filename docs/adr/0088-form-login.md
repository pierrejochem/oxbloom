# ADR 0088: Form login keeps the authentication in the session, rotates the id at login, and uses a same-origin check until CSRF tokens exist

- **Status:** Accepted
- **Date:** 2026-11-15
- **Source:** the security Slice 6c design spec `2026-11-15-form-login-design.md` (a local file, not linked) and its plan

## Context

A browser user needs to log in with an HTML form and stay logged in. How does the chain remember the user between requests? What does an anonymous browser see at a protected page, and what does an API client see? Where does the browser go after the login, and how do we keep that redirect from becoming an open redirect? How is a login protected against session fixation and, until CSRF tokens exist, against cross-site posts? What does the in-memory session store do when anonymous requests start to write sessions?

Sessions are [ADR 0087](0087-sessions.md), and the password provider and the failed-login rules are [ADR 0086](0086-password-login.md). CSRF tokens are Slice 6d.

## Decision

**The authentication lives in the session.** After a successful login the chain stores the principal, the authorities and the details as JSON under the session key `oxbloom.security.authentication`, together with the login time and a format version. `Authentication` implements `Serialize` and `Deserialize` for that. At the start of a request the chain restores it, but only when the request carries a session cookie that the session filter resolved to an existing session, so an anonymous request never creates or reads a session. A stored value that does not deserialize, has an unknown version or has an unknown shape is removed and the request is anonymous; it is never a 500. The keys under the prefix `oxbloom.security.` are reserved for Oxbloom, and application code must not set them.

**The stored authentication is not refreshed.** The chain does not ask the user store again on later requests. A role change or a lock of the account takes effect at the next login or when the session expires. The alternative, loading the user on every request, would put a store lookup into every request and was left out of this slice.

**Basic and bearer never touch the session.** They never create one. When a request has both a valid session and a `Basic` header, the Basic credentials are tried first, so Basic wins over a session.

**The login handler** (`POST` to `login_processing_url`, default `/login`) runs the same-origin check, reads an `application/x-www-form-urlencoded` body of at most 4 KiB, and authenticates through the existing `ProviderManager`. A body that is too large, a missing field or invalid UTF-8 is a failed login; another content type is a 400. On success it calls `rotate_id()` first, stores the authentication, consumes the saved request and answers 302 to the saved request or to `default_success_url`. On any failure it answers 302 to `failure_url` and changes nothing in the session. An unknown user, a wrong password, a locked or disabled account and an unavailable password provider (`ProviderUnavailable`) all give the same answer. The username is never put into the redirect or a log line; the `debug` log has the error variant and the length of the username.

**Login always rotates the id.** A planted or stolen anonymous session id therefore never carries the login. A user who is already logged in and posts again is treated like any login: the id is rotated and the authentication replaced.

**The logout handler** (`POST` to `logout_url`, default `/logout`) runs the same-origin check, calls `invalidate()` and answers 302 to `logout_success_url`. A `GET` of the logout path changes nothing and follows the authorization rules like any other path.

**A content-negotiated entry point.** With form login on and no custom `entry_point`, an anonymous `GET` or `HEAD` whose `Accept` header prefers `text/html` gets a 302 to `login_page`, and the original path and query are saved in the session under `oxbloom.security.saved-request`. A browser is recognised by an HTML type with the highest quality, or by a wildcard that is preferred while an HTML type is listed with at least the quality of `application/json`. Every other request, such as an API client, another method or a missing `Accept`, keeps the old 401 with its challenge and saves nothing. API clients get no session. A custom `entry_point(..)` replaces the default.

**The saved request and the redirect-target rule.** The saved value holds a path and an optional query. It must start with a single `/`, must not start with `//` or `/\`, must not contain a control character or a backslash, must not contain `://` before the first `?`, and is at most 2048 bytes long. It is checked when it is saved and again when it is used, so a tampered session cannot cause an open redirect. The same rule, exposed as `is_safe_redirect_target`, applies to every configured URL at build time, so `default-success-url = "https://evil.example"` fails the startup. The configured form login paths are never saved, and a later anonymous request replaces the saved one only when its path differs.

**The default login page.** The chain serves a small HTML form at `login_page` after the router answered 404 for a `GET` or `HEAD` of that path and `login_page_enabled` is true. Because the page is the answer to a 404, any 404 at that path is replaced by the page, also one that an application handler chose deliberately; an application that maps `GET /login` itself never reaches the fallback, and its handler always sees an anonymous user. The page has fixed texts for `?error` and `?logout` and reflects no other query value. It has `Cache-Control: no-store`, `X-Content-Type-Options: nosniff` and the fixed header `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'`. It sets no `frame-ancestors` and no `X-Frame-Options`, so it can be framed; that is a clickjacking limit that is to be addressed with 6d. Seen through `App::handle` a `HEAD` answer keeps its body.

**Which paths skip the authorization rules.** Only `POST` of the processing URL, `POST` of the logout URL and `GET` or `HEAD` of the login page are handled before the rules. The success and failure URLs are not exempt. The defaults work because `/`, `/login?error` and `/login?logout` resolve to paths the chain already handles or the application opens; a custom `failure_url` or `logout_success_url` on another path needs a `permit_all` rule, otherwise the user is sent to the login page again.

**The same-origin check, and no CSRF tokens yet.** The two posts are protected by this check and by the `SameSite=Lax` session cookie until Slice 6d adds tokens. The origin of the `Origin` header (scheme, host and port) must equal the scheme, host and port of the request, honouring the forwarded headers that the server already trusts. Without `Origin` the `Referer` is parsed the same way. `Origin: null`, a repeated or malformed value and a mismatch answer 403 with an empty body. A request with neither header is allowed, because non-browser clients send none; so a client that sends neither is not stopped by this check. The check is not a replacement for tokens.

**Sampled eviction in the in-memory store.** The saved request lets anonymous browsers write sessions, so the 6b store changed before this slice. At the cap a new session removes expired entries it meets and evicts the oldest-touched entry of a sample of at most 16 entries. The sample starts at a random shard and at a random position within the first 64 entries of that shard, and walks on in hash order. The cost per write is therefore constant and no longer depends on the size of the store; the session being saved is never the victim, and a cap of 1 works. The limits stay: an anonymous flood of `Accept: text/html` requests to protected paths can still fill the cap and evict real sessions, and overshoot from racing saves persists until expiry or a sweep. The sample is not uniform over the store, because of the start position limit.

**Configuration and boot.** `HttpSecurity::form_login(|f| ..)` takes a `FormLogin` value with the defaults `login_page` `/login`, `login_processing_url` `/login`, `logout_url` `/logout`, `default_success_url` `/`, `failure_url` `/login?error`, `logout_success_url` `/login?logout`, `username_field` `username`, `password_field` `password` and `login_page_enabled` true. `build` fails with `SecurityBuildError::FormLoginWithoutProvider` without a password provider and with `SecurityBuildError::InvalidFormLoginUrl` for a URL that is not a local path, an invalid field name, two equal field names, or a logout path equal to the login page or the processing path (compared exactly). `FormLoginConfig::from_config` reads the keys `security.form-login.{enabled,login-page,login-processing-url,logout-url,default-success-url,failure-url,logout-success-url,username-field,password-field,login-page-enabled}`, rejects unknown keys and names the key, not the value, in an error. `SecurityAutoConfiguration` applies `form_login` when `security.form-login.enabled` is true. It needs the table `security.session` and a password provider bean; without the table the startup fails with an error that names `security.session`. Its property metadata lists the ten keys, and a test pins that the list is exactly what the reader reads.

**The split.** Slice 6c is form login, logout, the saved request, the login page and sampled eviction. CSRF tokens are Slice 6d. Remember-me, concurrent-session limits, OAuth2 login, registration, password reset and rate limiting stay out.

## Consequences

Good:

- A browser user logs in with a plain HTML form and returns to the page it asked for.
- A planted session id never carries a login, and an anonymous API client never gets a session.
- Every failed login looks the same, and no credential or id reaches a log line, an error text or a redirect.
- The redirect target is validated twice, so neither a configuration value nor a tampered session can make an open redirect.
- The cost of a new session at the cap no longer depends on the size of the store.

Bad:

- There are no CSRF tokens yet. The same-origin check and `SameSite=Lax` are the interim measure, and a request with neither `Origin` nor `Referer` is allowed.
- The stored authentication is not refreshed, so a role change or a lock takes effect at the next login or at session expiry.
- The default login page can be framed and is served for any 404 at the login path.
- A custom success or failure URL needs an explicit `permit_all` rule.
- An anonymous flood of `Accept: text/html` requests to protected paths can fill the cap of the in-memory store and evict real sessions.
- The in-memory store, and with it every login, is lost at a restart and not shared between instances.

## Alternatives considered

- Refreshing the authentication from the user store on each request: rejected for now because of the cost per request; the limit is documented.
- A 302 to the login page for every unauthenticated request: rejected, because API clients need the 401 and its challenge, and a session for every API client would let them fill the store.
- Storing the saved request without validation: rejected, because the session value is a redirect target.
- Skipping `rotate_id` when the session is new: rejected, because a planted id is the case that fixation needs.
- A full scan or a sorted index for eviction: rejected, because it costs O(n) or memory per session; a sample is enough to keep old entries from living forever.
- Doing the CSRF work in this slice: rejected, see the split.
