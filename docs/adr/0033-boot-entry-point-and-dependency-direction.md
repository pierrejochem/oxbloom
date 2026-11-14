# ADR 0033: `oxbloom-boot` is the single entry point and sits above configuration and DI

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Application lifecycle (slice C3a): `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-design.md)

## Context

Before C3 every Oxbloom program wired itself by hand: it built a `Config`, a `Container` and an `App`, bound a port and handled Ctrl-C. Spring Boot has `SpringApplication` for this.

## Decision

A new crate `oxbloom-boot` provides `OxbloomApplication`, which runs config, container, runners and services in a fixed order, fires lifecycle events, handles signals with a deadline and turns the outcome into an exit status. `oxbloom-boot` depends on `oxbloom-config` and `oxbloom-di`; its `web` feature depends on `oxbloom-web`. `oxbloom-web` and `oxbloom-config` do not depend on boot. Boot registers `Config`, `ApplicationArguments` and `ShutdownHandle` as instances before user wiring, and `ApplicationArguments` shows only counts in `Debug` and `Display`, never values. The slice was split into C3a (lifecycle) and C3b (failure analysis).

## Consequences

Good:

- Applications start, serve and stop without hand-written glue.
- Lower crates stay usable without boot.

Bad:

- Everything that needs both the container and the web layer (auto-configuration, failure analyzers) must live in boot, which grows several features.
