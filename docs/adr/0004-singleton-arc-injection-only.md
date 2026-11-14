# ADR 0004: The first container supports only singleton `Arc<T>` injection

- **Status:** Superseded by [ADR 0026](0026-typed-provider-index.md)
- **Date:** 2026-10-23
- **Source:** [oxbloom-di: singleton dependency injection — Design](../superpowers/specs/2026-10-23-di-design.md)

## Context

Slice 3 introduced the container and had to stay small. Transient and request scopes, lifecycle hooks, trait-object injection and configuration injection were all candidates.

## Decision

The container has a singleton lifecycle only. Every managed type is `Send + Sync + 'static` and is handed out as one shared `Arc<T>`. `#[component]` accepts a struct with named fields whose types are all `Arc<_>`; trait objects, collections, init and destroy hooks are out of scope. `Arc<dyn Trait>` injection was named as the most likely next request, admissible later by registering the trait object as its own key.

## Consequences

Good:

- The first container was small and easy to review.
- The `TypeId`-keyed design left room for trait-object keys.

Bad:

- Applications could not inject trait objects, optional dependencies or collections, and had no lifecycle hooks; DI v2 lifted these limits.
