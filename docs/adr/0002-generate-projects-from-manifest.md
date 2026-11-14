# ADR 0002: Generated projects are rendered reproducibly from the `oxbloom.toml` manifest

- **Status:** Accepted
- **Date:** 2026-10-23
- **Source:** [oxbloom-cli: `new` and `list` — Design](../superpowers/specs/2026-10-23-cli-new-design.md)

## Context

The CLI must produce a project that compiles, runs and passes its own test, identically for identical input and CLI version. A later `oxbloom add` must be able to regenerate files without the original command line.

## Decision

The generation flow builds an `oxbloom.toml` manifest from the CLI arguments, serializes it, and renders every other file from the in-memory manifest value; rendering never reads CLI arguments directly. Every table uses `#[serde(deny_unknown_fields)]`. Files are written in a fixed, sorted order, and no timestamps, user names or random values appear in generated files. The only machine-specific value is the absolute framework path, which is an explicit input (`--oxbloom-path`) and becomes a path dependency until the crates are published on crates.io. Dependency versions of the generated project are injected from the workspace `Cargo.lock` at CLI build time.

## Consequences

Good:

- The same flags and CLI version give byte-identical output, which a test checks.
- The manifest is enough to regenerate a project, which keeps `oxbloom add` possible.
- `templates` and `manifest` do no I/O and are unit-testable without a filesystem.

Bad:

- The default framework path is only valid on the machine that built the CLI; the template must switch to version dependencies once the crates are published.
- Unknown manifest fields are rejected, so manifests written by a newer CLI do not load in an older one.
