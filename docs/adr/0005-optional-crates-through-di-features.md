# ADR 0005: Framework crates integrate with DI through optional `di` cargo features

- **Status:** Accepted
- **Date:** 2026-10-23
- **Source:** [oxbloom-di: singleton dependency injection — Design](../superpowers/specs/2026-10-23-di-design.md)

## Context

Not every Oxbloom user wants dependency injection, and the web layer, configuration and security must stay usable on their own. At the same time, an application that does use the container wants each crate to register itself conveniently.

## Decision

`oxbloom-di` depends only on `oxbloom-macros`; it has no web or tokio dependency. Every other crate that integrates with the container does so behind a cargo feature named `di`, off by default: `oxbloom-web` adds `App::controller` and `AppError`, `oxbloom-config` adds `ContainerBuilderExt::config`, `oxbloom-security` adds its container extension. The `Controller` trait itself is always available in `oxbloom-web`, so a controller type does not depend on the feature.

## Consequences

Good:

- Users who do not want DI pull in nothing from it.
- Crates stay small and are coupled through traits and features, as `AGENT.md` asks.

Bad:

- Every integration needs a feature-gated code path, and CI must test the crates with and without the feature.
