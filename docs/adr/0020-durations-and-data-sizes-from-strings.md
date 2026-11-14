# ADR 0020: Durations and data sizes bind from strings with explicit units

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-config v2 design (slice C2: binding, sources, diagnostics)](../superpowers/specs/2026-10-24-config-v2-design.md)

## Context

Timeouts and limits are easier to read as `30s` or `10MB` than as raw numbers. Rust's `std::time::Duration` deserializes from a `{ secs, nanos }` table by default, and there is no standard data-size type.

## Decision

`std::time::Duration` fields bind directly from strings such as `500ms`, `30s`, `5m`, `2h`, `1d` and fractions like `1.5s`; a bare number or TOML integer means seconds. The deserializer recognizes serde's request for `Duration` (a struct named `Duration` with fields `secs` and `nanos`), and a test pins that serde behavior; the native table form still works. A new type `DataSize` parses `512`, `10MB` or `4KiB` (integers only). `KB`, `MB`, `GB` and `TB` are 1000-based and `KiB`, `MiB`, `GiB` and `TiB` are 1024-based. This differs from Spring Boot, where `KB` is 1024, on purpose.

## Consequences

Good:

- Users keep plain `Duration` fields and still write readable values.
- Data-size units follow the SI and IEC meanings.

Bad:

- The `Duration` approach relies on serde's std implementation details; the pinning test turns a serde change into a clear failure.
- Users coming from Spring Boot may expect `KB` to mean 1024 bytes.
