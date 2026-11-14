# ADR 0003: Dependency injection uses a runtime container that reports every wiring error from `build()`

- **Status:** Accepted
- **Date:** 2026-10-23
- **Source:** [oxbloom-di: singleton dependency injection — Design](../superpowers/specs/2026-10-23-di-design.md)

## Context

Hand-wired `Arc::new(Controller { .. })` state does not scale, and the framework needs a container that builds services in dependency order. Rust offers two directions: a compile-time proof of the dependency graph, or a runtime container with explicit registration.

## Decision

`oxbloom-di` is a runtime container with explicit registration; there is no compile-time graph proof. `ContainerBuilder` methods never fail, so the builder stays chainable, and every error (missing dependency, cycle, duplicate registration, failing provider) surfaces from `build()`. An application builds the container before the server binds, so a wiring mistake fails the startup instead of a request. Types are keyed by `TypeId`, each type is built exactly once, and the build order is deterministic for the same registrations. A panicking provider is not caught; the panic propagates out of `build()`. DI v2 ([ADR 0026](0026-typed-provider-index.md)) kept this design explicitly.

## Consequences

Good:

- Wiring mistakes are found at startup with a precise error (`Missing` with its `required_by` chain, `Cycle` with its path).
- Registration stays ordinary Rust code: no build script, no code generation beyond small macros.

Bad:

- A wiring mistake is found when the application starts, not when it compiles.
- Errors carry type names from `std::any::type_name`, whose text is not guaranteed stable across compiler versions.

## Alternatives considered

- A compile-time dependency graph: rejected for this design; the spec records the choice as agreed without further comparison.
