# Actuator

The actuator gives a web application operational endpoints on its own web server: a health report with liveness and readiness probes, build and platform information, and the log levels at run time. The crate `oxbloom-actuator` holds the types and the handlers, and the feature `actuator` of `oxbloom-boot` wires them. In an application you add the feature, write keys, and register the indicators and contributors you need.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["actuator"] }   # turns `web` on as well
```

The design is in the [C8 spec](../superpowers/specs/2026-11-14-actuator-design.md), the decisions are in [ADR 0083](../adr/0083-actuator.md), and every public item has an example in the [API reference](../API.md#412-oxbloom-actuator). The example `examples/todos` has the actuator switched on.

The group is skipped when the application registers no `dyn Controller` of its own, because then no web server would start and the actuator must not turn a run into a server. The conditions report names the reason for a skip. It is also skipped by `management.endpoints.enabled`, described below.

## Endpoints

All paths are relative to the base path, which is `/actuator` by default.

| Request | Answer |
|---|---|
| `GET /health` | The overall status of all health indicators: 200 for `UP`, 503 for anything else. |
| `GET /health/liveness` | `{"status":"UP"}` (200) or `{"status":"DOWN"}` (503), from the liveness state only. |
| `GET /health/readiness` | `{"status":"UP"}` (200) or `{"status":"DOWN"}` (503), from the readiness state and the indicators in `management.health.readiness.include`. |
| `GET /info` | The build facts, the platform and the entries of the `InfoContributor` beans. |
| `GET /loggers` | The configured log levels. |
| `GET /loggers/{target}` | The configured and the effective level of one target. |
| `POST /loggers/{target}` | Sets the level of one target. |

The loggers endpoint needs the feature `logging` of `oxbloom-boot` and a `LoggingSystem` bean. An endpoint that is switched off, or that the application cannot serve, answers 404.

A health answer without details is just the status, and the status is one of `UP`, `DOWN`, `OUT_OF_SERVICE` and `UNKNOWN`:

```text
$ curl http://127.0.0.1:8081/actuator/health
{"status":"UP"}
```

With `management.health.show-details = "always"` the answer lists every indicator by name, with its status and, when it has any, its details:

```text
{"status":"UP","components":{"ssl":{"status":"UP","details":{"bundles":["web"]}}}}
```

The overall status is the worst status of the indicators, in the order `DOWN`, `OUT_OF_SERVICE`, `UNKNOWN`, `UP`. With no indicator at all it is `UP`. With the default `never`, `/health` never shows components, details or error text.

The info answer has an object `build` with `name`, `version`, `git_sha`, `git_branch` and `build_time` (seconds since the Unix epoch, as a string), a string `platform` that is `kubernetes` when the environment variable `KUBERNETES_SERVICE_HOST` is set and `none` otherwise, and then the entries of every contributor. A build fact that is not known is left out.

```text
{"build":{"name":"todos","version":"0.1.0","git_sha":"13c3d33","git_branch":"c8-actuator","build_time":"1794000000"},"platform":"none"}
```

## Configuration keys

```toml
[management.endpoints]
enabled   = true
base-path = "/actuator"

[management.endpoints.health]
public = false

[management.endpoint.health]
enabled = true

[management.endpoint.info]
enabled = true

[management.endpoint.loggers]
enabled = true

[management.health]
show-details = "never"
timeout      = "5s"

