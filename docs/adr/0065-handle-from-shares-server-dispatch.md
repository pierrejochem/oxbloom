# ADR 0065: In-memory test requests run the same dispatch steps as the server

- **Status:** Accepted
- **Date:** 2026-11-08
- **Source:** [C10b: Test support for web applications (`oxbloom-test`)](../superpowers/specs/2026-11-08-test-support-app-design.md)

## Context

`App::handle` runs only filters and router. The server path also applies the body limit, resolves the client address and forwarded headers, and reads the body. A test client built on `handle` would give answers a real server never gives.

## Decision

`App::handle_from(req, peer)` runs the server's dispatch steps (body limit, client resolution, filters, router) on a buffered request and shares code with `dispatch` instead of copying it; `handle` is unchanged. `TestClient` has one request builder over three transports (`in_memory`, `serve` on port 0, `connect` to an address) and always returns the same `TestResponse`. `TestApp` runs a whole boot application and is real-server only, because boot's `WebServer` takes the `App` on `start`; in-memory calls belong to `TestClient::in_memory`. Every `send` and `shutdown` is bounded at 10 seconds.

## Consequences

Good:

- The same request gives the same answer in memory and over a socket.
- One assertion style for every mode.

Bad:

- `TestApp` cannot make in-memory calls.
- Tests that need malformed requests or raw sockets keep their own `TcpStream`.
