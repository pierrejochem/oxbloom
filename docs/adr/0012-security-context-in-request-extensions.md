# ADR 0012: The security context is stored in request extensions, not in a thread-local holder

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-security design (Slice 5)](../superpowers/specs/2026-10-24-security-design.md)

## Context

Spring stores the current authentication in `SecurityContextHolder`, a thread-local. In async Rust a task moves between threads across `.await`, so a thread-local would be wrong or unsafe.

## Decision

`SecurityContext` wraps `Option<Authentication>` and is stored in the request extensions. An extension trait in `oxbloom-security` gives `Request::authentication()`. `Authentication` is always authenticated; an unauthenticated input is a different type (`AuthenticationRequest`). `Debug` for `Authentication` and `SecurityContext` is hand-written and prints the principal and authority names, never claim values.

## Consequences

Good:

- Correct across `.await` points and tasks.
- Handlers read the authentication from the request they already hold.

Bad:

- Code that does not have the request cannot read the authentication; it must be passed explicitly.
