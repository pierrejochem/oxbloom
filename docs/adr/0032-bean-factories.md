# ADR 0032: Bean factories exist as arity-generic builder methods and as `#[configuration]` with `#[bean]`

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-di v2 design (slice C1: traits, collections, lifecycle, beans, profiles)](../superpowers/specs/2026-10-24-di-v2-design.md)

## Context

Third-party types cannot carry `#[component]`, and computed values need a function. Spring offers `@Configuration` classes with `@Bean` methods.

## Decision

Both forms exist. `.bean(f)` and `.try_bean(f)` accept functions and closures of any arity whose parameters are injectable shapes. `#[configuration]` on an inherent, non-generic `impl` turns each `#[bean]` method into one entry that resolves `Arc<Self>` and the method's parameters. A fallible bean says so with `fallible` (or `try_bean`); there is no sniffing of the return type. Every entry has a source label (`Type::method`, `component Type`, `closure at file:line`) for diagnostics.

## Consequences

Good:

- Third-party and computed values register as easily as components.
- Spring users find a familiar `@Configuration`/`@Bean` shape.

Bad:

- Arity-generic code can produce hard-to-read compiler errors; a compile-fail test pins the message for a non-injectable parameter.
- `#[bean]` on free functions and generic `#[configuration]` impls is not supported.
