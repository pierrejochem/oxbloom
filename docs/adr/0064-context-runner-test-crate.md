# ADR 0064: Application contexts are tested with a hermetic `ContextRunner` in the `oxbloom-test` crate

- **Status:** Accepted
- **Date:** 2026-11-08
- **Source:** [C10a: Test support for application contexts (`oxbloom-test`)](../superpowers/specs/2026-11-08-test-support-context-design.md)

## Context

Each test suite rebuilt its own harness (`Capture`, `SharedBuf`, `serve()`). Tests must not depend on the developer's environment and must never hang.

## Decision

A new crate `oxbloom-test`, used as a dev-dependency, offers `ContextRunner`, which runs the real boot lifecycle to the ready event, lets the test inspect beans, configuration, conditions and captured output, and shuts down. Defaults are hermetic: `env_from([])`, banner and logging off, output captured, `server.port = 0` as a command-line style argument, failure report on. `property` is a command-line style override, so secrets are refused there as in production. A whole run is bounded at 10 seconds; boot's stop deadline is set to 5 seconds and a shutdown-failure line in the output fails the test (a text match on boot's wording, pinned by tests). Cargo allows the `oxbloom-boot` to `oxbloom-test` dev-dependency cycle only for integration tests under `tests/`, so in-`src` unit tests keep local helpers. Migration moves helpers and never rewrites assertions; the test count must not drop.

## Consequences

Good:

- One harness for every suite, and tests are isolated from the process environment.
- No test can hang forever.

Bad:

- Signal handling cannot be turned off in tests, because that would need a new builder field.
- The shutdown guard can give a false positive when a user service prints boot's wording.
