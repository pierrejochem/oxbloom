# ADR 0017: Configuration keys match in a relaxed canonical form

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Keys arrive from TOML files (`max-connections`), environment variables (`MAX_CONNECTIONS`) and Rust field names (`max_connections`). Spring Boot's relaxed binding lets all of them match one property.

## Decision

All keys from all sources are normalized to a canonical form: lowercase, with `-` and `_` removed. `maxConnections`, `max_connections`, `max-connections` and `MAX_CONNECTIONS` are one key. Two spellings of one canonical key in the same file are `ConfigError::DuplicateKey` naming both origins; in the environment or on the command line they collapse and the last one wins. Struct fields, unit enum variants and map keys match by canonical form, and an exact-name match beats a relaxed one. The same rule later applied to logging group and target names.

## Consequences

Good:

- Users write keys in the style of each source and they still bind.
- `deny_unknown_fields` still reports names that match nothing.

Bad:

- `ab` and `a_b` are the same key, a known and accepted trade-off.
- A typo can match an unintended key; two fields whose names are canonically equal are reported only in some declaration orders, a documented limit.
