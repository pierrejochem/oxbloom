# Module roadmap (from spring-boot/module)

Date: 2026-10-24

Source: the 130 directories under `spring-projects/spring-boot/tree/main/module` (listed with `gh api repos/spring-projects/spring-boot/contents/module`). The listing shows names only. What each module does comes from knowledge of Spring Boot, and the Rust libraries named below are candidates that each slice's design step must confirm.

Every item that is built becomes its own slice with its own spec, plan and review, as in Slices 1 to 5 and C2. Nothing here is implemented yet. It builds on the core roadmap in `docs/ROADMAP-core.md`: most of these crates need DI v2 (C1), the application lifecycle (C3) and auto-configuration (C4) first.

## How the 130 modules reduce

Spring Boot splits one feature into many modules: the feature itself, its `-test` module, its servlet or reactive variants, and one module per JSON or template library. Rust needs far fewer crates:

- **-test modules** (about 25) fold into `oxbloom-test` (slice C10) as helpers, not separate crates.
- **Servlet containers and reactive runtimes** (`tomcat`, `jetty`, `netty`, `reactor-netty`, `reactor`, `servlet`, `webflux`, `webmvc`, `web-server`) do not apply: Oxbloom runs on Hyper and Tokio, and `oxbloom-web` already covers routing and serving.
- **ORM and JPA stack** (`jpa`, `hibernate`, `persistence`, `data-jpa`, `data-commons`) has no direct Rust equivalent. The data slice below uses a SQL toolkit instead.
- **Packaging and legacy modules** (`autoconfigure-classic`, `autoconfigure-classic-modules`, `test-classic-modules`, `cloudfoundry`, `devtools`, `h2console`) do not apply or belong to tooling later.

## Mapping

| Spring Boot modules | Oxbloom plan | Status |
|---|---|---|
| `actuator`, `actuator-autoconfigure`, `health` | `oxbloom-actuator` (slice C8): health, liveness and readiness, info | done in core slice C8 |
| `micrometer-metrics`, `micrometer-observation`, `micrometer-tracing*`, `opentelemetry`, `zipkin` | `oxbloom-observability`: `tracing` spans, metrics, OpenTelemetry export | M2 |
| `http-client`, `restclient`, `webclient`, `http-converter`, `http-codec` | `oxbloom-http-client`: outbound HTTP with timeouts, retries and SSL bundles (candidate: `reqwest`) | done: M1, delivered in security Slice 9 part A on 2026-11-23 (the `HttpClient` trait, `ReqwestClient`, `RetryingClient`, `InterceptedClient`, the `http.client.*` keys, `HttpClientAutoConfiguration`, `MockHttpClient`; HTTP/2, a proxy setter and per-host clients are parked) |
| `jackson`, `jackson2`, `gson`, `jsonb`, `kotlinx-serialization-json` | covered by `serde_json` and the JSON work in slice C6; no separate crates | folded |
| `validation` | `oxbloom-validation`: a `Validate` trait, request-body validation, error mapping (shares the config validation hook from C2b) | M4 |
| `jdbc`, `r2dbc`, `data-jdbc`, `data-r2dbc`, `sql`, `transaction`, `jooq` | `oxbloom-data`: pooled SQL access, transactions and migrations (candidate: `sqlx`) | M3 |
| `jpa`, `hibernate`, `persistence`, `data-jpa`, `data-commons` | no ORM; maybe a thin repository layer on `oxbloom-data` later | skipped for now |
| `flyway`, `liquibase` | schema migrations inside `oxbloom-data` | M3 |
| `data-redis`, `session-data-redis` | `oxbloom-redis` (candidate: `redis`) | M5 |
| `cache` | `oxbloom-cache`: cache abstraction with in-memory and Redis backends | M5 |
| `mongodb`, `data-mongodb`, `cassandra`, `data-cassandra`, `couchbase`, `data-couchbase`, `elasticsearch`, `data-elasticsearch`, `neo4j`, `data-neo4j`, `ldap`, `data-ldap`, `hazelcast` | one crate per store on demand, after `oxbloom-data` and `oxbloom-redis` have settled the pattern | later, on demand |
| `amqp`, `amqp-rabbitmq`, `rabbitmq`, `kafka`, `jms`, `activemq`, `artemis`, `pulsar`, `integration` | `oxbloom-messaging` with Kafka and AMQP adapters first (candidates: `rdkafka`, `lapin`); feeds the planned `queue` and `worker` crates | M6 |
| `security`, `security-oauth2-resource-server` | done: `oxbloom-security` (Slice 5, and Slice 6a: password encoding, user details, HTTP Basic, Slice 6b: sessions with an in-memory store and a stateless encrypted cookie, Slice 6c: form login, logout, a saved request and a default login page, Slice 6d: CSRF tokens, Slice 7a: security headers and session management, Slice 7b: remember-me, and Slice 8: method security with `#[method_security]`, `#[pre_authorize]`, `#[post_authorize]`, `#[pre_filter]` and `#[post_filter]`, done on 2026-11-23, see ADR 0092 and the Slice 8 spec `2026-11-23-method-security-design.md` and plan `2026-11-23-method-security.md` (local files under `docs/superpowers`, named without a link); parked: asynchronous `custom` checks, an expression language, decision caching) | 5, 6a, 6b, 6c, 6d, 7a, 7b and 8 done |
| `security-oauth2-client`, `security-oauth2-authorization-server`, `security-saml2`, `security-test` | security Slice 9: OAuth2 and OpenID Connect login done in part B on 2026-11-24 (feature `oauth2` of `oxbloom-security` and `oxbloom-boot`, `security.oauth2.*` keys, `FakeProvider` in `oxbloom-test`; see ADR 0094 and the Slice 9 spec `2026-11-23-oauth2-design.md` and plan `2026-11-23-oauth2.md`, local files under `docs/superpowers`, named without a link; parked: RP-initiated logout, provider presets, a configurable callback path); the OAuth2 client is part C; authorization server and SAML2 later | 9 part B done, part C planned |
| `session`, `session-jdbc`, `session-data-redis` | done: `oxbloom-security` (security Slice 6b: the session filter, an in-memory store, a stateless cookie backend); CSRF came with Slice 6d; security Slice 7a added the per-user limit, `SessionRegistry` and revalidation for the in-memory store; remember-me (Slice 7b, a stateless signed cookie that works with both backends) is done; Redis and SQL stores are open | 7a and 7b done, stores planned |
| `mustache`, `freemarker`, `thymeleaf`, `groovy-templates` | `oxbloom-templates`: server-side rendering (candidates: `minijinja`, `askama`) | M7 |
| `websocket`, `rsocket` | `oxbloom-websocket` (candidate: `tokio-tungstenite`); RSocket skipped | M8 |
| `graphql` | `oxbloom-graphql` (candidate: `async-graphql`) | M8 |
| `grpc-client`, `grpc-server` | `oxbloom-grpc` (candidate: `tonic`) | M8 |
| `quartz`, `batch`, `batch-jdbc`, `batch-data-mongodb` | scheduling is `oxbloom-tasks` (cron, fixed rate, fixed delay), done in core slice C9a; `batch` becomes `oxbloom-batch` on top of `oxbloom-data` | C9a done, then M9 |
| `mail`, `sendgrid` | `oxbloom-mail` (candidate: `lettre`) | M9 |
| `hateoas`, `data-rest` | `oxbloom-hateoas` only if a hypermedia API is wanted; no auto-exposed repositories | skipped for now |
| `jersey`, `webservices` (SOAP), `restdocs` | do not apply | skipped |
| `*-test` (about 25 modules) | `oxbloom-test` helpers per feature crate | folded into C10 |

