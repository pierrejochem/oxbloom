# ADR 0081: The transport scheme is the default for a missing forwarded proto

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md)

## Context

With `App::serve_tls` the server terminates TLS itself, and the spec says `ClientInfo` reports the scheme `https`. The forwarded-headers resolution (`resolve_client` in `crates/oxbloom-web/src/forwarded.rs`) used the scheme of the connection only when it ignored the headers. Once the TCP peer was trusted, a missing `proto` meant `Http`, also when the request carried no forwarded header at all. With the strategy `framework` and the default loopback trusted list, every HTTPS request from `127.0.0.1` reported `Http`, which breaks redirects, absolute links and any security decision built on the scheme.

## Decision

The scheme of the connection (`Https` when the server terminated TLS, else `Http`) is the default whenever no `proto` applies: when the forwarded headers are not used, when a trusted peer sends no forwarded header, and when the client's `Forwarded` element or `X-Forwarded-Proto` has no value. A `proto` from a trusted proxy still wins, because it describes the client's connection to the first proxy: `https` gives `Https`, any other value `Http`. Plain HTTP behaviour does not change, since its transport scheme is `Http`.

## Consequences

Good:

- A request over TLS never reports `Http` unless a trusted proxy said so.
- Plain HTTP servers behave exactly as before.

Bad:

- A trusted proxy that terminates TLS and talks TLS to Oxbloom again, but does not send a `proto`, makes the request report `Https` even if the client used plain HTTP to the proxy. Such a proxy must send `X-Forwarded-Proto` or `Forwarded: proto=`, which the deployment requirement already asks for.

## Alternatives considered

- A missing `proto` from a trusted peer always means `Http` (the earlier behaviour): rejected, it reports `Http` for HTTPS requests from local clients and from proxies that forward only the address.
