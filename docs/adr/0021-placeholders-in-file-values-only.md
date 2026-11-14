# ADR 0021: Placeholders resolve in file values only, against configuration keys and then environment variables

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

`${name}` placeholders let one value refer to another or to an environment variable. Values from the environment and the command line can be set by less trusted parties, and expanding them would let input read other variables or secrets.

## Decision

The syntax is `${name}`, `${name:default}` and `$${` for a literal `${`. A name resolves first against the merged configuration (canonical match) and then against the environment variable of that exact name. Resolution runs once, after migration, over string leaves from files only (`application.toml`, profile files, imports, `Config::from_str` text); environment and command-line values are taken literally. A resolved leaf keeps the origin of the leaf that contained the placeholder. Depth is limited to 32, expansion is budgeted at 1 MiB per string and 16 MiB per load, and errors never contain resolved values. Configuration files are trusted input: a placeholder can read any environment variable, and this is documented.

## Consequences

Good:

- Files can compose values and pull secrets from the environment without copying them.
- Untrusted sources cannot trigger expansion.

Bad:

- A literal `${` in a file value must be written `$${`, a behavior change for existing files.
- Anyone who can edit a configuration file can read every environment variable through it.
