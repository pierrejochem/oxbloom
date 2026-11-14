# ADR 0019: Lists bind from environment and command line in comma-separated and indexed form

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Environment variables and command-line arguments are flat strings, but many settings are lists (JWT algorithms, audiences, CORS origins). Configuration v1 could not set lists from the environment.

## Decision

Both forms are supported. Indexed: a numeric path segment creates an array element (`APP_X__ALGORITHMS__0=RS256`, `--x.algorithms[0]=RS256`); indices must start at 0 without gaps. Comma-separated: a string leaf bound to a sequence is split on commas and trimmed, with `\,` as a literal comma; an empty string is an empty list; this applies only when the target is a sequence. A higher-precedence list replaces a lower one as a whole, never element by element. As a consequence, numeric map keys (`APP_HTTP__STATUS__404`) cannot come from the environment or the command line, only from files.

## Consequences

Good:

- Lists work from every source, and `oxbloom-security` could drop its limitation on `algorithms` and `audience`.
- Replacement as a whole keeps precedence predictable.

Bad:

- A numeric environment segment that used to set a map key is now an index and fails as a gap error, a documented behavior change.
- The backslash escape rules are subtle and need careful documentation.
