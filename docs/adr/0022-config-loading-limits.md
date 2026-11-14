# ADR 0022: Configuration loading is bounded by fixed limits

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Imports, profile groups, placeholders and indexed keys all allow inputs that grow without bound: an import fan-out where one file is reached many ways, deep group nesting, placeholder expansion bombs, or very long key paths.

## Decision

Each dimension has a fixed limit with a fixed-text error: imports nest at most 8 deep and one load follows at most 256 import entries; profile groups nest at most 8 levels and at most 64 profiles are active after expansion; every configuration file must be a regular file of at most 1 MiB by default (`ConfigBuilder::max_file_size`), read through `take(cap + 1)`; environment and command-line key paths have at most 64 segments; placeholder depth is 32, one resolved string at most 1 MiB and one load at most 16 MiB. These limits were added in the final review of the slice.

## Consequences

Good:

- Loading always terminates and memory use is bounded, even with hostile or broken files.
- Errors carry fixed texts and never echo the offending input.

Bad:

- Legitimate but unusually large configurations hit a limit and need `max_file_size` or a restructure.
- Every limit is another rule to document and test.
