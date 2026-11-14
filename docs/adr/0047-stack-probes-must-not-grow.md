# ADR 0047: Stack probes must not grow

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Conditions and auto-configuration (slice C4a)](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md), [C5a logging design](../superpowers/specs/2026-10-25-logging-design.md), [C10a test support design](../superpowers/specs/2026-11-08-test-support-context-design.md)

## Context

DI resolution is recursive ([ADR 0030](0030-dependency-depth-limit-and-cached-errors.md)), and boot's builder and run plan live on the stack. Small changes, such as a larger error type or a new builder field, raised the stack needed for a 256-deep chain in debug builds.

## Decision

Each slice re-runs manual stack probes (`OXBLOOM_DI_STACK_KB` in `crates/oxbloom-di/tests/depth.rs`, `OXBLOOM_BOOT_STACK_KB` in `crates/oxbloom-boot/tests/conditions_report.rs`) and the frame size of `ContainerBuilder::build`, and the numbers must not grow over the recorded baseline (DI 206, 277, 230 and 194 KB, boot 238 KB, 239 KB with `logging`, frame `0x178` at the end of C10b). Features that need state put it behind a box (the logging settings, the `with_config` value) instead of adding fields to `OxbloomApplicationBuilder`. The numbers are recorded in `docs/ROADMAP-core.md` after each slice.

## Consequences

Good:

- Deep dependency graphs keep working on default thread stacks.
- Stack regressions are caught when they are introduced.

Bad:

- Every slice that touches DI or boot pays for a probe run.
- Designs sometimes take a less direct shape (boxing, no new builder field) to keep the numbers.
