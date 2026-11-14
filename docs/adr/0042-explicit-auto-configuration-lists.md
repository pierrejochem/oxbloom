# ADR 0042: Auto-configurations are discovered through explicit lists, and user registrations come first

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Conditions and auto-configuration (slice C4a)](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md)

## Context

Spring Boot discovers auto-configurations by classpath scanning. Rust has no classpath, and link-time registries add global state and surprise.

## Decision

Discovery is explicit. Each crate that ships auto-configurations exposes `pub fn auto_configurations() -> AutoConfigurations`, and the application passes lists to the builder (`auto_configuration::<A>()`, `auto_configurations(list)`, `exclude::<A>()`). Boot appends each auto-configuration as a DI group after the user's `configure` closures, so user registrations come first and an auto-configured bean with `on_missing_bean` backs off when the user provided one. `exclude` requires `A: AutoConfiguration`, so a typo is a compile error. A link-time registry may come later as an opt-in feature that does not change the explicit API; exclusion by configuration key is not supported.

## Consequences

Good:

- No global state and no hidden discovery: the application's list is the whole truth.
- User beans always win over defaults.

Bad:

- Applications must add the list call themselves.
- An override placed in another auto-configuration that runs later keeps both beans; overrides belong in `configure`.

## Alternatives considered

- Link-time collection (for example a registry crate): deferred to an optional later feature.
