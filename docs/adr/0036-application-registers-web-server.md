# ADR 0036: Applications build the `App` and register the `WebServer` bean themselves

- **Status:** Superseded by [ADR 0048](0048-contributor-beans.md)
- **Date:** 2026-10-25
- **Source:** [Application lifecycle (slice C3a): `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-design.md)

## Context

C3a introduced `WebServer` before auto-configuration existed. Something had to assemble the `App` (routes, security chain) and register the server.

## Decision

The application builds its `App` and registers `WebServer::new(app, &config)` as a bean with `provides(dyn Service)`. Automatic registration of the web server and other services was deferred to C4 (auto-configuration).

## Consequences

Good:

- No auto-configuration machinery was needed for the first lifecycle slice.

Bad:

- Every application repeated the same factory code for the server and the security chain; C4b replaced it with auto-configuration and contributor beans.
