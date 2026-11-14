# ADR 0041: `DiError::Creation` carries a typed error source

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Startup failure analysis (slice C3b): `FailureAnalyzer` in `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-failure-analysis-design.md), [implementation plan](../superpowers/plans/2026-10-25-boot-failure-analysis.md)

## Context

Failure analyzers need the real error behind a failed bean (a `ConfigError`, a `SecurityBuildError`, a bind error). Before C3b a bean factory could only turn its error into a message string, so an analyzer would have to match message text.

## Decision

`DiError::Creation` gains a `source: ErrorSource` field. `ErrorSource` is a public newtype around an optional shared error, stored as `Option<Arc<Box<dyn Error + Send + Sync>>>`; the extra `Box` makes the `Arc` a thin pointer, so `DiError` stays at 56 bytes (a fat pointer grew it to 64 bytes and raised the stack needed for a 256-deep chain). Its `Debug` never prints the content, it is `Clone`, and its `PartialEq` is always true, so equality of `DiError` ignores the source and the derives stay. `DiError::creation_from` builds a `Creation` from any error, `From<ConfigError>` and `From<SecurityBuildError>` exist under the `di` features, and `try_bean` and `#[bean(fallible)]` keep the source when they rename the type. A size guard test pins the size of `DiError`.

## Consequences

Good:

- Analyzers recognize errors by type with `find_cause`, never by message text.
- Cached errors stay cheap to clone.

Bad:

- Adding the field is a breaking change for code outside the workspace that builds or destructures `Creation` without `..`; it was accepted and listed as a follow-up.
- Two `Creation` errors with different sources compare equal.
