# ADR 0079: Shutdown drops TLS handshakes that are still running

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C7: SSL bundles (`oxbloom-ssl`) design](../superpowers/specs/2026-11-14-ssl-bundles-design.md), [C7 plan](../superpowers/plans/2026-11-14-ssl-bundles.md), task 4

## Context

`App::serve_tls` handshakes each accepted connection in its own task, with a 10 second timeout, so a slow client never blocks accepting ([ADR 0071](0071-server-side-tls-scope.md)). Graceful shutdown uses hyper-util's `GracefulShutdown`, whose `Watcher` is taken when the connection is accepted and does not expose its signal. A task still waiting for a handshake holds its watcher, so an idle client that connected and never sent a ClientHello would hold the shutdown open for up to the handshake timeout or `shutdown_timeout`, and the server would log a misleading timeout warning.

## Decision

The accept loop owns a `tokio::sync::watch` channel (`stopping` and `stop`). Each connection task runs its handshake in a `select!` with `stop.changed()`. When shutdown starts, the loop sends on the channel before it calls `graceful.shutdown()`. A task still in its handshake drops its stream and its watcher; it has no request in flight. A handshake that finishes just after the signal is still watched by the watcher taken at accept time, so hyper shuts that connection down at once. Established TLS connections drain exactly like plain ones, under the same `shutdown_timeout`. The `select!` is `biased` with the handshake first, so plain `serve`, whose open step is always ready, keeps its old order.

## Consequences

Good:

- A client that never finishes its handshake cannot delay shutdown, and no false timeout warning is logged.
- Plain and TLS connections share one pipeline and one shutdown sequence.

Bad:

- A client whose handshake is in progress when shutdown starts gets its connection closed without an answer, even if it would have finished within the timeout.
