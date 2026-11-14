# ADR 0008: Prefixed environment variables override configuration files and are coerced at bind time

- **Status:** Superseded by [ADR 0018](0018-config-source-pipeline.md)
- **Date:** 2026-10-24
- **Source:** [oxbloom-config design (Slice 4)](../superpowers/specs/2026-10-24-config-design.md)

## Context

Applications need to override file settings per deployment without editing files, especially for secrets. Environment values are always strings, while target fields may be integers, floats or booleans.

## Decision

Precedence, highest first, is environment variables, then `application-{profile}.toml`, then `application.toml`. Tables merge deeply; scalars and arrays are replaced whole. A variable `{PREFIX}_{KEY}` (default prefix `APP`) maps to a key by stripping the prefix, lowercasing and splitting on `__`, so `APP_DB__MAX_CONNECTIONS` becomes `db.max_connections`. Environment values replace TOML nodes as strings, and a lenient deserializer parses them into integer, float or boolean fields at `get::<T>` time; string fields are never coerced. Lists cannot come from environment variables in this slice. The profile name must match `[A-Za-z0-9_-]+` so it cannot be used for path traversal.

## Consequences

Good:

- Deployments override settings without touching files.
- Typed fields work from string sources without a schema.

Bad:

- No command-line source, no imports, no list values from the environment; configuration v2 replaced the source model.
