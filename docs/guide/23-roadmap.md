# Roadmap

Security slices:

| Slice | Content |
|---|---|
| 5 (done) | Filter layer, security core, JWT resource server (static keys, JWKS) |
| 6a (done) | `PasswordEncoder` (Argon2id), `UserDetailsService`, `DaoAuthenticationProvider`, HTTP Basic; see [Password login](19-password-login.md#password-login) |
| 6b (done) | Sessions: `Session`, `SessionFilter`, an in-memory store and a stateless encrypted cookie; see [Sessions](20-sessions.md#sessions) |
| 6c (done) | Form login: login and logout handlers, the `Authentication` in the session, a saved request, a default login page, sampled eviction; see [Form login](21-form-login.md#form-login) |
| 6d (done) | CSRF protection: a synchronizer token in the session, checked in the chain, the login page field and framing headers, an optional token endpoint; see [CSRF protection](22-csrf.md#csrf-protection) |
| 7 | Session management, security headers |
| 8 | Method security (`@PreAuthorize` equivalent) |
| 9 | OAuth2 login and client |

Core features modelled on Spring Boot are tracked in [`docs/ROADMAP-core.md`](../ROADMAP-core.md). Slices C1 (DI v2), C3 (application lifecycle in `oxbloom-boot`: C3a, the lifecycle, and C3b, failure analyzers and the startup failure report), C4a (conditions and the auto-configuration mechanism) and C4b (the web server and security auto-configurations, property metadata) and C5a (logging: levels, groups, runtime change and startup events), C5b (file and JSON output, request logging) and C6a (filter order, uniform errors, `server.*` keys) and C6b-1 (`Json<T>`, `Request::json`, content negotiation) and C6b-2 (CORS, compression, static resources) and C10a (test support: `ContextRunner`, clocks) and C10b (`TestClient`, `TestApp`, `App::handle_from`) and C7 (SSL bundles: `oxbloom-ssl`, HTTPS for the web server, a bundle for the JWKS fetcher, `TestClient::connect_tls`; see [SSL bundles](16-ssl-bundles.md#ssl-bundles)) and C8 (the actuator: health, liveness and readiness, info and log level endpoints; see [Actuator](17-actuator.md#actuator)) and C9a (task scheduling: `#[scheduling]`, cron, fixed rate and fixed delay, the boot scheduler service; see [Task scheduling](18-tasks.md#task-scheduling)) and C9b (retry: `#[retryable]`, `RetryPolicy`, the `tasks.retry.*` keys; see [Retry](18-tasks.md#retry)) and security Slice 6a (password login: Argon2id encoder, user store, HTTP Basic; see [Password login](19-password-login.md#password-login)) and security Slice 6b (sessions: the session filter, an in-memory store, a stateless encrypted cookie; see [Sessions](20-sessions.md#sessions)) and security Slice 6c (form login and logout; see [Form login](21-form-login.md#form-login)) are done. The web layer has no open slice. The overall project plan is in `IDEA.md`.
