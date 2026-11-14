# ADR 0080: With `ssl.reload-interval = 0s` there is no 24 hour repeat of validity warnings

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md), [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md), task 3

## Context

[ADR 0072](0072-certificate-validity-only-warns.md) makes validity problems warn at load and again every 24 hours while the condition holds. [ADR 0070](0070-polling-certificate-reload.md) makes `0s` turn polling off. The 24 hour repeat needs something that runs over time; the reload task already wakes every `ssl.reload-interval` and checks the clock.

## Decision

The repeat runs inside the reload task: every tick checks each certificate against the `Clock` and warns when 24 hours of that clock have passed since the last warning, or when the condition changed. With `ssl.reload-interval = 0s` no task runs, so there is no repeat; only the load, `SslBundles::with_clock` and explicit `SslBundles::reload()` calls warn. The repeat comes within one reload interval of being due. No separate timer was added.

## Consequences

Good:

- One background task per set of bundles, and `0s` means that nothing runs in the background.

Bad:

- An application that turns polling off sees a validity warning only at startup or on an explicit reload. A separate timer can be added later if that proves too quiet.
- A repeat can come up to one reload interval late.