## Proposed order

Core slices C1 (DI v2), C3 (application lifecycle), C4 (auto-configuration) and C5 (logging) come first, because every crate below registers itself through them. Then, in this order:

1. **M1 `oxbloom-http-client`** (done on 2026-11-23, in security Slice 9 part A). The JWKS fetcher of `oxbloom-security` runs on it, and `http.client.ssl-bundle` makes the SSL bundles (C7) useful for outbound calls.
2. **M2 `oxbloom-observability` with the actuator (C8).** `tracing`, metrics, OpenTelemetry, health.
3. **M3 `oxbloom-data`.** Pooled SQL, transactions, migrations. The largest design decision of the list (toolkit choice, compile-time versus runtime query checking).
4. **M4 `oxbloom-validation`.** Shares the validation hook planned for config binding (C2b).
5. **M5 `oxbloom-redis` and `oxbloom-cache`.**
6. **M6 `oxbloom-messaging`.**
7. **M7 `oxbloom-templates`.**
8. **M8 `oxbloom-websocket`, `oxbloom-graphql`, `oxbloom-grpc`** as three independent slices.
9. **M9 `oxbloom-batch`, `oxbloom-mail`.**
10. Store-specific crates only on demand.

Security Slices 6a, 6b, 6c and 6d are done; Slices 7 to 9 interleave: sessions need Redis or SQL stores (after M3 or M5), OAuth2 login needed `oxbloom-http-client` (M1, done) and is done in Slice 9 part B; the OAuth2 client (part C) follows.

## What each crate follows

- Optional by default: no crate pulls its client library unless the cargo feature is on, as `oxbloom-security` does with `jwks`.
- Configuration through `oxbloom-config`: a typed `*.` prefix, secrets as `Secret<T>`, limits with documented defaults.
- Secure by default and value-free errors, with the hygiene test pattern from `oxbloom-config`.
- One doc example per public function, per `AGENT.md`.

## Open questions for the design steps

- M3: which SQL toolkit, and whether queries are checked at compile time.
- M2: whether `tracing` is the single logging and tracing facade, and how metrics are exposed (Prometheus text endpoint versus OpenTelemetry only).
- M1/M6/M8: the client libraries bring TLS back-ends; settle one TLS provider workspace-wide (`aws-lc-rs` is already pulled in by `reqwest`).
- Whether Oxbloom re-exports feature crates behind one umbrella crate with features, as Spring's starters do.
