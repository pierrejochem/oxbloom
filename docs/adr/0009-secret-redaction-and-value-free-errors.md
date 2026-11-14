# ADR 0009: Secrets are redacted and configuration values never appear in messages

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config design (Slice 4)](../superpowers/specs/2026-10-24-config-design.md)

## Context

Configuration holds passwords, keys and tokens. Error messages, `Debug` output and logs are copied into tickets and log systems, so any value they print may leak.

## Decision

`Secret<T>` wraps a value; its `Debug` and `Display` print `[REDACTED]`, and `expose()` is the only way to read it. `ConfigError` messages name the key and the file path and never contain a configuration value, including `Parse` errors, which drop the TOML parser's snippet of the offending line. A test places a marker secret in the configuration and checks that it appears in no error's `Display` or `Debug`. The rule became project-wide: later crates (security, boot, DI, logging, web, SSL) follow it with fixed error texts, hand-written `Debug` implementations and hygiene tests.

## Consequences

Good:

- A failing startup or a bug report does not leak secrets.
- Messages still say which key to fix, which is what a developer needs.

Bad:

- Debugging a wrong value is harder, because the message never shows what was read.
- Every new error type needs a hygiene test and careful formatting.
