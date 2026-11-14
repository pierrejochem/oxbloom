# ADR 0077: `TestClient::connect_tls` always uses the server name `localhost`

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md), [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md), task 6

## Context

A TLS client sends a server name (SNI) and verifies the server certificate against it. `TestClient::connect_tls(addr, ca_pem)` connects to a socket address, usually `127.0.0.1` on port 0. The test certificates of Oxbloom are generated with `rcgen::CertificateParams::new(vec!["localhost"])`, which gives a DNS name and no IP address, as in the `oxbloom-web` and boot TLS tests and the doc examples.

## Decision

Every `connect_tls` connection sends and verifies the server name `localhost` (the constant `TLS_SERVER_NAME` in `oxbloom-test`'s `client.rs`), whatever the IP address of `addr` is. A test certificate must carry the DNS name `localhost`. The trust store holds only the certificates of `ca_pem`, and a `ca_pem` without a usable certificate panics at once with one fixed message that never contains the PEM text. `RunningApp::client_tls(ca_pem)` follows the same rule.

## Consequences

Good:

- One rule that every existing test certificate meets, with no guessing and no retry.
- The client API stays one function with two arguments.

Bad:

- A test with a certificate that carries only an IP address, or another host name, cannot use `connect_tls` until a name parameter is added.

## Alternatives considered

- Derive the server name from the IP address of `addr`: rejected, it fails for every existing test certificate.
- A configurable name: more API than any test needs today; it can be added later without breaking `connect_tls`.
