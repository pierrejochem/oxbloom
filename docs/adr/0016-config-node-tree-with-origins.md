# ADR 0016: Configuration is stored in an own node tree with an origin per value

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Configuration v2 adds relaxed keys, more sources and origin-aware errors. The v1 storage, a thin layer over `toml::Value` ([ADR 0007](0007-thin-config-loader-over-toml-value.md)), carries no information about where a value came from, so an error could not say "from env APP_SERVER__PORT".

## Decision

An own value tree (`Node`) replaces `toml::Value` inside `Config`. Every node is a table, an array or a scalar and carries an `Origin`: `File { path, line }`, `Inline { line }`, `Env { name }` or `Cli { arg }`. `Origin` displays as `application-dev.toml:4`, `env APP_SERVER__PORT` or `--server.port` and never contains a value. Table entries are keyed by canonical key ([ADR 0017](0017-relaxed-canonical-keys.md)) and also store the spelling of the winning source. The public API of `Config` (`get`, `from_str`, `builder`) keeps its signatures. This supersedes [ADR 0007](0007-thin-config-loader-over-toml-value.md); the rejection of `config` and `figment` stands, now also because they cannot give relaxed keys, Oxbloom's placeholder rules or secret-safe errors.

## Consequences

Good:

- Errors, `Config::origin` and `Config::explain` can say where each value came from without revealing it.
- Merge, normalization and placeholders all work on one structure.

Bad:

- The merge logic and the lenient deserializer had to be rewritten over `Node`, the riskiest part of the slice.

## Alternatives considered

- A side table of origins next to `toml::Value`: two structures to keep in sync.
- The `figment` or `config` crates: cannot give relaxed keys, the placeholder rules or secret-safe errors.
