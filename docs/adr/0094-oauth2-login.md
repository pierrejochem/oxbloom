# ADR 0094: OAuth2 and OpenID Connect login reuses the form login session, has a fixed callback path, accepts asymmetric id tokens only and keeps the tokens in the session

- **Status:** Accepted
- **Date:** 2026-11-24
- **Source:** the security Slice 9 design spec `2026-11-23-oauth2-design.md`, part B (a local file, not linked), its plan `2026-11-23-oauth2.md` and the controller rulings R10 to R15 of that plan

## Context

Oxbloom can log users in with a password, a session, remember-me and JWT bearer tokens, but not through an external identity provider. Slice 9 part B adds the authorization code flow with PKCE for OpenID Connect providers and for plain OAuth2 providers, in `oxbloom-security` behind the feature `oauth2`, wired in boot by `SecurityAutoConfiguration`. The outbound calls run on the `HttpClient` of [ADR 0093](0093-http-client.md).

A login of this kind crosses several trust boundaries: the browser comes back from another site with a code and a state, the code is exchanged over a back channel, and an id token or a user-info answer decides who the user is. Each step has known attacks (login CSRF, code injection, mix-up, token substitution, open redirects) and each has to fit the session model that form login already has ([ADR 0088](0088-form-login.md), [ADR 0090](0090-security-headers-and-session-management.md), [ADR 0091](0091-remember-me.md)). The design choices below were made while building the flow, several of them as controller rulings during the reviews.

## Decision

**OAuth2 login reuses form login.** `HttpSecurity::oauth2_login` needs `form_login`, which gives it the session, the logout, the saved request, the success URL, the default failure URL and the session limit; without it `build()` fails with `SecurityBuildError::OAuth2LoginWithoutFormLogin`. With OAuth2 login on, form login needs no password provider. A successful callback calls the same `establish` step as a password login: a new session id, a new CSRF token, the `Authentication` stored, the session limit applied, then a redirect to the saved request (when it is a safe local target) or the default success URL. A missing session filter cannot be seen at `build()`, so, as for form login, it is logged at error level at run time (once per OAuth2 login configuration, so unauthenticated requests cannot flood the log) and every login fails (`OAuth2LoginWithoutSessions` is a log line, not a build error). A builder without an `HttpClient` fails with `SecurityBuildError::OAuth2LoginWithoutClient`. In boot, the error tells the user to set `security.form-login.enabled = true`.

**One pending key per state.** The start stores a pending record (registration, state, nonce, PKCE verifier, saved request, start time) under its own reserved session key `oxbloom.security.oauth2.pending.<state>`, with an index that keeps at most five. Two browser tabs that start a login each keep their own record. The callback takes the record before it uses it, so a state works once, and compares the state in constant time. A state of more than 64 characters or outside `[A-Za-z0-9_-]` is refused before the session is read. A pending login is valid for 10 minutes by default; zero fails the build.

**The `iss` parameter is checked.** When the callback carries an `iss` parameter (RFC 9207), it must equal the provider's configured `issuer-uri`, which defends against a mix-up between providers. A provider without an issuer therefore matches no `iss`. An absent `iss` is accepted.

**Asymmetric algorithms only.** An id token is accepted only when it is signed with RS256, RS384, RS512, ES256 or ES384, narrowed further by the algorithms the discovery document lists, with RS256 and ES256 as the default. `none` and every symmetric algorithm are refused, so a token signed with the public key as an HMAC secret cannot pass. PS256 is not supported (ruling R11): the key lookup of the JWKS source cannot match it, and the resource server refuses `PS*` on purpose. The header must carry a `kid`, and a `crit` header is refused.

**A provider without an issuer is accepted (ruling R12).** A provider with a `jwk-set-uri` and no `issuer-uri` is an OpenID Connect provider whose id token `iss` is only required to be non-empty. The risk is low: the token comes over TLS from the configured token endpoint, its signature is checked against the configured key set, and `aud` and `nonce` bind it to this client and this login. The build logs one warning per such registration, so the startup shows it.

**The principal may be a number (ruling R10).** The principal is the value of `user-name-attribute` (default `sub` for OpenID Connect). A JSON integer is accepted and turned into a string, because GitHub's stable `id` is a number while its `login` can be renamed and claimed again by someone else. A boolean, a float, an array, an object, `null` and an empty string are refused.

