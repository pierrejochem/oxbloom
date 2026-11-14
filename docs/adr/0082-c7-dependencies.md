# ADR 0082: C7 dependencies

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md) and the [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md)

## Context

`oxbloom-ssl` needs a PKCS#12 reader, a way to read certificate validity dates, a pointer that a reload can swap without blocking a handshake, a verifier for clients without a trust store, and test certificates. ADR 0068 fixes rustls with `aws-lc-rs` as the TLS stack; the spec asks for a pure-Rust PKCS#12 reader, no OpenSSL and no own cryptography. AGENT.md asks for few dependencies, pinned in one place. Versions younger than two weeks are not taken.

## Decision

`oxbloom-ssl` adds these dependencies, besides `rustls` and `rustls-pki-types` (ADR 0068):

- `p12-keystore` 0.4.1, with default features off and only `pbes1` on. It is pure Rust, and its `Pkcs12Archive` API hands over the key bags and certificate bags, so Oxbloom picks the leaf and the trust store itself. It pulls in RustCrypto pre-release crates (`cms 0.3.0-rc.2`, `pkcs12 0.2.0-rc.0`), the only versions upstream publishes; `Cargo.lock` pins them, and an update is applied by hand with the full gate. It also brings the RustCrypto primitives `aes`, `cbc`, `sha1`, `sha2`, `hmac`, `pbkdf2` and `scrypt` for PBES2 and the MAC. The `pbes1` feature adds `des` and `rc2`, so older files with 3DES or RC2 key bags can be read; only files written by the reader's own writer are covered by tests.
- `x509-parser` 0.18.1, with default features off, to read the subject and the validity dates of each certificate for `SslBundle::validity()` and the validity warnings. It was already in the lock through `p12-keystore`.
- `arc-swap` 1.9.3 (pinned with `cargo update --precise`, because 1.9.4 and 1.9.5 were less than two weeks old). It holds the material behind an atomically swapped pointer, so a reload never blocks a handshake (ADR 0070).
- `rustls-platform-verifier` 0.7.1, for `SslBundle::client_config()` on a bundle without a trust store. It was already in the lock through `reqwest` and pulls in no `ring`.
- `rcgen` 0.14.10, as a dev-dependency only, with default features off and `aws_lc_rs` and `pem` on, to generate every test certificate at test time (ADR 0073) without `ring` in the dev tree.

## Consequences

Good:

- No OpenSSL, no `ring`, and no own cryptography.
- The leaf and trust selection of a PKCS#12 file is Oxbloom's own code, tested on its own.

Bad:

- Pre-release RustCrypto crates sit in the tree until upstream releases them.
- The PKCS#12 reader adds a second set of cryptographic primitives next to `aws-lc-rs` (ADR 0068).
- `pbes1` costs the crates `des` and `rc2` for a format no test covers; dropping it would keep PBES2 files working.

## Alternatives considered

- Leaving `pbes1` off for a smaller tree: rejected for now, because older OpenSSL versions and Windows exports use 3DES or RC2; it can be dropped if a minimal tree matters more.
- A PKCS#12 reader bound to OpenSSL: rejected by the spec (no OpenSSL dependency, ADR 0068).
