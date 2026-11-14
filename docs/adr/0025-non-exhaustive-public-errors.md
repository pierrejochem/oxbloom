# ADR 0025: Public error enums are `#[non_exhaustive]` before the first release

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md), [DI v2 design](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

Error enums such as `ConfigError` and `DiError` gained variants slice by slice. Every new variant broke exhaustive matches in user code.

## Decision

`ConfigError`, `Origin` and `WarningKind` became `#[non_exhaustive]` in the final review of configuration v2; `DiError` became `#[non_exhaustive]` in DI v2, and later types that may grow (`StartupError`, `StartupPhase`, `LoggingError`, `FailureAnalysis`, `SslError`) follow the same rule. Matches outside the crate need a `_` arm, and struct-variant patterns need `..`.

## Consequences

Good:

- Variants can be added without a breaking change after the first release.

Bad:

- Users cannot match exhaustively, so the compiler does not tell them about new variants.
- Fields of an existing variant are not covered: adding a field to `DiError::Creation` was still a breaking change for literal construction ([ADR 0041](0041-typed-error-sources-through-di.md)).
