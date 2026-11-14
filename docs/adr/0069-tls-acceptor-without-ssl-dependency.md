# ADR 0069: `oxbloom-web` takes a `TlsAcceptor` and does not depend on `oxbloom-ssl`

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md)

## Context

SSL bundles are loaded from configuration and reloaded, which needs `oxbloom-config` and a background task. The web layer must stay usable without that machinery.

## Decision

A new crate `oxbloom-ssl` builds rustls configurations from `ssl.bundle.*` keys and depends on neither `oxbloom-web` nor `oxbloom-security`. `oxbloom-web` gets a `tls` feature (off by default) with `App::serve_tls(listener, acceptor, shutdown)` taking a `tokio_rustls::TlsAcceptor`. `oxbloom-security` gets `ReqwestJwksFetcher::with_tls(client_config)`. Boot's `ssl` feature wires them together through `SslAutoConfiguration` and `server.ssl.bundle`.

## Consequences

Good:

- Each crate stays usable alone; the web layer has no configuration dependency.
- Users can serve TLS with their own rustls configuration.

Bad:

- The wiring lives in boot, which gains another feature.
