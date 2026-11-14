# ADR 0001: The CLI exposes only components that are implemented and tested

- **Status:** Accepted
- **Date:** 2026-10-23
- **Source:** [oxbloom-cli: `new` and `list` — Design](../superpowers/specs/2026-10-23-cli-new-design.md)

## Context

`oxbloom new` generates projects, and the roadmap names many future components (security, database, jobs, AI, tests, a frontend). A CLI that accepted flags for them would let a user generate a project that silently lacks what was asked for.

## Decision

Only implemented and tested components are selectable. The flags `--security`, `--database`, `--jobs`, `--ai`, `--tests` and `--frontend` are parsed so that the failure is explicit: each exits with a non-zero status and the message `<feature> is not implemented yet`, and none of them writes anything into the manifest. One component table in the code drives both `oxbloom list` and this check, so the two cannot disagree.

## Consequences

Good:

- A user never gets a project that pretends to contain a feature it does not have.
- The list output and the flag validation share one table and stay consistent.

Bad:

- Each new component needs a CLI change before it becomes selectable.
- The CLI surface looks smaller than the roadmap until the components land.
