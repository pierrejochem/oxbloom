# ADR 0059: Response bodies stay whole in memory, with size caps for compression and static files

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6b-2: CORS, compression and static resources](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md)

## Context

The web layer uses `Full<Bytes>` bodies and has no streaming. Compression and static files are usually built on streams.

## Decision

Bodies stay `Full<Bytes>`. Compression and static resources work on whole in-memory bodies, each with a size cap: bodies above `server.compression.max-body-bytes` (8 MiB) pass uncompressed, and static files above `server.static.max-file-bytes` (10 MiB) answer 404 with a warning. Streaming bodies, range requests and pre-compressed sibling files are out of scope for a later slice.

## Consequences

Good:

- No streaming machinery; filters can inspect and rewrite whole bodies.
- Memory and CPU per request are bounded.

Bad:

- Large downloads are not possible through static resources.
- No range requests or resumable downloads.