**`Basic` is the default client authentication.** The client id and secret are form-URL-encoded and sent with HTTP Basic, as RFC 6749 section 2.3.1 says. `post` sends them as form fields, and `none` sends no secret, for a public client that relies on PKCE.

**The tokens live in the session and are cleared at every other login.** The access token, its expiry, the refresh token, the id token and the granted scopes are stored per registration under `oxbloom.security.oauth2.tokens.<registration>`. Every login removes the tokens of the session first: an OAuth2 login removes all of them before it stores its own, and a password or remember-me login removes them too, so a session never carries tokens of an earlier user or an earlier provider. A cookie-backed session can hold them, but they may not fit, so a server-side store is recommended.

**The session limit key is qualified by the registration (ruling R14).** An OAuth2 user is counted in the session limit and the per-user session index under `oauth2:<registration>:<principal>`, never under the bare principal. Otherwise a remote user who registers the name `alice` at a provider could evict the sessions of the local password user `alice`. An application that calls `SessionRegistry::invalidate_all` or `sessions_of` for an OAuth2 user passes the qualified key. As a consequence usernames that start with `oauth2:` are reserved: a password user literally named `oauth2:github:alice` would share the OAuth2 user's key. The reservation is enforced: with the feature `oauth2`, a password or remember-me login whose principal starts with `oauth2:` fails like a wrong password.

**The callback path is fixed (ruling R15).** The callback is always `GET /login/oauth2/code/{id}` and the start is `GET /oauth2/authorization/{id}`. The redirect URI of every registration must be exactly `{base-url}/login/oauth2/code/{id}`, or the build fails with `SecurityBuildError::InvalidOAuth2Login`; boot reports it on `security.oauth2.client.registration.<id>.redirect-uri`. This also keeps two registrations from sharing one redirect URI. The redirect URI is built from the configured base URL and never from the `Host` header. In boot, `security.oauth2.base-url` is required when registrations exist and may hold no path beyond an optional `/`, no query and no fragment, because a path prefix could never reach the fixed callback path.

**Revalidation skips an OAuth2 principal only with two markers.** An OAuth2 principal has no `UserDetailsService`, so revalidation would end its session. The details carry `oauth2.principal = true` and `oauth2.registration` (a string), and revalidation skips the principal only when both are present; a string `"true"` or a missing registration is revalidated as usual.

**No provider presets.** There is no built-in configuration for GitHub, Google or any other provider. Each provider is configured with its endpoints or its issuer, which keeps Oxbloom out of tracking other services' URL changes.

**The failure URL is local and cannot loop.** Every failure redirects to the failure URL with an empty body and logs the cause at debug level with fixed text. The failure URL defaults to the failure URL of form login (`/login?error`); when set, it must be a safe local redirect target and must not lie under `/oauth2/authorization/` or `/login/oauth2/code/`, which would loop. The loop check is literal; dot segments and encodings that lead back to an endpoint are a developer misconfiguration that is not detected.

**Only GET is answered.** Any other method on the two endpoints, HEAD included, is a 405 with `Allow: GET`, although the rest of the chain treats HEAD as GET.

**Discovery is cached and its failures back off.** The discovery document of an `issuer-uri` is fetched lazily, its `issuer` must equal the configured one, and it is cached for an hour. Concurrent callers share one fetch. A failed fetch is remembered for a short backoff window of 5 seconds, so callers during an outage get the failure at once instead of queueing one request each.

**`SCOPE_*` authorities are not tied to a registration.** The authorities are `OIDC_USER` or `OAUTH2_USER` plus `SCOPE_<scope>` for each granted scope that is a valid RFC 6749 scope token. A provider of low trust that grants a scope named `admin` produces `SCOPE_admin`, the same authority a trusted provider would give. Rules that must trust one provider should also check the `oauth2.registration` detail.

**Provider calls go through the shared client with a 10 second timeout (ruling R18).** Discovery, the key set, the token exchange and the user info are sent through the one `dyn HttpClient` bean, so the `http.client.*` keys govern them too: `follow-redirects` (default 0, so a 3xx answer of a provider endpoint is a failed login), `max-body-bytes`, and the proxy and SSL bundle settings. Each discovery, token and user-info request carries a per-request timeout of 10 seconds, which replaces `http.client.timeout` for these calls, and each key set fetch is bounded to 10 seconds by its key source, because the JWKS fetcher sends its request without a per-request timeout. When an application enables redirects for other calls, provider calls follow them too, within the hop rules of [ADR 0093](0093-http-client.md). A dedicated client for provider calls was rejected: it would duplicate the SSL bundle and proxy wiring of the bean.

