# ADR 0070: Certificates are reloaded by polling file metadata, not by a file watcher

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md)

## Context

Certificates rotate (for example Kubernetes secret mounts that swap a symlink). A file-system watcher is platform-specific and misses symlink swaps in some setups; a restart for every rotation is not acceptable.

## Decision

`SslBundles` runs one background task that wakes every `ssl.reload-interval` (`0s` turns it off) and compares the size and modification time of every file a bundle uses, following symlinks. A changed bundle is rebuilt from all its sources and swapped in behind an atomically swapped pointer in a reloadable `ResolvesServerCert`, so a reload never blocks a handshake and new connections get the new certificate. Metadata is read before and after the files; if it differs the attempt is dropped (torn write). A failed reload keeps the old material and warns once until the files change again. The task stops through the container's `Lifecycle` shutdown hook.

## Consequences

Good:

- Portable, simple, and it detects symlink swaps.
- A bad file never replaces a working certificate.

Bad:

- A change applies only within one reload interval.
- Inline PEM and passwords are not watched, and the JWKS client does not reload.

## Alternatives considered

- A file-system watcher: rejected.
