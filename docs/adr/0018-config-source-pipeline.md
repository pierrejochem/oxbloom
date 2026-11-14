# ADR 0018: Configuration sources load in a fixed pipeline with imports, profile groups, environment and command line

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Configuration v1 ([ADR 0008](0008-env-overrides-coerced-at-bind-time.md)) had files and environment variables only. Spring Boot users expect command-line overrides, imported files, several active profiles and profile groups.

## Decision

Sources load from lowest to highest precedence: `application.toml` with its `oxbloom.config.import` files merged over it; for each active profile, expanded through profile groups in order, `application-{profile}.toml` and its imports (later profiles win); environment variables; command-line arguments (`--key=value`). Then key migration runs, then placeholders, then the tree is frozen. Active profiles come from `ConfigBuilder::profile` calls, else `--oxbloom.profiles.active=a,b`, else `OXBLOOM_PROFILE` (comma-separated), else `oxbloom.profiles.active` in `application.toml`; that key in a profile file is invalid. The framework's own keys live under the single reserved top-level table `oxbloom` (`oxbloom.config.import`, `oxbloom.profiles.active`, `oxbloom.profiles.group`), so the old unprefixed names became ordinary data. Imports are relative to the config directory, may not leave it (checked on canonicalized paths), and support an `optional:` prefix. This supersedes [ADR 0008](0008-env-overrides-coerced-at-bind-time.md).

## Consequences

Good:

- The source model matches what Spring Boot users expect.
- User keys such as `[profiles] active = true` never collide with framework keys.

Bad:

- A root struct with `deny_unknown_fields` must declare an `oxbloom` field or bind a sub-prefix, because `oxbloom.profiles.*` stays in the tree.
- The pipeline has many rules and limits ([ADR 0022](0022-config-loading-limits.md)) to document and test.
