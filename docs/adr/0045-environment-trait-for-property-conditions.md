# ADR 0045: Property conditions read configuration through an `Environment` trait

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Conditions and auto-configuration (slice C4a)](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md), [C4b web and security auto-configuration design](../superpowers/specs/2026-10-25-web-security-autoconfig-design.md)

## Context

Property conditions need configuration values inside `oxbloom-di`, which must stay independent of `oxbloom-config` ([ADR 0031](0031-di-independent-of-config.md)).

## Decision

`oxbloom-di` defines `trait Environment { fn property(&self, key) -> Option<String>; fn contains(&self, key) -> bool }`, and `oxbloom-config` implements it for `Config` under its `di` feature. Boot registers `Config` as the environment before the user's closures. `on_property(key)` asks `contains`, so a table such as `[security.jwt]` counts as present (amended in C4b); `on_property_value` compares scalar text exactly and case-sensitively. Two differences from Spring are documented: `on_property` matches a present key whose value is `false`, and value comparison is case-sensitive.

## Consequences

Good:

- DI stays independent of the configuration crate.
- Custom environments can be plugged in for tests.

Bad:

- Spring users may expect `false` to fail a presence condition and values to compare case-insensitively.
