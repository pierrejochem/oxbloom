# ADR 0024: Renamed and removed keys migrate through a builder registry with collected warnings

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Configuration keys change over time. Users need old keys to keep working for a while and need to be told about it, but `oxbloom-config` must not write logs itself, because logging is installed later by boot.

## Decision

The builder registers `renamed("old.key", "new.key")` and `removed("old.key", "reason")`, matched by canonical key and validated in `build()`. If the old key is present and the new one unset, the node moves and keeps its origin; if both are present, the new key wins and a warning is produced. Rules apply in registration order, so rename chains work, and migration runs before placeholders. Warnings (`ConfigWarning { key, origin, kind }`) never contain values; `Config::warnings()` returns them and printing them is the application's job. `deny_deprecated(true)` turns the first warning into a `ConfigError::Deprecated`.

## Consequences

Good:

- Key renames do not break deployments, and users learn about them.
- The configuration crate stays free of logging.

Bad:

- Warnings are silent unless the application prints them (boot later logs them as `warn` events).
