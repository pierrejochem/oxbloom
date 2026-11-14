# ADR 0049: Configuration keys are described by runtime property metadata

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [C4b: Auto-configure the web server, security chain and JWT decoder](../superpowers/specs/2026-10-25-web-security-autoconfig-design.md)

## Context

Spring Boot generates configuration metadata at build time for IDE completion. Oxbloom needed a list of the keys each auto-configuration reads, so users can list them and tools can consume them.

## Decision

Property metadata is a runtime descriptor: `PropertyMetadata { key, type_name, default, description }`. `AutoConfiguration::properties()` returns a static slice, `boot_properties()` lists boot's own keys, the builder's `property_metadata()` merges them (without excluded configurations, sorted, without duplicates) and `PropertyMetadata::to_json` writes a stable JSON array. A test per declared key exercises the code that consumes it, so a key renamed in code without updating the metadata fails. Crates that cannot depend on boot (logging) expose plain static tuples that boot converts.

## Consequences

Good:

- No build-time processor or derive macro to maintain.
- The metadata cannot silently drift from the code.

Bad:

- Metadata is written by hand next to the code, and the consumption tests are the only guard.
- No compile-time check against the real configuration structs.

## Alternatives considered

- A build-time metadata processor: rejected.
- A derive macro: rejected.
