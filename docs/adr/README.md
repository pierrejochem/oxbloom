# Architecture decision records

An architecture decision record (ADR) is a short document that captures one architectural decision: the
context that made it necessary, what was decided, and what follows from it. Oxbloom keeps its ADRs in
`docs/adr/`. The Superpowers design specs in `docs/superpowers/specs/` remain the design documents of each
slice and the history of how it was built; the ADRs are the long-term record of the decisions inside them.
Each ADR links to the spec it came from.

## When to write one

Write an ADR for every architectural decision: a choice between approaches, a dependency choice, a limit,
or a rejected alternative that a later maintainer could wonder about or want to reverse. Write it in the
same change that implements the decision. One decision per ADR; group small choices only when they were
made together and cannot be separated.

## Rules

- File names are `NNNN-short-kebab-title.md`: four digits, ascending in the order the decisions were made.
  Take the next free number.
- Never edit the decision of an accepted ADR. To change a decision, write a new ADR, say in its Context
  which ADR it replaces and why, and set the old one's status to `Superseded by ADR NNNN`. Fixing a typo
  or a broken link is allowed.
- Link the ADR to its source spec and, where useful, to the plan. Link new ADRs from the spec or plan
  that introduces them.
- Write in plain full sentences. Write "Oxbloom" in prose and `oxbloom` in code.

## Status values

| Status | Meaning |
| --- | --- |
| Proposed | Written down, not yet agreed or implemented. |
| Accepted | Agreed and in force. |
| Superseded by ADR NNNN | Replaced by a later decision; the record stays as history. |
| Deprecated | No longer relevant, without a replacement. |

## Template

