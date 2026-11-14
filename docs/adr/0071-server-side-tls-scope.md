# ADR 0071: TLS is server-side only, with ALPN `http/1.1` and no redirect

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md)

## Context

TLS has a large surface: mutual TLS, HTTP to HTTPS redirects, HTTP/2 via ALPN, OCSP stapling, revocation. The first slice had to stay reviewable.

## Decision

The web server supports server-side TLS only: no mutual TLS and no `client-auth`, no HTTP to HTTPS redirect, and ALPN `http/1.1` only, so a client that offers only `h2` fails the handshake. A port with `server.ssl.bundle` speaks only HTTPS. Each handshake runs in its own task with a 10 second timeout, and a failed handshake is logged at `debug` without affecting other connections. No OCSP stapling and no revocation checks.

## Consequences

Good:

- A small, testable first TLS slice.
- The plain HTTP pipeline (limits, filters, graceful shutdown) is reused unchanged.

Bad:

- Clients that need HTTP/2 or mutual TLS are not served.
- Redirects from HTTP must come from a proxy.
