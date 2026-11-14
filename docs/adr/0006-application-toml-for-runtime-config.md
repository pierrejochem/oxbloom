# ADR 0006: Runtime configuration lives in `application.toml`, separate from `oxbloom.toml`

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config design (Slice 4)](../superpowers/specs/2026-10-24-config-design.md)

## Context

`oxbloom.toml` already existed as the build-time component manifest of the CLI. Runtime settings (server, security, later logging) needed a home.

## Decision

Runtime configuration is read from `application.toml` plus an optional `application-{profile}.toml` overlay. `oxbloom-config` neither reads nor writes `oxbloom.toml`, which stays the CLI manifest. Binding to user types goes through `serde::Deserialize`, with no custom macro in this slice.

## Consequences

Good:

- Build-time project structure and runtime settings cannot be confused.
- The file layout follows Spring Boot conventions that users already know.

Bad:

- A project has two TOML files with similar names and different purposes.
