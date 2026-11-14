# ADR 0063: A wall-clock `Clock` trait in `oxbloom-core` reaches JWT time checks and log timestamps

- **Status:** Accepted
- **Date:** 2026-11-08
- **Source:** [C10a: Test support for application contexts (`oxbloom-test`)](../superpowers/specs/2026-11-08-test-support-context-design.md)

## Context

Tests need to control time, for example to check JWT expiry. The framework had no clock abstraction; the roadmap listed a fixed clock under C10, and the user decided to add one.

## Decision

`oxbloom-core` gets `Clock` (`now() -> SystemTime`) and `SystemClock`. There is no monotonic reading, because the JWKS cache uses `tokio::time::Instant`, which tests already control with paused time. The clock reaches the two production wall-clock reads: JWT `exp` and `nbf` validation, and the JSON log timestamp. With a clock set, `DefaultJwtDecoder` turns off `jsonwebtoken`'s time validation and checks both claims itself with the same arithmetic and `FailureReason`s; the checks then run after issuer and audience, so a token failing several checks may report a different reason, which is accepted. In boot a clock is an ordinary `Arc<dyn Clock>` bean; logging gets it through `logging_clock` on the boxed logging settings, because logging starts before the container.

## Consequences

Good:

- Time-dependent behavior is testable with `FixedClock` and `ManualClock`.
- Without a clock, the decoder behaves exactly as before.

Bad:

- Two code paths for JWT time checks, kept in step by a parity test.
- Other time reads stay on the real clock.
