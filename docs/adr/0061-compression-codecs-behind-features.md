# ADR 0061: Each compression codec sits behind its own feature, and compression is off at runtime by default

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6b-2: CORS, compression and static resources](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md)

## Context

Compression codecs pull native or large dependencies. Compression also has security implications (BREACH) and CPU cost.

## Decision

The features `gzip` (flate2), `brotli` and `zstd` enable the codecs; the default feature `compression` enables all three. At runtime compression is off until `server.compression.enabled` is true, and enabling it in a build without any codec fails the startup naming the key. The codec is chosen from `Accept-Encoding` with q-values; ties go to zstd, then brotli, then gzip. `CompressionFilter` sits at `order::COMPRESSION`, inside the body limit and outside security, and skips small bodies, non-matching types, `no-transform` and bodiless statuses. A compressed response gets `Vary: Accept-Encoding` and a weak `ETag`. The README warns about BREACH.

## Consequences

Good:

- Builds can drop codecs they do not need.
- No surprise CPU cost or BREACH exposure unless the operator turns compression on.

Bad:

- problem+json answers rendered further out are not compressed (they are small).
- Three optional dependencies to keep current.