[management.health.readiness]
include = ["db"]
```

| Key | Default | Meaning |
|---|---|---|
| `management.endpoints.enabled` | `true` | Only the value `true`, or a missing key, switches the actuator on. |
| `management.endpoints.base-path` | `/actuator` | The path the endpoints are served under. A missing leading `/` is added and a trailing `/` is dropped; `/` alone and an empty value are errors. |
| `management.endpoints.health.public` | `false` | With security, `true` permits `GET <base-path>/health/**` without authentication. |
| `management.endpoint.health.enabled` | `true` | `false` removes `/health`, `/health/liveness` and `/health/readiness`. |
| `management.endpoint.info.enabled` | `true` | `false` removes `/info`. |
| `management.endpoint.loggers.enabled` | `true` | `false` removes the three `/loggers` routes. |
| `management.health.show-details` | `never` | `never` or `always`; any other value is an error. |
| `management.health.timeout` | `5s` | The time limit of each health indicator. |
| `management.health.readiness.include` | empty | The names of the indicators that also decide readiness. |

An unknown key directly under `management`, `management.endpoints`, `management.endpoint` or `management.health` is an error that names the key and lists the valid keys, as under `ssl`. The keys are also listed by `ActuatorAutoConfiguration::properties()`.

Two behaviours of `management.endpoints.enabled` are worth knowing. The key is compared with the text `true`: any other value, for example `yes` or `false`, silently skips the whole group, with no error and no endpoint, no availability listener and no indicator. The skip shows up in the conditions report, so check it when the endpoints are missing. A key that is absent is the same as `true`.

## Liveness and readiness

Liveness answers whether the application is internally correct. It is `Correct` from the start and stays so until the application calls `Availability::break_liveness()`, which is a one-way change: only a restart repairs a broken application. `/health/liveness` reads only this state and never runs an indicator.

Readiness answers whether the application accepts traffic. It starts as `RefusingTraffic`. The actuator registers a `RunListener` that sets `AcceptingTraffic` at the `ready` event, after the services, and so the web server, have started, and sets `RefusingTraffic` again at `stopping`, right before the services are stopped. The application can also change it at any time with `Availability::set_readiness`. `/health/readiness` is `UP` when the state is `AcceptingTraffic` and every indicator named in `management.health.readiness.include` is `UP`. With the default empty list, no indicator takes part.

During a shutdown the web server stops right after `stopping`. A new connection during the drain is usually refused, so the 503 of `/health/readiness` shows on connections that are already open. Orchestrators that check readiness should treat a refused connection as not ready.

The `Availability` bean is registered unless the application registers its own. Inject `Arc<Availability>` to read or change the states.

## Health indicators

A `HealthIndicator` reports the health of one dependency. Register it as a `dyn HealthIndicator` bean; the actuator collects all of them.

```rust
use std::sync::Arc;
use oxbloom_actuator::{Health, HealthIndicator};
use oxbloom_boot::async_trait;
use oxbloom_di::ContainerBuilder;

struct Database;

#[async_trait]
impl HealthIndicator for Database {
    fn name(&self) -> &str {
        "db"
    }

    async fn health(&self) -> Health {
        Health::up().detail("database", "postgres")
    }
}

fn register(builder: ContainerBuilder) -> ContainerBuilder {
    builder
        .bean(|| Database)
        .provides(|d: Arc<Database>| -> Arc<dyn HealthIndicator> { d })
}
```

The rules for indicators:

- All indicators run at the same time, each in its own task and under `management.health.timeout`. One that exceeds the limit or panics is reported `DOWN` with the detail `error` set to `timed out` or `panicked`. It never makes the endpoint answer 500.
- An indicator must not block the thread. The timeout cannot interrupt a blocking call, and a blocked worker thread stays blocked. Use asynchronous I/O in `health`.
- Two indicators with the same name are both kept: the second gets the key `name#2`, the third `name#3`, and so on. Every one counts towards the overall status.
- An entry of `management.health.readiness.include` must name a registered indicator. A name that matches none fails the startup with an error on that key that lists the unknown names and the registered ones, so a misspelled name can never drop out of readiness unnoticed.
- Details are shown only with `show-details = "always"`. Keep secrets out of them anyway.

### The SSL indicator

With the features `ssl` and `actuator` of `oxbloom-boot` and an `SslBundles` bean, a built-in indicator named `ssl` is registered. It is `DOWN` when a certificate of any bundle has expired and `UP` otherwise. Its details list `bundles` (every bundle name), `expired` (the bundles with an expired certificate, when there is one) and `warning` (the bundles with a certificate that is not yet valid or that expires within `ssl.expiry-warning`, when there is one). It names bundles only, never a certificate subject. It reads the time from the `dyn Clock` bean when there is one.

## Application info

An `InfoContributor` adds entries to the `/info` answer. Register it as a `dyn InfoContributor` bean:

```rust
use std::sync::Arc;
use oxbloom_actuator::InfoContributor;
use oxbloom_di::ContainerBuilder;

struct Owner;

impl InfoContributor for Owner {
    fn contribute(&self, out: &mut serde_json::Map<String, serde_json::Value>) {
        out.insert("owner".into(), "platform-team".into());
    }
}

fn register(builder: ContainerBuilder) -> ContainerBuilder {
    builder
        .bean(|| Owner)
        .provides(|o: Arc<Owner>| -> Arc<dyn InfoContributor> { o })
}
```

Contributors run after the `build` and `platform` entries and in bean order, so a contributor that inserts the key `build` or `platform` replaces that entry.

### Build info

The build facts must be recorded when the application is compiled. A library cannot read the environment of the application's build, so the application does it in two steps. First, a `build.rs` next to the application's `Cargo.toml` calls the helper, with `oxbloom-actuator` as a build dependency:

```toml
# Cargo.toml
[build-dependencies]
oxbloom-actuator = "0.1"
```

```rust
// build.rs
fn main() {
    oxbloom_actuator::build::emit();
}
```

Second, the application creates a `BuildInfo` with the macro `build_info!` and registers it as a bean. The macro expands in the application's crate, which is why it sees the variables of the application's build:

```rust
use oxbloom_actuator::BuildInfo;

pub fn build_info() -> BuildInfo {
    oxbloom_actuator::build_info!()
}

// In the builder: `.bean(build_info)`
```

`emit` records the package name and version of the application, the short git sha and the git branch, and the build time in seconds since the Unix epoch. A missing `git` or a directory that is not a repository leaves the git facts out and never fails the build. On a detached checkout the branch is left out. Without a `BuildInfo` bean all facts are absent.

The build script runs again when `build.rs`, the git `HEAD`, the ref of the current branch or `packed-refs` changes. `emit` finds the git directory with `git rev-parse --absolute-git-dir`, so this works from any member of a workspace, and a new commit or a branch switch refreshes the sha. Outside a git checkout only `build.rs` is watched, and an unchanged checkout does not rebuild the application.

## Log levels

`GET {base}/loggers` answers the configured levels, in upper case, with the root level included:

```text
$ curl http://127.0.0.1:8081/actuator/loggers
{"levels":{"root":"INFO","todos":"DEBUG"}}
```

`GET {base}/loggers/{target}` answers the configured and the effective level of one target. A target that has no level of its own is not an error: `configuredLevel` is `null` and `effectiveLevel` is the level it inherits.

```text
$ curl http://127.0.0.1:8081/actuator/loggers/todos
{"configuredLevel":"DEBUG","effectiveLevel":"DEBUG"}
```

`POST {base}/loggers/{target}` sets the level and answers 204 without a body:

```text
curl -X POST -H 'content-type: application/json' \
  -d '{"configuredLevel":"DEBUG"}' \
  http://127.0.0.1:8081/actuator/loggers/todos
```

The rules:

- Targets are written with `::` as the separator, for example `oxbloom::web`. A dotted target such as `app.db` is not a valid target and is rejected with 400, by `GET` and `POST` alike. The rule is the one `LoggingSystem::validate_target` checks: letters, digits, `_`, `-` and `:` only.
- `POST` needs the header `Content-Type: application/json` (a `+json` type works too). A missing or other type is a 415 and changes nothing, so a cross-origin form or `text/plain` request cannot change a level without a CORS preflight.
- The level is one of `OFF`, `ERROR`, `WARN`, `INFO`, `DEBUG` and `TRACE`, in any case. Any other level is a 400.
- A body that is not a JSON object with a `configuredLevel` string, or that is larger than 1 KiB, is a 400. `{"configuredLevel":null}` is a 400 too: there is no way to reset a level to its inherited value in this slice.
- An empty target is a 400.
- A 400 or 415 changes nothing. The error is a problem+json answer.
- The change is the one `LoggingSystem::set_level` makes. It is not persisted: a restart returns to the configured levels.

## Security

The actuator routes go through the normal filter chain. With `oxbloom-boot/security` and the default chain, every actuator endpoint requires authentication, as every other path does.

`management.endpoints.health.public = true` opens only `GET <base-path>/health/**` (`/health`, `/health/liveness` and `/health/readiness`) and keeps `/info` and `/loggers` protected. It does that with a `SecurityCustomizer` that the actuator registers, and that customizer adds its own `authorize` section: permit the health path, authenticate every other request. A chain accepts only one `authorize` section, so an application with a customizer of its own that also calls `authorize` fails the startup with `DuplicateAuthorizeSection`. In that case leave `health.public` off and open the health path in your own customizer, as `examples/todos` does:

```rust
use oxbloom_boot::SecurityCustomizer;
use oxbloom_security::HttpSecurity;

struct PublicPaths;

impl SecurityCustomizer for PublicPaths {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| {
            a.get("/actuator/health/**")
                .permit_all()
                .any_request()
                .authenticated()
        })
    }
}
```

That pattern names the default base path. If you change `management.endpoints.base-path`, change the pattern too; until you do, the health endpoints answer 401, which fails closed.

There are no roles per endpoint yet: any authenticated caller can read `/info` and change log levels. Treat the `POST` on `/loggers` as an administrative action and put the actuator behind an issuer that you control.

## Limits

- The endpoints share the application's port. A separate management port is not available.
- Authorization is the same for every endpoint; there are no roles per endpoint.
- The levels set through `/loggers` cannot be reset by the endpoint, and `configuredLevel: null` is rejected.
- A health indicator that blocks the thread cannot be interrupted by the timeout.
- `management.endpoints.enabled` accepts only `true` as on; every other value turns the group off silently.
- There are no `/metrics`, `/env`, `/beans` or `/threaddump` endpoints and no `diskSpace` indicator.
- The git facts in `/info` can be older than the checkout after an incremental build.
- The 503 of the readiness probe during shutdown shows only on connections that are already open, because the server stops listening right after the state flips.
