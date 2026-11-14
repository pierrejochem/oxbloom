# ADR 0011: One deny-by-default security chain per application

- **Status:** Accepted
- **Date:** 2026-10-24
- **Source:** [oxbloom-security design (Slice 5)](../superpowers/specs/2026-10-24-security-design.md)

## Context

Spring Security allows several filter chains with different matchers, and authorization rules that do not match anything fall through. Both are sources of accidentally unprotected endpoints.

## Decision

There is one `SecurityFilterChain` per application; several chains with different matchers are a deliberate YAGNI cut. Rules are evaluated in declaration order, the first matching matcher wins, and if no matcher matches the request is denied. `HttpSecurity::build()` refuses configurations that would be unsafe or misleading: no `authorize` section, a matcher after `any_request()`, a second `authorize` call, and patterns with `{`, `}`, `?` or partial wildcards, because such a rule would never match and a permissive rule after it would then apply. `HasRole("ADMIN")` checks the authority `ROLE_ADMIN`, as in Spring. A `HEAD` request is authorized as `GET`, because the router runs the GET handler for HEAD.

## Consequences

Good:

- An unprotected chain cannot be built by accident.
- A mistyped pattern fails the startup instead of opening a route.

Bad:

- Applications that need different policies per path prefix must express them as rules in one chain.
- Only Ant-style whole-segment wildcards are supported; finer matching belongs in handlers.

## Alternatives considered

- Several chains with matchers (Spring): deferred as YAGNI.
