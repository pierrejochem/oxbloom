# ADR 0073: Test certificates and keys are generated at test time

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md)

## Context

TLS tests need certificates and private keys. Committed key files leak into the repository history, trip secret scanners, and expire.

## Decision

Test certificates and keys are generated at test time with `rcgen`. No `.pem` file is committed. Validity tests use certificates with explicit dates together with `FixedClock`.

## Consequences

Good:

- No key material in the repository.
- Tests never break because a committed certificate expired.

Bad:

- Tests depend on `rcgen` as a dev-dependency and spend some time generating keys.
