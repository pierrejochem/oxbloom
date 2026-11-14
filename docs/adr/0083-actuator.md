# ADR 0083: The actuator runs on the application's server, with its availability state in the actuator crate, its listener in boot, and its build facts from the application's `build.rs`

- **Status:** Accepted
- **Date:** 2026-11-14
- **Source:** [C8: Availability, info and health (`oxbloom-actuator`)](../superpowers/specs/2026-11-14-actuator-design.md) and the [C8 plan](../superpowers/plans/2026-11-14-actuator.md)

## Context

A web application needs operational endpoints: liveness and readiness probes for an orchestrator, an aggregate health report, build information and the log levels at run time. Several decisions shape them. Where do the endpoints live? Who owns the availability state, and who changes it? Where do the build facts come from? What does a health check do when a dependency is slow or panics? What does the configuration do with an indicator name that does not exist?

The crate dependency direction is fixed by [ADR 0033](0033-boot-entry-point-and-dependency-direction.md): `oxbloom-boot` sits above the other crates. The actuator is a new crate that boot wires through a cargo feature and an auto-configuration, as it does for logging, security and SSL ([ADR 0048](0048-contributor-beans.md)). Application code must be able to read and change the availability state, so the state cannot belong to boot.

## Decision

**Placement.** The endpoints are routes of the application's own web server, served by one `ActuatorController` (a `dyn Controller`) under the base path `management.endpoints.base-path`, default `/actuator`. They pass through the normal filter chain, so with security on they require authentication unless the application opens them. A separate management port is out of scope and is a follow-up.

**Availability model.** The state lives in `oxbloom_actuator::Availability`, a bean that holds a `Liveness` and a `Readiness` in atomics. The listener that changes it lives in `oxbloom-boot`, because the actuator crate cannot depend on boot (ADR 0033): a `RunListener` bean sets readiness to `AcceptingTraffic` at `ready`, after the services have started, and to `RefusingTraffic` at `stopping`. The listener has the lowest `order`, so it runs before the other `RunListener` beans, and builder listeners still run first. The state is `RefusingTraffic` until `ready`. Liveness only goes from `Correct` to `Broken`, through `Availability::break_liveness()`.

**Build info through the application.** `oxbloom_actuator::build::emit()` is called from the application's own `build.rs` and prints `cargo:rustc-env=` lines. The macro `build_info!()`, exported by the actuator crate, expands in the application's crate and reads them with `option_env!`. A library that called `option_env!` itself would see the environment of its own build, not the application's, so a function in the library cannot do this. A missing `git` leaves the git facts out and never fails the build. `rerun-if-changed` follows `build.rs` and `.git/HEAD`, relative to the application's package directory, so the git sha and branch of an incremental build can be stale.

**Down on timeout or panic.** A `HealthRunner` runs every `HealthIndicator` in its own task under `management.health.timeout` (default 5 s). An indicator that exceeds the limit or panics is reported `DOWN` with an `error` detail of `timed out` or `panicked`, and `/health` answers 503 as for any other `DOWN`. It never answers 500. A probe that cannot reach a dependency is a health result, not a failure of the endpoint, and an orchestrator treats the 503 the right way. A blocking indicator cannot be interrupted by the timeout; the rule that indicators must not block the thread is documented instead.

**Fail-closed readiness include.** Every entry of `management.health.readiness.include` must name a registered `dyn HealthIndicator`. A name that matches none fails the startup with a `ConfigError` on that key that lists the unknown and the registered names. Ignoring a misspelled name would take that dependency out of the readiness check without a trace, and an application could be reported ready while its database is down.

**Further rulings of the slice.**

- `management.endpoints.enabled` is a condition on the text `true` (`on_property_value_or_missing`). The DI conditions have no "not equal", so every other value skips the group, silently, and the conditions report names the skip. The alternative of reading any value as on fails open.
- The group is skipped when the application registers no `dyn Controller` (`on_bean`), the condition `WebServerAutoConfiguration` uses. Otherwise the actuator alone would turn a run without a web application into a server.
- `management.endpoints.health.public = true` registers a `SecurityCustomizer` that permits `GET <base-path>/health/**` and requires authentication for everything else. It adds its own `authorize` section, so an application customizer that also calls `authorize` fails with `DuplicateAuthorizeSection`; such an application opens the health path in its own customizer, as `examples/todos` does.
- Log targets of `/loggers` use `::` as the separator, as the logging system does; a dotted target is a 400. `configuredLevel: null` is a 400, not a reset.

## Consequences

Good:

- No second server, port or security configuration; the existing chain protects the endpoints.
- The application and its tests can read and flip the availability state without depending on boot.
- The actuator crate stays below boot in the dependency graph.
- A slow or broken dependency produces a clear `DOWN` instead of a hanging or failing probe.
- A typo in the readiness include list stops the startup instead of weakening readiness.
- The build facts are the application's own, with no runtime lookup of git.

Bad:

- The endpoints share the port of the application. Network policy cannot separate them from application traffic, and an application that wants them private needs a proxy rule.
- The application needs a `build.rs` and a `build_info` bean to get build facts, and the git facts can be stale after an incremental build.
- A value of `management.endpoints.enabled` other than `true` disables the actuator without an error.
- The timeout cannot stop an indicator that blocks the thread.
- During a shutdown the web server stops right after the readiness state flips, so a new connection is usually refused and the 503 shows only on open connections.
- An application with its own `authorize` section cannot use `health.public`.

## Alternatives considered

- A separate management port: deferred, because it needs a second server with its own security chain and shutdown order.
- The availability state in `oxbloom-boot`: rejected, because application code and the actuator crate would then depend on boot.
- A function `BuildInfo::from_env` in the library: rejected, because `option_env!` in a library reads the library's build environment.
- A 500 answer for an indicator that times out or panics: rejected, because it hides which dependency failed and makes the endpoint itself look broken.
- Ignoring unknown readiness names: rejected because it fails open.