Copy [`0000-template.md`](0000-template.md). The fields are `Status`, `Date` (the date of the decision,
`YYYY-MM-DD`; for decisions taken from a spec, the spec's date) and `Source`; the sections are `Context`,
`Decision`, `Consequences` (good and bad) and, when there were real alternatives, `Alternatives
considered`.

## Index

| Number | Title | Status | Date | Source spec |
| --- | --- | --- | --- | --- |
| [0001](0001-cli-exposes-only-implemented-components.md) | The CLI exposes only components that are implemented and tested | Accepted | 2026-10-23 | [2026-10-23-cli-new-design.md](../superpowers/specs/2026-10-23-cli-new-design.md) |
| [0002](0002-generate-projects-from-manifest.md) | Generated projects are rendered reproducibly from the `oxbloom.toml` manifest | Accepted | 2026-10-23 | [2026-10-23-cli-new-design.md](../superpowers/specs/2026-10-23-cli-new-design.md) |
| [0003](0003-runtime-di-container.md) | Dependency injection uses a runtime container that reports every wiring error from `build()` | Accepted | 2026-10-23 | [2026-10-23-di-design.md](../superpowers/specs/2026-10-23-di-design.md) |
| [0004](0004-singleton-arc-injection-only.md) | The first container supports only singleton `Arc<T>` injection | Superseded by [0026](0026-typed-provider-index.md) | 2026-10-23 | [2026-10-23-di-design.md](../superpowers/specs/2026-10-23-di-design.md) |
| [0005](0005-optional-crates-through-di-features.md) | Framework crates integrate with DI through optional `di` cargo features | Accepted | 2026-10-23 | [2026-10-23-di-design.md](../superpowers/specs/2026-10-23-di-design.md) |
| [0006](0006-application-toml-for-runtime-config.md) | Runtime configuration lives in `application.toml`, separate from `oxbloom.toml` | Accepted | 2026-10-24 | [2026-10-24-config-design.md](../superpowers/specs/2026-10-24-config-design.md) |
| [0007](0007-thin-config-loader-over-toml-value.md) | Configuration is loaded by an own thin layer over `toml::Value` and serde | Superseded by [0016](0016-config-node-tree-with-origins.md) | 2026-10-24 | [2026-10-24-config-design.md](../superpowers/specs/2026-10-24-config-design.md) |
| [0008](0008-env-overrides-coerced-at-bind-time.md) | Prefixed environment variables override configuration files and are coerced at bind time | Superseded by [0018](0018-config-source-pipeline.md) | 2026-10-24 | [2026-10-24-config-design.md](../superpowers/specs/2026-10-24-config-design.md) |
| [0009](0009-secret-redaction-and-value-free-errors.md) | Secrets are redacted and configuration values never appear in messages | Accepted | 2026-10-24 | [2026-10-24-config-design.md](../superpowers/specs/2026-10-24-config-design.md) |
| [0010](0010-filters-run-before-routing.md) | Filters run before routing and render their own responses | Accepted | 2026-10-24 | [2026-10-24-security-design.md](../superpowers/specs/2026-10-24-security-design.md) |
| [0011](0011-one-deny-by-default-security-chain.md) | One deny-by-default security chain per application | Accepted | 2026-10-24 | [2026-10-24-security-design.md](../superpowers/specs/2026-10-24-security-design.md) |
| [0012](0012-security-context-in-request-extensions.md) | The security context is stored in request extensions, not in a thread-local holder | Accepted | 2026-10-24 | [2026-10-24-security-design.md](../superpowers/specs/2026-10-24-security-design.md) |
| [0013](0013-jwt-via-jsonwebtoken-with-fixed-allow-list.md) | JWTs are verified with `jsonwebtoken` and an algorithm allow-list fixed at build time | Accepted | 2026-10-24 | [2026-10-24-security-design.md](../superpowers/specs/2026-10-24-security-design.md) |
| [0014](0014-jwks-cache-fails-closed.md) | JWKS keys are cached with a rate-limited refresh and fail closed after a stale window | Accepted | 2026-10-24 | [2026-10-24-security-design.md](../superpowers/specs/2026-10-24-security-design.md) |
| [0015](0015-authentication-failure-responses.md) | Authentication failures answer 401, 403 or 503 with token-free challenges | Accepted | 2026-10-24 | [2026-10-24-security-design.md](../superpowers/specs/2026-10-24-security-design.md) |
| [0016](0016-config-node-tree-with-origins.md) | Configuration is stored in an own node tree with an origin per value | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0017](0017-relaxed-canonical-keys.md) | Configuration keys match in a relaxed canonical form | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0018](0018-config-source-pipeline.md) | Configuration sources load in a fixed pipeline with imports, profile groups, environment and command line | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0019](0019-lists-from-env-and-cli.md) | Lists bind from environment and command line in comma-separated and indexed form | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0020](0020-durations-and-data-sizes-from-strings.md) | Durations and data sizes bind from strings with explicit units | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0021](0021-placeholders-in-file-values-only.md) | Placeholders resolve in file values only, against configuration keys and then environment variables | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0022](0022-config-loading-limits.md) | Configuration loading is bounded by fixed limits | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0023](0023-secret-keys-refused-from-command-line.md) | Secret keys are refused from the command line | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0024](0024-key-migration-with-collected-warnings.md) | Renamed and removed keys migrate through a builder registry with collected warnings | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0025](0025-non-exhaustive-public-errors.md) | Public error enums are `#[non_exhaustive]` before the first release | Accepted | 2026-10-24 | [2026-10-24-config-v2-design.md](../superpowers/specs/2026-10-24-config-v2-design.md) |
| [0026](0026-typed-provider-index.md) | All injectable shapes resolve through one typed provider index with explicit trait provision | Accepted | 2026-10-24 | [2026-10-24-di-v2-design.md](../superpowers/specs/2026-10-24-di-v2-design.md) |
| [0027](0027-ambiguity-requires-single-primary.md) | Several providers for one single dependency are an error unless exactly one is `primary` | Accepted | 2026-10-24 | [2026-10-24-di-v2-design.md](../superpowers/specs/2026-10-24-di-v2-design.md) |
| [0028](0028-explicit-lifecycle-and-shutdown.md) | Bean lifecycle hooks run through a trait and an explicit `shutdown` | Accepted | 2026-10-24 | [2026-10-24-di-v2-design.md](../superpowers/specs/2026-10-24-di-v2-design.md) |
| [0029](0029-lazy-beans-reentrant-container-lock.md) | Lazy bean creation is serialized by one reentrant container lock | Accepted | 2026-10-24 | [2026-10-24-di-v2-design.md](../superpowers/specs/2026-10-24-di-v2-design.md) |
| [0030](0030-dependency-depth-limit-and-cached-errors.md) | Dependency depth is capped at 256 and creation errors are cached per entry | Accepted | 2026-10-24 | [2026-10-24-di-v2-design.md](../superpowers/specs/2026-10-24-di-v2-design.md) |
| [0031](0031-di-independent-of-config.md) | `oxbloom-di` does not depend on `oxbloom-config` | Accepted | 2026-10-24 | [2026-10-24-di-v2-design.md](../superpowers/specs/2026-10-24-di-v2-design.md) |
| [0032](0032-bean-factories.md) | Bean factories exist as arity-generic builder methods and as `#[configuration]` with `#[bean]` | Accepted | 2026-10-24 | [2026-10-24-di-v2-design.md](../superpowers/specs/2026-10-24-di-v2-design.md) |
| [0033](0033-boot-entry-point-and-dependency-direction.md) | `oxbloom-boot` is the single entry point and sits above configuration and DI | Accepted | 2026-10-25 | [2026-10-25-boot-design.md](../superpowers/specs/2026-10-25-boot-design.md) |
| [0034](0034-no-global-state-in-boot.md) | Boot holds no global state | Accepted | 2026-10-25 | [2026-10-25-boot-design.md](../superpowers/specs/2026-10-25-boot-design.md) |
| [0035](0035-web-server-as-service-bean.md) | The web server is a `Service` bean that starts last and stops first | Accepted | 2026-10-25 | [2026-10-25-boot-design.md](../superpowers/specs/2026-10-25-boot-design.md) |
| [0036](0036-application-registers-web-server.md) | Applications build the `App` and register the `WebServer` bean themselves | Superseded by [0048](0048-contributor-beans.md) | 2026-10-25 | [2026-10-25-boot-design.md](../superpowers/specs/2026-10-25-boot-design.md) |
| [0037](0037-shared-shutdown-deadline.md) | All services stop under one shared shutdown deadline | Accepted | 2026-10-25 | [2026-10-25-boot-design.md](../superpowers/specs/2026-10-25-boot-design.md) |
| [0038](0038-run-listener-trait.md) | Lifecycle events go through one `RunListener` trait with no-op defaults | Accepted | 2026-10-25 | [2026-10-25-boot-design.md](../superpowers/specs/2026-10-25-boot-design.md) |
| [0039](0039-exit-status-from-generators.md) | The exit status comes from `ExitCodeGenerator` beans and `run` returns a `Result` | Accepted | 2026-10-25 | [2026-10-25-boot-design.md](../superpowers/specs/2026-10-25-boot-design.md) |
| [0040](0040-failure-analyzers.md) | Startup failures are matched against ordered `FailureAnalyzer`s and printed as a report | Accepted | 2026-10-25 | [2026-10-25-boot-failure-analysis-design.md](../superpowers/specs/2026-10-25-boot-failure-analysis-design.md) |
| [0041](0041-typed-error-sources-through-di.md) | `DiError::Creation` carries a typed error source | Accepted | 2026-10-25 | [2026-10-25-boot-failure-analysis-design.md](../superpowers/specs/2026-10-25-boot-failure-analysis-design.md) |
| [0042](0042-explicit-auto-configuration-lists.md) | Auto-configurations are discovered through explicit lists, and user registrations come first | Accepted | 2026-10-25 | [2026-10-25-conditions-autoconfig-design.md](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md) |
| [0043](0043-conditions-in-registration-order.md) | Conditions are evaluated in registration order against the registrations kept so far | Accepted | 2026-10-25 | [2026-10-25-conditions-autoconfig-design.md](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md) |
| [0044](0044-condition-groups.md) | Class-level conditions are decided once per group | Accepted | 2026-10-25 | [2026-10-25-conditions-autoconfig-design.md](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md) |
| [0045](0045-environment-trait-for-property-conditions.md) | Property conditions read configuration through an `Environment` trait | Accepted | 2026-10-25 | [2026-10-25-conditions-autoconfig-design.md](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md) |
| [0046](0046-auto-configuration-ordering.md) | Auto-configurations are ordered by a stable topological sort | Accepted | 2026-10-25 | [2026-10-25-conditions-autoconfig-design.md](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md) |
| [0047](0047-stack-probes-must-not-grow.md) | Stack probes must not grow | Accepted | 2026-10-25 | [2026-10-25-conditions-autoconfig-design.md](../superpowers/specs/2026-10-25-conditions-autoconfig-design.md) |
| [0048](0048-contributor-beans.md) | Auto-configured web and security are extended through contributor beans | Accepted | 2026-10-25 | [2026-10-25-web-security-autoconfig-design.md](../superpowers/specs/2026-10-25-web-security-autoconfig-design.md) |
| [0049](0049-runtime-property-metadata.md) | Configuration keys are described by runtime property metadata | Accepted | 2026-10-25 | [2026-10-25-web-security-autoconfig-design.md](../superpowers/specs/2026-10-25-web-security-autoconfig-design.md) |
| [0050](0050-tracing-logging-stack.md) | Logging uses `tracing` with `tracing-subscriber` and the `tracing-log` bridge | Accepted | 2026-10-25 | [2026-10-25-logging-design.md](../superpowers/specs/2026-10-25-logging-design.md) |
| [0051](0051-boot-installs-subscriber-early.md) | Boot installs the logging subscriber right after the configuration loads | Accepted | 2026-10-25 | [2026-10-25-logging-design.md](../superpowers/specs/2026-10-25-logging-design.md) |
| [0052](0052-runtime-levels-without-watcher.md) | Log levels change at runtime through a handle and an explicit `reload`, without a file watcher | Accepted | 2026-10-25 | [2026-10-25-logging-design.md](../superpowers/specs/2026-10-25-logging-design.md) |
| [0053](0053-file-rotation-and-fixed-json-fields.md) | Log files rotate by time only, and JSON output has a fixed, configurable field set | Accepted | 2026-11-01 | [2026-11-01-logging-output-design.md](../superpowers/specs/2026-11-01-logging-output-design.md) |
| [0054](0054-request-log-filter-in-web.md) | Request logging is a filter in `oxbloom-web` that redacts credential headers | Accepted | 2026-11-01 | [2026-11-01-logging-output-design.md](../superpowers/specs/2026-11-01-logging-output-design.md) |
| [0055](0055-problem-json-errors.md) | Every error answer is rendered as problem+json through one replaceable `ErrorRenderer` | Accepted | 2026-11-01 | [2026-11-01-web-errors-ordering-design.md](../superpowers/specs/2026-11-01-web-errors-ordering-design.md) |
| [0056](0056-filter-order-slots.md) | Filters are ordered by integer slots | Accepted | 2026-11-01 | [2026-11-01-web-errors-ordering-design.md](../superpowers/specs/2026-11-01-web-errors-ordering-design.md) |
| [0057](0057-trusted-proxy-forwarded-headers.md) | Forwarded headers are honored only from trusted proxies | Accepted | 2026-11-01 | [2026-11-01-web-errors-ordering-design.md](../superpowers/specs/2026-11-01-web-errors-ordering-design.md) |
| [0058](0058-json-and-negotiation.md) | JSON is offered as `Json<T>` and `Request::json`, negotiated after the handler | Accepted | 2026-11-01 | [2026-11-01-web-json-design.md](../superpowers/specs/2026-11-01-web-json-design.md) |
| [0059](0059-whole-in-memory-bodies.md) | Response bodies stay whole in memory, with size caps for compression and static files | Accepted | 2026-11-01 | [2026-11-01-web-cors-compression-static-design.md](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md) |
| [0060](0060-cors-policy.md) | CORS is off until origins are configured and never turns a disallowed origin into 403 | Accepted | 2026-11-01 | [2026-11-01-web-cors-compression-static-design.md](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md) |
| [0061](0061-compression-codecs-behind-features.md) | Each compression codec sits behind its own feature, and compression is off at runtime by default | Accepted | 2026-11-01 | [2026-11-01-web-cors-compression-static-design.md](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md) |
| [0062](0062-static-resources-canonical-paths.md) | Static resources are a controller that accepts only canonical raw paths | Accepted | 2026-11-01 | [2026-11-01-web-cors-compression-static-design.md](../superpowers/specs/2026-11-01-web-cors-compression-static-design.md) |
| [0063](0063-clock-abstraction.md) | A wall-clock `Clock` trait in `oxbloom-core` reaches JWT time checks and log timestamps | Accepted | 2026-11-08 | [2026-11-08-test-support-context-design.md](../superpowers/specs/2026-11-08-test-support-context-design.md) |
| [0064](0064-context-runner-test-crate.md) | Application contexts are tested with a hermetic `ContextRunner` in the `oxbloom-test` crate | Accepted | 2026-11-08 | [2026-11-08-test-support-context-design.md](../superpowers/specs/2026-11-08-test-support-context-design.md) |
| [0065](0065-handle-from-shares-server-dispatch.md) | In-memory test requests run the same dispatch steps as the server | Accepted | 2026-11-08 | [2026-11-08-test-support-app-design.md](../superpowers/specs/2026-11-08-test-support-app-design.md) |
| [0066](0066-one-http1-connection-per-test-client.md) | Each socket test client uses one HTTP/1.1 connection | Accepted | 2026-11-08 | [2026-11-08-test-support-app-design.md](../superpowers/specs/2026-11-08-test-support-app-design.md) |
| [0067](0067-oxbloom-test-feature-unification.md) | The feature-unification cost of the `oxbloom-test` dev-dependency is accepted | Accepted | 2026-11-08 | [2026-11-08-test-support-app-design.md](../superpowers/specs/2026-11-08-test-support-app-design.md) |
| [0068](0068-rustls-with-aws-lc-rs.md) | TLS uses rustls 0.23 with the aws-lc-rs provider | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0069](0069-tls-acceptor-without-ssl-dependency.md) | `oxbloom-web` takes a `TlsAcceptor` and does not depend on `oxbloom-ssl` | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0070](0070-polling-certificate-reload.md) | Certificates are reloaded by polling file metadata, not by a file watcher | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0071](0071-server-side-tls-scope.md) | TLS is server-side only, with ALPN `http/1.1` and no redirect | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0072](0072-certificate-validity-only-warns.md) | Certificate validity problems only warn and never stop the startup | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0073](0073-test-certificates-generated-at-test-time.md) | Test certificates and keys are generated at test time | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0074](0074-jwks-ssl-bundle-resolved-by-boot.md) | Boot resolves `security.jwt.jwks-ssl-bundle`, and `oxbloom-security` does not depend on `oxbloom-ssl` | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0075](0075-boot-tests-enable-oxbloom-test-tls.md) | Boot's `oxbloom-test` dev-dependency enables `tls`, and the unification cost is accepted | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0076](0076-web-server-new-refuses-ssl-bundle.md) | `WebServer::new` refuses a set `server.ssl.bundle` | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0077](0077-test-client-tls-server-name-localhost.md) | `TestClient::connect_tls` always uses the server name `localhost` | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0078](0078-alpn-from-bundle-server-config.md) | ALPN `http/1.1` comes from `SslBundle::server_config()`, and `serve_tls` closes other protocols | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0079](0079-shutdown-drops-pending-tls-handshakes.md) | Shutdown drops TLS handshakes that are still running | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0080](0080-zero-reload-interval-has-no-validity-repeat.md) | With `ssl.reload-interval = 0s` there is no 24 hour repeat of validity warnings | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
| [0081](0081-transport-scheme-is-default-for-missing-forwarded-proto.md) | The transport scheme is the default for a missing forwarded proto | Accepted | 2026-11-14 | [2026-11-14-ssl-bundles-design.md](../superpowers/specs/2026-11-14-ssl-bundles-design.md) |
