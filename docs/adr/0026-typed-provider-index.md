# ADR 0026: All injectable shapes resolve through one typed provider index with explicit trait provision

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-di v2 design (slice C1: traits, collections, lifecycle, beans, profiles)](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

The first container ([ADR 0004](0004-singleton-arc-injection-only.md)) injected only `Arc<T>`. Applications needed trait objects, optional dependencies and collections, and the three parts of DI v2 (injection, lifecycle, factories and profiles) needed one shared registration model.

## Decision

Every registration is an entry with a concrete key and zero or more provided keys, and resolution of every shape works on the same index: `Arc<T>` and `Arc<dyn Trait>` (exactly one provider or the single `primary`), `Option<Arc<_>>` (`None` when no provider exists, but a failing provider is still an error) and `Vec<Arc<_>>` (all providers, sorted by `order` then registration order). Traits are provided explicitly, never discovered: `#[component(provides(dyn T))]` or `.provides(|a: Arc<Impl>| -> Arc<dyn T> { a })`. The closure names the concrete type because the unsizing coercion cannot be written generically on stable Rust; `build()` checks that it matches the last registered entry. The runtime container itself ([ADR 0003](0003-runtime-di-container.md)) remains the design. This supersedes [ADR 0004](0004-singleton-arc-injection-only.md).

## Consequences

Good:

- One model serves single, optional and collection injection, profiles and later conditions.
- No hidden discovery: what a bean provides is visible at its registration.

Bad:

- Every trait provision is written out, including a closure that only coerces.
- The registration model changed under every existing path; the existing tests and a characterization of build order were the regression net.
