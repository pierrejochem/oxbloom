# ADR 0072: Certificate validity problems only warn and never stop the startup

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md)

## Context

An expired or soon-expiring certificate is a problem operators must see. Refusing to start would turn a certificate warning into an outage, sometimes at the worst moment (a restart during an incident).

## Decision

After every successful load, each certificate's `notBefore` and `notAfter` are checked against the `Clock` bean ([ADR 0063](0063-clock-abstraction.md)). Expired, not yet valid, and expiring within `ssl.expiry-warning` (default 14 days) each produce a `warn` event on `oxbloom::ssl` at load and again every 24 hours while the condition holds. A bad date never stops the startup. `SslBundle::validity()` exposes the dates for a later health endpoint.

## Consequences

Good:

- No outage caused by the validity check itself.
- Tests control the check with `FixedClock`.

Bad:

- An application can start and serve an expired certificate; clients will reject it.
