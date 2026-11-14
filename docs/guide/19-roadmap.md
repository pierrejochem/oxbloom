# Roadmap

Security slices:

| Slice | Content |
|---|---|
| 5 (done) | Filter layer, security core, JWT resource server (static keys, JWKS) |
| 6 | `PasswordEncoder`, `UserDetailsService`, HTTP Basic, form login with sessions |
| 7 | CSRF, session management, security headers |
| 8 | Method security (`@PreAuthorize` equivalent) |
| 9 | OAuth2 login and client |

Core features modelled on Spring Boot are tracked in [`docs/ROADMAP-core.md`](../ROADMAP-core.md). Slices C1 (DI v2), C3 (application lifecycle in `oxbloom-boot`: C3a, the lifecycle, and C3b, failure analyzers and the startup failure report), C4a (conditions and the auto-configuration mechanism) and C4b (the web server and security auto-configurations, property metadata) and C5a (logging: levels, groups, runtime change and startup events), C5b (file and JSON output, request logging) and C6a (filter order, uniform errors, `server.*` keys) and C6b-1 (`Json<T>`, `Request::json`, content negotiation) and C6b-2 (CORS, compression, static resources) and C10a (test support: `ContextRunner`, clocks) and C10b (`TestClient`, `TestApp`, `App::handle_from`) and C7 (SSL bundles: `oxbloom-ssl`, HTTPS for the web server, a bundle for the JWKS fetcher, `TestClient::connect_tls`; see [SSL bundles](16-ssl-bundles.md#ssl-bundles)) and C8 (the actuator: health, liveness and readiness, info and log level endpoints; see [Actuator](17-actuator.md#actuator)) and C9a (task scheduling: `#[scheduling]`, cron, fixed rate and fixed delay, the boot scheduler service; see [Task scheduling](18-tasks.md#task-scheduling)) are done. Retry (C9b) is still planned, and the web layer has no open slice. The overall project plan is in `IDEA.md`.
