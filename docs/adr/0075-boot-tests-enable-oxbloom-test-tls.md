# ADR 0075: Boot's `oxbloom-test` dev-dependency enables `tls`, and the unification cost is accepted

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md), [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md), task 6

## Context

The boot tests of `server.ssl.bundle` need an HTTPS client. `TestClient::connect_tls` and `RunningApp::client_tls` live behind the feature `tls` of `oxbloom-test`, which turns on boot's `ssl`. [ADR 0067](0067-oxbloom-test-feature-unification.md) accepted the same kind of cost for the features `web` and `security`: Cargo unifies the features of one build, so a feature that boot's dev-dependency on `oxbloom-test` asks for is on in every `cargo test -p oxbloom-boot` run, including `--no-default-features`. The alternatives were to keep a hand-written rustls client in boot's tests, or to give `oxbloom-test` the feature and pay the unification cost again.

## Decision

Boot's dev-dependency on `oxbloom-test` has the features `web`, `security` and `tls`. The condition for this choice was that no boot test count drops and the stack probes stay unchanged; both held. The boot feature matrix was re-baselined: `--no-default-features`, `--features web` and `--features security` went from 420 to 443, `--features logging` from 440 to 463, with `--features ssl` at 443 and `--all-features` at 463 (447 and 467 at the end of C7, with the JWKS tests). The probes stayed at 238 KB and 239 KB with `logging`. Feature-off coverage of boot without `ssl` comes from the compile-only builds (`cargo build -p oxbloom-boot --no-default-features`, with `logging`, `web`, `security` or `ssl` alone), which do not see dev-dependencies, and from `cargo test -p oxbloom-test --features web`, the only test run that executes boot without `ssl`. CI runs these commands. Boot keeps its own `rustls` and `tokio-rustls` dev-dependencies for a handshake-only helper that reads the served certificate, because `TestClient` does not expose it.

## Consequences

Good:

- Boot's HTTPS tests use the shared test client instead of their own HTTP code.
- The cost is written down in the roadmap, `docs/API.md` and CI, as in [ADR 0067](0067-oxbloom-test-feature-unification.md).

Bad:

- No `cargo test -p oxbloom-boot` run builds boot without `ssl`; the `#[cfg(not(feature = "ssl"))]` paths in `web.rs`, `autoconfigure/server.rs` and `autoconfigure/jwt.rs` are compiled but run only through `oxbloom-test`, and the refusal of `security.jwt.jwks-ssl-bundle` without `ssl` is not run by any test.
- `rcgen` and `aws-lc-sys` are compiled for every `oxbloom-test` test build, because `rcgen` is a plain dev-dependency there.

## Alternatives considered

- Keep a hand-written rustls client in boot's tests: no unification cost, but a second HTTP client to maintain next to `TestClient`, and the boot tests would not exercise the client that users get.