**One shape check for configuration and code.** The `security.oauth2.client.*` keys and a login built in code with `OAuth2Login` go through the same shape validator: a registration id and provider name of `a` to `z`, `0` to `9` and `-`; a non-empty client id; scopes of RFC 6749 characters; a non-empty secret unless the client authentication is `none`; a provider with an issuer or both an authorization and a token endpoint; only `https` endpoints (`http` for a loopback host); and for a plain OAuth2 provider (no issuer, no key set) both a user-info endpoint and a user name attribute, because its login cannot name the user without them. The configuration reports a failure as a `ConfigError::Invalid` that names the full key; the builder fails with `SecurityBuildError::OAuth2LoginWithoutRegistration`. Either way the shape fails at startup, not after the code exchange of every login. The builder also refuses a base URL with a path, as boot does.

**Nothing secret is printed.** No error text, log line or `Debug` output holds a token, a code, a state, a nonce, a verifier, a client secret or a provider response body. The `Debug` output of `ProviderConfig`, `ClientRegistration`, `OAuth2Login` and the pending login shows non-secret configuration (endpoints, the redirect URI, the failure URL and the saved request path) and never a secret, a token, a code, the state, the nonce or the verifier; error texts and log lines hold no URL. Provider error codes are logged only when they are one of the RFC 6749 codes; `error_description` is never used. Boot configuration errors name the full key and never the value. Provider attributes whose names start with `oxbloom.` or `oauth2.` are dropped from the details, so a provider cannot set a framework detail such as `oxbloom.remembered`.

## Consequences

Good:

- An OAuth2 user is a normal session user: logout, the saved request, the CSRF token, the session limit and the session registry work as for a password user.
- Login CSRF, replayed states, code injection across browsers, provider mix-up, token substitution with `alg: none` or an HMAC on the public key, and open redirects after login are refused, and tests pin each case.
- Two tabs can log in at the same time.
- A provider outage does not pile up discovery requests, and no provider call waits longer than 10 seconds.
- A registration that could never log anyone in fails the start, whether it comes from the configuration or from code.
- The redirect URI can never drift from the path the chain answers.

Bad:

- Form login must be on, even in an application without passwords.
- The callback path cannot be changed, and deployments under a path prefix are not supported for OAuth2 login; behind a reverse proxy, the proxy must route `/oauth2/authorization/` and `/login/oauth2/code/` at the root of the external host to the application, and `base-url` is that external origin.
- A provider that signs only with PS256 cannot be used yet.
- Tokens in a cookie-backed session can make the cookie too large.
- `SCOPE_*` from different providers look the same; rules must check the registration when that matters.
- Usernames that start with `oauth2:` are reserved (a password or remember-me login of such a user fails), and `invalidate_all` for an OAuth2 user needs the qualified key.
- HEAD on the two endpoints is a 405.
- Provider calls share the `http.client.*` settings of the application; an application that enables redirects for other calls also lets provider calls follow them, within the ADR 0093 hop rules.
- A `SameSite=Strict` session cookie is not sent on the provider's redirect back, so the login fails; the default `Lax` works.

## Alternatives considered

- A separate OAuth2 session model next to form login: rejected. It would duplicate the logout, the saved request and the session limit, and the two could disagree.
- One pending key per session: rejected, because a second tab would overwrite the first tab's state and its login would fail.
- Accepting every algorithm the provider's JWKS offers: rejected; symmetric and `none` tokens must never pass, so the list is a fixed allow-list narrowed by discovery.
- Refusing providers without an issuer: rejected for this slice (ruling R12); several providers publish a key set without a discovery document, and the other checks bind the token to this client and this login.
- Refusing non-string principals: rejected (ruling R10), because GitHub's stable identifier is a number.
- Counting OAuth2 users under the bare principal in the session limit, as the spec first said: rejected (ruling R14) because of the eviction attack above.
- A configurable callback path: parked (ruling R15). A fixed path keeps the redirect URI check exact.
- Provider presets: rejected for now; they go stale and hide what is configured.
- Building the redirect URI from the `Host` header: rejected; a spoofed host could send codes elsewhere.
- Answering HEAD like GET on the endpoints: rejected; a HEAD that starts a login has no use and would create pending records.
