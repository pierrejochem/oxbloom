# ADR 0068: TLS uses rustls 0.23 with the aws-lc-rs provider

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md)

## Context

Oxbloom had no TLS. HTTPS and custom JWKS trust need a TLS stack, and `reqwest` (used by the JWKS fetcher) already pulls in rustls.

## Decision

TLS uses rustls 0.23 with the `aws-lc-rs` crypto provider, the same one `reqwest` already uses, and `tokio-rustls` for the server. Bundles support PEM (files and inline) and PKCS#12 through a pure-Rust reader; JKS is not supported.

## Consequences

Good:

- One TLS stack and one crypto provider in the dependency tree.
- No OpenSSL dependency and no own cryptography.

Bad:

- `aws-lc-rs` needs a C toolchain to build.
- Users with JKS keystores must convert them.
