# ADR 0035: The web server is a `Service` bean that starts last and stops first

- **Status:** Accepted
- **Date:** 2026-10-25
- **Source:** [Application lifecycle (slice C3a): `oxbloom-boot`](../superpowers/specs/2026-10-25-boot-design.md)

## Context

Boot manages long-running services and the web server is one of them. Treating the server as a special case in the run loop would make other services second-class and the run loop harder to test.

## Decision

The web server is a managed `Service` bean, not a special case. `Service` has async `start` and `stop` and a `phase`; services start in ascending phase and stop in reverse. `WebServer::phase()` returns `i32::MAX`, so it starts after every other service and stops before them. `run()` returns when every started service has stopped: with at least one service it waits for a shutdown signal; with none it returns after the runners, like a command-line tool. `WebServer::local_addr()` makes port 0 usable in tests.

## Consequences

Good:

- One mechanism for every long-running component.
- The same entry point serves command-line tools and servers.

Bad:

- A program that registers no service exits after its runners, which can surprise users who forget to register the server.
