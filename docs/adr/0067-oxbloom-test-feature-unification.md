# ADR 0067: The feature-unification cost of the `oxbloom-test` dev-dependency is accepted

- **Status:** Accepted
- **Date:** 2026-11-08
- **Source:** [C10b: Test support for web applications (`oxbloom-test`)](../superpowers/specs/2026-11-08-test-support-app-design.md), [implementation plan](../superpowers/plans/2026-11-08-test-support-app.md)

## Context

`oxbloom-boot`'s integration tests use `TestApp`, so boot dev-depends on `oxbloom-test` with the features `web` and `security`. Cargo unifies features within one build, so those boot features are on in every `cargo test -p oxbloom-boot` run, including `--no-default-features`.

## Decision

The cost is accepted and documented instead of hidden. The boot feature-matrix counts were re-baselined (for example `--no-default-features` 298 to 420). Feature-off coverage of boot now comes from compile-only builds (`cargo build -p oxbloom-boot --no-default-features`, and with `logging`, `web` or `security` alone), which do not see dev-dependencies, and from `cargo test -p oxbloom-test --features web`, which builds boot with `web` and without `security`. CI runs exactly these commands.

## Consequences

Good:

- Boot's real-HTTP tests use the shared harness instead of their own.
- The consequence is explicit in the roadmap, `docs/API.md` and CI.

Bad:

- No `cargo test -p oxbloom-boot` run builds `web` without `security`; the `#[cfg(not(feature = "security"))]` paths are tested only through `oxbloom-test`.
- Any crate that gives its `oxbloom-test` dev-dependency features loses feature-off test runs the same way.
