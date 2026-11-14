# ADR 0040: Startup failures are matched against ordered `FailureAnalyzer`s and printed as a report

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Startup failure analysis (slice C3b): `FailureAnalyzer` in `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-failure-analysis-design.md)

## Context

A `StartupError` names a phase and the underlying error, which is accurate but terse. Spring Boot's `FailureAnalyzer` matches a failure against known causes and prints a description and an action.

## Decision

`FailureAnalyzer::analyze(&StartupError) -> Option<FailureAnalysis>` is matched in a fixed order: analyzers from the builder in call order, then `Arc<dyn FailureAnalyzer>` beans when a container exists, then the crate-private built-ins (missing config key, bean wiring, the auto-configuration cycle, port in use, security and logging configuration). The first match wins. `StartupError::find_cause::<E>()` walks the source chain (at most 64 levels). The analysis is attached before the `failed` listeners run. `run()` prints the report only with `failure_report(true)`; `run_and_exit()` prints it by default in place of the bare error line. Analyzers run inside `catch_unwind`. Analyzer text contains no configuration values, arguments or secrets; the port-in-use report is the one place that shows a socket address, by decision.

## Consequences

Good:

- The most common startup mistakes get a description and a concrete action.
- Applications can add or override analyzers.

Bad:

- Analyzers must be kept in step with the errors they match (the security analyzer uses an exhaustive `match` to force this).
- Analyzers must not emit text that imitates the report block markers; this is documented rather than enforced.
