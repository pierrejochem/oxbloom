# ADR 0007: Configuration is loaded by an own thin layer over `toml::Value` and serde

- **Status:** Superseded by [ADR 0016](0016-config-node-tree-with-origins.md)
- **Date:** 2026-10-24
- **Source:** [oxbloom-config design (Slice 4)](../superpowers/specs/2026-10-24-config-design.md)

## Context

Rust has established configuration crates (`config`, `figment`). Oxbloom needed control over error messages, which must never contain values, and over secret handling.

## Decision

The loader is an own thin layer on `toml::Value` and serde. The `config` and `figment` crates were rejected as too heavy and as giving less control over error messages and secret handling. The public API does not expose the internals, so they can be swapped later.

## Consequences

Good:

- Full control over value-free error messages and `Secret<T>` handling.
- No heavy dependency.

Bad:

- `toml::Value` carries no origin per value, so errors could not say where a value came from; configuration v2 replaced the storage with an own node tree.

## Alternatives considered

- `config` crate: too heavy, less control over errors and secrets.
- `figment`: same reasons.
