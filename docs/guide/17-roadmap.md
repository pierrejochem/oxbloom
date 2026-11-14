# Roadmap

Security slices:

| Slice | Content |
|---|---|
| 5 (done) | Filter layer, security core, JWT resource server (static keys, JWKS) |
| 6 | `PasswordEncoder`, `UserDetailsService`, HTTP Basic, form login with sessions |
| 7 | CSRF, session management, security headers |
| 8 | Method security (`@PreAuthorize` equivalent) |
| 9 | OAuth2 login and client |

Core features modelled on Spring Boot (application lifecycle, configuration v2, DI v2, logging, conditions and auto-configuration, SSL bundles, health, tasks, test support) are planned in [`docs/ROADMAP-core.md`](../ROADMAP-core.md); slices C1 (DI v2), C3 (application lifecycle in `oxbloom-boot`: C3a, the lifecycle, and C3b, failure analyzers and the startup failure report), C4a (conditions and the auto-configuration mechanism) and C4b (the web server and security auto-configurations, property metadata) and C5a (logging: levels, groups, runtime change and startup events), C5b (file and JSON output, request logging) and C6a (filter order, uniform errors, `server.*` keys) and C6b-1 (`Json<T>`, `Request::json`, content negotiation) and C6b-2 (CORS, compression, static resources) and C10a (test support: `ContextRunner`, clocks) and C10b (`TestClient`, `TestApp`, `App::handle_from`) are done; the web layer has no open slice. The overall project plan is in `IDEA.md`.
