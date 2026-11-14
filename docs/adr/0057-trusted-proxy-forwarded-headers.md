# ADR 0057: Forwarded headers are honored only from trusted proxies

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6a: Filter ordering, uniform errors and `server.*` properties](../superpowers/specs/2026-11-01-web-errors-ordering-design.md)

## Context

Behind a reverse proxy the peer address is the proxy's. `Forwarded` and `X-Forwarded-*` headers carry the client address, scheme and host, but any client can send them.

## Decision

`server.forward-headers.strategy` is `none` by default. With `framework`, forwarded values apply only when the peer address is in `server.forward-headers.trusted-proxies` (default loopback only). `Forwarded` (RFC 7239) is read first, else `X-Forwarded-For`, `-Proto` and `-Host`. `X-Forwarded-For` is walked from right to left and stops at the first untrusted hop, which is the client; a malformed value makes the whole header unusable and the peer stays the client. The CIDR parser is written in the crate and adds no dependency.

## Consequences

Good:

- A client cannot spoof its address by sending headers directly.
- Spoofed leftmost entries behind a trusted proxy are ignored.

Bad:

- Deployments behind proxies must configure the strategy and the trusted list explicitly.
- An own CIDR parser must be maintained and tested.
