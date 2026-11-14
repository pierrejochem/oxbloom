# ADR 0078: ALPN `http/1.1` comes from `SslBundle::server_config()`, and `serve_tls` closes other protocols

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md), [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md), tasks 1, 4 and 5

## Context

[ADR 0071](0071-server-side-tls-scope.md) limits the server to ALPN `http/1.1`, so a client that offers only `h2` must fail the handshake. `App::serve_tls` in `oxbloom-web` takes a `tokio_rustls::TlsAcceptor` built from a `rustls::ServerConfig` that the caller owns ([ADR 0069](0069-tls-acceptor-without-ssl-dependency.md)). It cannot change the protocols that configuration offers, and rustls refuses an `h2`-only client during the handshake only when the configuration does not offer `h2`.

## Decision

The ALPN list is set where the configuration is built. `SslBundle::server_config()` in `oxbloom-ssl` sets `alpn_protocols` to `http/1.1` alone, and boot builds the server's acceptor only through it (`WebServer::with_ssl`), so rustls refuses an `h2`-only client during the handshake. `App::serve_tls` does not touch the caller's configuration. After the handshake it serves a connection that negotiated `http/1.1` or no ALPN at all, and closes one that negotiated any other protocol, logged at `debug` on `oxbloom::web` with kind `Unsupported`. The `serve_tls` documentation tells callers who build their own configuration to set `http/1.1` alone.

## Consequences

Good:

- Boot's HTTPS port follows [ADR 0071](0071-server-side-tls-scope.md) during the handshake, without `oxbloom-web` knowing about bundles.
- A caller's configuration that offers `h2` by mistake never reaches the HTTP/1.1 server with an HTTP/2 connection.

Bad:

- With such a configuration an `h2` client completes the handshake and is then closed, which is harder to diagnose than a failed handshake. The log message for this case still reads "TLS handshake failed" and has no test (a deferred minor of C7).
