# ADR 0062: Static resources are a controller that accepts only canonical raw paths

- **Status:** Accepted
- **Date:** 2026-11-01
- **Source:** [C6b-2: CORS, compression and static resources](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md)

## Context

Static files are a classic path-traversal target. The security chain matches the raw request path, so if the static handler decoded paths more leniently, an encoded path could reach a file that a security rule meant to protect.

## Decision

`StaticResources` implements `Controller`, so it sits behind the security chain like any route. The path after the prefix must already be in canonical form: every byte has exactly one spelling, either raw (visible ASCII except a small set) or as an upper-case percent escape (non-ASCII bytes, that set and `%`); the other spelling, lower-case escapes, empty or `.` segments, `..`, NUL, backslashes, encoded `/`, dotfiles and symlinks that leave the canonical root answer 404. An index file is the one exception and is also served at its directory's path, so directories are protected with `/dir/**`. Sources are `DiskResources` (`tokio::fs`) and `EmbeddedResources` (a static list, no macro). ETags use a fixed FNV-1a 64 hash, because the std hasher is not stable across releases.

## Consequences

Good:

- A path rule in `oxbloom-security` cannot be bypassed by encoding.
- No new dependency for content types, hashing or path handling.

Bad:

- Some unusual but valid URLs answer 404.
- Directory index paths need the `/dir/**` pattern in security rules.
