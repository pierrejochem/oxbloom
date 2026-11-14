# ADR 0039: The exit status comes from `ExitCodeGenerator` beans and `run` returns a `Result`

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Application lifecycle (slice C3a): `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-design.md), [implementation plan](../superpowers/plans/2026-10-25-boot.md)

## Context

A boot application may be a server or a command-line tool, and both need a meaningful process exit code. Tests need the outcome as a value, not a process exit.

## Decision

`run()` returns `Result<ExitStatus, StartupError>`, and `run_and_exit()` converts it to `std::process::ExitCode` (an error is code 1). The exit code is the maximum of all `ExitCodeGenerator` beans, or 0 when there are none. Each code is mapped before the maximum is taken: 0 to 255 stays, a negative code or one above 255 becomes 255. Generators are collected before any service starts, so a failure to create one fails early, and evaluated after the services have stopped. `StartupError` names a phase (`Config`, `Container`, `Runner`, `Service`) and wraps the value-free underlying error.

## Consequences

Good:

- Tests assert on the outcome without exiting the process.
- Several components can contribute an exit code without coordination.

Bad:

- A negative code from a generator becomes 255, which may surprise authors who expect wrap-around.
