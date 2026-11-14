# ADR 0031: `oxbloom-di` does not depend on `oxbloom-config`

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-di v2 design (slice C1: traits, collections, lifecycle, beans, profiles)](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

DI v2 adds profile conditions, and later slices add property conditions. Both need information that lives in `oxbloom-config`, but a dependency from the container to the configuration crate would make DI unusable without it and invert the layering.

## Decision

The container does not depend on `oxbloom-config`. Active profile names are passed to the builder (`ContainerBuilder::active_profiles`), and `Config::active_profiles()` gives the list after group expansion so boot can pass it on. Profile conditions take names, any-of lists and negation (`!name`). C4a kept the same direction for properties through the `Environment` trait ([ADR 0045](0045-environment-trait-for-property-conditions.md)).

## Consequences

Good:

- `oxbloom-di` stays a small, standalone crate.
- Tests can drive profiles without building a configuration.

Bad:

- Applications that use the container without boot must pass the profile names themselves.
