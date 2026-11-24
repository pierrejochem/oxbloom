# Oxbloom API reference

One example per public function. Kept in sync by the coding agent (see `AGENT.md`).
Only what is implemented and tested is documented as available. Anything else is listed under
[Not implemented](#not-implemented). Do not describe planned features as done.

## 1. Workspace map

| Crate | Purpose | Key public items |
|---|---|---|
| `oxbloom-core` | Shared types | `HttpMethod`, `InvalidHttpMethod`, `Clock`, `SystemClock` |
| `oxbloom-macros` | Procedural macros | `#[controller]`, `#[get]`, `#[post]`, `#[put]`, `#[patch]`, `#[delete]`, `#[head]`, `#[options]`, `#[component]`, `#[configuration]`, `#[bean]` (with condition options), `#[auto_configuration]` (used through `oxbloom-boot`), `#[scheduling]`, `#[scheduled]` and `#[retryable]` (used through `oxbloom-tasks`) |
| `oxbloom-web` | Router, request/response, Hyper server, filters | `App`, `Router`, `RouterConfig`, `Route`, `Request`, `Response`, `IntoResponse`, `Controller`, `RouteError`, `Filter`, `Next`, `Ordered`, `order` (with `order::CORS`, `order::NEGOTIATION` and `order::COMPRESSION`), `RequestLogFilter`, `CorsConfig`, `CorsFilter`, `CorsError`, `CompressionConfig`, `CompressionLevel`, `CompressionFilter`, `codecs`, `StaticResources`, `ResourceSource`, `DiskResources`, `EmbeddedResources`, `Resource`, `ResourceError`, `Json` and `JsonBody` (feature `json`), `HttpError`, `ErrorValue`, `ErrorAttributes`, `RequestInfo`, `ErrorRenderer`, `ProblemRenderer`, `ClientInfo`, `Scheme`, `ForwardedConfig`, `ForwardStrategy`, `TrustedProxies`, `ProxyParseError`, `App::serve_tls` (feature `tls`) |
| `oxbloom-di` | Singleton dependency injection: traits, collections, lifecycle, lazy beans, bean factories, profiles, conditions and groups | `ContainerBuilder`, `Container`, `Resolver`, `Component`, `Configuration`, `Lifecycle`, `ShutdownReport`, `DestroyFailure`, `Inject`, `BeanFn`, `TryBeanFn`, `DiError`, `ErrorSource`, `Environment`, `ConditionContext`, `ConditionOutcome`, `Conditions`, `ConditionReport`, `ConditionRecord` |
| `oxbloom-config` | Typed config: TOML + imports, profiles, env and CLI sources, placeholders, origins | `Config`, `ConfigBuilder`, `Secret`, `ConfigError`, `Origin`, `DataSize`, `ConfigWarning` |
| `oxbloom-security` | Filter-chain security, JWT resource server, password login with HTTP Basic, sessions, form login, remember-me, method security, OAuth2 and OpenID Connect login | `HttpSecurity`, `PasswordEncoder`, `Argon2PasswordEncoder`, `DelegatingPasswordEncoder`, `UserDetails`, `UserDetailsService`, `InMemoryUserDetailsService`, `DaoAuthenticationProvider`, `SecurityFilterChain`, `JwtDecoderBuilder`, `JwksSource`, `Authentication`, `jwt_decoder_from_config`, `jwks_ssl_bundle`, `Session`, `SessionFilter`, `SessionStore`, `CookieSessionBackend`, `FormLogin`, `FormLoginConfig`, `is_safe_redirect_target`, `ReqwestJwksFetcher` with `with_tls` and `with_client` (feature `jwks`), `OAuth2Login`, `OAuth2Settings`, `ClientRegistration`, `ProviderConfig` and `ClientAuthentication` (feature `oauth2`) |
| `oxbloom-boot` | Application entry point: lifecycle events, runners, services, signals, exit codes, banner, failure analysis, auto-configuration and the conditions report | `OxbloomApplication`, `OxbloomApplicationBuilder`, `ApplicationArguments`, `RunListener`, `ApplicationRunner`, `CommandLineRunner`, `Service`, `ExitCodeGenerator`, `ShutdownHandle`, `StartupError`, `StartupPhase`, `ServiceError`, `RunnerError`, `ExitStatus`, `Banner`, `FailureAnalysis`, `FailureAnalyzer`, `AutoConfiguration`, `AutoConfigurations`, `ConfigRef`, `auto_configuration`, `Conditions`, `Configuration`, `PropertyMetadata`, `boot_properties`, `WebServer`, `BindError`, `WebServerAutoConfiguration` and `RequestLogAutoConfiguration` (feature `web`), `auto_configurations` (feature `security` or `web`), `JwtDecoderAutoConfiguration`, `PasswordEncoderAutoConfiguration`, `SecurityAutoConfiguration` and `SecurityCustomizer` (feature `security`), `WebServer::with_ssl` and `SslAutoConfiguration` (feature `ssl`), `ActuatorAutoConfiguration` (feature `actuator`), `TasksAutoConfiguration`, `RetryAutoConfiguration` and `SchedulerService` (feature `tasks`), `OxbloomApplicationBuilder::logging` (feature `logging`), `async_trait` |
| `oxbloom-logging` | `tracing` subscriber from the `logging.*` keys: levels, groups, console format and color, rotating file output, JSON lines, runtime change | `Level`, `ParseLevelError`, `ConsoleFormat`, `ConsoleColor`, `FileRotation`, `FileFormat`, `JsonField`, `LoggingError`, `LoggingSystem`, `PROPERTIES` |
| `oxbloom-test` | Test support (dev-dependency): run an application context through the real boot lifecycle, captured output, temporary config directories, controllable clocks, a test client and whole applications on port 0 | `ContextRunner`, `ContextView`, `FailureView`, `Captured`, `Log`, `TempConfigDir`, `FixedClock`, `ManualClock`, `TestClient`, `TestResponse`, `TestApp` and `RunningApp` (feature `web`), `TestClient::connect_tls` and `RunningApp::client_tls` (feature `tls`), `MockHttpClient` and `MockResponse` (feature `http-client`), `FakeProvider`, `FakeUser`, `Fault` and `TokenRequest` (feature `oauth2`) |
| `oxbloom-ssl` | Named SSL bundles from the `ssl.*` keys (PEM and PKCS#12), reloaded when their files change, with certificate validity warnings | `SslBundles`, `SslBundle`, `SslError`, `CertValidity`, `ReloadTask`, `PROPERTIES` |
| `oxbloom-actuator` | Operational endpoints: availability state, health indicators, build and platform info, runtime log levels, a certificate validity indicator | `Availability`, `Liveness`, `Readiness`, `Status`, `Health`, `HealthIndicator`, `HealthRunner`, `BuildInfo`, `build_info!`, `build::emit`, `Platform`, `Info`, `InfoContributor`, `ActuatorSettings`, `ActuatorController` (feature `web`), `SslHealthIndicator` (feature `ssl`) |
| `oxbloom-tasks` | Task scheduling and retry: fixed-rate, fixed-delay and cron triggers with zones, the `Scheduler`, `RetryPolicy` with backoff and jitter, and the `#[scheduling]`, `#[scheduled]` and `#[retryable]` macros | `Trigger`, `CronExpr`, `Zone`, `TriggerError`, `parse_duration`, `Task`, `Scheduling`, `Outcome`, `TaskError`, `BoxFuture`, `RunFn`, `Scheduler`, `SchedulerConfig`, `TaskStatus`, `TasksSettings`, `RetryPolicy`, `Backoff`, `Jitter`, `RetryDefaults`, `RetryError`, `defaults`, `set_defaults`, `reset_defaults`, `scheduling`, `scheduled`, `retryable` |
| `oxbloom-http-client` | Outbound HTTP: the `HttpClient` trait, requests and responses, a `reqwest` client with safe defaults, retry for idempotent requests, interceptors, the `http.client.*` settings | `HttpClient`, `Request`, `Response`, `Body`, `HttpClientError`, `Interceptor`, `InterceptedClient`, `ClientBuilder` and `ReqwestClient` (feature `reqwest`), `RetryingClient` (feature `retry`), `HttpClientSettings` (feature `config`) |
| `oxbloom-cli` | `oxbloom` binary: project generator | `new`, `list` |
| `examples/todos` | A CRUD REST API on the web, security and logging stack (controller with `#[post]`, `#[get]`, `#[put]`, `#[patch]`, `#[delete]`, a thread-safe in-memory store, `Json<T>` and `Request::json`, `tracing` events at the target `todos`, a scheduled cleanup task) | `cargo run -p todos -- --server.port=0` or `make run-todos` |
| `examples/hello` | A small application that uses the current features (configuration and beans, trait injection, lifecycle, runners, listeners, exit codes, security and `WebServer` auto-configured through a `SecurityCustomizer` and a `dyn Controller`, a custom failure analyzer, an auto-configuration with a user override) | `cargo run -p hello -- --server.port=0` |

Feature flags: `oxbloom-web/json` (default) adds `Json`, `JsonBody`, `Request::json` and the built-in `Accept` check; `oxbloom-web/gzip`, `brotli` and `zstd` enable one codec each of the compression filter and `oxbloom-web/compression` (default) enables all three (CORS and static resources need no feature); `oxbloom-web/di` adds `App::controller` and `AppError`. `oxbloom-config/di`
adds `ContainerBuilderExt::config`, `From<ConfigError> for DiError` and `impl Environment for Config`. `oxbloom-security/jwks` adds
`ReqwestJwksFetcher` (with `with_tls` and `with_client`; it runs on `oxbloom-http-client`); `oxbloom-security/di` adds `SecurityContainerExt::security_chain` and
`From<SecurityBuildError> for DiError`; `oxbloom-security/oauth2` (it implies `jwks`) adds OAuth2 login (`HttpSecurity::oauth2_login`, `OAuth2Login`, `OAuth2Settings`, `ClientRegistration`, `ProviderConfig`, `ClientAuthentication`). `oxbloom-boot/oauth2` (it turns `security` and `http-client` on) adds OAuth2 login from the `security.oauth2.*` keys to `SecurityAutoConfiguration` and an analyzer for them; `oxbloom-test/oauth2` (it implies `http-client`) adds `FakeProvider`, `FakeUser`, `Fault` and `TokenRequest`. `oxbloom-boot/web` adds `WebServer`, `BindError` and the
port-in-use analyzer and `WebServerAutoConfiguration`; `oxbloom-boot/security` enables
`oxbloom-security/di` and adds the security analyzer, a built-in analyzer for invalid `security.jwt`
configuration, `JwtDecoderAutoConfiguration`, `SecurityAutoConfiguration` and `SecurityCustomizer`;
`oxbloom-boot/logging` adds `oxbloom-logging`, the `OxbloomApplicationBuilder::logging` method, the
startup events, the `LoggingSystem` bean and a built-in analyzer for invalid `logging` configuration;
`oxbloom-boot/ssl` (it turns `web` on) enables `oxbloom-ssl/di` and `oxbloom-web/tls` and adds
`SslAutoConfiguration`, `WebServer::with_ssl`, the `server.ssl.bundle` key and a built-in analyzer for
SSL bundle errors. `oxbloom-web/tls` adds `App::serve_tls`. `oxbloom-ssl/di` implements
`oxbloom_di::Lifecycle` for `SslBundles`. `oxbloom-boot/actuator` (it turns `web` on) enables `oxbloom-actuator/web` and adds
`ActuatorAutoConfiguration`; with `logging` or `ssl` it also enables `oxbloom-actuator/logging` or
`oxbloom-actuator/ssl`. `oxbloom-boot/tasks` adds `oxbloom-tasks`, `TasksAutoConfiguration`, `RetryAutoConfiguration` and
`SchedulerService`; the macros `#[scheduling]`, `#[scheduled]` and `#[retryable]` have no feature flag. `oxbloom-http-client/json` (default) adds `Body::Json`; `reqwest` (default) adds `ClientBuilder` and `ReqwestClient`; `retry` adds `RetryingClient` and `oxbloom-tasks`; `config` (it implies `reqwest` and `retry`) adds `HttpClientSettings`. `oxbloom-boot/http-client` enables `oxbloom-http-client/config` and adds `HttpClientAutoConfiguration` and a built-in analyzer for invalid `http.client` configuration; `oxbloom-test/http-client` adds `MockHttpClient` and `MockResponse`. `oxbloom-test/tls` adds `TestClient::connect_tls` and
`RunningApp::client_tls`. With either
feature, `oxbloom_boot::auto_configurations()` lists the auto-configurations of the enabled
features. `oxbloom-boot` always enables `oxbloom-config/di`.

Specs and plans live in `docs/superpowers/specs` and `docs/superpowers/plans`. The roadmap is `IDEA.md`.

## 2. How to work here

1. Read the real state first: `Cargo.toml` files, `rust-toolchain.toml`, tests. Do not trust old notes.
2. Pick one small, coherent step. Touch only the files it needs.
3. Write the failing test first, then the implementation. Public APIs get an integration test in `tests/`.
4. Verify before claiming success:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   cargo test -p oxbloom-web --features di
   cargo test -p oxbloom-config --features di
   cargo test -p oxbloom-boot --features web
   ```
5. Update `README.md` and this file when a public API changes.
6. Report honestly which commands you ran and what they printed. Never say tests pass if you did not run them.

Rules:

- No breaking changes without a stated reason. Keep dependencies few and versions managed in one place.
- Secure by default (TRACE and CONNECT are off, bodies are capped, secrets are redacted).
- Never write your own cryptography. Never put secrets in source, logs or error messages.
- Cover HTTP edge cases in tests: `HEAD`, `OPTIONS`, 404, 405, disabled `TRACE`/`CONNECT`.
- Unclear requirement: ask one targeted question, otherwise choose the conservative, extensible option.

## 3. Quick start

```bash
cargo run -p oxbloom-cli -- new my-app --dir /tmp     # creates /tmp/my-app
cd /tmp/my-app && cargo test && cargo run
```

Minimal app wired by hand, without configuration or a container. A full application starts through `OxbloomApplication` and `WebServer` instead; see [5](#5-recipe-a-complete-small-service) and `examples/hello`:

```rust
use std::sync::Arc;
use oxbloom_web::{App, IntoResponse, Request, controller};

struct Hello;

#[controller("/api/hello")]
impl Hello {
    #[get("/{name}")]
    async fn greet(&self, req: Request) -> impl IntoResponse {
        format!("Hello, {}!", req.param("name").unwrap_or("stranger"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = App::new().routes(Arc::new(Hello).routes())?;
    app.run("127.0.0.1:8080".parse()?).await?;
    Ok(())
}
```

```bash
curl http://127.0.0.1:8080/api/hello/Ada     # Hello, Ada!
```

## 4. API reference with examples

All examples assume the imports shown in the first example of each section.

### 4.1 `oxbloom-core`

#### `HttpMethod`

Enum: `Get, Post, Put, Patch, Delete, Head, Options, Trace, Connect, Other(String)`.
Names are case-sensitive: `"get"` is `Other("get")`, not `Get`.

```rust
use oxbloom_core::HttpMethod;

let m: HttpMethod = "PATCH".parse().unwrap();
assert_eq!(m, HttpMethod::Patch);
assert_eq!(HttpMethod::Delete.to_string(), "DELETE");
assert_eq!(HttpMethod::Get.as_str(), "GET");
```

#### `HttpMethod::as_str(&self) -> &str`

Canonical method name.

```rust
assert_eq!(HttpMethod::Other("PURGE".into()).as_str(), "PURGE");
```

#### `FromStr for HttpMethod` / `InvalidHttpMethod`

Parses a method token. A string that is not a valid token returns `InvalidHttpMethod`.

```rust
use oxbloom_core::{HttpMethod, InvalidHttpMethod};

assert_eq!("get".parse::<HttpMethod>().unwrap(), HttpMethod::Other("get".into()));
let err: InvalidHttpMethod = "BAD METHOD".parse::<HttpMethod>().unwrap_err();
println!("{err}"); // invalid HTTP method token: "BAD METHOD"
```

#### `From<&http::Method> for HttpMethod`

Used by the server to convert Hyper's method.

```rust
assert_eq!(HttpMethod::from(&http::Method::POST), HttpMethod::Post);
```

### 4.2 `oxbloom-macros` (re-exported by `oxbloom-web` and `oxbloom-di`)

#### `#[controller("/base")]`

Goes on an `impl` block. Adds `fn routes(self: Arc<Self>) -> Vec<Route>` and implements `Controller`.
Handler signature is always `async fn name(&self, req: Request) -> impl IntoResponse`.

```rust
use std::sync::Arc;
use oxbloom_web::{controller, IntoResponse, Request};

struct Users;

#[controller("/users")]
impl Users {
    #[get("")]            // GET /users
    async fn list(&self, _req: Request) -> impl IntoResponse { "all users" }
}

let routes = Arc::new(Users).routes();
assert_eq!(routes.len(), 1);
```

A method path of `""` means "the base path itself". Method paths must start with `/`.

Gotcha: routes that share a path position must use the same parameter name. `GET /users/{id}` and
`PUT /users/{name}` fail at registration with `RouteError::Path` (conflict). Use `{id}` in both.
The `#[get]`/`#[put]`/... snippets below use `{id}`; use one name per controller path.

#### `#[get("/path")]`

```rust
#[get("/{id}")]
async fn show(&self, req: Request) -> impl IntoResponse {
    format!("user {}", req.param("id").unwrap())
}
```

#### `#[post("/path")]`

```rust
#[post("")]
async fn create(&self, req: Request) -> impl IntoResponse {
    (http::StatusCode::CREATED, String::from_utf8_lossy(&req.body).into_owned())
}
```

#### `#[put("/path")]`

```rust
#[put("/{id}")]
async fn replace(&self, _req: Request) -> impl IntoResponse { http::StatusCode::NO_CONTENT }
```

#### `#[patch("/path")]`

```rust
#[patch("/{id}")]
async fn update(&self, _req: Request) -> impl IntoResponse { "patched" }
```

#### `#[delete("/path")]`

```rust
#[delete("/{id}")]
async fn remove(&self, _req: Request) -> impl IntoResponse { http::StatusCode::NO_CONTENT }
```

#### `#[head("/path")]`

An explicit HEAD handler. Without one, HEAD falls back to the GET handler with the body stripped.

```rust
#[head("/{id}")]
async fn exists(&self, _req: Request) -> impl IntoResponse { http::StatusCode::OK }
```

#### `#[options("/path")]`

An explicit OPTIONS handler. Without one, OPTIONS answers 204 with an `Allow` header.

```rust
#[options("/{id}")]
async fn opts(&self, _req: Request) -> impl IntoResponse { http::StatusCode::NO_CONTENT }
```

Compile errors you can hit (all covered by trybuild tests): a route attribute outside a
`#[controller]` impl, the same method + path twice, a wrong handler signature, a route path that
does not start with `/`.

#### `#[component]` and `#[component(..)]`

Goes on a struct (also a unit struct) whose fields are injectable shapes: `Arc<T>`,
`Arc<dyn Trait>`, `Option<Arc<..>>` or `Vec<Arc<..>>`. Generates `impl Component`, which resolves
each field from the container, and `Component::configure` from the options. See
[4.4](#44-oxbloom-di).

Options: `provides(dyn A, dyn B)`, `primary`, `order = n`, `lazy`, `lifecycle` (uses the struct's
`impl Lifecycle`) and `profile = "dev" | "!prod" | ["dev", "test"]`.

```rust
use std::sync::Arc;
use oxbloom_di::{component, ContainerBuilder, DiError};

trait Notifier: Send + Sync { fn send(&self) -> &'static str; }

#[component(provides(dyn Notifier), primary)]
struct Mail;
impl Notifier for Mail { fn send(&self) -> &'static str { "mail" } }

#[component(provides(dyn Notifier), order = -1)]
struct Sms;
impl Notifier for Sms { fn send(&self) -> &'static str { "sms" } }

struct Metrics;

#[component]
struct Alerts {
    main: Arc<dyn Notifier>,             // the primary provider
    all: Vec<Arc<dyn Notifier>>,         // every provider, `order` first
    metrics: Option<Arc<Metrics>>,       // not registered: None
}

let c = ContainerBuilder::new()
    .component::<Alerts>()
    .component::<Mail>()
    .component::<Sms>()
    .build()?;
let alerts = c.get::<Alerts>()?;
assert_eq!(alerts.main.send(), "mail");
assert_eq!(alerts.all.len(), 2);
assert!(alerts.metrics.is_none());
```

Rejected at compile time: enums, tuple structs, generic structs, unknown or repeated options,
`provides` with a type that is not `dyn Trait` or that the struct does not implement, `lifecycle`
without `impl Lifecycle`, empty profile names, fields of another shape (plain values, `Rc<T>`) and
`#[cfg]` on fields. The generated code names the crate `oxbloom_di`, so do not rename that
dependency in `Cargo.toml`.

#### `#[configuration]` and `#[bean]`

`#[configuration]` goes on an inherent, non-generic `impl` block and generates
`impl Configuration`. Each `#[bean]` method (taking `&self`) becomes one registration, in
declaration order; its bean is the return type, and its parameters are injected like `bean`
parameters. The configuration type itself must be registered too (as a `#[component]` or an
`.instance(..)`), because every bean method resolves `Arc<Self>`. Its source label is
`Type::method`.

`#[bean]` options: `fallible` (the method returns `Result<T, DiError>`, the bean is `T`, and an
`Err` fails like `try_bean`; without `fallible` a `Result` is the bean), `provides(dyn A, ..)`,
`primary`, `order = n`, `lazy`, `profile = ..`, and the conditions `on_bean(T)`, `on_missing_bean`,
`on_missing_bean(T)`, `on_property("key")` and `on_property("key", "value")` (see the next entries).

```rust
use std::sync::Arc;
use oxbloom_di::{component, configuration, ContainerBuilder, DiError};

struct Pool { url: String }
trait Store: Send + Sync { fn name(&self) -> String; }
struct PgStore { pool: Arc<Pool> }
impl Store for PgStore { fn name(&self) -> String { format!("pg at {}", self.pool.url) } }

#[component]
struct Persistence;

#[configuration]
impl Persistence {
    #[bean]
    fn pool(&self) -> Pool {
        Pool { url: "postgres://db/app".into() }
    }

    #[bean(fallible, provides(dyn Store))]
    fn store(&self, pool: Arc<Pool>) -> Result<PgStore, DiError> {
        if pool.url.is_empty() {
            return Err(DiError::creation::<PgStore>("no database url"));
        }
        Ok(PgStore { pool })
    }
}

let c = ContainerBuilder::new()
    .component::<Persistence>()
    .configuration::<Persistence>()
    .build()?;
assert_eq!(c.get::<dyn Store>()?.name(), "pg at postgres://db/app");
```

Rules:

- Use one `#[configuration]` impl per type; a second one is a conflicting-implementations error.
- A `#[cfg(..)]` or `#[cfg_attr(pred, cfg(..))]` on a bean method also removes its registration. A
  more deeply nested `cfg_attr` is not copied and fails with E0599 when the method is compiled out;
  write it as `#[cfg_attr(all(a, b), cfg(..))]`.
- Generic impls, trait impls, `#[bean]` outside a `#[configuration]` impl, methods that do not take
  `&self` (or are `async`, `unsafe` or generic), parameters that are not injectable shapes,
  `impl Trait` parameters or return types, a missing return type, `#[cfg]` on parameters, and
  unknown or repeated options are compile errors.
- The generated code names the crate `oxbloom_di`, so do not rename that dependency.
- `#[bean]` methods cannot take lifecycle hooks (the `lifecycle` option is rejected); register the resource as an `instance` or `factory` followed by `.lifecycle::<T>()`, or add `.pre_destroy(..)` right after `.bean(..)`.

#### `#[bean(on_bean(T), on_missing_bean, on_property(..))]`: conditions on a bean method

Each option becomes the builder modifier of the same name on the method's registration (see
[4.4](#44-oxbloom-di), "Conditions: the model"):

- `on_bean(T)`: `.on_bean::<T>()`.
- `on_missing_bean`: `.on_missing_bean::<R>()` for the method's own bean type `R` (the return type,
  or the `T` of a `fallible` method's `Result<T, DiError>`), as in Spring.
- `on_missing_bean(T)`: `.on_missing_bean::<T>()`, also for `dyn Trait`.
- `on_property("key")`: `.on_property("key")`. It matches whenever the key is present, also with the
  value `false`; for a switch, write `on_property("key", "true")`.
- `on_property("key", "value")`: `.on_property_value("key", "value")`, an exact, case-sensitive
  comparison. The expected value sits in the source code: never put a secret there.

Warning: bare `on_missing_bean` guards the concrete return type only, also next to
`provides(dyn Trait)`. `#[bean(provides(dyn Clock), on_missing_bean)]` on a method that returns
`SystemClock` does not back off when the application provides `dyn Clock` through another type:
both beans stay and a single `Arc<dyn Clock>` request is `Ambiguous`. Write
`#[bean(provides(dyn Clock), on_missing_bean(dyn Clock))]` to back off for the trait.

The options may repeat and are ANDed in source order. An empty key, a third `on_property`
argument, `on_bean` without a type and a non-type argument are compile errors. Conditions are not
available as `#[component(..)]` options; use the builder modifiers after `.component::<T>()`.

```rust
use std::collections::HashMap;
use oxbloom_di::{configuration, ContainerBuilder, DiError, Environment};

struct Props(HashMap<&'static str, &'static str>);
impl Environment for Props {
    fn property(&self, key: &str) -> Option<String> {
        self.0.get(key).map(|v| v.to_string())
    }
}

struct Cache { size: usize }
struct Metrics;
struct Defaults;

#[configuration]
impl Defaults {
    // Backs off when the application registered its own `Cache`.
    #[bean(on_missing_bean)]
    fn cache(&self) -> Cache { Cache { size: 16 } }

    // Only with `metrics.export = "on"` and a `Cache` kept before it.
    #[bean(on_bean(Cache), on_property("metrics.export", "on"))]
    fn metrics(&self) -> Metrics { Metrics }
}

let c = ContainerBuilder::new()
    .environment(Props(HashMap::from([("metrics.export", "on")])))
    .instance(Defaults)
    .configuration::<Defaults>()
    .build()?;
assert_eq!(c.get::<Cache>()?.size, 16);
assert!(c.get_opt::<Metrics>()?.is_some());

let c = ContainerBuilder::new()
    .instance(Cache { size: 1024 })
    .instance(Defaults)
    .configuration::<Defaults>()
    .build()?;
assert_eq!(c.get::<Cache>()?.size, 1024);
assert!(c.get_opt::<Metrics>()?.is_none());
```

#### `#[auto_configuration(..)]` (used through `oxbloom_boot`)

Goes on an inherent `impl` block like `#[configuration]`, generates `impl Configuration` exactly as
`#[configuration]` does, and also `impl oxbloom_boot::AutoConfiguration`. Import it as
`oxbloom_boot::auto_configuration`; the generated code names `::oxbloom_boot` and `::oxbloom_di`,
so the crate must depend on both under those names. Options:

- `after(A, B, ..)`: `A` and `B` are processed before this configuration.
- `before(C, ..)`: this configuration is processed before `C`.
- `on_bean(T)`, `on_missing_bean(T)` (also `dyn Trait`), `on_property("key")`,
  `on_property("key", "value")`: the class-level conditions, decided once for all of the
  configuration's beans. `on_missing_bean` needs its type here, because a configuration has no
  return type to stand for.

`after` and `before` take at least one type and may appear once each; conditions may repeat.
Unknown or repeated options, an empty `after()` or `before()`, a non-type argument and an empty
key are compile errors. The type must also implement `Component`, usually with `#[component]` on
the struct (a unit struct works, fields are injected): boot registers the instance as the first
registration of the configuration's group, so the `#[bean]` methods get `&self` and the instance
is skipped together with the group.

```rust
use oxbloom_boot::{auto_configuration, AutoConfiguration, ConfigRef};
use oxbloom_di::component;

pub struct Pool { pub size: usize }
pub struct Cache { pub size: usize }

#[component]
pub struct DataSourceAutoConfig;

#[auto_configuration]
impl DataSourceAutoConfig {
    #[bean]
    fn pool(&self) -> Pool { Pool { size: 4 } }
}

#[component]
pub struct CacheAutoConfig;

// After the data source, only with `cache.enabled` set and no `Cache` of the application.
#[auto_configuration(after(DataSourceAutoConfig), on_property("cache.enabled"), on_missing_bean(Cache))]
impl CacheAutoConfig {
    #[bean]
    fn cache(&self) -> Cache { Cache { size: 64 } }
}

assert_eq!(CacheAutoConfig::after(), [ConfigRef::of::<DataSourceAutoConfig>()]);
assert!(DataSourceAutoConfig::before().is_empty());
```

### 4.3 `oxbloom-web`

Imports for this section:

```rust
use std::sync::Arc;
use oxbloom_web::{App, Controller, HttpMethod, IntoResponse, Request, Response,
                     Route, RouteError, Router, RouterConfig};
use http::StatusCode;
```

#### `Request::new(method, path) -> Request`

Builds a request. Mostly used in tests; the server builds real ones. Public fields: `method`, `path`,
`query: Option<String>`, `headers`, `body: Bytes`.

```rust
let mut req = Request::new(HttpMethod::Post, "/users");
req.query = Some("page=2".into());
req.body = "name=Ada".into();
assert_eq!(req.path, "/users");
```

#### `Request::param(&self, name) -> Option<&str>`

Value of a `{name}` path parameter.

```rust
// Inside a handler registered at "/users/{id}" and called with GET /users/42:
let id: Option<&str> = req.param("id");   // Some("42")
let nope = req.param("other");            // None
```

#### `IntoResponse` and `Response`

`Response` is `http::Response<Bytes>`. Anything that implements `IntoResponse` can be returned from a
handler. Built-in implementations:

```rust
let a: Response = "plain text".into_response();                  // 200, text/plain; charset=utf-8
let b: Response = String::from("owned").into_response();         // 200, text/plain
let c: Response = StatusCode::NOT_FOUND.into_response();         // 404, empty body
let d: Response = (StatusCode::CREATED, "made").into_response(); // 201, text/plain
let e: Response = Response::new("raw".into()).into_response();   // unchanged
assert_eq!(c.status(), 404);
```

Custom type:

```rust
struct Teapot;
impl IntoResponse for Teapot {
    fn into_response(self) -> Response {
        (StatusCode::IM_A_TEAPOT, "short and stout").into_response()
    }
}
```

#### `Route::new(method, path, handler) -> Route`

Builds a route from a closure without macros. Paths use `{name}` for parameters.

```rust
let route = Route::new(HttpMethod::Get, "/ping", |_req| async { "pong" });
let with_param = Route::new(HttpMethod::Get, "/hi/{name}", |req: Request| async move {
    format!("hi {}", req.param("name").unwrap())
});
```

#### `Controller` trait

`pub trait Controller: Send + Sync`, with `fn routes(self: Arc<Self>) -> Vec<Route>`. `#[controller]`
implements it for you. Implement it by hand only for unusual cases. The `Send + Sync` supertraits let
a controller be registered as `Arc<dyn Controller>`, which is how `oxbloom-boot`'s
`WebServerAutoConfiguration` collects routes (`.provides(|c: Arc<X>| -> Arc<dyn Controller> { c })`).

```rust
struct Manual;
impl Controller for Manual {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/manual", |_| async { "ok" })]
    }
}
let routes = Controller::routes(Arc::new(Manual));
```

#### `RouterConfig`

Fields `allow_trace` and `allow_connect`, both `false` by default (TRACE/CONNECT give 405).

```rust
let strict = RouterConfig::default();
let with_trace = RouterConfig { allow_trace: true, ..RouterConfig::default() };
```

#### `Router::new(config)`, `Router::add(route)`, `Router::handle(req)`

The dispatcher on its own, without a server.

```rust
# async fn demo() -> Result<(), RouteError> {
let mut router = Router::new(RouterConfig::default());
router.add(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;

let res = router.handle(Request::new(HttpMethod::Get, "/ping")).await;
assert_eq!(res.body(), "pong");

let res = router.handle(Request::new(HttpMethod::Get, "/missing")).await;
assert_eq!(res.status(), 404);

let res = router.handle(Request::new(HttpMethod::Post, "/ping")).await;
assert_eq!(res.status(), 405);          // Allow header lists GET, HEAD, OPTIONS
# Ok(()) }
```

#### `RouteError`

Returned by `add`, `App::route`, `App::routes`. Variants: `Path(String, matchit::InsertError)` for an
invalid or conflicting pattern, `Duplicate(HttpMethod, String)` for the same method + path twice.

```rust
let mut router = Router::new(RouterConfig::default());
let get = || Route::new(HttpMethod::Get, "/x", |_| async { "x" });
router.add(get()).unwrap();
match router.add(get()) {
    Err(RouteError::Duplicate(method, path)) => println!("duplicate {method} {path}"),
    other => panic!("{other:?}"),
}
```

#### `App::new()`, `App::with_config(config)`

```rust
let app = App::new();
let app = App::with_config(RouterConfig { allow_trace: true, ..Default::default() });
```

#### `App::max_body_bytes(n)`

Largest accepted request body. Larger bodies get 413. Default 1 MiB.

```rust
let app = App::new().max_body_bytes(64 * 1024);
```

#### `App::header_read_timeout(duration)`

Time a client may take to send its request headers. Default 10 s.

```rust
let app = App::new().header_read_timeout(std::time::Duration::from_secs(5));
```

#### `App::route(route)` and `App::routes(iter)`

Both return `Result<App, RouteError>`, so use `?`.

```rust
let app = App::new()
    .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?
    .routes(Arc::new(Users).routes())?;     // Users from the #[controller] example
```

#### `App::controller::<C>(&container)` (feature `di`)

Resolves `C` from a DI container and registers its routes. `C` must be registered in the container,
usually with `#[component]`. Returns `AppError`.

```rust
#[component]
struct Greeter {}

#[component]
struct GreetController { greeter: Arc<Greeter> }

#[controller("/g")]
impl GreetController {
    #[get("")]
    async fn greet(&self, _req: Request) -> impl IntoResponse { "hi" }
}

let container = ContainerBuilder::new()
    .component::<Greeter>()
    .component::<GreetController>()
    .build()?;
let app = App::new().controller::<GreetController>(&container)?;
```

#### `AppError` (feature `di`)

`AppError::Di(DiError)` when the controller is not registered or broken, `AppError::Route(RouteError)`
when its routes conflict. Both convert with `?`. `std::error::Error::source` returns the wrapped
`DiError` or `RouteError`, so error reporters can walk the chain.

```rust
let empty = ContainerBuilder::new().build()?;
match App::new().controller::<GreetController>(&empty) {
    Err(AppError::Di(e)) => println!("{e}"),
    Err(AppError::Route(e)) => println!("{e}"),
    Ok(_) => unreachable!(),
}

let err = App::new().controller::<GreetController>(&empty).err().unwrap();
let source = std::error::Error::source(&err).and_then(|s| s.downcast_ref::<DiError>());
assert!(matches!(source, Some(DiError::Missing { .. })));
```

#### `App::handle(req)`

Dispatches without a network. This is how you unit-test handlers.

```rust
#[tokio::test]
async fn ping_works() {
    let app = App::new()
        .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))
        .unwrap();
    let res = app.handle(Request::new(HttpMethod::Get, "/ping")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "pong");
}
```

#### `Filter`, `Next`, `App::filter(f)`, `Request::extensions()`

A `Filter` wraps the router: it either calls `next.run(req).await` or returns its own `Response`
(then the router is never reached). Filters run in the order of `Filter::order()` (lower first, ties in
registration order; see the next section) and **before routing**, so they see unknown paths too. A request body
over the limit gets 413 from the built-in body limit filter, which runs before the security chain and every
`DEFAULT` filter.
`Request::extensions()` / `extensions_mut()` carry per-request typed data between filters and handlers.

```rust
use oxbloom_web::{Filter, Next};

struct Tag;

#[async_trait::async_trait]
impl Filter for Tag {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let mut res = next.run(req).await;
        res.headers_mut().insert("x-tag", "1".parse().unwrap());
        res
    }
}

let app = App::new().filter(Tag);
let res = app.handle(Request::new(HttpMethod::Get, "/nothing")).await;
assert_eq!(res.status(), 404);
assert_eq!(res.headers()["x-tag"], "1");
```

#### `order` module, `Filter::order()`, `Ordered`, `App::filter_ordered(order, f)`

The slots of the filter chain are `i32` constants in `oxbloom_web::order`. A lower value runs first (outer);
equal values keep registration order.

| Constant | Value | Who uses it |
|---|---|---|
| `order::REQUEST_LOG` | -400 | `RequestLogFilter` |
| `order::CORS` | -350 | `CorsFilter` |
| `order::ERROR_HANDLING` | -300 | built-in error filter (installed in every `App`) |
| `order::NEGOTIATION` | -250 | built-in `Accept` check for `Json<T>` answers (feature `json`; the constant exists without it) |
| `order::BODY_LIMIT` | -200 | built-in body limit filter (installed in every `App`) |
| `order::COMPRESSION` | -150 | `CompressionFilter` |
| `order::SECURITY` | -100 | `SecurityFilterChain` |
| `order::DEFAULT` | 0 | every filter that does not override `order()` |

`Filter::order(&self) -> i32` is a provided method that returns `order::DEFAULT`. `App::filter(f)` places `f`
at `f.order()`. `App::filter_ordered(order, f)` wraps `f` in `Ordered(order, f)`, a public tuple struct that
implements `Filter` and overrides the slot; it derives no traits. `oxbloom-boot` registers the security chain
and every `dyn Filter` bean with plain `App::filter`, so each declares its own slot.

```rust
use oxbloom_web::{App, Filter, Next, Ordered, Request, Response, order};

struct Tag;

#[async_trait::async_trait]
impl Filter for Tag {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        next.run(req).await
    }
    fn order(&self) -> i32 {
        order::SECURITY + 1        // just after security
    }
}

assert_eq!(Tag.order(), -99);
assert_eq!(Ordered(order::REQUEST_LOG, Tag).order(), -400);
let app = App::new().filter(Tag).filter_ordered(order::BODY_LIMIT + 1, Tag);
```

Effects: the request log (outermost) records 401, 403, 404, 405, 413 and 500; an oversized body gets 413 before
security runs, so a client without credentials sees 413; a filter at `DEFAULT` runs after security.

A filter ordered below `order::SECURITY` runs before authentication: it sees unauthenticated requests (no
`Authentication` in the extensions), and a response it returns itself is sent without any authorization check.
A filter ordered below `order::BODY_LIMIT` sees an empty body for a request whose body was over the limit, and
it can tell why only by calling `Request::body_too_large()`.

#### `Request::body_too_large()`

`Request::body_too_large(&self) -> bool` is `true` when the server cut the body of the request because it was
larger than `App::max_body_bytes` (`server.max-body-bytes`); the body is then empty. The built-in body limit
filter answers such a request with 413, so only a filter ordered before `order::BODY_LIMIT` sees one. A request
that did not come through `serve`, such as one passed to `App::handle`, always gives `false`.

```rust
use oxbloom_web::{Filter, IntoResponse, Next, Request, Response, order};

struct Probe;

#[async_trait::async_trait]
impl Filter for Probe {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        if req.body_too_large() {
            return "the body was cut".into_response();
        }
        next.run(req).await
    }
}

assert!(!Request::new(HttpMethod::Post, "/").body_too_large());
let app = App::new().max_body_bytes(4).filter_ordered(order::BODY_LIMIT - 50, Probe);
```

#### `Json`, `JsonBody`, `Request::json`, `order::NEGOTIATION` (feature `json`, default)

`Json<T>(pub T)` is a response when `T: Serialize`: status 200, `content-type: application/json`, the serialized
body and a `JsonBody` marker in the response extensions. `(StatusCode, Json<T>)` sets another status. A
serialization failure is a 500 problem without `detail`; the error text is logged at `error` on `oxbloom::web`.

```rust
use oxbloom_web::{IntoResponse, Json, JsonBody};

let res = Json(vec![1, 2, 3]).into_response();
assert_eq!(res.status(), 200);
assert_eq!(res.headers()["content-type"], "application/json");
assert_eq!(res.body(), "[1,2,3]");
assert_eq!(res.extensions().get::<JsonBody>(), Some(&JsonBody));
```

`Request::json::<T>() -> Result<T, HttpError>` reads the body. The `Content-Type` must be `application/json` or a
`+json` type (parameters and case are ignored); otherwise the error is 415 with `accepted = "application/json"`,
also when the header is missing. A body that is empty or invalid for `T` is 400 with the `detail`
`invalid JSON body: <class> error at line L column C`, where the class is `syntax`, `data` or `eof`.

```rust
use http::{HeaderValue, header};
use oxbloom_web::{HttpMethod, Request};

let mut req = Request::new(HttpMethod::Post, "/numbers");
req.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
req.body = "[1,2]".into();
let numbers: Vec<u8> = req.json().unwrap();
assert_eq!(numbers, [1, 2]);

req.body = "[1,".into();
assert!(req.json::<Vec<u8>>().is_err());
```

`order::NEGOTIATION` is `-250`. The built-in filter at that slot (feature `json`, installed in every `App`) answers
406 with `supported = "application/json"` when the handler returned a `Json<T>` and `Accept` excludes JSON; other
responses are never changed, and `HEAD` gets the same answer as `GET`. `Accept` ranges are `*/*`, `type/*` and
`type/subtype`; others are skipped. `q` follows RFC 9110 and an invalid `q` skips the range. The most specific
matching range decides (acceptable when `q > 0`); no header or no valid range is acceptable.

```rust
use oxbloom_web::order;

assert_eq!(order::NEGOTIATION, -250);
assert!(order::ERROR_HANDLING < order::NEGOTIATION && order::NEGOTIATION < order::BODY_LIMIT);
```

#### `HttpError`, `ErrorValue`, `ErrorAttributes`, `IntoResponse for Result<T, E>`

`HttpError::new(status)` builds an error; `.title(..)`, `.detail(..)`, `.type_uri(..)` and `.with(key, value)`
(value: `ErrorValue`, from `&str`, `String`, `i64`, `i32`, `u32` or `bool`) set the members. Converting it gives
a response with that status, an empty body and an `ErrorAttributes` value in the response extensions. The
built-in error filter renders it. `ErrorAttributes` has the public fields `status`, `title`, `detail`,
`type_uri`, `extensions: Vec<(String, ErrorValue)>` and `internal: Option<String>` (set by the framework only,
for example the panic message), and `ErrorAttributes::from_status(status)`. `ErrorValue` is `Str`, `Int` or
`Bool`; it is `#[non_exhaustive]`, so a `match` outside `oxbloom-web` needs a wildcard arm.

`IntoResponse` is implemented for `Result<T, E>` where `T: IntoResponse` and `E: IntoResponse`. An application
error type maps to a response by implementing `IntoResponse`, usually through an `HttpError`.

```rust
use oxbloom_web::{ErrorValue, HttpError, IntoResponse};

let res = HttpError::new(StatusCode::UNPROCESSABLE_ENTITY)
    .title("Invalid todo")
    .detail("the title must not be empty")
    .type_uri("https://example.com/problems/blank-title")
    .with("field", "title")
    .with("max", 200)
    .into_response();
assert_eq!(res.status(), 422);
assert!(res.body().is_empty());                       // rendered later by the error filter

let r: Result<&'static str, HttpError> = Err(HttpError::new(StatusCode::GONE));
assert_eq!(r.into_response().status(), 410);
assert_eq!(ErrorValue::from(true), ErrorValue::Bool(true));
```

Rules: an author `detail` is always shown. A response that already has a body is never rewritten, not even
when it carries `ErrorAttributes`. A `HEAD` answer carries the status and headers but no body. Every bodiless 4xx
or 5xx response is rendered too (with attributes derived from the status). Extension values are strings,
integers and booleans; two `with` calls with the same key can produce two members; an extension whose key is
`type`, `title`, `status`, `detail` or `instance` is dropped by `ProblemRenderer`. A status without a reason
phrase (499, 599) renders an empty `title`. `ErrorAttributes` on a 2xx or 3xx response is rendered as
problem+json. The rendered response drops the extensions of the original response.

#### `ErrorRenderer`, `ProblemRenderer`, `RequestInfo`, `App::error_renderer(r)`

`trait ErrorRenderer: Send + Sync { fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response }`
turns attributes into a fresh response. `RequestInfo { method: HttpMethod, path: String }` has no query.
`ProblemRenderer::new(include_internal: bool)` is the default: RFC 9457 `application/problem+json` with the
members `type` (default `about:blank`), `title` (the author's, else the reason phrase), `status`, `detail` (the
author's; else the internal message only when `include_internal`), `instance` (the path) and the extension
members. Headers already on the response (`WWW-Authenticate`, `Allow`) are kept; the renderer returns a new
response and the filter copies them over. `App::error_renderer(r)` replaces the renderer; with
`oxbloom-boot` register a `dyn ErrorRenderer` bean instead. `Arc<R>` implements `ErrorRenderer` for every
`R: ErrorRenderer + ?Sized`, so an `Arc<dyn ErrorRenderer>` can be passed to `App::error_renderer` as it is. A
renderer that panics escapes the outer catch.

```rust
use oxbloom_web::{ErrorAttributes, ErrorRenderer, HttpMethod, ProblemRenderer, RequestInfo};

let info = RequestInfo { method: HttpMethod::Get, path: "/x".into() };
let res = ProblemRenderer::new(false)
    .render(&ErrorAttributes::from_status(StatusCode::NOT_FOUND), &info);
assert_eq!(res.headers()["content-type"], "application/problem+json");
assert_eq!(
    res.body(),
    r#"{"type":"about:blank","title":"Not Found","status":404,"instance":"/x"}"#
);
```

A panic in a handler or filter is caught, logged at `error` on target `oxbloom::web` as `request panicked: <message>`
(Rust's default panic hook also prints it to stderr) and answered as 500. The log line escapes the control
characters of the message (`\n`, `\r`, `\u{1b}`), so it stays on one line; `ErrorAttributes.internal` keeps the
raw message. The message reaches the response only with `include_internal`
(`server.error.include-message = always`). A panic in a handler or in a filter ordered after
`order::ERROR_HANDLING` is answered by the built-in error filter, and the request log records the 500. A panic
in a filter ordered before `order::ERROR_HANDLING` is caught by an outer catch and answered as 500, but it
unwinds through every filter outside it: the request log event is lost when the request log itself panics or
when any filter ordered between `order::REQUEST_LOG` and `order::ERROR_HANDLING` (for example at -350) panics.
A filter ordered before `order::REQUEST_LOG` loses the event only when it panics before it calls `next.run`.

#### `App::shutdown_timeout(d)`, `App::forwarded(cfg)`, `App::forwarded_config()`

`shutdown_timeout`: how long `serve` waits for open connections after the shutdown signal (default 30 s).
When it expires the remaining connections are dropped and a `warn` event is logged on target `oxbloom::web`.
`Duration::ZERO` drops open connections at once. Dropping the future of `serve` aborts in-flight connections.
`max_body_bytes(0)` rejects every non-empty body. `header_read_timeout` clamps values above one day to one day.

`forwarded(cfg)` takes a `ForwardedConfig` (below); `forwarded_config()` returns the one in effect (the default
until `forwarded` is called).

```rust
use std::time::Duration;
let app = App::new()
    .shutdown_timeout(Duration::from_secs(5))
    .max_body_bytes(64 * 1024);
```

#### `ClientInfo`, `Scheme`, `Request::client_addr()`, `scheme()`, `host()`

`serve` stores a `ClientInfo { addr: SocketAddr, scheme: Scheme, host: Option<String> }` in the extensions of
every request. `Scheme` is `Http` (the default) or `Https`. `Request::client_addr() -> Option<SocketAddr>`,
`scheme() -> Scheme` and `host() -> Option<&str>` read it. Without trusted forwarded headers `addr` is the TCP
peer, `scheme` is `Http` and `host` is `None`. Under `App::handle` there is no socket, so `client_addr()` is
`None` unless a test inserts a `ClientInfo`. A forwarded address without a port has port `0`. `ClientInfo.addr`
keeps the IPv4-mapped form of an IPv6 peer. Nothing in `oxbloom-security` uses the client address yet.

```rust
use oxbloom_web::{ClientInfo, Scheme};

let mut req = Request::new(HttpMethod::Get, "/");
assert_eq!(req.client_addr(), None);
req.extensions_mut().insert(ClientInfo {
    addr: "203.0.113.9:0".parse().unwrap(),
    scheme: Scheme::Https,
    host: Some("example.com".into()),
});
assert_eq!(req.scheme(), Scheme::Https);
assert_eq!(req.host(), Some("example.com"));
```

#### `ForwardedConfig`, `ForwardStrategy`, `TrustedProxies`, `ProxyParseError`

`ForwardedConfig { strategy: ForwardStrategy, trusted: TrustedProxies }`; its default is `ForwardStrategy::None`
with loopback trusted. `ForwardStrategy` is `None` (default; every forwarded header is ignored) or `Framework`; it
is `#[non_exhaustive]`, so a `match` outside `oxbloom-web` needs a wildcard arm.
`TrustedProxies::parse(&[String]) -> Result<_, ProxyParseError>` accepts IP addresses and CIDR networks (IPv4 and
IPv6), `TrustedProxies::loopback()` is `127.0.0.0/8` and `::1`, and `contains(IpAddr)` tests an address
(IPv4-mapped IPv6 addresses are matched as IPv4). An empty list trusts nobody. `ProxyParseError::index()` is the
index of the bad entry; its `Display` never shows the entry text.

```rust
use oxbloom_web::{ForwardStrategy, ForwardedConfig, TrustedProxies};

let trusted = TrustedProxies::parse(&["10.0.0.0/8".into(), "::1".into()])?;
assert!(trusted.contains("10.1.2.3".parse().unwrap()));
assert!(!trusted.contains("192.0.2.1".parse().unwrap()));
let err = TrustedProxies::parse(&["10.0.0.0/33".into()]).unwrap_err();
assert_eq!(err.index(), 0);

let app = App::new().forwarded(ForwardedConfig { strategy: ForwardStrategy::Framework, trusted });
```

Algorithm with `Framework`: the headers count only when the TCP peer is in `trusted`. `Forwarded` (RFC 7239)
is read first; only if it is absent are `X-Forwarded-For`, `-Proto` and `-Host` read, and a present but
unusable `Forwarded` does not fall back to `X-Forwarded-*`. The hops are walked from right to left; the first
untrusted hop is the client, and when every hop is trusted the leftmost is. With `Forwarded`, `proto` and `host`
come from the client's element. A list in `X-Forwarded-Proto` or `X-Forwarded-Host` is read at the client's hop
when it has as many entries as `X-Forwarded-For`; otherwise (also without `X-Forwarded-For`) its rightmost entry
is used. Any element that is not a valid IP address (optional port, bracketed IPv6) makes the whole header
unusable; then any `proto` and `host` are dropped too, so the peer stays the client, the scheme is the
connection's own and the host is `None`. `proto` gives `Https` when it is `https` (in any case) and `Http` for
any other value. Without a `proto` (also without any forwarded header) the scheme is the connection's own:
`Http`, or `Https` when the server terminated TLS. `host` must be a valid authority, otherwise the host is
`None`.

**Deployment requirement.** The trusted proxy must set or strip `Forwarded`, overwrite or strip
`X-Forwarded-Proto` and `X-Forwarded-Host`, and append to `X-Forwarded-For`. Otherwise clients control their
address, scheme and host. The framework cannot detect a proxy that does not do this. The default trusted list is
loopback only, so a proxy on another host must be listed.

#### `CorsConfig`, `CorsFilter`, `CorsError`, `App::cors(config)`, `order::CORS`

`CorsConfig::new()` starts a configuration (methods `GET, HEAD, POST`, no allowed or exposed headers, credentials
off, max age 30 minutes, no origin); the builder methods `allowed_origins`, `allowed_methods`, `allowed_headers`,
`exposed_headers`, `allow_credentials(bool)` and `max_age(Duration)` take it by value. `CorsFilter::new(config)`
checks it and returns a `CorsError` (`setting()` names the key, such as `allowed-origins`, `reason()` is fixed text
that says why, and the message never contains a value). `App::cors(config)` builds the filter and registers it at `order::CORS` (-350), outside error
handling and security, so error answers carry the CORS headers and an allowed preflight never reaches security. No
Cargo feature is needed.

```rust
use std::time::Duration;
use oxbloom_web::{App, CorsConfig, CorsFilter, HttpMethod, Request, Route};

let config = CorsConfig::new()
    .allowed_origins(["https://app.example"])
    .allowed_methods(["GET", "PUT"])
    .allowed_headers(["Content-Type"])
    .exposed_headers(["X-Total"])
    .allow_credentials(true)
    .max_age(Duration::from_secs(600));
assert!(CorsFilter::new(config.clone()).is_ok());

let app = App::new()
    .cors(config)?
    .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
let mut req = Request::new(HttpMethod::Get, "/ping");
req.headers.insert("origin", "https://app.example".parse()?);
let res = app.handle(req).await;
assert_eq!(res.headers()["access-control-allow-origin"], "https://app.example");
assert_eq!(res.headers()["vary"], "Origin");

let err = CorsFilter::new(CorsConfig::new().allowed_origins(["*"]).allow_credentials(true)).err().unwrap();
assert_eq!(err.setting(), "allow-credentials");
```

Behavior: a request without `Origin`, from a disallowed origin, or with two `Origin` headers passes with no CORS
headers. A preflight (`OPTIONS` with `Origin` and `Access-Control-Request-Method`) from an allowed origin that asks
for an allowed method and headers is answered 204 by the filter; any other preflight passes through. Any other
request from an allowed origin gets `Access-Control-Allow-Origin` (the echoed origin, or `*`), `Vary: Origin`, and
when configured `Access-Control-Expose-Headers` and `Access-Control-Allow-Credentials`. When the allowed origins are a
list (not `*`), every answer that is not a preflight answer gets `Vary: Origin`, also for a request without or from a
disallowed origin, so a shared cache never serves a copy without CORS headers to an allowed origin; a `Vary` that
already lists `Origin` or `*` is not repeated.

Rules: the origin list must not be empty; an entry is `scheme://host[:port]` (no path, no trailing slash,
scheme and host compared without regard to case), `null`, or the single entry `*`. Default ports are not stripped
(list `https://a.example`, not `https://a.example:443`). `*` must be the only entry of its list, in origins, methods
and headers, and none of them can be combined with credentials. Methods are compared exactly and case-sensitively
(use upper case); header names without regard to case. `null` together with credentials is accepted but a foot-gun,
because sandboxed frames and local files send `Origin: null`.

#### `CompressionConfig`, `CompressionLevel`, `CompressionFilter`, `codecs()`, `App::compression(config)`, `order::COMPRESSION`

`CompressionConfig::new()` has the defaults (`min_size` 1024, `max_body_bytes` 8 MiB, level `Default`, types `text/*`,
`application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml`, `application/xml`) and the
methods `min_size(usize)`, `mime_types(..)`, `level(CompressionLevel)` and `max_body_bytes(usize)`. `CompressionLevel`
is `Fast`, `Default` or `Best` (`#[non_exhaustive]`; gzip 1/6/9, brotli 2/5/11, zstd 1/3/19). `CompressionFilter::new(config)`
builds the filter, and `App::compression(config)` registers it at `order::COMPRESSION` (-150), outside security.
`codecs()` returns the names of the compiled-in codecs (`gzip`, `br`, `zstd`). The features `gzip`, `brotli` and `zstd`
enable the codecs; `compression` enables all three and is in `default`. A codec whose feature is off is never chosen.

```rust
use oxbloom_web::{App, CompressionConfig, CompressionFilter, CompressionLevel, Filter, HttpMethod, Request, Route, codecs, order};

assert_eq!(CompressionFilter::new(CompressionConfig::new()).order(), order::COMPRESSION);
assert!(codecs().contains(&"gzip"));

let app = App::new()
    .compression(CompressionConfig::new().min_size(16).level(CompressionLevel::Fast))
    .route(Route::new(HttpMethod::Get, "/t", |_| async { "ab".repeat(100) }))?;
let mut req = Request::new(HttpMethod::Get, "/t");
req.headers.insert("accept-encoding", "gzip".parse()?);
let res = app.handle(req).await;
assert_eq!(res.headers()["content-encoding"], "gzip");
assert_eq!(res.headers()["vary"], "Accept-Encoding");
```

A response is compressed only when a compiled codec is acceptable, the body size is within the limits, the
`Content-Type` is in the list (an entry `*/*` matches nothing), it has no `Content-Encoding` or `Content-Range`, the
body is not empty, the status is not 1xx, 204 or 304, and `Cache-Control` has no `no-transform`. A compressed
response gets `Content-Encoding`, loses `Content-Length`, gets `Vary: Accept-Encoding`, and a strong `ETag` becomes
weak. `HEAD` gets the same headers as `GET` with an empty body (the router records the stripped length). In
`Accept-Encoding`, names are case-insensitive, the first entry of a repeated name wins, an unlisted codec takes the
`q` of `*`, and `x-gzip` is not recognised. `identity;q=0` with no usable codec gives 406, only for a response that
would otherwise be compressed. Error answers rendered by the error filter are not compressed. Encoding is
synchronous: level `Best` on a large body can take seconds, and it costs memory too (brotli quality 11 with a 4 MiB
window and zstd level 19 can use tens of MB per encode); there is no limit on concurrent encodes. Do not compress responses that mix secrets with
attacker-controlled input over TLS (BREACH).

#### `StaticResources`, `ResourceSource`, `DiskResources`, `EmbeddedResources`, `Resource`, `ResourceError`

`StaticResources::new(prefix, source)` is a `Controller` that serves `GET` and `HEAD` below `prefix` (a leading `/` is
added, trailing ones are removed, braces are literal); register it with `App::routes(Controller::routes(Arc::new(s)))`.
`.index(name)` sets the directory file (default `index.html`; an unclean name is ignored and turns the index off) and
`.cache_max_age(Duration)` the `Cache-Control` (`no-cache` for zero, else `public, max-age=N`). A source implements
`ResourceSource::read(&self, path) -> Result<Resource, ResourceError>` for an already cleaned relative path.
`Resource` has `bytes`, `content_type`, `etag` (with quotes) and `last_modified`; `ResourceError` is `NotFound`,
`TooLarge` (both 404, `TooLarge` logs a warning) and `Io` (500, logged, never shown). `DiskResources::new(root)`
canonicalises the root once and `.max_file_bytes(n)` caps the file size (default 10 MiB); `EmbeddedResources::new(&'static [(name, bytes)])`
serves compiled-in files. No Cargo feature is needed.

```rust
use std::sync::Arc;
use oxbloom_web::{App, Controller, EmbeddedResources, HttpMethod, Request, StaticResources};

static FILES: &[(&str, &[u8])] = &[("index.html", b"<h1>hello</h1>"), ("css/app.css", b"h1 { color: red }")];

let site = StaticResources::new("/", EmbeddedResources::new(FILES));
let app = App::new().routes(Controller::routes(Arc::new(site)))?;

let res = app.handle(Request::new(HttpMethod::Get, "/css/app.css")).await;
assert_eq!(res.headers()["content-type"], "text/css; charset=utf-8");
let res = app.handle(Request::new(HttpMethod::Get, "/..%2Fsecret")).await;
assert_eq!(res.status(), 404);
```

Paths must be in canonical raw form, because `oxbloom-security` matches raw paths and a file must have exactly one
spelling. These answer 404: an empty (`//`) or `.` segment (one trailing `/` is allowed), `..`, a segment starting
with `.`, a backslash, NUL or encoded `/`; a raw byte that browsers escape (non-ASCII, space, `"`, `#`, `<`, `>`, `?`,
a backtick, `{`, `}`); an escape of any other byte (so `%61`, `%2B`, `%3A` and `%5B`, `%5D`, `%5E`, `%7C` all fail;
write `[ ] ^ |` raw); an escape with lower-case hex; a segment over 255 bytes, a path over 1024 bytes, a symlink
leaving the root, and a file over the cap. The allowed escapes (non-ASCII bytes, the escape-only characters above and
`%25`) are decoded once, in upper-case hex: `/caf%C3%A9`, not `/café`. Index files are the one exception to the
single spelling: `/docs`, `/docs/` and `/docs/index.html` all serve `docs/index.html`, so protect a directory with
`/docs/**` (which also matches `/docs`), not with a rule on the index name or `/docs/*`. Mounted at `/`, the
catch-all route turns a non-`GET` request to an unknown path into 405 instead of 404. On unix, `DiskResources` opens files with `O_NOFOLLOW | O_NONBLOCK` on Linux (including
Android), Apple systems, FreeBSD, OpenBSD and NetBSD; other unix targets rely on the earlier checks. On a
case-insensitive file system `/ADMIN/x` can serve `admin/x`, which a case-sensitive raw rule does not match. A 405
carries `Allow: GET, HEAD, OPTIONS`. `If-None-Match` and `If-Modified-Since` give 304. Not supported: streaming
bodies, range requests, directory listings, SPA fallback, pre-compressed files.

#### `RequestLogFilter`

`RequestLogFilter::new(level: tracing::Level, headers: Vec<String>)` is a `Filter` that logs one event per
request after the response is produced. The event has the target `oxbloom::web::request` and the fields
`method`, `path`, `status`, `latency_ms` and `headers`. The crate depends on `tracing` only; a subscriber is
the application's business. With `oxbloom-boot` use `RequestLogAutoConfiguration` instead of building it by hand.

- The query string is never logged; only the path, cut at the first `?` when a hand-built request left the query
  in it. Control characters in the path and in header values are escaped.
- `headers` is the allowlist, matched case-insensitively. It is the operator's responsibility. Header values print
  as `name="value"`; these print as `name=<redacted>` even when allowlisted: `authorization`,
  `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`, `x-csrf-token` and `x-xsrf-token`, in any case and with `-` or `_`
  (not `.`), and any value flagged sensitive. A name that only contains a credential word, such as `x-my-token`,
  is not redacted; this includes a custom `security.csrf.header-name`, which must not be allowlisted.
- It takes the slot `order::REQUEST_LOG` (-400), the outermost, so it wraps error handling, the body limit and the
  security chain. The request log records 401, 403, 404, 405, 413 and 500 answers.

```rust
use oxbloom_web::{App, HttpMethod, Request, RequestLogFilter};

// `authorization` is allowlisted, but is logged as `authorization=<redacted>`.
let filter = RequestLogFilter::new(
    tracing::Level::DEBUG,
    vec!["Accept".into(), "Authorization".into()],
);
let res = App::new().filter(filter).handle(Request::new(HttpMethod::Get, "/x")).await;
assert_eq!(res.status(), 404);
```

#### `App::run(addr)`

Binds `addr` and serves until Ctrl-C, then drains open connections.

```rust
App::new().routes(Arc::new(Users).routes())?
    .run("127.0.0.1:8080".parse()?).await?;
```

#### `App::serve(listener, shutdown)`

Serves on a listener you own until the `shutdown` future completes. Use it in tests to get a free port.

```rust
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
let addr = listener.local_addr()?;
let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
let server = tokio::spawn(App::new().serve(listener, async { let _ = stopped.await; }));
// ... send requests to `addr` ...
stop.send(()).unwrap();
server.await??;
```

#### `App::serve_tls(listener, acceptor, shutdown)` (feature `tls`)

Serves HTTPS on a listener you own: `App::serve` with a TLS handshake in front of every connection. Use it
when you build the `rustls::ServerConfig` yourself; in an application, `server.ssl.bundle` does this for
you (see [`WebServer::with_ssl`](#webserverwith_sslapp-config-bundles-feature-ssl)). The feature `tls` is off
by default and adds `tokio-rustls` without a crypto provider, so the caller's configuration decides it.

- Shutdown, `shutdown_timeout`, the header read timeout, the body limit, the filters and the router work as
  in `App::serve`.
- Each connection is handshaken in its own task. A client that has not finished its handshake after
  10 seconds is dropped, and a handshake still running when shutdown starts is dropped at once.
- A failed handshake is logged at `debug` on target `oxbloom::web` with the peer address and the error kind,
  never with bytes the client sent.
- HTTP/1.1 only: set `alpn_protocols` to `http/1.1` alone, so an `h2`-only client fails the handshake. A
  connection that still negotiates another protocol is closed; a client that offers no ALPN is served.
- `Request::scheme()` is `Scheme::Https`, unless a trusted proxy sends a forwarded `proto` that says
  otherwise. A trusted peer without a `proto` (for example a local client under the default loopback list)
  keeps `Https`.

```rust
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

// `certs` and `key` are the server's certificate chain and private key.
let mut config = ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?;
config.alpn_protocols = vec![b"http/1.1".to_vec()];

let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
let acceptor = TlsAcceptor::from(Arc::new(config));
app.serve_tls(listener, acceptor, async { let _ = tokio::signal::ctrl_c().await; }).await?;
```

HTTP behavior to remember: unknown path → 404; known path with wrong method → 405 plus `Allow`;
`HEAD` falls back to `GET` with the body stripped; `OPTIONS` → 204 plus `Allow`.

### 4.4 `oxbloom-di`

A singleton container. Every bean is created once and shared as `Arc<T>`: eager beans during
`build()`, lazy beans on their first request. Wiring errors of eager beans surface from `build()`.

Imports:

```rust
use std::sync::Arc;
use oxbloom_di::{
    bean, component, configuration, Component, Configuration, Container, ContainerBuilder,
    DiError, Lifecycle, Resolver, ShutdownReport,
};
```

Injectable shapes, usable as `#[component]` fields, as `bean` parameters and through `Resolver`
and `Container`:

| Shape | Meaning |
|---|---|
| `Arc<T>`, `Arc<dyn Trait>` | exactly one provider, or the single `primary` provider; otherwise `Missing` or `Ambiguous` |
| `Option<Arc<..>>` | `None` when nothing provides the key; a provider that fails to create is still an error |
| `Vec<Arc<..>>` | every provider, sorted by `order` and then by registration order; empty when there is none |

A provided trait must be `Send + Sync` (`trait Notifier: Send + Sync`). `dyn Notifier` and
`dyn Notifier + Send + Sync` are different keys; request the one you provide.

Modifiers (`provides`, `primary`, `order`, `lazy`, `profile`, `profile_not`, the conditions
`condition`, `on_bean`, `on_missing_bean`, `on_property`, `on_property_value` and
`on_property_value_or_missing`, `post_construct`, `pre_destroy`, `lifecycle`) apply to the
registration just before them. Builder methods never fail
mid-chain: a modifier before any registration, or one whose type does not match the last
registration, makes `build()` return `DiError::Invalid`.

#### `ContainerBuilder::new()`

```rust
let builder = ContainerBuilder::new();
```

#### `ContainerBuilder::component::<T>()`

Registers a type that implements `Component` (use `#[component]`). Registration order does not matter;
dependencies are built first. After pushing the entry it calls `T::configure`, which applies the
`#[component(..)]` options.

```rust
#[component]
struct UserRepo {}

#[component]
struct UserService { repo: Arc<UserRepo> }

let container = ContainerBuilder::new()
    .component::<UserService>()      // depends on UserRepo, registered after: fine
    .component::<UserRepo>()
    .build()?;
```

#### `ContainerBuilder::instance(value)`

Registers a ready-made value, for things you create yourself (pools, clients, settings).

```rust
struct Settings { name: &'static str }

let container = ContainerBuilder::new()
    .instance(Settings { name: "demo" })
    .build()?;
assert_eq!(container.get::<Settings>()?.name, "demo");
```

#### `ContainerBuilder::factory(closure)`

Registers a closure that builds `T` and may request other beans through the `Resolver`.

```rust
struct Banner(String);

let container = ContainerBuilder::new()
    .instance(Settings { name: "demo" })
    .factory(|r: &Resolver| {
        let s = r.get::<Settings>()?;
        Ok(Banner(format!("== {} ==", s.name)))
    })
    .build()?;
assert_eq!(container.get::<Banner>()?.0, "== demo ==");
```

#### `ContainerBuilder::bean(f)`

Registers the result of a closure or function whose parameters (0 to 12) are injected. Use it for
third-party types and computed values. The bean is the return type: a function returning `Arc<X>`
registers the bean `Arc<X>`, and one returning `Result<T, DiError>` registers that `Result` (use
`try_bean` instead). The source label is `closure at file:line`. A parameter that is not an
injectable shape is a compile error.

```rust
struct Settings { db_url: &'static str }
struct Pool { url: String, size: u32 }
trait Clock: Send + Sync { fn now(&self) -> u64; }
struct Audit { pool: Arc<Pool>, clock: Option<Arc<dyn Clock>> }

fn pool(settings: Arc<Settings>) -> Pool {
    Pool { url: settings.db_url.to_owned(), size: 4 }
}

let c = ContainerBuilder::new()
    .instance(Settings { db_url: "postgres://db/app" })
    .bean(pool)
    .bean(|pool: Arc<Pool>, clock: Option<Arc<dyn Clock>>| Audit { pool, clock })
    .build()?;
assert!(c.get::<Audit>()?.clock.is_none());

let err = ContainerBuilder::new().bean(pool).build();     // Settings is missing
assert!(matches!(err, Err(DiError::Missing { .. })));
```

#### `ContainerBuilder::try_bean(f)`

Like `bean`, for a function returning `Result<T, DiError>`; the bean is `T`. An `Err` fails the
bean's creation with `DiError::Creation` (a `Creation` keeps its message; another variant becomes the
fixed text `bean failed: {error}`).

```rust
struct Port(u16);
struct Listener { port: u16 }

fn listener(port: Arc<Port>) -> Result<Listener, DiError> {
    if port.0 == 0 {
        return Err(DiError::creation::<Listener>("port must not be 0"));
    }
    Ok(Listener { port: port.0 })
}

let c = ContainerBuilder::new().instance(Port(8080)).try_bean(listener).build()?;
assert_eq!(c.get::<Listener>()?.port, 8080);

let failed = ContainerBuilder::new().instance(Port(0)).try_bean(listener).build();
assert!(matches!(failed, Err(DiError::Creation { .. })));
```

#### `ContainerBuilder::configuration::<C>()` and `Configuration`

Adds the registrations of a `Configuration` (`C::register(builder)`). Use it to group the beans of
one module or library. A modifier written after `.configuration::<C>()` applies only to the last
registration `C` made; put modifiers inside `register`.

```rust
trait Mailer: Send + Sync { fn send(&self) -> &'static str; }
struct Smtp;
impl Mailer for Smtp { fn send(&self) -> &'static str { "smtp" } }

struct MailConfig;
impl Configuration for MailConfig {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.bean(|| Smtp).provides(|s: Arc<Smtp>| -> Arc<dyn Mailer> { s })
    }
}

let c = ContainerBuilder::new().configuration::<MailConfig>().build()?;
assert_eq!(c.get::<dyn Mailer>()?.send(), "smtp");
```

#### `ContainerBuilder::provides(coerce)`

Makes the last registration also available as `D`, typically a trait object. The closure names the
concrete type and returns the unsized `Arc`; both keys share one allocation. A closure whose type is
not the last registration's, or a sized `D`, is `Invalid` at `build()`.

```rust
trait Notifier: Send + Sync { fn notify(&self) -> String; }
struct Mailer;
impl Notifier for Mailer { fn notify(&self) -> String { "mail sent".into() } }

let c = ContainerBuilder::new()
    .instance(Mailer)
    .provides(|m: Arc<Mailer>| -> Arc<dyn Notifier> { m })
    .build()?;
assert_eq!(c.get::<dyn Notifier>()?.notify(), "mail sent");
assert!(c.get::<Mailer>().is_ok());                          // still available as itself

struct Sms;
impl Notifier for Sms { fn notify(&self) -> String { "sms sent".into() } }
let wrong = ContainerBuilder::new()
    .instance(Mailer)
    .provides(|s: Arc<Sms>| -> Arc<dyn Notifier> { s })      // not the last registration
    .build();
assert!(matches!(wrong, Err(DiError::Invalid { .. })));
```

#### `ContainerBuilder::primary()`

Prefers the last registration when a single value of one of its keys has several providers. Without
exactly one `primary`, a single request is `Ambiguous`; the container never picks silently.

```rust
trait Store: Send + Sync { fn kind(&self) -> &'static str; }
struct Memory;
impl Store for Memory { fn kind(&self) -> &'static str { "memory" } }
struct Disk;
impl Store for Disk { fn kind(&self) -> &'static str { "disk" } }

let c = ContainerBuilder::new()
    .instance(Memory).provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
    .instance(Disk).provides(|s: Arc<Disk>| -> Arc<dyn Store> { s }).primary()
    .build()?;
assert_eq!(c.get::<dyn Store>()?.kind(), "disk");

let c = ContainerBuilder::new()
    .instance(Memory).provides(|s: Arc<Memory>| -> Arc<dyn Store> { s })
    .instance(Disk).provides(|s: Arc<Disk>| -> Arc<dyn Store> { s })
    .build()?;
assert!(matches!(c.get::<dyn Store>(), Err(DiError::Ambiguous { .. })));
```

#### `ContainerBuilder::order(n)`

Sets the position of the last registration in `Vec` requests: ascending `n` (default `0`), then
registration order.

```rust
trait Step: Send + Sync { fn name(&self) -> &'static str; }
struct Auth;
impl Step for Auth { fn name(&self) -> &'static str { "auth" } }
struct Log;
impl Step for Log { fn name(&self) -> &'static str { "log" } }

let c = ContainerBuilder::new()
    .instance(Log).provides(|s: Arc<Log>| -> Arc<dyn Step> { s }).order(10)
    .instance(Auth).provides(|s: Arc<Auth>| -> Arc<dyn Step> { s }).order(-1)
    .build()?;
let names: Vec<_> = c.get_all::<dyn Step>()?.iter().map(|s| s.name()).collect();
assert_eq!(names, ["auth", "log"]);
```

#### `ContainerBuilder::lazy()`

Creates the last registration on its first request instead of during `build()`. An eager bean that
depends on it creates it during `build()`. Its errors appear at first use and are cached like any
other; an `Option` request whose only provider is lazy and fails is that error, never `None`.

Lazy creation is serialized by one container-wide lock, reentrant for the creating thread:
concurrent first requests create the bean once, and two threads never deadlock on each other's
lazy chains. A lazy provider must not wait for another thread that itself creates a lazy bean of
the same container; that would deadlock.

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

struct SearchIndex { docs: usize }

let loads = Arc::new(AtomicUsize::new(0));
let counter = Arc::clone(&loads);
let c = ContainerBuilder::new()
    .factory(move |_: &Resolver| {
        counter.fetch_add(1, Ordering::SeqCst);   // expensive: runs only when needed
        Ok(SearchIndex { docs: 3 })
    })
    .lazy()
    .build()?;
assert_eq!(loads.load(Ordering::SeqCst), 0);
assert_eq!(c.get::<SearchIndex>()?.docs, 3);
assert_eq!(loads.load(Ordering::SeqCst), 1);
```

#### `ContainerBuilder::active_profiles(names)`

Sets the active profile names (default: none). It is not a modifier and may come anywhere in the
chain; calling it again replaces the list. The container does not read configuration itself; with
`oxbloom-config`, pass `Config::active_profiles`:

```rust
let container = ContainerBuilder::new()
    .active_profiles(config.active_profiles().iter().cloned())
    .config(config)                         // `ContainerBuilderExt::config` does not forward them
    .component::<DemoData>()
    .build()?;
```

#### `ContainerBuilder::profile(name)` and `ContainerBuilder::profile_not(name)`

Keep the last registration only when the profile is active (`profile("dev")`) or inactive
(`profile("!prod")`, `profile_not("prod")`). Several conditions on one registration are any-of. A
skipped registration is invisible: it is not created, not a candidate for any key, never
`Duplicate`, and a dependency on it is `Missing`. An empty name (also `"!"`) is `Invalid`. Both set
a condition on the last registration; they do not activate a profile. The active set comes from
`ContainerBuilder::active_profiles`.

```rust
trait Mailer: Send + Sync { fn kind(&self) -> &'static str; }
struct Smtp;
impl Mailer for Smtp { fn kind(&self) -> &'static str { "smtp" } }
struct Console;
impl Mailer for Console { fn kind(&self) -> &'static str { "console" } }

let builder = |active: &[&'static str]| {
    ContainerBuilder::new()
        .active_profiles(active.to_vec())
        .instance(Smtp).provides(|m: Arc<Smtp>| -> Arc<dyn Mailer> { m }).profile("prod")
        .instance(Console).provides(|m: Arc<Console>| -> Arc<dyn Mailer> { m })
        .profile("dev").profile("test")     // dev or test
};
assert_eq!(builder(&["prod"]).build()?.get::<dyn Mailer>()?.kind(), "smtp");
assert_eq!(builder(&["test"]).build()?.get::<dyn Mailer>()?.kind(), "console");
assert!(matches!(builder(&[]).build()?.get::<dyn Mailer>(), Err(DiError::Missing { .. })));

struct DemoData;
let c = ContainerBuilder::new().active_profiles(["prod"]).instance(DemoData).profile_not("prod").build()?;
assert!(matches!(c.get::<DemoData>(), Err(DiError::Missing { .. })));
```

#### Conditions: the model

A condition keeps the last registration only when it holds. Conditions are modifiers like
`profile`; `build()` evaluates them once, after profile filtering and before any provider runs,
in registration order. Each registration sees only the registrations kept before it: never
itself, never a later one, and never one skipped by a profile or another condition. Several
conditions on one registration are combined with AND (profile conditions are any-of); they run
in the order they were added and evaluation stops at the first one that does not match, so the
later ones are neither run nor recorded. A skipped registration is invisible, exactly like one
skipped by a profile. A condition modifier before any registration makes `build()` return
`Invalid` ("`on_bean` needs a registration before it"). A panicking condition is not caught: the
panic propagates out of `build()`. Reasons in the report are fixed text, type names and key
names, never configuration values.

Imports for the examples below:

```rust
use oxbloom_di::{
    ConditionContext, ConditionOutcome, ConditionRecord, ConditionReport, Conditions,
    ContainerBuilder, DiError, Environment, Resolver,
};
```

#### `Environment`

The property source of the property conditions: `property(key)` returns the text of the scalar
at `key`, or `None` when it is absent or not a scalar. `contains(key)` says whether `key` is
present at all, a section or a list included; its default is `property(key).is_some()`, and
`on_property` asks it. `oxbloom-config`'s `Config` overrides it, so a table or a list counts as
present. Implement it for a test map or use
`oxbloom-config`'s `Config` (feature `di`, see [4.5](#45-oxbloom-config)). The examples below
reuse `Props` from this one.

```rust
use std::collections::HashMap;

struct Props(HashMap<&'static str, &'static str>);
impl Environment for Props {
    fn property(&self, key: &str) -> Option<String> {
        self.0.get(key).map(|v| (*v).to_owned())
    }
}

let env = Props(HashMap::from([("metrics.enabled", "true")]));
assert_eq!(env.property("metrics.enabled").as_deref(), Some("true"));
assert_eq!(env.property("metrics.port"), None);
// The default `contains` is `property(key).is_some()`.
assert!(env.contains("metrics.enabled"));
assert!(!env.contains("metrics"));
```

#### `ContainerBuilder::environment(env)`

Sets the `Environment` of the property conditions and of `ConditionContext::property`. Without
one, every property is absent. It is not a modifier: it may come anywhere in the chain, and a
second call replaces the first.

```rust
struct Audit;

let c = ContainerBuilder::new()
    .instance(Audit).on_property_value("feature.audit", "on")
    .environment(Props(HashMap::from([("feature.audit", "on")])))
    .build()?;
assert!(c.get_opt::<Audit>()?.is_some());
```

#### `ContainerBuilder::condition(name, f)`, `ConditionContext` and `ConditionOutcome`

A custom condition. `f` gets a `ConditionContext` with `has_bean::<T>()` (a registration kept
before this one provides `T`), `property(key)`, `contains(key)` (the key is present, a section
or a list included), `active_profiles()` and `registration()` (the
label of the registration being decided, as the report shows it), and returns
`ConditionOutcome::matched()` or `ConditionOutcome::skipped(reason)`. `name` labels the condition
in the report. `ConditionOutcome::is_matched()` and `reason()` read an outcome back; the reason of
`matched()` is `matched`.

```rust
struct Cache;
struct Warmup;

let needs_cache = |cx: &ConditionContext| {
    if cx.has_bean::<Cache>() && cx.active_profiles().iter().any(|p| p == "dev") {
        ConditionOutcome::matched()
    } else {
        ConditionOutcome::skipped("needs a cache in the dev profile")
    }
};
let c = ContainerBuilder::new()
    .active_profiles(["dev"])
    .instance(Cache)
    .instance(Warmup).condition("needs_cache", needs_cache)
    .build()?;
assert!(c.get_opt::<Warmup>()?.is_some());

let c = ContainerBuilder::new()
    .instance(Warmup).condition("needs_cache", needs_cache)
    .build()?;
assert!(c.get_opt::<Warmup>()?.is_none());
assert_eq!(c.condition_report().records()[0].reason(), "needs a cache in the dev profile");

let outcome = ConditionOutcome::skipped("disabled");
assert!(!outcome.is_matched());
assert_eq!(ConditionOutcome::matched().reason(), "matched");
```

#### `ContainerBuilder::on_bean::<T>()`

Keeps the last registration only when a registration kept before it provides `T`, as its concrete
type or through `provides` (so `T` may be `dyn Trait`). A `T` registered later does not count.

```rust
struct Pool;
struct HealthCheck;

let c = ContainerBuilder::new()
    .instance(Pool)
    .instance(HealthCheck).on_bean::<Pool>()
    .build()?;
assert!(c.get_opt::<HealthCheck>()?.is_some());

// `Pool` registered after the condition does not count.
let c = ContainerBuilder::new()
    .instance(HealthCheck).on_bean::<Pool>()
    .instance(Pool)
    .build()?;
assert!(c.get_opt::<HealthCheck>()?.is_none());
```

#### `ContainerBuilder::on_missing_bean::<T>()`

Keeps the last registration only when no registration kept before it provides `T`: the back-off
rule for defaults. Register the application's beans first; a default registered after them with
`on_missing_bean` steps aside.

```rust
trait Clock: Send + Sync { fn now(&self) -> u64; }
struct SystemClock;
impl Clock for SystemClock { fn now(&self) -> u64 { 1 } }
struct FixedClock;
impl Clock for FixedClock { fn now(&self) -> u64 { 42 } }

let with_default = |b: ContainerBuilder| {
    b.instance(SystemClock)
        .provides(|c: Arc<SystemClock>| -> Arc<dyn Clock> { c })
        .on_missing_bean::<dyn Clock>()
};
assert_eq!(with_default(ContainerBuilder::new()).build()?.get::<dyn Clock>()?.now(), 1);

let user = ContainerBuilder::new()
    .instance(FixedClock)
    .provides(|c: Arc<FixedClock>| -> Arc<dyn Clock> { c });
assert_eq!(with_default(user).build()?.get::<dyn Clock>()?.now(), 42);
```

#### `ContainerBuilder::on_property(key)`

Keeps the last registration only when the environment has the property `key`, whatever its value.
Presence is `Environment::contains`, so for `Config` a table or a list also counts. It also matches when the value is `false`, unlike Spring's `@ConditionalOnProperty`; for an on/off
switch use `on_property_value(key, "true")`. An empty key makes `build()` return `Invalid`
("`on_property` needs a non-empty key"), as for `on_property_value` and
`on_property_value_or_missing`.

```rust
struct Mailer;

let c = ContainerBuilder::new()
    .environment(Props(HashMap::from([("smtp.host", "mail.internal")])))
    .instance(Mailer).on_property("smtp.host")
    .build()?;
assert!(c.get_opt::<Mailer>()?.is_some());

let c = ContainerBuilder::new().instance(Mailer).on_property("smtp.host").build()?;
assert!(c.get_opt::<Mailer>()?.is_none()); // no environment: absent
```

#### `ContainerBuilder::on_property_value(key, value)`

Keeps the last registration only when the property `key` equals `value` exactly: case-sensitive,
no trimming, no substring match. `TRUE` does not match `"true"`, unlike Spring's `havingValue`. An
absent key does not match. The report records `property "<key>" does not have the expected value`
and never shows either value. The expected value sits in the source code and is compared as plain
text, so never pass a secret as `value`.

```rust
struct RedisCache;

let build = |kind: &'static str| {
    ContainerBuilder::new()
        .environment(Props(HashMap::from([("cache.kind", kind)])))
        .instance(RedisCache).on_property_value("cache.kind", "redis")
        .build()
};
assert!(build("redis")?.get_opt::<RedisCache>()?.is_some());
assert!(build("redis-cluster")?.get_opt::<RedisCache>()?.is_none());
```

#### `ContainerBuilder::on_property_value_or_missing(key, value)`

Like `on_property_value`, but also matches when `key` is absent: for a default that applies unless
it is configured otherwise. On a `Config` table or list the key still counts as not set (only
`on_property` counts a section as present), so the condition matches.

```rust
struct MemoryCache;

let build = |props: Props| {
    ContainerBuilder::new()
        .environment(props)
        .instance(MemoryCache).on_property_value_or_missing("cache.kind", "memory")
        .build()
};
assert!(build(Props(HashMap::new()))?.get_opt::<MemoryCache>()?.is_some());
assert!(build(Props(HashMap::from([("cache.kind", "redis")])))?.get_opt::<MemoryCache>()?.is_none());
```

#### `Conditions` and `ContainerBuilder::group(name, conditions, f)`

Class-level conditions. `Conditions` holds the same conditions as the builder modifiers
(`new()`, `condition`, `on_bean`, `on_missing_bean`, `on_property`, `on_property_value`,
`on_property_value_or_missing`), ANDed in the order they were added. `group` runs `f` and treats
every registration `f` makes as one group named `name`:

- The group's conditions are decided once, at the group's first registration that a profile keeps,
  against the registrations kept before it; the group's own registrations do not count, so a group
  may register the `T` its `on_missing_bean::<T>()` asks about.
- When they do not match, every registration of the group is skipped, nested groups included,
  without evaluating their own conditions. When they match, each registration's own conditions are
  evaluated as usual.
- Nested groups AND their conditions, the outermost decided first. A group with no registrations
  is a no-op. A group whose registrations are all skipped by profiles is never decided: its
  conditions do not run and it leaves no record.
- The start of a group is a modifier barrier: a modifier written inside `f` before its first
  registration is `Invalid` and never changes the registration before the group. A modifier after
  `group(..)` applies to the last registration made inside it.
- `f` should extend the builder it is given. If it returns a different builder, that builder is
  used as it is and the group has no effect.
- A property condition in `conditions` with an empty key makes `build()` return `Invalid`.

```rust
trait Cache: Send + Sync { fn kind(&self) -> &'static str; }
struct MemoryCache;
impl Cache for MemoryCache { fn kind(&self) -> &'static str { "memory" } }
struct Redis;
impl Cache for Redis { fn kind(&self) -> &'static str { "redis" } }
struct CacheStats { cache: Arc<dyn Cache> }

let defaults = |b: ContainerBuilder| {
    b.group("CacheDefaults", Conditions::new().on_missing_bean::<dyn Cache>(), |b| {
        b.instance(MemoryCache)
            .provides(|c: Arc<MemoryCache>| -> Arc<dyn Cache> { c })
            .factory(|r: &Resolver| Ok(CacheStats { cache: r.get::<dyn Cache>()? }))
    })
};
assert_eq!(defaults(ContainerBuilder::new()).build()?.get::<CacheStats>()?.cache.kind(), "memory");

// The application's cache comes first: the whole group backs off.
let user = ContainerBuilder::new()
    .instance(Redis)
    .provides(|c: Arc<Redis>| -> Arc<dyn Cache> { c });
let c = defaults(user).build()?;
assert_eq!(c.get::<dyn Cache>()?.kind(), "redis");
assert!(c.get_opt::<CacheStats>()?.is_none());
let record = &c.condition_report().records()[0];
assert_eq!((record.registration(), record.matched()), ("CacheDefaults", false));
```

#### `ContainerBuilder::post_construct(f)`

Runs `f(&T)` once, right after the last registration's bean is created and after the hooks of its
dependencies; no dependent receives the bean before it succeeds. An `Err` is a `Creation` error and
aborts `build()`; beans created before it are destroyed in reverse order first.

```rust
struct Limits { max_users: u32 }

let c = ContainerBuilder::new()
    .instance(Limits { max_users: 100 })
    .post_construct(|l: &Limits| {
        if l.max_users == 0 {
            return Err(DiError::creation::<Limits>("max_users must be positive"));
        }
        Ok(())
    })
    .build()?;
```

#### `ContainerBuilder::pre_destroy(f)`

Runs `f(&T)` when the bean is destroyed by `Container::shutdown` (or by a failed `build()` after the
bean was created). A panic in it is caught.

```rust
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Connection { closed: AtomicBool }

let c = ContainerBuilder::new()
    .instance(Connection::default())
    .pre_destroy(|conn: &Connection| conn.closed.store(true, Ordering::SeqCst))
    .build()?;
let conn = c.get::<Connection>()?;
assert_eq!(c.shutdown().destroyed, 1);
assert!(conn.closed.load(Ordering::SeqCst));
```

#### `Lifecycle` and `ContainerBuilder::lifecycle::<T>()`

A type implements `Lifecycle` (`post_construct`, `pre_destroy`, both with empty defaults) on itself;
`.lifecycle::<T>()` registers both hooks for the last registration, which must be a `T`.
`#[component(lifecycle)]` does the same for a component.

```rust
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Pool { open: AtomicBool }

impl Lifecycle for Pool {
    fn post_construct(&self) -> Result<(), DiError> {
        self.open.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn pre_destroy(&self) {
        self.open.store(false, Ordering::SeqCst);
    }
}

let c = ContainerBuilder::new().instance(Pool::default()).lifecycle::<Pool>().build()?;
let pool = c.get::<Pool>()?;
assert!(pool.open.load(Ordering::SeqCst));
c.shutdown();
assert!(!pool.open.load(Ordering::SeqCst));
```

#### `ContainerBuilder::build()`

Reports builder misuse as `Invalid`, drops the registrations whose profile conditions fail, then
evaluates the other conditions in registration order and drops the registrations they skip (see
"Conditions: the model"), then creates every eager bean in dependency order and returns `Result<Container, DiError>`. A failed
creation is cached: later requests get the same error and the provider does not run again.
`Cycle` and `TooDeep` are never cached, because they describe the requesting stack; a provider that
wraps such an error into its own `Creation` (for example with `map_err`) makes it an ordinary,
cached failure. A lazy bean in a cycle therefore re-runs the cycle's providers on every request. A
panicking provider or `post_construct` is not caught.

Creation is recursive and capped at a dependency depth of 256 (`TooDeep`). The stack a 256-deep
chain needs grows with the providers' stack frames. Measured on x86_64 Linux in a debug build: about
206 KB for simple closures, 278 KB for `#[component]` structs with four fields, 298 KB for `.bean`
functions with four parameters and 322 KB for `#[bean]` methods with four parameters; release
builds need 79 to 119 KB. The main thread has 8 MB; run very deep graphs on a thread with enough
stack.

```rust
let result = ContainerBuilder::new().component::<UserService>().build();
assert!(matches!(result, Err(DiError::Missing { .. })));   // UserRepo was never registered

// Build a very deep graph on a thread with a large stack.
let container = std::thread::Builder::new()
    .stack_size(8 * 1024 * 1024)
    .spawn(|| ContainerBuilder::new().component::<UserRepo>().component::<UserService>().build())?
    .join()
    .unwrap()?;
```

#### `ContainerBuilder::build_with_report()`

Like `build()`, and also returns the `ConditionReport`, also when the build fails:
`(Result<Container, DiError>, ConditionReport)`. The steps are: builder misuse is `Invalid` with an
empty report; profile filtering; the condition walk; then provider resolution. A failure in the
last step still returns the report of the walk.

```rust
struct Server;
struct Db;
struct Repo { _db: Arc<Db> }

let (built, report) = ContainerBuilder::new()
    .instance(Server).on_missing_bean::<Server>()
    .factory(|r: &Resolver| Ok(Repo { _db: r.get::<Db>()? }))
    .build_with_report();
assert!(matches!(built, Err(DiError::Missing { .. }))); // nothing provides `Db`
assert_eq!(report.records().len(), 1);
assert!(report.records()[0].matched());
```

#### `Container::condition_report()`

The `ConditionReport` of the build that created the container.

```rust
struct Server;
struct DefaultServer;

let c = ContainerBuilder::new()
    .instance(Server)
    .instance(DefaultServer).on_missing_bean::<Server>()
    .build()?;
let record = &c.condition_report().records()[0];
assert!(!record.matched());
assert!(record.reason().starts_with("an earlier registration already provides"));
```

#### `ConditionReport` and `ConditionRecord`

`ConditionReport::records()` lists every evaluated condition in evaluation order: registration
order, and within one registration its conditions up to the first one that did not match. A
group's records use the group's name as registration and come before the records of the
registration that decided it. A registration skipped by a profile has no record. Each
`ConditionRecord` has `registration()` (a source label such as `instance app::Cache`,
`closure at src/main.rs:12` or `Type::method`), `condition()` (`on_bean`, `on_missing_bean`,
`on_property`, `on_property_value`, `on_property_value_or_missing` or a custom name), `matched()`
and `reason()`.

`Display` renders one block with a `Matched:` and a `Skipped:` section, one line
`  <registration>: <condition> - <reason>` per record, `  (none)` for an empty section and no
trailing newline. Records are sorted into the sections by their own `matched` flag: a registration
whose first condition matched and whose second did not is skipped, yet its first record is listed
under `Matched:`.

```rust
struct Metrics;

let (built, report) = ContainerBuilder::new()
    .instance(Metrics).on_property("metrics.enabled")
    .build_with_report();
assert!(built.is_ok());
let record: &ConditionRecord = &report.records()[0];
assert_eq!(record.condition(), "on_property");
assert!(!record.matched());
let label = format!("instance {}", std::any::type_name::<Metrics>());
assert_eq!(record.registration(), label);
assert_eq!(
    report.to_string(),
    format!(
        "CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n  \
         {label}: on_property - property \"metrics.enabled\" is not set"
    )
);
assert!(ConditionReport::default().records().is_empty());
```

#### `Container::get::<T>()`

Returns the bean `T`, where `T` is a registered type or a provided key such as `dyn Trait`. A lazy
bean is created by its first request. Nothing providing `T` → `Missing`; after `shutdown()` →
`ShutDown`.

```rust
let service: Arc<UserService> = container.get::<UserService>()?;
let again = container.get::<UserService>()?;
assert!(Arc::ptr_eq(&service, &again));                    // same singleton
```

#### `Container::get_opt::<T>()`

Returns `None` when nothing provides `T`. A provider that exists but fails is an error, not `None`.

```rust
struct Metrics;
let c = ContainerBuilder::new().instance(7u32).build()?;
assert_eq!(c.get_opt::<u32>()?.as_deref(), Some(&7));
assert!(c.get_opt::<Metrics>()?.is_none());
```

#### `Container::get_all::<T>()`

Returns every provider of `T`, sorted by `order` then registration order; empty when there is none. Inside a
provider, use its `Resolver::get_all`, which leaves out the requester; `Container::get_all` through a
captured container does not and reports `Cycle`.

```rust
trait Plugin: Send + Sync { fn name(&self) -> &'static str; }
struct Gzip;
impl Plugin for Gzip { fn name(&self) -> &'static str { "gzip" } }
struct Cors;
impl Plugin for Cors { fn name(&self) -> &'static str { "cors" } }

let c = ContainerBuilder::new()
    .instance(Gzip).provides(|p: Arc<Gzip>| -> Arc<dyn Plugin> { p })
    .instance(Cors).provides(|p: Arc<Cors>| -> Arc<dyn Plugin> { p })
    .build()?;
let names: Vec<_> = c.get_all::<dyn Plugin>()?.iter().map(|p| p.name()).collect();
assert_eq!(names, ["gzip", "cors"]);
assert!(c.get_all::<u8>()?.is_empty());
```

#### `Container::shutdown()`, `ShutdownReport`, `DestroyFailure`

Runs the `pre_destroy` hooks of every created bean in reverse creation order (a lazy bean counts at
the position where it was created; one never created is not destroyed) and closes the container.
It returns a `ShutdownReport` with the fields `destroyed` and `failures`; a panicking hook is caught
and recorded as a `DestroyFailure` (fields `type_name`, `message`) with the fixed message
`pre_destroy panicked`. Both structs are `#[non_exhaustive]`: read their fields, do not build them. A second call
returns an empty report. Afterwards every `get*` is `ShutDown`; `Arc` handles handed out earlier stay
valid. Dropping a container does not shut it down. Shutdown waits for a lazy creation in flight on
another thread and destroys that bean too.

Called from inside a provider or `post_construct` hook on the thread that is creating a lazy bean,
`shutdown` closes the container at once: the rest of that chain creates nothing (its requests are
`ShutDown`), the bean being created is not handed out, and a bean whose own `post_construct` called
`shutdown` has its `pre_destroy` hooks run right after. Nothing created is left undestroyed.

```rust
use std::sync::Mutex;

struct Db;
struct Repo { _db: Arc<Db> }

let log = Arc::new(Mutex::new(Vec::new()));
let (l1, l2) = (log.clone(), log.clone());
let c = ContainerBuilder::new()
    .instance(Db).pre_destroy(move |_: &Db| l1.lock().unwrap().push("db closed"))
    .factory(|r: &Resolver| Ok(Repo { _db: r.get::<Db>()? }))
    .pre_destroy(move |_: &Repo| l2.lock().unwrap().push("repo flushed"))
    .build()?;

let report = c.shutdown();
assert_eq!((report.destroyed, report.failures.len()), (2, 0));
assert_eq!(*log.lock().unwrap(), ["repo flushed", "db closed"]);
assert!(matches!(c.get::<Repo>(), Err(DiError::ShutDown)));
assert_eq!(c.shutdown(), ShutdownReport::default());       // idempotent
```

#### `Component` trait and `Component::configure`

`fn create(r: &Resolver) -> Result<Self, DiError>`. Implement it by hand when the constructor needs
code or can fail. The defaulted `fn configure(builder) -> builder` adds modifiers to the component's
own registration; `#[component(..)]` generates it from its options.

```rust
struct Clock { start: std::time::Instant }

impl Component for Clock {
    fn create(_: &Resolver) -> Result<Self, DiError> {
        Ok(Clock { start: std::time::Instant::now() })
    }
}

trait Greeter: Send + Sync { fn greet(&self) -> &'static str; }
struct English;
impl Greeter for English { fn greet(&self) -> &'static str { "hello" } }

impl Component for English {
    fn create(_: &Resolver) -> Result<Self, DiError> { Ok(English) }
    fn configure(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provides(|e: Arc<English>| -> Arc<dyn Greeter> { e }).primary()
    }
}

let c = ContainerBuilder::new().component::<Clock>().component::<English>().build()?;
assert_eq!(c.get::<dyn Greeter>()?.greet(), "hello");
```

#### `Resolver::get::<D>()`, `Resolver::get_opt::<D>()`, `Resolver::get_all::<D>()`

Used inside `Component::create` and `factory` closures to fetch a dependency in one of the three
shapes; the dependency is created on demand.

```rust
trait Plugin: Send + Sync {}
struct Metrics;
struct Report { repo: Arc<UserRepo>, metrics: Option<Arc<Metrics>>, plugins: Vec<Arc<dyn Plugin>> }

impl Component for Report {
    fn create(r: &Resolver) -> Result<Self, DiError> {
        Ok(Report { repo: r.get()?, metrics: r.get_opt()?, plugins: r.get_all()? })
    }
}
```

#### `Inject`, `BeanFn`, `TryBeanFn`

`Inject` is implemented for the three shapes (`Arc<..>`, `Option<Arc<..>>`, `Vec<Arc<..>>`) and is
sealed: you cannot add your own shapes. `BeanFn<Args, T>` and `TryBeanFn<Args, T>` are implemented
for every `Fn` with 0 to 12 `Inject` parameters returning `T` or `Result<T, DiError>`; they appear
in the bounds of `bean` and `try_bean` and in generic code that forwards a bean function.

```rust
use oxbloom_di::{BeanFn, Inject};

struct Clock;
struct Audit { clock: Option<Arc<Clock>> }
let c = ContainerBuilder::new()
    .factory(|r: &Resolver| Ok(Audit { clock: Inject::inject(r)? }))
    .build()?;
assert!(c.get::<Audit>()?.clock.is_none());

struct Host(&'static str);
struct Url(String);
fn url(host: Arc<Host>) -> Url { Url(format!("https://{}", host.0)) }
fn with_host<Args, F: BeanFn<Args, Url>>(f: F) -> ContainerBuilder {
    ContainerBuilder::new().instance(Host("example.org")).bean(f)
}
assert_eq!(with_host(url).build()?.get::<Url>()?.0, "https://example.org");
```

#### `DiError`

`#[non_exhaustive]`; a `match` outside the crate needs a wildcard arm (`_ => ...`). Messages carry
type names, source labels and fixed text, never values; only `Creation.message` is the developer's
own text.

| Variant | Meaning |
|---|---|
| `Missing { requested, required_by }` | nothing provides a required key |
| `Ambiguous { requested, candidates }` | several providers and no single `primary`; `candidates` are source labels |
| `Cycle { path }` | circular dependency, e.g. `A -> B -> A`; never cached |
| `Duplicate { type_name, sources }` | the same concrete type registered twice; `sources` names both registrations |
| `Creation { type_name, message, source }` | a provider or `post_construct` failed; `source` is an `ErrorSource` with the typed cause, if any |
| `Invalid { message }` | builder misuse (modifier before any registration, wrong type, empty profile name) |
| `ShutDown` | a request after `shutdown()` |
| `TooDeep { limit, path }` | dependency depth over 256; `path` holds the first 8 entries; never cached |

Source labels: `component Type`, `instance Type`, `factory Type`, `Type::method` for `#[bean]`
methods, `closure at file:line` for `bean` and `try_bean`. All errors except `Cycle` and `TooDeep`
are cached per bean.

`Creation.source` is not part of equality, `Display` or `Debug`. Adding the field is a breaking change
for code outside the workspace that builds `Creation { type_name, message }` literally or matches it
without `..`, because the enum-level `#[non_exhaustive]` does not cover the fields of a variant. Build
the error with `DiError::creation` or `creation_from` and match it as `Creation { type_name, .. }`.

```rust
#[component]
struct A { b: Arc<B> }
#[component]
struct B { a: Arc<A> }

let err = ContainerBuilder::new().component::<A>().component::<B>().build().err().unwrap();
assert!(matches!(err, DiError::Cycle { .. }));
println!("{err}");     // message shows the cycle path A -> B -> A

let dup = ContainerBuilder::new().instance(1u8).instance(2u8).build().err().unwrap();
assert!(matches!(dup, DiError::Duplicate { .. }));   // sources: "instance u8" twice

match err {
    DiError::Missing { requested, .. } => println!("register {requested}"),
    DiError::Cycle { path } => println!("break the cycle {}", path.join(" -> ")),
    other => println!("{other}"),
}
```

#### `DiError::creation::<T>(message)`

Builds a `Creation` error. Return it from a factory, `create`, `try_bean` function or
`post_construct` hook when construction fails.

```rust
struct Db;
let result = ContainerBuilder::new()
    .factory(|_| -> Result<Db, DiError> { Err(DiError::creation::<Db>("cannot connect")) })
    .build();
assert!(matches!(result, Err(DiError::Creation { .. })));
```

#### `DiError::creation_from::<T>(error)`

Builds a `Creation` error for `T` that keeps `error` as its typed source. The message is the
error's `Display`, so do not pass an error whose text contains configuration values, argument
values or secrets. `std::error::Error::source` on the `DiError` returns the error, so a caller (or a
boot `FailureAnalyzer` through `StartupError::find_cause`) can downcast it. `try_bean` and
`#[bean(fallible)]` replace the type name with the failing bean's and keep the source. Use it
instead of `creation::<T>(e.to_string())` whenever the error type matters to someone downstream.

```rust
use std::error::Error;

struct Db;
let err = ContainerBuilder::new()
    .try_bean(|| -> Result<Db, DiError> {
        Err(DiError::creation_from::<Db>(std::io::Error::other("refused")))
    })
    .build()
    .err()
    .unwrap();
assert_eq!(err.to_string(), format!("failed to create {}: refused", std::any::type_name::<Db>()));
let io = err.source().and_then(|s| s.downcast_ref::<std::io::Error>());
assert_eq!(io.map(|e| e.kind()), Some(std::io::ErrorKind::Other));
```

#### `ErrorSource`: `new`, `none`, `is_some`, `as_error`

The `source` field of `DiError::Creation`: an optional error shared behind an `Arc`, so cloning a
cached `DiError` stays cheap. `ErrorSource::new(error)` holds an error, `ErrorSource::none()` (also
`Default`) is empty, `is_some()` tells them apart and `as_error()` returns the error for
`downcast_ref`. `Debug` prints `ErrorSource(..)` in both states, and any two sources are equal,
held or empty, so a source never changes the equality of a `DiError`. You need it only to build a
`Creation` literally; `creation` and `creation_from` fill it for you.

```rust
use oxbloom_di::ErrorSource;

let err = DiError::Creation {
    type_name: "app::Db".to_owned(),
    message: "cannot connect".to_owned(),
    source: ErrorSource::new(std::io::Error::other("refused")),
};
assert!(std::error::Error::source(&err).is_some());
// Equality ignores the source.
assert_eq!(err, DiError::Creation {
    type_name: "app::Db".to_owned(),
    message: "cannot connect".to_owned(),
    source: ErrorSource::none(),
});
let source = ErrorSource::new(std::fmt::Error);
assert!(source.is_some());
assert!(source.as_error().and_then(|e| e.downcast_ref::<std::fmt::Error>()).is_some());
assert_eq!(format!("{source:?}"), "ErrorSource(..)");
assert!(!ErrorSource::default().is_some());
```

### 4.5 `oxbloom-config`

Sources, lowest to highest priority: `application.toml` (+ its `oxbloom.config.import` files), the
`application-{profile}.toml` files of the active profiles (+ their imports), environment
variables, command-line arguments. Then renamed and removed keys are migrated and placeholders
are resolved. Tables merge deeply; scalars and arrays are replaced whole. The README
Configuration section describes every rule in one place.

Reserved top-level table: `oxbloom`. Only `oxbloom.config.import`, `oxbloom.profiles.active` and
`oxbloom.profiles.group.*` are interpreted by the loader; a user's own `[profiles] active = true` or
`[config] import = "auto"` are ordinary keys. `build()` removes `oxbloom.config.import` (and
`oxbloom.config`, then `oxbloom`, when that leaves them empty); `oxbloom.profiles.*` stays in the final
tree, so a root struct with `#[serde(deny_unknown_fields)]` must declare an `oxbloom` field or be
bound below the root. The pre-namespacing names `config.import`, `profiles.active`,
`profiles.group.<name>` and `--profiles.active=` are no longer interpreted: they silently become
plain data.

Imports:

```rust
use oxbloom_config::{Config, ConfigError, DataSize, Origin, Secret};
use serde::Deserialize;
```

Sample files used below:

```toml
# config/application.toml
[server]
host = "0.0.0.0"
port = 8080

[db]
password = "change-me"
```

```toml
# config/application-dev.toml
[server]
host = "127.0.0.1"
```

#### `Config::builder() -> ConfigBuilder`

```rust
let builder = Config::builder();
```

#### `ConfigBuilder::dir(path)`

Directory with the `application*.toml` files. Default `.`. A missing base file gives an empty config.

```rust
let config = Config::builder().dir("config").build()?;
```

#### `ConfigBuilder::profile(name)`

Activates `application-{name}.toml`. Repeatable: profiles apply in call order and later ones win.
When `profile` is never called, the active profiles come from the first non-empty of:
`--oxbloom.profiles.active=a,b` (see `args`), the `OXBLOOM_PROFILE` variable (`a,b`), `oxbloom.profiles.active`
in `application.toml` (string `"a,b"` or array). `oxbloom.profiles.active` in a profile file is
`ConfigError::Invalid`. Names allow letters, digits, `_` and `-` only. A requested profile
without a file is `ConfigError::Io`, also for a group name.

Groups: `oxbloom.profiles.group.prod = ["db", "metrics"]` in `application.toml` makes activating `prod`
activate `prod`, `db`, `metrics` in that order. Group definitions are read only from the base
file (and its imports); a group that includes itself is `Invalid`. Groups nest at most 8 levels
deep (`Invalid` for `oxbloom.profiles.group.<name>`) and at most 64 profiles are active after expansion,
repeats counted once (`Invalid` for `oxbloom.profiles.active`), whichever way the profiles are named.
`oxbloom.profiles.active` is also read from the base file's imports. `APP_OXBLOOM__PROFILES__ACTIVE` and the
indexed form `--oxbloom.profiles.active[0]=a` do not activate profiles; they only set the key.

```rust
let config = Config::builder().dir("config").profile("dev").build()?;
// server.host is "127.0.0.1" now, server.port still 8080

let config = Config::builder().dir("config").profile("prod").profile("eu").build()?;
// application-prod.toml, then application-eu.toml on top
```

#### `ConfigBuilder::env_prefix(prefix)`

Prefix of overriding environment variables. Default `APP`. `APP_SERVER__PORT=9000` sets `server.port`.
`__` separates path segments; a single `_` stays inside a key; keys are lowercased and match
relaxed. A numeric segment after the first is a list index (`APP_HOSTS__0=a`), so maps with
numeric keys (`APP_HTTP__STATUS__404`) cannot come from the environment or the command line, only
from files. More than 64
segments, a gap in the indices, or an index and a key under one path in the same source are
`ConfigError::Invalid`. Two spellings of one key in the environment or on the command line are not reported; they collapse to one key and the last variable name in byte order (command line: the last argument) wins.

```rust
// MYAPP_SERVER__PORT=9000 cargo run
let config = Config::builder().dir("config").env_prefix("MYAPP").build()?;
```

#### `ConfigBuilder::env_from(vars)`

Uses the given pairs instead of the process environment, for the environment source, for
`OXBLOOM_PROFILE` and for placeholder lookups in file values. Use it in tests so they do not depend on, or
change, real environment variables.

```rust
let config = Config::builder()
    .dir("config")
    .env_from([("APP_SERVER__PORT".to_string(), "9000".to_string())])
    .build()?;
```

#### `ConfigBuilder::args(iter)`

Command-line arguments, the highest-precedence source. Only `--key=value` is read: `.` separates
segments, `[n]` or `.n` is a list index. Arguments without `=`, not starting with `--`, or with
an empty key are ignored; a repeated key keeps the last value. The same 64-segment and
index/key-mix rules as for the environment apply.

```rust
let config = Config::builder()
    .dir("config")
    .args(["--server.port=9000", "--hosts[0]=a", "--verbose"])   // `--verbose` is ignored
    .build()?;
assert_eq!(config.get::<u16>("server.port")?, 9000);
assert_eq!(config.get::<Vec<String>>("hosts")?, ["a"]);
```

#### `ConfigBuilder::secret_key(key)`

Registers a dotted key (and everything below it) that must not be set on the command line, where
process listings show it. An argument for it is `ConfigError::Invalid` pointing to environment
variables or files; the value is never part of the error. A command-line value that `renamed`
would move onto a secret key is rejected the same way.

```rust
let result = Config::builder()
    .dir("config")
    .secret_key("db.password")
    .args(["--db.password=hunter2"])
    .build();
assert!(matches!(result, Err(ConfigError::Invalid { .. })));
```

#### `ConfigBuilder::renamed(old, new)`

Migrates an old key. If `old` is set and `new` is not, the value moves and keeps its origin; if
both are set, `new` wins and the old node is dropped. Either way a `ConfigWarning` is recorded.
Matching is canonical per segment, rules apply in registration order (so chains work), whole
tables can be renamed, and tables emptied by the move are removed. An empty or malformed path,
`old` equal to `new`, or one path inside the other makes `build()` fail with `Invalid`.
Migration runs before placeholders, so `${old}` no longer resolves.

```rust
let config = Config::builder().dir("config").renamed("server.hostname", "server.host").build()?;
for warning in config.warnings() {
    eprintln!("{warning}");   // key `server.hostname` is renamed to `server.host` (from application.toml:3)
}
```

#### `ConfigBuilder::removed(old, reason)`

Marks a key as removed. If it is set, the node stays in the tree and a `ConfigWarning` with
`reason` is recorded.

```rust
let config = Config::builder().dir("config").removed("legacy.mode", "no longer supported").build()?;
```

#### `ConfigBuilder::deny_deprecated(deny)`

With `true`, `build()` fails with `ConfigError::Deprecated` for the first warning instead of
recording it. Use it in CI to find stale keys.

```rust
let result = Config::builder()
    .dir("config")
    .removed("db.password", "use db.secret")
    .deny_deprecated(true)
    .build();
assert!(matches!(result, Err(ConfigError::Deprecated { .. })));
```

#### `ConfigBuilder::build()`

Reads the files and their imports, merges the profiles, applies environment and command-line
sources, migrates renamed keys, resolves placeholders. Returns `Result<Config, ConfigError>`.

`oxbloom.config.import = ["extra.toml", "optional:local.toml"]` in any config file imports files
relative to the config directory, merged over the importing file in list order. An absolute path,
`..`, or a file that resolves outside the directory (symlinks included) is `ImportInvalid`; a
missing file is `ImportMissing` unless prefixed `optional:`; a cycle is `ImportCycle`; more than
8 nested levels is `ImportTooDeep`; more than 256 entries followed in one load is `ImportLimit`.
The `oxbloom.config.import` key is not kept in the final tree. `application.toml` and the profile files
themselves are not symlink-checked. Every file read must be a regular file after following
symlinks (otherwise `ConfigError::Io` with `ErrorKind::InvalidInput`; a FIFO or device is never
opened) of at most the size limit (otherwise `ConfigError::FileTooLarge`).

```rust
let config = Config::builder().dir("config").profile("dev").build()?;
```

#### `ConfigBuilder::max_file_size(bytes)`

Largest configuration file `build()` reads, for `application.toml`, profile files and imports.
Default 1 MiB; at most `bytes + 1` bytes are read from a file. A larger file is
`ConfigError::FileTooLarge`; `0` makes every non-empty file too large.

```rust
let config = Config::builder().dir("config").max_file_size(64 * 1024).build()?;
```

#### `Config::from_str(toml)`

Parses a TOML string. No filesystem, no environment. Placeholders resolve against the document
and their defaults only. Imports, profiles, environment, command line and migration are skipped,
and a `oxbloom.config.import` key stays an ordinary key (`build()` follows and removes it). Ideal for
unit tests.

```rust
let config = Config::from_str("[server]\nport = 8080")?;
```

#### `Config::get::<T>(prefix)`

Binds the subtree at a dotted prefix to any `serde::Deserialize` type. Use `""` for the whole config.
Environment and command-line values are strings; they are parsed into the integer, float or bool a
field expects, and string fields are never coerced. Keys match relaxed: case, `-` and `_` are
ignored, so `maxConnections`, `max-connections` and `MAX_CONNECTIONS` all bind the field
`max_connections` (also in the prefix). Map keys are canonical as well: they keep the spelling of the
source that set them (an environment override gives the lowercased variable spelling), and two
spellings of one map key in one file are `DuplicateKey`. Two
spellings of one key in the same file are `ConfigError::DuplicateKey` (not detected in the environment or on the
command line, where the last variable name in byte order, or the last argument, wins); across sources the higher
one wins.

Key matching details: a key equal to a declared name (field name, `rename` or `alias`) binds that
name; otherwise the first declared name with the same canonical form wins. Two declared names with
one canonical form are `Invalid` only when they cannot be the sorted name run of a single field;
other cases (fields `jwt_secret` and `jwtsecret` in that order) are not detected, so do not declare
canonically equal field names. `rename` plus `alias` of one field (`jwk-set-uri`, `jwk_set_uri`) is
fine. Fields of a struct with `#[serde(flatten)]`, and fields and tags of internally tagged enums,
are matched exactly.

Lists: a string bound to a sequence is split on commas (items trimmed, `\,` is a literal comma, a
backslash is special only before a comma (one backslash before a comma, `a\,b`, gives the item `a,b`; two, `a\\,b`, give `a\,b`; the TOML string `"a\\,b"` has one backslash), `"a,b,"` has a trailing empty item, an empty string is an
empty list); a string bound to a `String` field is never split. Indexed entries
(`APP_HOSTS__0=a`, `--hosts[1]=b`) must start at 0 without gaps. A list from a higher source
replaces a lower one whole. `std::time::Duration` fields read `ns`, `us`, `ms`, `s`, `m`, `h`,
`d`, fractions and bare seconds (`"1.5s"`, `"30"`); negative, overflowing or unknown-unit values are
`Invalid`, digits below one nanosecond are truncated. A user struct literally named `Duration`
with fields `secs` and `nanos` is treated as `std::time::Duration`. `DataSize` reads sizes (below).

```rust
#[derive(Deserialize)]
struct Server { host: String, port: u16 }

let config = Config::from_str("[server]\nhost = \"localhost\"\nport = 8080")?;
let server: Server = config.get("server")?;
assert_eq!(server.port, 8080);

let port: u16 = config.get("server.port")?;      // a single value works too

#[derive(Deserialize)]
struct Pool { max_connections: u32 }

let config = Config::from_str("[db.pool]\nmaxConnections = 10")?;
assert_eq!(config.get::<Pool>("DB.pool")?.max_connections, 10);

let config = Config::from_str("t = \"1.5s\"\nports = \"80, 443\"")?;
assert_eq!(config.get::<std::time::Duration>("t")?, std::time::Duration::from_millis(1500));
assert_eq!(config.get::<Vec<u16>>("ports")?, [80, 443]);
```

#### Placeholders

`${name}` and `${name:default}` inside a string value from a file (`application.toml`, profile
files, imports, `Config::from_str` text). Environment and command-line values are taken
literally: `${` and `$${` stay as written, and a placeholder that refers to such a value gets its
text as is. A name is a config key
(canonical match, `list.0` indexes an array) and then an environment variable of that exact name.
A default may contain placeholders; names cannot (`${${x}}`). `$${` is a literal `${`. Values stay
strings until bound. Errors: `UnresolvedPlaceholder`, `PlaceholderCycle` (also for nesting deeper
than 32; leaf references and defaults share the limit), `PlaceholderTooLarge` (over 1 MiB for one
resolved string or 16 MiB for one load). Config files are trusted and can read any environment
variable. A resolved leaf keeps the origin of the leaf that contains the placeholder. A literal
`${` in an old file value now needs `$${`. Errors name the key and the placeholder name, which is
text from a file value: at most 32 characters, plus a trailing `…` when cut; for an unterminated
`${` it is the text after the `${`, never anything after a `:`. Write `$${` for a literal `${` in
files, especially in values that may be secrets. No other error contains a value.

```rust
let config = Config::from_str(r#"
    host = "db.internal"
    url = "postgres://${host}:${DB_PORT:5432}/app"
    literal = "$${not-a-placeholder}"
"#)?;
assert_eq!(config.get::<String>("url")?, "postgres://db.internal:5432/app");
assert_eq!(config.get::<String>("literal")?, "${not-a-placeholder}");
```

#### `Config::active_profiles() -> &[String]`

The final list of active profiles, after group expansion, in activation order (each name once).
It is empty when no profile is active and for `Config::from_str`. Pass it to the dependency
container so profile-conditional beans follow the same profiles as the configuration files.

```rust
let config = Config::builder().dir("config").profile("prod").build()?;
// with `[oxbloom.profiles.group] prod = ["db", "metrics"]` in application.toml:
assert_eq!(config.active_profiles(), ["prod", "db", "metrics"]);

let container = ContainerBuilder::new()
    .active_profiles(config.active_profiles().iter().cloned())
    .build()?;
```

#### `Config::origin(key) -> Option<&Origin>`

Where the leaf at `key` came from. `key` is matched like `get`; array elements are `list[0]` or
`list.0`. Only scalar leaves and array elements have an origin: a table, an array as a whole, an
unknown key and the empty key give `None`. Quoted keys containing `.` cannot be addressed.

```rust
let config = Config::from_str("[server]\nport = 8080")?;
assert_eq!(config.origin("SERVER.port"), Some(&Origin::Inline { line: 2 }));
assert_eq!(config.origin("server"), None);
```

#### `Origin`

Where a value came from, never the value: `File { path, line }` (`application-dev.toml:4`),
`Inline { line }` (`line 3`), `Env { name }` (`env APP_SERVER__PORT`), `Cli { arg }`
(`--server.port`). Implements `Display`, `Clone`, `PartialEq`.

```rust
let origin = Origin::Env { name: "APP_SERVER__PORT".into() };
assert_eq!(origin.to_string(), "env APP_SERVER__PORT");
```

#### `Config::explain() -> Vec<Entry>` and `Entry`

One `Entry { key, origin, kind }` per leaf, sorted by key bytes (so `a[10]` comes before `a[2]`).
`key` uses the spelling of the winning source and `list[0]` for array elements; `kind` is
`string`, `integer`, `float`, `boolean` or `datetime`. Values are never included, so the output is
safe to log. A quoted key containing `.` is listed but not reachable through `origin` or `keys`.

```rust
let config = Config::from_str("name = \"demo\"\nports = [80, 443]")?;
for entry in config.explain() {
    println!("{} <- {} ({})", entry.key, entry.origin, entry.kind);   // name <- line 1 (string)
}
```

#### `Config::keys(prefix) -> Vec<String>`

Leaf keys under a dotted prefix (`""` for all), sorted and spelled like `explain`. A prefix that
is itself a leaf returns that key; an unknown prefix returns an empty list.

```rust
let config = Config::from_str("[db]\nurl = \"x\"\nhosts = [\"a\"]")?;
assert_eq!(config.keys("db"), ["db.hosts[0]", "db.url"]);
```

#### `Config::warnings() -> &[ConfigWarning]`, `ConfigWarning`, `WarningKind`

Migration findings in rule registration order. `ConfigWarning { key, origin, kind }` with `kind`
`WarningKind::Renamed { to }` or `WarningKind::Removed { reason }`. A warning never contains a
value and implements `Display`.

```rust
use oxbloom_config::WarningKind;

let config = Config::builder().dir("config").renamed("db.pass", "db.password").build()?;
if let Some(w) = config.warnings().first() {
    assert_eq!(w.kind, WarningKind::Renamed { to: "db.password".into() });
}
```

#### `DataSize`

A size in bytes: `DataSize::new(bytes)`, `bytes()`, `Display` (`512B`), `Deserialize`. Reads a
string or an integer. `KB`, `MB`, `GB`, `TB` are 1000-based, `KiB`, `MiB`, `GiB`, `TiB` are
1024-based (unlike Spring Boot, where `KB` is 1024), `B` or a bare number is bytes. Integers
only; negative, fractional, overflowing and unknown-unit values are `Invalid`.

```rust
#[derive(Deserialize)]
struct Limits { upload: DataSize, cache: DataSize }

let config = Config::from_str("[limits]\nupload = \"10MB\"\ncache = \"4KiB\"")?;
let limits: Limits = config.get("limits")?;
assert_eq!(limits.upload.bytes(), 10_000_000);
assert_eq!(limits.cache.bytes(), 4096);
assert_eq!(limits.cache.to_string(), "4096B");
```

#### `ConfigError`

Variants: `Io { path, source }`, `Parse { path, line, message }`, `Missing { key }`,
`Invalid { key, message, origin }`, `DuplicateKey { key, first, second }`, `ImportCycle { files }`,
`ImportMissing { path }`, `ImportInvalid { path }`, `ImportTooDeep`, `ImportLimit`,
`FileTooLarge { path }`, `UnresolvedPlaceholder { key, name }`, `PlaceholderCycle { keys }`,
`PlaceholderTooLarge { key }`, `Deprecated { key, origin }`. The enum is `#[non_exhaustive]` (so
are `Origin` and `WarningKind`): a `match` outside the crate needs a `_` arm. `Display` names keys,
file paths and origins and never contains values; the one exception is the placeholder name of
`UnresolvedPlaceholder`, text from a file value (at most 32 characters plus `…`; for an
unterminated `${` never anything after a `:`). Environment and command-line values are never
resolved, so they never appear there. The `path` fields of the `Import*` errors may appear in `Debug` (file names as written
in the config); `Display` of those errors is fixed text. `Invalid.origin` (an `Origin`) says where
the failing value came from:
``invalid configuration key `server.port` (from env APP_SERVER__PORT): expected unsigned integer``.
A failing list item is named by index (`hosts[1]`).

```rust
let config = Config::from_str("[server]\nport = \"abc\"")?;
match config.get::<u16>("server.port") {
    Err(ConfigError::Invalid { key, .. }) => println!("bad value at {key}"),
    other => panic!("{other:?}"),
}
assert!(matches!(config.get::<u16>("nope"), Err(ConfigError::Missing { .. })));

let err = Config::from_str("a = \"${B}\"").unwrap_err();
assert!(matches!(err, ConfigError::UnresolvedPlaceholder { .. }));

let err = Config::from_str("a = \"${b}\"\nb = \"${a}\"").unwrap_err();
assert!(matches!(err, ConfigError::PlaceholderCycle { .. }));

let dup = Config::from_str("max-conn = 1\nmax_conn = 2").unwrap_err();
assert!(matches!(dup, ConfigError::DuplicateKey { .. }));
```

Import and size-limit variants are produced by `ConfigBuilder::build` (see there):
`ImportCycle`, `ImportMissing`, `ImportInvalid`, `ImportTooDeep`, `ImportLimit`, `FileTooLarge`
(fixed text, no path or content in `Display`).
`PlaceholderTooLarge` has a one-line fixed text and no value.

#### `ConfigError::invalid(key, message)`

Builds an `Invalid` error without an origin, for checks your own code makes after binding.

```rust
let err = ConfigError::invalid("security.jwt.secret", "secret is too short");
assert_eq!(err.to_string(), "invalid configuration key `security.jwt.secret`: secret is too short");
```

#### `Secret<T>`: `new`, `expose`

Wraps a value that must not leak. `Debug` and `Display` print `[REDACTED]`. It deserializes like `T`.

```rust
#[derive(Deserialize, Debug)]
struct Db { user: String, password: Secret<String> }

let config = Config::from_str("[db]\nuser = \"app\"\npassword = \"hunter2\"")?;
let db: Db = config.get("db")?;
println!("{db:?}");                       // Db { user: "app", password: [REDACTED] }
let pw: &String = db.password.expose();   // only at the point of use

let token = Secret::new("abc".to_string());
assert_eq!(token.to_string(), "[REDACTED]");
```

#### `ContainerBuilderExt::config(config)` (feature `di`)

Registers `Config` as a singleton so components can take `Arc<Config>`. It does not pass the
active profiles to the container; call `.active_profiles(config.active_profiles().iter().cloned())` on the
builder as well.

```rust
use oxbloom_config::ContainerBuilderExt;

#[component]
struct Mailer { config: Arc<Config> }

let config = Config::from_str("[mail]\nfrom = \"noreply@example.com\"")?;
let container = ContainerBuilder::new()
    .config(config)
    .component::<Mailer>()
    .build()?;
let mailer = container.get::<Mailer>()?;
let from: String = mailer.config.get("mail.from")?;
```

#### `impl Environment for Config` (feature `di`)

`Config` is an `oxbloom_di::Environment`, so the property conditions can read it; boot sets it as
the container environment for you. `property(key)` finds `key` with the rules of `Config::get`
(relaxed keys: case-insensitive, `-` and `_` ignored) and returns the text of a scalar leaf:
strings as they are, integers in decimal, floats in TOML form, booleans as `true` or `false`, and
dates and times in their TOML text form. Absent keys, tables, lists, indexed list elements such
as `hosts[0]` and the empty key are `None`. `contains(key)`, which `on_property` asks, is
true for any present key found by the same rules, a table or a list included, so
`on_property("security.jwt")` matches a `[security.jwt]` section; it is false for absent keys, indexed
elements and the empty key. Values are compared as text, so the file integer
`port = 8080` and the environment or command-line string `8080` both equal `"8080"`. A `Secret`
is an ordinary leaf here: `on_property_value` compares its plain text, and the report never shows
it. The expected value of `on_property_value(key, value)` sits in the source code, so never compare
against a secret.

```rust
use oxbloom_di::Environment;

struct Metrics;

let config = Config::from_str("[metrics]\nenabled = true\nport = 9100\nhosts = [\"a\"]")?;
assert_eq!(config.property("Metrics.ENABLED").as_deref(), Some("true"));
assert_eq!(config.property("metrics.port").as_deref(), Some("9100"));
assert_eq!(config.property("metrics"), None);       // a table
assert_eq!(config.property("metrics.hosts"), None); // a list

let container = ContainerBuilder::new()
    .environment(config)
    .instance(Metrics)
    .on_property_value("metrics.enabled", "true")
    .build()?;
assert!(container.get_opt::<Metrics>()?.is_some());
```

#### `From<ConfigError> for DiError` (feature `di`)

Lets `?` turn a `ConfigError` into a `DiError` inside a `try_bean` factory or a `#[bean(fallible)]`
method. The result is a `Creation` error whose message is the error's `Display` (which never
contains configuration values) and whose source is the `ConfigError`; `try_bean` sets the type name
to the bean being built. Boot's missing-key analyzer finds the `ConfigError` through that source.
Only `try_bean` and `#[bean(fallible)]` rewrite the type name. A `factory` or `Component::create` that uses `?` on a `ConfigError` is not rewritten: its error is labelled with `ConfigError`'s own type name. Use `try_bean` there, or `map_err(DiError::creation_from::<T>)` inside a `factory`.

```rust
use std::error::Error;
use oxbloom_config::ConfigError;
use oxbloom_di::DiError;

struct Pool { size: u16 }

let err = ContainerBuilder::new()
    .try_bean(|| -> Result<Pool, DiError> {
        let config = Config::from_str("")?;
        Ok(Pool { size: config.get("pool.size")? })
    })
    .build()
    .err()
    .unwrap();
assert!(matches!(&err, DiError::Creation { type_name, .. } if type_name.ends_with("Pool")));
let source = err.source().and_then(|s| s.downcast_ref::<ConfigError>());
assert!(matches!(source, Some(ConfigError::Missing { key }) if key == "pool.size"));
```

### 4.6 `oxbloom-cli`

Binary name: `oxbloom`. Run from the workspace with `cargo run -p oxbloom-cli -- <args>`.

#### `oxbloom new <name>`

Creates a project with `.gitignore`, `oxbloom.toml`, `Cargo.toml`, `application.toml`, `src/lib.rs`,
`src/main.rs` and `tests/hello.rs`. The project depends on `oxbloom-boot` (feature `web`),
`oxbloom-config`, `oxbloom-di` and `oxbloom-web` by path. Its `src/main.rs` starts a `WebServer`
through `OxbloomApplication::builder()...run_and_exit()`, so a startup failure prints the failure
report, and `application.toml` sets `server.port = 8080`, read from the working directory. The name
allows lowercase letters, digits, `-` and `_`, and must not be a Rust keyword.

```bash
cargo run -p oxbloom-cli -- new my-app
cargo run -p oxbloom-cli -- new my-app --dir /some/parent     # creates /some/parent/my-app
```

#### `--dir <path>`

Parent directory for the new project. Default: the current directory.

```bash
oxbloom new shop --dir ~/projects
```

#### `--oxbloom-path <path>`

Workspace root used for the generated path dependencies. Default: this workspace. The path must
contain `crates/oxbloom-web/Cargo.toml`, `crates/oxbloom-boot/Cargo.toml`,
`crates/oxbloom-config/Cargo.toml` and `crates/oxbloom-di/Cargo.toml`; otherwise the command fails
with `not an oxbloom workspace: missing <path>` and exit code 2.

```bash
oxbloom new shop --oxbloom-path ~/Repositories/oxbloom
```

#### `--web rest`

Web style. `rest` is the only value and the default.

```bash
oxbloom new shop --web rest
```

#### `oxbloom list`

Prints each component and whether it is available or planned.

```bash
oxbloom list
```

#### `--security`, `--database`, `--frontend`, `--jobs`, `--tests`, `--ai`

Accepted by the parser but not implemented. Each exits with `<feature> is not implemented yet`.

```bash
oxbloom new shop --ai        # error: ai is not implemented yet   (exit 2)
```

Exit codes: `0` success, `2` bad input (bad name, unimplemented flag), `1` filesystem error (for
example the target directory already exists).

### 4.7 `oxbloom-security`

A `SecurityFilterChain` is a `Filter`. It extracts `Authorization: Bearer <token>` (and, with HTTP Basic on, `Authorization: Basic <credentials>`), authenticates it,
stores a `SecurityContext` in the request extensions and authorizes against ordered rules. Rules
and limits to remember:

- Deny by default, one chain per app. The first matching rule wins; no match means denied.
- Runs before routing: an unknown path is denied (401/403) unless a rule permits it, so anonymous clients cannot probe routes; allowed callers still get the router's 404 (a body over the limit still gets 413 first). The chain takes the slot `order::SECURITY` (-100) through `Filter::order()`.
- Matching is on the raw undecoded path with Ant patterns (`*` one segment, `**` zero or more). A leading slash in the pattern is optional. Any other `*` and any `{`, `}` or `?` in a segment (`/users/{id}`, `*.pdf`) makes `build()` fail with `InvalidPattern`.
- `HEAD` is authorized as `GET`: a `get(..)` rule also governs `HEAD`; rules are tried in order with both views, so an earlier `get(..)` rule shadows a later `head(..)` rule and vice versa.
- 401 = `WWW-Authenticate: Bearer` (+ `error="invalid_token"` for a failed token), 403 = authenticated but denied, 503 = key source outage. The chain builds empty-bodied responses and `App`'s error filter renders them as `application/problem+json`, keeping `WWW-Authenticate`. A malformed `Authorization` header gets 401 from the chain (`invalid_request` with a bearer provider; only the Basic challenge in a Basic-only chain) and bypasses a custom entry point.
- Errors, responses and `Debug` of the security types never contain tokens, keys or claim values (`JwksSource` prints its URI without userinfo or query). The server marks `Authorization`, `Proxy-Authorization` and `Cookie` values sensitive, and the chain marks `Authorization` again, so `Debug` of a `Request` shows `Sensitive` for them; other headers, the query and the body are printed as sent.

Imports:

```rust
use std::sync::Arc;
use oxbloom_config::Secret;
use oxbloom_security::{Algorithm, HttpSecurity, JwtAuthenticationConverter, JwtDecoderBuilder,
                          RequestSecurityExt};
```

#### `HttpSecurity::new()`, `.authorize(..)`, `.oauth2_resource_server(..)`, `.build()`

Builds the chain. `build()` fails with `SecurityBuildError` without an `authorize` section, when `authorize`
is called more than once (sections are never merged), with a rule after `any_request()`, with an invalid
pattern, or when `oauth2_resource_server` has no decoder. Rule builders: `get`, `post`,
`put`, `patch`, `delete`, `head`, `options`, `path` (any method), `any_request`; outcomes `permit_all`,
`deny_all`, `authenticated`, `has_role`, `has_any_role`, `has_authority`, `has_any_authority`, `access`
(custom `AuthorizationManager`). `has_role("ADMIN")` checks `ROLE_ADMIN`.

```rust
let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec())).build()?;
let chain = HttpSecurity::new()
    .authorize(|a| a
        .get("/public/**").permit_all()
        .path("/admin/**").has_role("ADMIN")
        .any_request().authenticated())
    .oauth2_resource_server(|r| r
        .jwt(Arc::new(decoder))
        .converter(JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_")))
    .build()?;
let app = App::new().filter(chain).route(Route::new(HttpMethod::Get, "/me", |req: Request| async move {
    req.authentication().map(|a| a.principal().to_owned()).unwrap_or_default()
}))?;
assert_eq!(app.handle(Request::new(HttpMethod::Get, "/me")).await.status(), 401);
```

If `oauth2_resource_server` is used while another bearer provider is registered
(`bearer_provider(..)`), the first bearer provider wins.

#### `HttpSecurity::entry_point(..)`, `.access_denied_handler(..)`

Replace the 401 / 403 responders (`AuthenticationEntryPoint`, `AccessDeniedHandler`). The entry point is
also asked for failed-token and 503 cases, but never for a malformed `Authorization` header.

```rust
let chain = HttpSecurity::new()
    .entry_point(Arc::new(DefaultEntryPoint))
    .access_denied_handler(Arc::new(DefaultAccessDeniedHandler))
    .authorize(|a| a.any_request().authenticated())
    .build()?;
```

#### `JwtDecoderBuilder`

`with_secret(alg, Secret<Vec<u8>>)` (HS256/384/512, use at least 32 bytes), `with_public_key_pem(alg, pem)`
(RS256/384/512, ES256/384), `with_jwks(source, algorithms)`. Options: `issuer`, `audience`, `clock_skew`,
`require_exp`, `on_failure`. The algorithm allow-list is fixed at build time and `alg: none` is rejected.
`exp` is required, skew is 60 s by default (max 1 h), and **`aud` is only checked when you call
`audience(..)`; set it**. `on_failure` receives a `FailureReason` (no token text) and is not called for JWKS outages.

```rust
let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec()))
    .issuer("https://issuer.example")
    .audience(["my-api"])
    .on_failure(Arc::new(|reason| eprintln!("jwt rejected: {reason}")))
    .build()?;
```

#### `JwtAuthenticationConverter`

Maps a verified token to an `Authentication`: principal from `sub`, `SCOPE_<x>` authorities from `scope`
(string) or `scp` (array). `authorities_claim(claim, prefix)` adds a mapping on top of the default one.

```rust
let converter = JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_");
```

#### `JwksSource`, `JwksFetcher`, `ReqwestJwksFetcher` (feature `jwks`)

`JwksSource::builder(fetcher, uri)` caches keys: https only unless `allow_insecure_http()`, TTL 5 min
(`ttl`), one refresh per 30 s (`min_refresh_interval`), stale set served up to 2x TTL and then 503, 1 MiB
body cap, 5 s timeout, no redirects. A token with a brand-new `kid` can be rejected for up to 30 s after a key
rotation. `ReqwestJwksFetcher` runs on `oxbloom-http-client` (a `ReqwestClient`, see [section 4.14](#414-oxbloom-http-client)), needs a C compiler (rustls with aws-lc-rs) and honors `HTTP_PROXY`/`HTTPS_PROXY`.
Implement `JwksFetcher` yourself for tests, or build the fetcher over a `MockHttpClient` with `with_client`.

```rust
let source = JwksSource::builder(
    Arc::new(ReqwestJwksFetcher::new()?),
    "https://issuer.example/.well-known/jwks.json",
).build()?;
let decoder = JwtDecoderBuilder::with_jwks(source, vec![Algorithm::RS256]).build()?;
```

#### `ReqwestJwksFetcher::with_tls(client_config)` (feature `jwks`)

A fetcher that verifies the JWKS server with a `rustls::ClientConfig` you built instead of the platform
certificate verifier. Use it when the issuer's certificate comes from a private CA, or when the issuer
wants a client certificate. Everything else is as in `ReqwestJwksFetcher::new`: no redirects, no `Referer`,
a 5 second timeout and a 1 MiB body cap. Trust follows `client_config` alone: a configuration with its own
root store trusts only those CAs, so a certificate from a public CA is then rejected. The HTTP client is
built once, so a later change of the configuration's certificates does not reach it. `FetchError::Network`
when the client cannot be built. An `SslBundle` gives such a configuration with
`SslBundle::client_config`; in an application, `security.jwt.jwks-ssl-bundle` does this for you.

#### `ReqwestJwksFetcher::with_client(client)` (feature `jwks`)

A fetcher over an `Arc<dyn HttpClient>` that you already have, for example the application's shared client or a
`MockHttpClient` in a test. The client then decides the timeout, redirects, proxies and TLS, and the fetcher adds no
deadline of its own. It sends `Accept: application/json`, treats any non-2xx status as `FetchError::BadStatus` and
refuses a body over 1 MiB with `FetchError::TooLarge`, but only after the client has buffered the body, so the real
memory bound is the limit of the client you give. A client that follows redirects follows them for the fetch too.
`new`, `with_timeout` and `with_tls` keep their signatures and limits (the 1 MiB check holds for them because their own
client has that limit).

```rust
use rustls::{ClientConfig, RootCertStore};

// Trust only the corporate CA that signed the issuer's certificate.
let mut roots = RootCertStore::empty();
roots.add(corporate_ca())?;
let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
let client_config = ClientConfig::builder_with_provider(provider)
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();

let fetcher = ReqwestJwksFetcher::with_tls(Arc::new(client_config))?;
let source = JwksSource::builder(Arc::new(fetcher), "https://issuer.corp.example/jwks.json").build()?;
```

Audit note: the `jsonwebtoken` backend `rust_crypto` pulls in `rsa` 0.9 (advisory RUSTSEC-2023-0071,
private-key operations only). This crate only verifies signatures; `cargo audit` still reports it.

#### `jwt_decoder_from_config(&config, fetcher)`

Builds a decoder from `security.jwt`: `algorithms` (required), one key source out of `secret`,
`public-key-pem` or `jwk-set-uri` (needs `Some(fetcher)`), optional `issuer`, `audience`, `clock-skew-secs`.
Unknown keys under `security.jwt` are rejected. `algorithms` and `audience` can be set from environment
variables comma-separated (`APP_SECURITY__JWT__AUDIENCE=a,b`) or indexed (`..._AUDIENCE__0=a`); `secret` can
(`APP_SECURITY__JWT__SECRET`). Keys match case-insensitively ignoring `-` and `_` (`jwk-set-uri`, `jwk_set_uri`,
`jwkSetUri`, `JWK_SET_URI`); two spellings of one key in one file are a `DuplicateKey` error. A secret whose origin is the command line is refused even without `secret_key`, and empty or whitespace-only `audience`/`algorithms` items are `Invalid`. Register
`SECRET_KEY` (`"security.jwt.secret"`) with `Config::builder().secret_key(SECRET_KEY)` so the secret cannot
come from the command line.

```rust
let config = Config::from_str(r#"
    [security.jwt]
    algorithms = ["HS256"]
    secret = "an-example-secret-of-32-bytes!!!"
"#)?;
let decoder = jwt_decoder_from_config(&config, None)?;
```

#### `jwks_ssl_bundle(&config)`

The name in `security.jwt.jwks-ssl-bundle`, or `None` when the key is not set. `oxbloom-security` does not
load SSL bundles: the caller resolves the name, builds the fetcher with `ReqwestJwksFetcher::with_tls` and
passes it to `jwt_decoder_from_config`. `oxbloom-boot` does this with its feature `ssl`, so call it yourself
only when you build the decoder by hand. A value that is not a string, or an empty name, is
`ConfigError::Invalid` on the key, and `jwt_decoder_from_config` refuses the key without `jwk-set-uri`. No
message contains the value.

```rust
let config = Config::from_str("[security.jwt]\njwks-ssl-bundle = \"corporate\"")?;
assert_eq!(jwks_ssl_bundle(&config)?.as_deref(), Some("corporate"));
assert_eq!(jwks_ssl_bundle(&Config::from_str("")?)?, None); // the platform verifier stays

let empty = Config::from_str("[security.jwt]\njwks-ssl-bundle = \"\"")?;
assert!(matches!(
    jwks_ssl_bundle(&empty),
    Err(ConfigError::Invalid { key, .. }) if key == "security.jwt.jwks-ssl-bundle"
));
```

#### `RequestSecurityExt::authentication()`, `Authentication`, `GrantedAuthority`

Inside a handler: `req.authentication()` is `Some(&Authentication)` for an authenticated request.
`principal()`, `authorities()`, `has_role("X")`, `has_authority("SCOPE_read")`, `details()` (claims; never printed by `Debug`), `remembered()` (builder: marks the authentication as remembered, slice 7b) and `is_remembered()` (whether the stored `details` carry the marker; a JWT claim `oxbloom.remembered: true` counts too, which only makes `fully_authenticated()` refuse the token).

```rust
async fn me(req: Request) -> impl IntoResponse {
    match req.authentication() {
        Some(auth) if auth.has_role("ADMIN") => format!("admin {}", auth.principal()),
        Some(auth) => format!("user {}", auth.principal()),
        None => "anonymous".to_string(),
    }
}
```

#### `SecurityContainerExt::security_chain(chain)` (feature `di`)

Registers the chain as a singleton in a `ContainerBuilder`; resolve `Arc<SecurityFilterChain>` and pass it to `App::filter`.

```rust
let container = ContainerBuilder::new().security_chain(chain).build()?;
let app = App::new().filter(container.get::<SecurityFilterChain>()?);
```

#### `From<SecurityBuildError> for DiError` (feature `di`)

Lets `?` turn a `SecurityBuildError` into a `DiError` inside a `try_bean` factory. The result is a
`Creation` error whose message is the error's `Display` and whose source is the
`SecurityBuildError`; `try_bean` sets the type name to the bean being built. Boot's security
analyzer (feature `security` of `oxbloom-boot`) finds the error through that source.

```rust
use std::error::Error;
use oxbloom_di::{ContainerBuilder, DiError};
use oxbloom_security::{SecurityBuildError, SecurityFilterChain};

let err = ContainerBuilder::new()
    .try_bean(|| -> Result<SecurityFilterChain, DiError> { Ok(HttpSecurity::new().build()?) })
    .build()
    .err()
    .unwrap();
let source = err.source().and_then(|s| s.downcast_ref::<SecurityBuildError>());
assert_eq!(source, Some(&SecurityBuildError::NoAuthorizeSection));
```

#### `RequestMatcher`, `AuthorizationRule`, `ProviderManager`, `AuthenticationProvider`

Building blocks used by the builder. `RequestMatcher::new(Some(HttpMethod::Get), "/api/**")` matches the raw undecoded
path; `AuthorizationRule` is `PermitAll`, `DenyAll`, `Authenticated`, `FullyAuthenticated` (slice 7b: an authenticated request that is not remembered; an exhaustive `match` on the enum stops compiling), `HasRole`, `HasAnyRole`, `HasAuthority`,
`HasAnyAuthority` or `Custom`; `ProviderManager` asks providers in order and the first that supports a request decides.
Each has a runnable rustdoc example (`cargo doc -p oxbloom-security --open`).

With the `di` feature, providers can be beans. Each one is made available as
`dyn AuthenticationProvider` with `provides`, and a `bean` taking
`Vec<Arc<dyn AuthenticationProvider>>` receives all of them, sorted by `order`, to build the
`ProviderManager` (full example on `SecurityContainerExt`):

```rust
let container = ContainerBuilder::new()
    .bean(|| Anyone)
    .provides(|p: Arc<Anyone>| -> Arc<dyn AuthenticationProvider> { p })
    .order(10) // asked last, although registered first
    .bean(|| ServiceTokens)
    .provides(|p: Arc<ServiceTokens>| -> Arc<dyn AuthenticationProvider> { p })
    .bean(|providers: Vec<Arc<dyn AuthenticationProvider>>| ProviderManager::new(providers))
    .build()?;
let manager = container.get::<ProviderManager>()?;
```

```rust
let matcher = RequestMatcher::new(Some(HttpMethod::Get), "/api/**");
assert!(matcher.matches(&Request::new(HttpMethod::Get, "/api/users/1")));
assert!(!matcher.matches(&Request::new(HttpMethod::Post, "/api/users/1")));
```

#### Password login: `PasswordEncoder`, `UserDetailsService`, `DaoAuthenticationProvider`, HTTP Basic (slice 6a)

Guide: [Password login](guide/19-password-login.md). Decisions: [ADR 0086](adr/0086-password-login.md).

```rust
use std::sync::Arc;
use oxbloom_security::{
    Argon2PasswordEncoder, DaoAuthenticationProvider, DelegatingPasswordEncoder, HttpSecurity,
    InMemoryUserDetailsService, PasswordEncoder, UserDetails,
};

let encoder = Arc::new(DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1)?));
let stored = encoder.encode("s3cret"); // `{argon2}$argon2id$v=19$...`
assert!(encoder.matches("s3cret", &stored));

let users = Arc::new(InMemoryUserDetailsService::new(vec![
    UserDetails::new("alice", stored, []).roles(&["ADMIN"]),
])?);
let chain = HttpSecurity::new()
    .http_basic(|b| b.realm("Admin area"))
    .password_provider(Arc::new(DaoAuthenticationProvider::new(users, encoder)))
    .authorize(|a| a.path("/admin/**").has_role("ADMIN").any_request().authenticated())
    .build()?;
```

- `PasswordEncoder` (`Send + Sync`): `encode(&str) -> String`, `matches(&str, &str) -> bool` (a malformed hash never matches) and `upgrade_encoding(&str) -> bool` (true for weaker parameters or an unreadable hash).
- `Argon2PasswordEncoder::new(memory_kib, iterations, parallelism)` returns `PasswordEncoderError::InvalidParameters` when Argon2 rejects the values; `default()` is 19456 KiB, 2 iterations and 1 lane; accessors `memory_kib()`, `iterations()`, `parallelism()`. It stores PHC strings.
- `DelegatingPasswordEncoder::new(default_id, vec![(id, Arc<dyn PasswordEncoder>)])` and `::argon2(encoder)` (id `ARGON2_ID` = `"argon2"`) store `{id}hash`. An unknown or missing prefix never matches and always asks for an upgrade. Errors: `UnknownDefaultId`, `DuplicateId`.
- `UserDetails::new(username, password_hash, authorities)` with `.roles(&["ADMIN"])` (adds `ROLE_ADMIN`), `.enabled(bool)` and `.locked(bool)`; getters `username()`, `password()`, `authorities()`, `is_enabled()`, `is_locked()`. `Debug` shows `[REDACTED]` for the password.
- `UserDetailsService` (async trait): `load_user_by_username(&str) -> Result<Option<UserDetails>, UserDetailsError>`. `InMemoryUserDetailsService::new(Vec<UserDetails>)` fails with `UserStoreError::DuplicateUsername` or `EmptyUsername`; names are compared exactly.
- `DaoAuthenticationProvider::new(users, encoder)` supports `AuthenticationRequest::UsernamePassword { username, password: Secret<String> }`. Order: a username or password over 1024 bytes, an empty username or an unknown user fail with `BadCredentials` (an unknown user is verified against a dummy hash first, made by the encoder when the provider is built); then the password is checked in `tokio::task::spawn_blocking`; an empty or wrong password is `BadCredentials`; only after a right password does a disabled user give `AccountDisabled` and a locked one `AccountLocked`; a store error or a failed hash task is `ProviderUnavailable`. Failures log at `debug` on `oxbloom::security` with the variant and the username length.
- `HttpSecurity::http_basic(|b| b.realm(..))` turns Basic on (default realm `Oxbloom`); `password_provider(Arc<dyn AuthenticationProvider>)` adds a provider (Basic needs at least one). `build()` fails with `SecurityBuildError::HttpBasicWithoutProvider` or `InvalidRealm` (a realm with `"`, `\` or a control character). Without `http_basic`, a `Basic` header leaves the request anonymous.
- Answers with Basic on: anonymous gives 401 with `WWW-Authenticate: Basic realm="..", charset="UTF-8"` (and `Bearer` after it when a bearer provider exists); every failed login, including a disabled or locked account, gives 401 with the Basic challenge; a malformed Basic header (bad Base64, no colon, not UTF-8, empty token) gives 401 and never 400; `ProviderUnavailable` gives 503 without a challenge; an authenticated user that a rule denies gives 403. A custom `entry_point` replaces the challenge, except for a malformed `Authorization` header, which the chain answers itself.
- Settings readers: `PasswordSettings::from_config`, `Argon2Settings::from_config`, `HttpBasicSettings::from_config` and `users_from_config(&Config) -> Result<Vec<UserDetails>, ConfigError>`. A password under `security.users` must be `{argon2}` and a PHC string.
- Keys: `security.password.default-encoder` (`argon2` only), `security.password.argon2.memory-kib` (8 to 1048576, default 19456, at least 8 per lane), `.iterations` (1 to 100, default 2), `.parallelism` (1 to 64, default 1), `security.http-basic.enabled` (default `false`), `security.http-basic.realm` (default `Oxbloom`), `security.users.<name>.password`, `.roles`, `.enabled` (default `true`) and `.locked` (default `false`). Unknown keys are rejected with the key named.
- User names follow the configuration crate: keys are matched ignoring case, `-` and `_`, so two names that differ only in these fail to load from one file with `DuplicateKey` and merge across sources. The spelling written is kept and compared exactly at login.
- Semver: `AuthenticationRequest::UsernamePassword`, `AuthenticationError::AccountDisabled` and `AccountLocked`, and `SecurityBuildError::HttpBasicWithoutProvider` and `InvalidRealm` were added to enums that are not `#[non_exhaustive]`. A `match` without a wildcard arm stops compiling.
- Limits: no rate limiting or lockout counters, no database store, no re-encoding of a stale hash on login, no zeroizing of the raw password. `matches` verifies with the cost stored in the hash, so each concurrent login uses that memory (19 MiB by default); the unknown-user dummy hash uses the configured cost, so equal timing holds only while stored hashes use the configured parameters. Anonymous requests are bounded only by the blocking pool of tokio (512 threads by default), so the worst case is about 512 x 19 MiB, roughly 9.5 GiB at the default cost. A login limit or a semaphore around the hash is deferred. A user with a malformed stored hash answers faster than other users.

#### Sessions: `Session`, `SessionFilter`, `SessionStore`, `CookieSessionBackend` (slice 6b)

Guide: [Sessions](guide/20-sessions.md). Decisions: [ADR 0087](adr/0087-sessions.md).

```rust
use std::sync::Arc;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    InMemorySessionStore, RequestSessionExt, SessionBackend, SessionFilter, SessionSettings,
};
use oxbloom_web::{App, HttpMethod, Request, Route};

let settings = SessionSettings::default();
let store = InMemorySessionStore::new(&settings, Arc::new(SystemClock));
let filter = SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings)?;

let app = App::new()
    .filter(filter)
    .route(Route::new(HttpMethod::Get, "/visit", |req: Request| async move {
        let session = req.session().expect("the session filter ran");
        let visits = session.get::<u32>("visits").ok().flatten().unwrap_or(0) + 1;
        session.set("visits", &visits).ok();
        visits.to_string()
    }))?;
```

- `Session` (a cheap clone around shared state): `get::<T>(key) -> Result<Option<T>, SessionError>`, `set(key, &value) -> Result<(), SessionError>`, `remove(key)`, `invalidate()`, `rotate_id()`, `is_new()` and `id()`. Values are JSON. Keys that start with `oxbloom.security.` are reserved (slice 7b): `set` returns `SessionError::ReservedKey` and writes nothing, `remove` ignores them and stays infallible, and `get` still reads them. The `#[doc(hidden)]` methods `set_reserved(key, &value) -> Result<(), SessionError>` and `remove_reserved(key)` write them, for Oxbloom's own code and the tests of this repository; the reservation guards against accidental collisions and not against hostile code in the same process. The filter commits once after the chain; a clone used after that loses its writes silently. With the stateless backend `id()` is random on every request.
- `RequestSessionExt::session(&self) -> Option<Session>` on `Request`; `None` when no session filter ran.
- `SessionStore` (async trait): `load(id) -> Result<Option<SessionData>, SessionError>`, `save(id, &data, ttl)` (insert or replace, for a session minted in this request), `update(id, &data, ttl) -> Result<bool, SessionError>` (writes only when the id still exists; `false` means the session was deleted meanwhile, and the filter then stores nothing and leaves the cookie alone), `rotate(old, new) -> Result<bool, SessionError>` (`false` when the old id is missing), `delete(id)` and `maintain()` (default no-op, called at most once a minute). A Redis store uses a conditional write (`SET .. XX` or a script) for `update` and `RENAMENX` or a script for `rotate`; an SQL store uses `UPDATE .. WHERE id = ?` with a row-count check, and for `rotate` `UPDATE .. SET id = new WHERE id = old AND expires > now` with a row-count check.
- `SessionData`: `new(now)`, `from_parts`, `values()`, `values_mut()`, `created()`, `touched()`, `touch(now)` and `is_empty()`.
- `InMemorySessionStore::new(&settings, clock)` with `len()`, `is_empty()` and `sweep()`. Per process; `max_sessions` caps it and a new session at the cap removes the expired entries it meets and evicts the oldest-touched entry of a sample of at most 16 entries, which starts at a random shard and at a random position within the first 64 entries of it, at constant cost per write (the cap can be exceeded by the number of racing saves, and the overshoot persists until expiry or a sweep). `eviction_probes()` counts the sampled entries and is hidden from the rustdoc; it exists for tests.
- `CookieSessionBackend::new(&Secret<String>, &[Secret<String>], &settings, clock) -> Result<_, SessionError>`: AES-256-GCM through `aws-lc-rs`, key from HKDF-SHA256 over the whole secret with an empty salt and the info `oxbloom session cookie v1`, the cookie name as associated data, a random 96-bit nonce per write, a secret of at least 32 bytes, old secrets for rotation. Rotate the secret periodically: random nonces cap one key at about 2^32 writes. A copy of a cookie stays valid until its expiry; there is no server-side revocation.
- `SessionBackend`: `Server(Arc<dyn SessionStore>)` or `Cookie(Arc<CookieSessionBackend>)`.
- `SessionFilter::new(backend, settings) -> Result<_, SessionError>` (validates the settings) and `with_clock(clock)`. Order `SESSION_ORDER` = `oxbloom_web::order::SECURITY - 1` (-101). It tries at most 8 same-name cookie values per request (the server backend only values of the 43-character id shape); a `Cookie` header over 8 KiB counts as none; an empty untouched session is never stored and sets no cookie; an untouched session is saved again only when its last touch is more than 60 seconds old; `Cache-Control: no-store` is added with the cookie unless the handler set one; existing `Set-Cookie` headers are kept; a store error or `TooLarge` answers 503 with no session cookie and the side effects of the handler stand. A loaded session is written only with `update` and moved only with `rotate`, so a logout that races another request cannot bring the session back; the request that loses the race stores nothing and leaves the cookie alone, because the dead id is never adopted and a clear could delete a newer cookie; stored data past its creation time plus the absolute timeout counts as unknown at load, and the `ttl` given to `save` and `update` is the remaining lifetime of the session (the idle timeout, cut short by the absolute deadline), so a store that honours only the TTL never serves a session past its absolute deadline; if a rotate commits before a concurrent logout of the old id, the session lives on under the new id; concurrent requests on one session are last-writer-wins.
- `SessionSettings` (builder setters `cookie_name`, `path`, `secure`, `same_site`, `idle_timeout`, `absolute_timeout`, `max_sessions`, `max_cookie_bytes`, and `validate()`), `SameSite` (`Lax`, `Strict`, `None`), `SecureMode` (`Auto`, `Always`, `Never`; `Auto` follows the request scheme, including a forwarded `https`) and `SessionError` (`Store`, `Codec`, `TooLarge`, `InvalidSettings`, `Serialization`, `ReservedKey`; the texts never hold an id, a value or a key). `ReservedKey` is new in slice 7b, so an exhaustive `match` on `SessionError` stops compiling.
- `SessionConfig::from_config(&Config)` with `StoreKind` (`Memory`, `Cookie`) and the constants `SESSION_SECRET_KEY` and `SESSION_OLD_SECRETS_KEY`. Keys: `security.session.store` (`memory`), `.cookie-name` (`OXBLOOM_SESSION`), `.secure` (`auto`), `.same-site` (`lax`; `none` needs `secure = "always"`), `.idle-timeout` (`30m`, at least one second and at most `absolute-timeout`), `.absolute-timeout` (`8h`, positive), `.max-sessions` (`100000`, at least 1), `.max-cookie-bytes` (`3072`, at least 256), `.secret` (at least 32 bytes, required with `store = "cookie"`) and `.old-secrets` (each at least 32 bytes). `secret` and `old-secrets` are refused on the command line (a placeholder can evade this, as for `security.jwt.secret`); `old-secrets` in one environment variable is split on commas, so use the indexed form for a secret that contains a comma.
- Boot: `SessionAutoConfiguration` (feature `security`). It runs only when the table `security.session` exists, after `PasswordEncoderAutoConfiguration` and before `SecurityAutoConfiguration`. A user `dyn SessionStore` bean replaces the in-memory store with `store = "memory"` and is ignored with `store = "cookie"`; a user `SessionFilter` bean steps the configured one aside but must also be provided as `dyn Filter`; one optional `Arc<dyn Clock>` bean feeds the filter, the store and the cookie backend. The feature `security` of `oxbloom-boot` now also enables `dep:oxbloom-web`.
- Semver: the trait `SessionStore` has `update` and `maintain`, and `rotate` returns `Result<bool>`.
- Limits: the session filter does no CSRF check itself (`SameSite=Lax` does not stop every cross-site request; `HttpOnly` only keeps scripts from reading the cookie); with form login on, the chain checks a CSRF token (slice 6d, see below); form login and logout are in slice 6c (see below), the in-memory store is per process and not shared between instances.

#### Form login: `FormLogin`, `HttpSecurity::form_login`, `FormLoginConfig`, `is_safe_redirect_target` (slice 6c)

Guide: [Form login](guide/21-form-login.md). Decisions: [ADR 0088](adr/0088-form-login.md).

```rust
use oxbloom_security::{FormLogin, HttpSecurity, is_safe_redirect_target};

// `form_login` needs a password provider, and a session filter in the application.
let settings = FormLogin::default().default_success_url("/home");
assert!(settings.validate().is_ok());
assert!(is_safe_redirect_target("/orders?page=2"));
assert!(!is_safe_redirect_target("//evil.example"));
let _http = HttpSecurity::new().form_login(move |_| settings);
```

- `HttpSecurity::form_login(|f| ..)` turns the feature on and takes the `FormLogin` builder (a closure from the defaults to the settings). `build` fails with `SecurityBuildError::FormLoginWithoutProvider` without a `password_provider` and with `SecurityBuildError::InvalidFormLoginUrl` when `FormLogin::validate` fails. Both variants are new.
- `FormLogin` (`Debug`, `Clone`, `PartialEq`; nothing in it is secret) has the public fields and builder setters `login_page` (`/login`), `login_processing_url` (`/login`), `logout_url` (`/logout`), `default_success_url` (`/`), `failure_url` (`/login?error`), `logout_success_url` (`/login?logout`), `username_field` (`username`), `password_field` (`password`) and `login_page_enabled` (`true`), and `validate()`. `validate` rejects a URL that fails the redirect rule, a field name that is empty or uses characters other than letters, digits, `_` and `-`, two equal field names, and a logout path equal to the login page or the processing path (compared exactly).
- `is_safe_redirect_target(&str) -> bool`: a single leading `/`, no `//` or `/\` start, no control character or backslash, no `://` before the first `?`, at most 2048 bytes. The saved request is checked with it when saved and again when used.
- `FormLoginConfig::from_config(&Config) -> Result<Option<FormLogin>, ConfigError>` reads `security.form-login.{enabled,login-page,login-processing-url,logout-url,default-success-url,failure-url,logout-success-url,username-field,password-field,login-page-enabled}` with the defaults above; `None` when `enabled` is false or the section is absent. It rejects unknown keys, wrong types, bad URLs and field names, equal field names and a clashing logout path, naming the key and never the value.
- `Authentication` now implements `Serialize` and `Deserialize`, which the session store uses. A deserialized value does not go through `Authentication::new`.
- Behaviour of the chain: `POST` of the processing URL checks the same origin (a mismatch is 403; the handler's empty body is rendered as `application/problem+json` by the error filter inside an `App`), reads an `application/x-www-form-urlencoded` body of at most 4 KiB (another type is 400; a larger body, a missing field or invalid UTF-8 is a failed login), authenticates through the `ProviderManager`, rotates the session id, stores the authentication and answers 302 to the saved request or `default_success_url`. Every failure, a provider outage (`ProviderUnavailable`) included, answers the same 302 to `failure_url` and changes no session. `POST` of the logout URL checks the origin, invalidates the session and answers 302 to `logout_success_url`; `GET` of it changes nothing.
- The session keys `oxbloom.security.authentication` and `oxbloom.security.saved-request` are reserved: the prefix `oxbloom.security.` belongs to Oxbloom, and `Session::set` refuses a key under it since slice 7b (`SessionError::ReservedKey`). The stored authentication is checked again against the user service on an interval (slice 7a, `security.session.revalidate-interval`, default 5 minutes), so a role change or a lock takes effect within one interval. Basic and bearer never create a session, and Basic wins over a session.
- The default entry point is content-negotiated: an anonymous `GET` or `HEAD` that prefers `text/html` gets a 302 to `login_page` and the path and query are saved; every other request keeps the 401 and its challenge and saves nothing. A custom `entry_point(..)` replaces it.
- The default login page is served only after the router answered 404 for a `GET` or `HEAD` of the login path, when `login_page_enabled` is true, so any 404 at that path (also one an application handler chose) is replaced by the page, and the application's own `GET /login` handler always sees an anonymous user. It has fixed texts for `?error` and `?logout`, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff` and the fixed `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; form-action 'self'`. Since slice 6d it always has `frame-ancestors 'none'` in the policy and `X-Frame-Options: DENY` (also with CSRF off), and with CSRF on a hidden CSRF field. Through `App::handle` a `HEAD` answer keeps its body.
- Only `POST` of the processing and logout URLs and `GET` or `HEAD` of the login page skip the authorization rules. The success and failure URLs are not exempt: a custom `failure_url` or `logout_success_url` on another path needs a `permit_all` rule.
- Limits: the same-origin check (`Origin`, else `Referer`, against the request's own scheme, host and port, honouring the trusted forwarded headers; `Origin: null` is rejected) covers only the login and logout posts, and a request with neither `Origin` nor `Referer` is allowed. The CSRF token of slice 6d is checked after it on those two posts and protects the rest (see below).
- Boot: see `SecurityAutoConfiguration` in section 4.8.

#### CSRF tokens: `CsrfConfig`, `CsrfToken`, `CsrfProperties`, `HttpSecurity::csrf`, `RequestSessionExt::csrf_token` (slice 6d)

Guide: [CSRF protection](guide/22-csrf.md). Decisions: [ADR 0089](adr/0089-csrf-tokens.md).

```rust
use oxbloom_security::{CsrfConfig, HttpSecurity, RequestSessionExt};
use oxbloom_web::{HttpMethod, Request};

// With `form_login` on, CSRF protection is on with these defaults; `csrf` tunes or disables it.
let defaults = CsrfConfig::default();
assert!(defaults.enabled);
assert_eq!(defaults.field_name, "_csrf");
assert_eq!(defaults.header_name, "X-CSRF-TOKEN");
let _http = HttpSecurity::new().csrf(|c| c.token_endpoint("/csrf"));
// `None` when no session filter ran; a token has no `Display`, use `as_str` for a template.
assert!(Request::new(HttpMethod::Get, "/").csrf_token().is_none());
```

- `HttpSecurity::csrf(|c| ..)` takes a closure from the defaults to a `CsrfConfig`. With `form_login` on and `csrf` never called, the check is on with the defaults. `build` fails with `SecurityBuildError::CsrfWithoutFormLogin` when `csrf` was called without `form_login` (also with `enabled(false)`) and with `SecurityBuildError::InvalidCsrfConfig` when `CsrfConfig::validate` fails. Both variants are new, and the texts hold no value.
- `CsrfConfig` (`Debug`, `Clone`, `PartialEq`, `Eq`; nothing in it is secret) has the public fields and builder setters `enabled` (`true`), `field_name` (`_csrf`), `header_name` (`X-CSRF-TOKEN`) and `token_endpoint` (none), and `validate(&[&str]) -> Result<(), SecurityBuildError>`. `validate` rejects an empty name or one with characters other than ASCII letters, digits, `-` and `_`, and a token endpoint that fails `is_safe_redirect_target` or has the same path (before `?`) as one of the given form login paths.
- `CsrfToken`: 32 random bytes from the `aws-lc-rs` provider, 43 characters of base64url without padding. It has no `Display` and a redacting `Debug`; `as_str()` gives the value for a template, and it must never be logged. Comparison with a submitted value is constant time, also in the length, and an empty value fails.
- `RequestSessionExt::csrf_token(&self) -> Option<CsrfToken>` (a defaulted method, so an outside implementer of the trait keeps compiling): the token of the session of the request, created and stored when missing; `None` when no session filter ran. It creates no session, and a token created in a new session marks it for writing, so the cookie is set. A stored value that is not a valid token is replaced.
- `CsrfProperties::from_config(&Config, form_login_enabled: bool) -> Result<Option<CsrfConfig>, ConfigError>` reads `security.csrf.{enabled,field-name,header-name,token-endpoint}`. With form login on it always gives `Some`, with `enabled` defaulting to `true`. With form login off it gives `None`, and `enabled = true` and every other `security.csrf` key are errors that name the key (`enabled = false` alone is accepted). It rejects unknown keys, wrong types, a bad name and a bad endpoint, naming the key and never the value. That the endpoint differs from the form login paths is checked later, by `HttpSecurity::build`.
- Behaviour of the chain: a request is checked when its method is not `GET`, `HEAD`, `OPTIONS` or `TRACE` (so `CONNECT` and unknown methods are checked) and the session authenticates it, and on `POST` of the login and logout URLs, which also pass the same-origin check first. The token comes from the header `header_name`, else from the field `field_name` of an `application/x-www-form-urlencoded` body of at most 4 KiB (a larger urlencoded form, or a body with one pair that does not decode, always gets 403 and must send the header); the first value wins, the query string is never read, and a multipart or JSON body without the header fails. A failed check answers a fixed 403 with an empty body (rendered as `application/problem+json` inside an `App`), skips a custom `access_denied` handler and changes nothing. `Basic` and bearer requests, anonymous requests without a session and anonymous requests with a pre-login session are not checked. Login rotates the session id and replaces the token; logout invalidates the session. A refused login without a session filter logs the fixed error "form login has no session: add a SessionFilter to the application".
- The token header is marked sensitive by the chain, so the `Debug` output of a request shows `Sensitive` for it. The form field is part of the body and is visible in that output, like the password of the login form.
- The token is stored under the session key `oxbloom.security.csrf-token`; the prefix `oxbloom.security.` is reserved.
- The default login page, with CSRF on, creates the pre-login session and the token (when the fallback renders it), writes the hidden field (only the field depends on CSRF); the framing headers are always added (see the default page above). When `token_endpoint` is set, a `GET` of that exact path answers 200 with `Content-Type: application/json`, `Cache-Control: no-store` and `{"headerName":"..","parameterName":"..","token":".."}`, creating the session and token when missing. The chain answers it before the application route. Without a session filter it logs the same fixed error and the request goes on to the application. The page and the endpoint are anonymous session writers, bounded by sampled eviction.
- Limits: a script on the same origin (XSS) can read the token; a multipart form needs the header; with either session backend an attacker who controls a sibling subdomain can plant the cookie `OXBLOOM_SESSION` (no `__Host-` prefix) of a session they logged in to and so know its token; `Basic` requests are exempt, but a browser with cached Basic credentials sends them on forged requests; every chain response carries `X-Frame-Options: DENY` by default (slice 7a) and only the login page also has `frame-ancestors 'none'`; the same-origin check still covers only login and logout; anonymous requests with no session, and with a pre-login session, are exempt; an application that rotates the session inside its handler while it answers a `404` for the login page gets a page with a dead token.
- Boot: see `SecurityAutoConfiguration` in section 4.8.

#### Security headers and session management (slice 7a)

- `HttpSecurity::headers(|h| ..)` takes a closure from the current `HeadersConfig` (a new `HttpSecurity` starts from the defaults, and each call changes only what it touches). `HeadersConfig` (`Debug`, `Clone`, `PartialEq`, `Eq`) has the builder setters `enabled`, `content_type_options`, `referrer_policy`, `frame_options` and `cache_control` (each takes `Option<&str>`, `None` removes the header), `hsts(|s| ..)` over `HstsConfig` (`enabled`, `max_age`, `include_subdomains`, `preload`) and `validate`. `HstsConfig` and `HeadersConfig` are exported. `build` fails with `SecurityBuildError::InvalidHeadersConfig` for a name or value that is not a valid header token (visible ASCII, no control character), also when the headers are disabled. The text holds no value.
- The chain adds the headers to every response it returns or passes on, including 401, 403, redirects, the default login page and the token endpoint. A header that the response has is never replaced, and names are compared without regard to case. Defaults, all on: `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, `Cache-Control: no-store` (only when the request was authenticated by the session, `Basic` or a bearer token and the response has no `Cache-Control`) and `Strict-Transport-Security: max-age=31536000; includeSubDomains` (only when `Request::scheme()` is `Scheme::Https`). There is no default content security policy.
- `HeadersProperties::from_config(&Config) -> Result<HeadersConfig, ConfigError>` reads `security.headers.{enabled, content-type-options, frame-options, referrer-policy, cache-control}` and `security.headers.hsts.{enabled, max-age, include-subdomains, preload}`. The string `off`, in any ASCII case, removes a header, `hsts.max-age` is at least one second, and unknown keys, wrong types and bad values fail naming the key and never the value.
- `PrincipalSessions` (an async trait, separate from `SessionStore`) has `sessions_of(principal) -> Result<Vec<SessionSummary>, SessionError>` (oldest touched first), `delete_of(principal, except: Option<&str>) -> Result<usize, SessionError>` and `evict_oldest(principal, keep, except)`. `InMemorySessionStore` implements it by scanning its shards for the value of the reserved key `oxbloom.security.principal`; the cookie backend does not. `SessionStore::principal_sessions(self: Arc<Self>) -> Option<Arc<dyn PrincipalSessions>>` is a defaulted method (`None`) that the in-memory store overrides, and `SessionFilter::principal_sessions` exposes it (`None` for the cookie backend).
- `SessionSummary` has the accessors `id`, `created` and `touched`; its `Debug` prints none of them.
- `SessionRegistry::new(Arc<dyn PrincipalSessions>)` with `sessions_of(name)`, `invalidate_all(name) -> usize` and `invalidate_others(name, current_id) -> usize`, all `Result<_, SessionError>` (`SessionError::Store` when the store fails). It is `Clone` and its `Debug` prints `SessionRegistry { .. }`.
- `HttpSecurity::session_limit(sessions: Arc<dyn PrincipalSessions>, max_per_user: usize)`: at a successful form login the sessions of the user touched longest ago are ended until `max - 1` others remain (the last use of a session is recorded at most once a minute); the session that the login replaces is never counted, and a failed login evicts nothing. A store error is logged with a fixed text and never fails the login. `build` fails with `SecurityBuildError::SessionLimitWithoutFormLogin` without `form_login` and with `SecurityBuildError::InvalidSessionLimit` for zero.
- `HttpSecurity::revalidate(interval: Duration, users: Arc<dyn UserDetailsService>)` and `HttpSecurity::clock(Arc<dyn Clock>)` (default `SystemClock`). When a session login has no check time, or one older than `interval` or in the future, the chain reloads the user by the principal of the stored authentication. A user that is gone, disabled or locked ends the session and the request goes on anonymous; changed authorities are stored and used; no change only advances the time; a user service error keeps the session and does not advance the time. `build` fails with `SecurityBuildError::RevalidateWithoutFormLogin` without `form_login` and with `SecurityBuildError::InvalidRevalidateInterval` for a zero interval.
- Form login writes the reserved session keys `oxbloom.security.principal` (the user name as a JSON string) and `oxbloom.security.revalidated-at` (seconds since the epoch of the last check), at every login, and the second also at every check.
- `SessionManagementProperties::from_config(&Config) -> Result<SessionManagement, ConfigError>` reads `security.session.max-sessions-per-user` (at least 1, default none) and `security.session.revalidate-interval` (default 5 minutes; `0` is off; a value under one second is refused). `SessionManagement` holds `max_sessions_per_user`, `revalidate_interval` and `revalidate_explicit`. The existing `SessionConfig::from_config` accepts both keys and ignores them. The key `security.session.max-sessions` is the cap of the whole in-memory store and is unchanged.
- Limits: concurrent logins of one user can overshoot the limit by N - 1 sessions for N logins at the same moment; a re-login can race a login of the same user that evicts the session it replaces, which leaves the user anonymous (fail-closed); the cookie backend has no index, so no limit, no registry and no "log out everywhere", and with it a copy of the cookie that was taken after the last check stays valid for up to one interval after the session ended; a changed password does not end sessions; revalidation is bounded by the interval; a user service outage keeps the sessions; `Session::set` did not enforce the reserved prefix `oxbloom.security.` in this slice, so application code could write the principal key and the per-user index trusts it (slice 7b enforces the prefix); `delete_of` and `invalidate_all` do not end a session that is saved while they run.
- Boot: see `SecurityAutoConfiguration` and `SessionRegistry` in section 4.8. The usage is in the guide chapter [Session management and security headers](guide/23-session-management.md).

#### Remember-me (slice 7b)

Guide: [Remember-me](guide/24-remember-me.md). Decisions: [ADR 0091](adr/0091-remember-me.md).

```rust
use std::sync::Arc;
use std::time::Duration;
use oxbloom_config::Secret;
use oxbloom_security::{
    Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity, InMemoryUserDetailsService,
};

let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
let chain = HttpSecurity::new()
    .password_provider(Arc::new(DaoAuthenticationProvider::new(users.clone(), encoder)))
    .form_login(|f| f)
    .remember_me(|r| {
        r.secret(Secret::new("a-remember-me-secret-of-32-bytes".to_owned()))
            .user_details(users)
            .lifetime(Duration::from_secs(7 * 24 * 3600))
    })
    .authorize(|a| a.path("/account/**").fully_authenticated().any_request().authenticated())
    .build();
assert!(chain.is_ok());
```

- `HttpSecurity::remember_me(|r| ..)` takes a closure from the current `RememberMe` and composes: a new `HttpSecurity` starts with none, and a later call sees what an earlier call set. `RememberMe` (`Clone`; `Debug` shows no secret, only the number of old secrets and whether a user service is set) has the setters `secret(Secret<String>)`, `old_secrets(Vec<Secret<String>>)`, `lifetime(Duration)` (default 14 days; 1 minute to 365 days), `field_name(impl Into<String>)` (default `remember-me`), `cookie_name(impl Into<String>)` (default `OXBLOOM_REMEMBER_ME`; an HTTP token), `user_details(Arc<dyn UserDetailsService>)` and `clock(Arc<dyn Clock>)` (default `SystemClock`). `build` fails with `SecurityBuildError::RememberMeWithoutFormLogin` without `form_login` and with `SecurityBuildError::InvalidRememberMe` for a missing secret, a secret (current or old) under 32 bytes, a missing user service, a lifetime outside the range, an invalid cookie name or an invalid field name; the texts name no value. Both variants are new, so an exhaustive `match` on `SecurityBuildError` stops compiling.
- `RememberMeProperties::from_config(&Config) -> Result<Option<RememberMeSettings>, ConfigError>` reads `security.remember-me.{enabled, secret, old-secrets, lifetime, field-name, cookie-name}`. It returns `None` when `enabled` is not `true`, but it checks every value either way, so a bad value in a switched-off section fails. A secret on the command line is refused, and unknown keys and wrong types fail naming the key and never the value. `RememberMeSettings` holds the values (`secret` as an `Option`, because boot decides what a missing secret means; `Debug` prints `[REDACTED]`) and `apply(RememberMe) -> RememberMe` copies them into the builder settings.
- The cookie is `v1.<base64url(username)>.<expiry seconds>.<base64url(HMAC-SHA256)>`. The MAC covers a version tag, the length-prefixed user name, the expiry and `UserDetails::password()` (the stored hash), under a key from HKDF-SHA256 over the secret with the info `oxbloom remember-me v1`; each comparison is constant time (`hmac::verify`), current key first and then each old key; the number of comparisons shows which key matched, only for a valid MAC. A token over 1 KiB is refused, and so is an expiry that `SystemTime` cannot hold. The user name is in the clear (base64url, not encrypted). A changed password, a deleted, disabled or locked user ends the cookie at once.
- Issue: after a successful password login whose form has the field with `on`, `true`, `yes` or `1` (ASCII case ignored), the answer sets the cookie: `HttpOnly`, `SameSite=Lax`, `Path` and `Secure` from the session settings through the request extension `CookieScope` that the session filter sets (crate-private), and a fixed `Max-Age` equal to the lifetime. A failed login sets and clears nothing. A successful login without the box clears a remember-me cookie that the request carries (nothing when it carries none). Logout clears the cookie with `Max-Age=0`. The default login page shows the checkbox when remember-me is on; a custom page posts the field itself.
- Auto login: in the chain, only when the request has no `Authorization` header (of any scheme) and the session gave no login. Valid means the format parses, the expiry is in the future, the user exists in the `UserDetailsService`, is enabled and not locked, and the MAC verifies. The chain then creates or rotates the session, stores the authentication marked `remembered`, writes `oxbloom.security.principal` and `oxbloom.security.revalidated-at`, replaces the CSRF token, applies the slice 7a session limit and continues authenticated. An invalid cookie of any kind gets a clearing `Set-Cookie` and an anonymous request. A user service error keeps the cookie and continues anonymous with a fixed log text. A cookie header over 8 KiB counts as no cookie. Without a session filter there is a fixed log text and no auto login.
- `Authentication::is_remembered()` and `remembered()` (see above), `AuthorizationRule::FullyAuthenticated` and `RuleBuilder::fully_authenticated()`: grants an authenticated request that is not remembered. Basic and bearer count as fully authenticated, and `authenticated()` still accepts remembered users. A password login stores an authentication without the marker.
- `TestResponse::header_all(name) -> Vec<&str>` in `oxbloom-test` returns every value of a header, for example two `Set-Cookie` lines.
- Limits: no per-device revocation and no theft detection; a stolen cookie works until it expires or the password changes; the lifetime is fixed; an auto login on an unsafe request is a 403 until a `GET` (also a logout `POST` after the session expired with CSRF on, which keeps the cookie); the cookie holds the user name in the clear; the `UserDetailsService` must know every form-login user; any `Authorization` header blocks auto login; log out everywhere (`SessionRegistry`) and the session limit do not revoke remember-me, so an evicted or ended session logs straight back in, and a burst of parallel requests after the session expired can each auto log in and count against the limit; a JWT claim `oxbloom.remembered: true` counts as remembered (deny-only); a customizer's `http.clock(..)` does not change the clock of remember-me; the keys are applied only by `SecurityAutoConfiguration`.
- Boot: see `SecurityAutoConfiguration` in section 4.8. The usage is in the guide chapter [Remember-me](guide/24-remember-me.md).

#### Method security (slice 8)

Guide: [Method security](guide/25-method-security.md). Decisions: [ADR 0092](adr/0092-method-security.md). There are no configuration keys.

```rust
use oxbloom_security::{
    AccessDenied, Authentication, GrantedAuthority, SecurityContext, method_security,
};

struct Accounts;

#[method_security]
impl Accounts {
    #[pre_authorize(role = "ADMIN")]
    fn close(&self, id: u64) -> Result<u64, AccessDenied> {
        Ok(id)
    }
}

# tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
let admin = Authentication::new("ada", [GrantedAuthority::new("ROLE_ADMIN")]);
let ctx = SecurityContext::authenticated(admin);
assert_eq!(SecurityContext::scope(ctx, async { Accounts.close(7) }).await, Ok(7));
assert_eq!(Accounts.close(7), Err(AccessDenied::Unauthenticated));
# });
# Ok::<(), std::io::Error>(())
```

- `#[method_security]` (crate `oxbloom-macros`, re-exported as `oxbloom_security::method_security`) goes on an inherent `impl` block and rewrites its methods in place; there is no proxy. The method attributes `#[pre_authorize(rule)]`, `#[post_authorize(rule)]`, `#[pre_filter(arg = ids, custom = keep)]` and `#[post_filter(custom = keep)]` are re-exported too, but only work inside such a block (elsewhere each is a compile error), and need no import there. The generated code names the crate `oxbloom_security`, so the dependency must not be renamed. A trait impl, a repeated attribute, an unknown rule or option and a visibly non-`Result` return type are compile errors.
- Rules: `role = ".."`, `any_role(..)`, `authority = ".."`, `any_authority(..)`, `authenticated`, `fully_authenticated`, `permit_all`, `deny_all`, `all(..)`, `any(..)`, `not(..)` and `custom = check(arg, ..)`. `check` is a synchronous `fn(Option<&Authentication>, &A, ..) -> bool` and each argument is a plain parameter name of the method (a pattern parameter cannot be named). In `post_authorize` the reserved name `returned` is `&T` of the `Ok` value, and a parameter can be named there only when the body did not move it. Roles and authorities use the same functions as the path rules of the chain.
- Execution order, whatever the order of the attributes: `pre_filter`, `pre_authorize`, the body, `post_authorize`, `post_filter`. A denied `pre_authorize` skips the body and the post steps. A denied `post_authorize` drops the value. An `Err` from the body skips the post steps. Filters never deny. `pre_filter` takes a by-value `Vec<T>` parameter; `post_filter` takes a `Vec<T>` or any type with `retain(FnMut(&T) -> bool)` in the `Ok` value.
- Return types: with `pre_authorize`, `post_authorize` or `post_filter` the method returns `Result<T, E>` with `E: From<AccessDenied>`; with a post attribute the type must be written out (no `impl Trait`). A method with only `pre_filter` may return anything.
- A post attribute wraps the body (a closure for a synchronous method, an `async` block for an `async` one) so that `return` and `?` leave only the body. In an `async` method the macro rewrites the body's `return`s into `break`s out of a block of the declared return type, which keeps their coercions. A `return` inside a macro call is not rewritten and, when it needs a coercion, fails with E0308 at the attribute line (a documented limitation). A trailing `return Ok(x);` there becomes a `break`, so `clippy::needless_return` no longer reports it.
- `AccessDenied` (`Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Display`, `Error`) has the variants `Unauthenticated` (an anonymous caller) and `Forbidden` (any authenticated caller). `AccessDenied::for_caller(Option<&Authentication>) -> AccessDenied` picks one. `From<AccessDenied> for HttpError` gives 401 and 403, the same status codes as the chain (only the status codes: a method denial does not use the entry point or the access-denied handler).
- `SecurityContext::current() -> SecurityContext` returns a clone of the context of the running request, or the anonymous context outside any scope. `async SecurityContext::scope(context, future) -> F::Output` runs the future with that context. The context is a Tokio task-local: the chain wraps the rest of every request in `scope`, and a task started with a bare `tokio::spawn` does not inherit it, so its secured calls are denied (fail closed). `oxbloom_security::spawn(future) -> JoinHandle<F::Output>` starts a task that carries the current context.
- The check of an `async` method runs at the first poll of its future, not when the method is called. A denial logs the method (`Type::method`) and the principal at debug level on the target `oxbloom::security`; argument values and returned values are never logged.
- `oxbloom_test::with_authentication(authentication, future)` (feature `security` of `oxbloom-test`) runs a future with that caller, for service tests without HTTP. See section 4.10.
- Limits: method denials do not use the entry point or the access-denied handler of the chain, so a 401 carries no `WWW-Authenticate` challenge, a form-login application does not redirect an anonymous browser to the login page, and a custom 403 handler is skipped; `custom = f(self)` cannot name the receiver (error "not a parameter of the method"); converting `AccessDenied` into an error type such as `anyhow::Error` or `Box<dyn Error>` loses the 401/403 mapping and gives a 500, because only `From<AccessDenied> for HttpError` maps; `custom` checks are synchronous; there is no expression language, no caching of decisions, and only inherent impls are supported; the check of an `async` method sees the context of its first poll; the role rules of `HasAnyRole` and `HasAnyAuthority` allocate a small `Vec<&str>` per call.

Roadmap: slices 7a (security headers, session management), 7b (remember-me), 8 (method security) and 9 part B (OAuth2 login, see [section 4.15](#415-oauth2-login)) are done; the OAuth2 client (Slice 9 part C) follows. Not implemented:
the OAuth2 client, RP-initiated logout, opaque-token introspection, several chains per app, login rate limiting, database user stores.

### 4.8 `oxbloom-boot`

The application entry point. `OxbloomApplication::builder()` loads the configuration, builds the
container, fires `RunListener` events, runs the `ApplicationRunner` and `CommandLineRunner` beans,
starts the `Service` beans, waits for Ctrl-C, SIGTERM or a `ShutdownHandle` trigger, stops the
services in reverse order under one deadline, shuts the container down and returns an
`ExitStatus`. The README section "Application lifecycle" describes the run order and the limits in
one place. Rules to remember:

- With no `Service` bean, `run()` returns right after the runners and listens for no signal.
- Boot registers `Config`, `ApplicationArguments` and `ShutdownHandle` as instances; registering
  one of them again is a `StartupError` in the `Container` phase. It also sets `Config` as the
  container `environment` before the `configure` closures run, so a property condition in a
  closure reads the configuration; a closure that calls `environment(..)` replaces it for every
  condition, those of the auto-configurations included.
- Auto-configurations are registered after the `configure` closures, so user beans win over
  `on_missing_bean` defaults. The README section "Conditions and auto-configuration" describes the
  model, the ordering rules and the limits in one place.
- Runner and `Service::start` panics propagate. Only a `Service::stop` panic is caught and reported.
- A Ctrl-C or SIGTERM while the services stop forces code 130 or 143 and skips `Container::shutdown()`.
- Signal handlers are installed from just before the services start; a signal during a slow `start`
  is kept and ends the wait as soon as the services run.
- The stop deadline cannot interrupt a `stop` that blocks the thread (a blocking
  `std::thread::sleep` on a current-thread runtime delays `run()`); the `pre_destroy` hooks are not
  bounded by it and `Service::start` has no timeout.
- Dropping the `run()` future, or losing it in a `select!`, leaves the started services running and
  never shuts the container down; use `ShutdownHandle::trigger()` instead.
- No message produced by boot contains configuration values, argument values or secrets. The one
  exception is the port-in-use failure report, which shows the socket address.
- A startup failure is analyzed by the `FailureAnalyzer`s (builder, then beans, then built-ins);
  the first `FailureAnalysis` is attached to the `StartupError` and printed as the failure report by
  `run_and_exit()` (by `run()` only with `failure_report(true)`). Analyzers do not see failures after
  startup. The README section "Application lifecycle", subsection "Failure analysis", lists the
  built-in analyzers and the limits.

Cargo features: `web` adds `WebServer`, `BindError`, the port-in-use analyzer and
`WebServerAutoConfiguration` and needs `oxbloom-web`; `security` enables `oxbloom-security` with
its `di` feature and adds the security analyzer, a built-in analyzer for invalid `security.jwt`
configuration, `JwtDecoderAutoConfiguration`, `SecurityAutoConfiguration` and the `SecurityCustomizer` trait;
`logging` adds `oxbloom-logging`, `OxbloomApplicationBuilder::logging`, the startup events and the
`LoggingSystem` bean (see section 4.9).
`auto_configurations()` lists the auto-configurations of the enabled features. The examples below use a
current-thread runtime with `enable_all()` (as `#[tokio::test]` does) and run inside an `async`
function that returns `Result<(), Box<dyn std::error::Error>>`.

Imports:

```rust
use std::sync::Arc;
use std::time::Duration;
use oxbloom_boot::{async_trait, ApplicationArguments, ApplicationRunner, Banner, BindError,
                   CommandLineRunner, ExitCodeGenerator, ExitStatus, FailureAnalysis,
                   FailureAnalyzer, OxbloomApplication, RunListener, RunnerError, Service,
                   ServiceError, ShutdownHandle, StartupError, StartupPhase, WebServer};
use oxbloom_boot::{auto_configuration, AutoConfiguration, AutoConfigurations, ConfigRef,
                   Conditions, Configuration};
use oxbloom_config::Config;
use oxbloom_di::Container;
```

#### `ApplicationArguments::new(args)`

Parses a command line, program name excluded. `--k=v` is an option (the value is everything after
the first `=`), `--k` is an option with the empty value, `--` ends option parsing, and everything
else (`-x`, `--=x`, plain words, anything after `--`) is a non-option argument. Names are
case-sensitive. Boot builds one from `.args(..)` and registers it as a bean; build one yourself in
tests. `Debug` and `Display` show counts only.

```rust
let args = ApplicationArguments::new(["--name=oxbloom", "--verbose", "serve", "--", "--rest"]);
assert_eq!(args.option("name"), Some("oxbloom"));
assert_eq!(args.option("verbose"), Some(""));
assert_eq!(args.non_option_args(), ["serve", "--rest"]);
assert_eq!(format!("{args:?}"), "ApplicationArguments { options: 2, non_option_args: 2 }");
assert_eq!(args.to_string(), "ApplicationArguments(2 options, 2 non-option arguments)");
```

#### `ApplicationArguments::option(name)` and `ApplicationArguments::options(name)`

`option` returns the first value of an option, `options` all of them in order (empty when absent).
Use `options` for repeatable flags such as `--tag=a --tag=b`.

```rust
let args = ApplicationArguments::new(["--tag=a", "--tag=b"]);
assert_eq!(args.option("tag"), Some("a"));
assert_eq!(args.options("tag"), ["a", "b"]);
assert_eq!(args.option("other"), None);
assert!(args.options("other").is_empty());
```

#### `ApplicationArguments::option_names()`

Every option name once, in first-seen order.

```rust
let args = ApplicationArguments::new(["--b=1", "--a=2", "--b=3"]);
assert_eq!(args.option_names().collect::<Vec<_>>(), ["b", "a"]);
```

#### `ApplicationArguments::non_option_args()` and `ApplicationArguments::raw()`

`non_option_args` returns the arguments that are not options, in order; `raw` returns every
argument exactly as given. A `CommandLineRunner` receives `raw()`.

```rust
let args = ApplicationArguments::new(["run", "--a=1", "-x"]);
assert_eq!(args.non_option_args(), ["run", "-x"]);
assert_eq!(args.raw(), ["run", "--a=1", "-x"]);
```

#### `OxbloomApplication::builder()`

Starts configuring a run. `run()` returns `Result<ExitStatus, StartupError>`.

```rust
let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .output(std::io::sink())
    .run()
    .await?;
assert_eq!(status.code(), 0);
```

#### `OxbloomApplicationBuilder::args(iter)`

The command-line arguments, program name excluded. Default: none. They become the
`ApplicationArguments` bean and are applied to the `ConfigBuilder` after the `config` closure,
so `--key=value` overrides and `--oxbloom.profiles.active=dev` work and a closure cannot drop them.
Arguments are `String`: a program that must survive non-UTF-8 arguments passes
`std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned())`.

```rust
let builder = OxbloomApplication::builder().args(std::env::args().skip(1));
```

#### `OxbloomApplicationBuilder::config(closure)`

Adjusts the `ConfigBuilder` that boot creates with `Config::builder()`. The last of `config` and
`with_config` wins.

```rust
let status = OxbloomApplication::builder()
    .args(["--app.name=demo"])
    .config(|b| b.dir("config").env_from([]))
    .configure(|b| b.bean(|config: Arc<Config>| config.get::<String>("app.name").unwrap_or_default()))
    .output(std::io::sink())
    .run()
    .await?;
assert_eq!(status.code(), 0);
```

#### `OxbloomApplicationBuilder::with_config(config)`

Uses a ready `Config`, typically in tests. The `args` still populate `ApplicationArguments` but are
not applied to this config.

```rust
let builder = OxbloomApplication::builder().with_config(Config::from_str("[server]\nport = 0")?);
```

#### `OxbloomApplicationBuilder::configure(closure)`

Adds user wiring to the container. Repeatable; closures apply in call order, after boot's own
instances, which you can inject. `Config` is already the container environment, so
`.on_property(..)` and the other property conditions in a closure read the configuration.

```rust
struct Greeting(String);

let status = OxbloomApplication::builder()
    .args(["--name=Ada"])
    .with_config(Config::from_str("")?)
    .configure(|b| {
        b.bean(|args: Arc<ApplicationArguments>| {
            Greeting(format!("hello {}", args.option("name").unwrap_or("world")))
        })
    })
    .output(std::io::sink())
    .run()
    .await?;
assert_eq!(status.code(), 0);

// Boot already registers `Config`; a second one is a `Container`-phase error.
let err = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .configure(|b| b.instance(Config::from_str("").unwrap()))
    .output(std::io::sink())
    .run()
    .await
    .unwrap_err();
assert_eq!(err.phase(), StartupPhase::Container);
```

#### `AutoConfiguration`: `conditions`, `after`, `before`, `properties`

A `Configuration` that an application adds as a unit. Its registrations form one group whose
`conditions()` are decided once; `after()` and `before()` place it among the other
auto-configurations. Every method has a default (no conditions, no constraints). The trait
requires `Configuration + Component`: boot registers the instance with `component::<A>()` as the
first registration of the group, then calls `A::register`. Implement it with
`#[auto_configuration]` (see [4.2](#42-oxbloom-macros-re-exported-by-oxbloom-web-and-oxbloom-di)) or
by hand:

```rust
use oxbloom_boot::{AutoConfiguration, ConfigRef, Conditions, Configuration};
use oxbloom_di::{component, ContainerBuilder};

pub struct Limits { pub max: u32 }

#[component]
pub struct CoreDefaults;
impl Configuration for CoreDefaults {
    fn register(builder: ContainerBuilder) -> ContainerBuilder { builder }
}
impl AutoConfiguration for CoreDefaults {}

#[component]
pub struct LimitsDefaults;
impl Configuration for LimitsDefaults {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.bean(|| Limits { max: 10 })
    }
}
impl AutoConfiguration for LimitsDefaults {
    // Back off when the application registered its own `Limits`.
    fn conditions() -> Conditions {
        Conditions::new().on_missing_bean::<Limits>()
    }
    fn after() -> Vec<ConfigRef> {
        vec![ConfigRef::of::<CoreDefaults>()]
    }
}

let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .auto_configuration::<LimitsDefaults>()
    .auto_configuration::<CoreDefaults>()
    .configure(|b| b.instance(Limits { max: 99 }))
    .output(std::io::sink())
    .run()
    .await?;
assert_eq!(status.code(), 0);
```

#### `ConfigRef`: `of`, `name`

Identifies a configuration type in `after()` and `before()` by its `TypeId`, with its type name for
messages. Two refs are equal exactly when they name the same type.

```rust
use oxbloom_boot::ConfigRef;

struct DataSourceConfig;
struct CacheConfig;

let r = ConfigRef::of::<DataSourceConfig>();
assert_eq!(r, ConfigRef::of::<DataSourceConfig>());
assert_ne!(r, ConfigRef::of::<CacheConfig>());
assert!(r.name().ends_with("DataSourceConfig"));
```

#### `AutoConfigurations`: `new`, `add`, `extend`, `len`, `is_empty`

An ordered list of auto-configurations, the unit a crate that ships them exposes as
`pub fn auto_configurations() -> AutoConfigurations`. A configuration appears at most once: adding
one that is already in the list, directly or through `extend`, keeps the first entry and its
position.

```rust
use oxbloom_boot::{auto_configuration, AutoConfigurations};
use oxbloom_di::component;

#[component]
pub struct JsonAutoConfig;
#[auto_configuration]
impl JsonAutoConfig {}

#[component]
pub struct WebAutoConfig;
#[auto_configuration(after(JsonAutoConfig))]
impl WebAutoConfig {}

/// What a starter crate exposes.
pub fn auto_configurations() -> AutoConfigurations {
    AutoConfigurations::new().add::<JsonAutoConfig>().add::<WebAutoConfig>()
}

let mut list = auto_configurations().add::<JsonAutoConfig>(); // already present
assert_eq!(list.len(), 2);
list.extend(AutoConfigurations::new().add::<WebAutoConfig>());
assert_eq!(list.len(), 2);
assert!(AutoConfigurations::new().is_empty());
```

#### `OxbloomApplicationBuilder::auto_configuration::<A>()` and `auto_configurations(list)`

Add one auto-configuration, or every entry of a list, to the application's list. Lists combine in
call order; a configuration already in the list keeps its first position. Boot registers them after
the `configure` closures, each as one group named by its type name with its class-level
conditions, in the order their `after` and `before` entries give. User registrations therefore come
first, and a default with `on_missing_bean` backs off. Put overrides of a starter's bean in
`configure`: an auto-configuration of your own that comes after the starter's in the list is
registered after its default, so the default is kept and both beans stay (a `Duplicate`, or an
`Ambiguous` trait). The ordering is a stable topological sort:
among the configurations whose predecessors are all placed, the earliest in the list goes next. A
reference to a configuration that is not in the list, and a reference to itself, are ignored. An
ordering cycle fails the run in the `Container` phase before the container is built, with the text
`auto-configurations form an ordering cycle: A -> B -> A` (each arrow points from a configuration
to one it must come after) and a failure report with an action.

```rust
use oxbloom_boot::auto_configuration;
use oxbloom_di::component;

pub struct Greeter(pub &'static str);

#[component]
pub struct GreeterAutoConfig;

#[auto_configuration]
impl GreeterAutoConfig {
    // Backs off when the application registered its own `Greeter`.
    #[bean(on_missing_bean)]
    fn greeter(&self) -> Greeter { Greeter("default") }
}

let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .auto_configuration::<GreeterAutoConfig>()
    .configure(|b| b.instance(Greeter("mine")))
    .configure(|b| b.bean(|g: Arc<Greeter>| { assert_eq!(g.0, "mine"); 1_u8 }))
    .output(std::io::sink())
    .run()
    .await?;
assert_eq!(status.code(), 0);
```

#### `OxbloomApplicationBuilder::exclude::<A>()`

Drops the auto-configuration `A` from the combined list: it is not registered, and `after` or
`before` entries that name it are ignored, so the order of the rest stays valid. It applies when
`run()` resolves the list, so it works before or after `A` is added; excluding a configuration that
is not in the list does nothing. `A` must implement `AutoConfiguration`. Use it to replace a
starter's default with your own wiring.

```rust
use oxbloom_boot::{auto_configuration, AutoConfigurations};
use oxbloom_di::component;

pub struct Metrics(pub &'static str);

#[component]
pub struct MetricsAutoConfig;
#[auto_configuration]
impl MetricsAutoConfig {
    #[bean]
    fn metrics(&self) -> Metrics { Metrics("default") }
}

let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .exclude::<MetricsAutoConfig>()
    .auto_configurations(AutoConfigurations::new().add::<MetricsAutoConfig>())
    // Without the exclusion this would be a duplicate `Metrics`.
    .configure(|b| b.instance(Metrics("mine")))
    .output(std::io::sink())
    .run()
    .await?;
assert_eq!(status.code(), 0);
```

#### Setting `oxbloom.main.conditions-report`

`off` (the default, also when the key is absent) or `console`. In `console` mode boot builds the
container with `build_with_report` and writes the conditions evaluation report to the output target
right after the build: before `context_ready` and the runners on success, and before `failed` and
the failure report when the build fails (the two blocks follow each other without a blank line).
Any other value, including another case such as `Console` or a list, fails the run in the `Config`
phase with an error that names the key but not the value. Boot reads the setting before it prints
the banner, so a rejected value prints nothing. An ordering cycle fails before the build, so it
prints no conditions report. A build that fails with `Invalid` (builder misuse) evaluates no
condition, so its report shows `(none)` in both sections.

```rust
#[derive(Clone, Default)]
struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

struct Cache;

let out = Buf::default();
OxbloomApplication::builder()
    // The same as `--oxbloom.main.conditions-report=console` on the command line.
    .with_config(Config::from_str("oxbloom.main.conditions-report = \"console\"")?)
    .banner(Banner::off())
    .configure(|b| b.instance(Cache).on_property("cache.enabled"))
    .output(out.clone())
    .run()
    .await?;
let text = String::from_utf8(out.0.lock().unwrap().clone())?;
assert!(text.starts_with("CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n"));
assert!(text.ends_with("on_property - property \"cache.enabled\" is not set\n"));

let err = OxbloomApplication::builder()
    .with_config(Config::from_str("oxbloom.main.conditions-report = \"verbose\"")?)
    .output(std::io::sink())
    .run()
    .await
    .unwrap_err();
assert_eq!(err.phase(), StartupPhase::Config);
assert!(!err.to_string().contains("verbose"));
```

#### `PropertyMetadata`: `new`, `to_json`

Describes one configuration key: `key` (dotted), `type_name` (text such as `u16`), `default`
(`Option<&'static str>`) and `description`. All fields are public and `&'static`, so a list can live in
a `static`. The text describes a key and never carries a value, so a secret key is described and
nothing more. `PropertyMetadata::new(key, type_name, default, description)` is a `const fn`.
`PropertyMetadata::to_json(&[PropertyMetadata]) -> String` writes a JSON array of objects with the
fields `key`, `type`, `default` (`null` when absent) and `description`, in that order, without
whitespace; quotes, backslashes and control characters are escaped.

```rust
use oxbloom_boot::PropertyMetadata;

static KEYS: [PropertyMetadata; 1] =
    [PropertyMetadata::new("cache.size", "usize", Some("100"), "Maximum entries.")];
assert_eq!(
    PropertyMetadata::to_json(&KEYS),
    r#"[{"key":"cache.size","type":"usize","default":"100","description":"Maximum entries."}]"#
);
assert_eq!(PropertyMetadata::to_json(&[]), "[]");
```

#### `boot_properties()`

`pub fn boot_properties() -> &'static [PropertyMetadata]`. The seven keys that the framework reads itself:
`oxbloom.config.import`, `oxbloom.profiles.active` and `oxbloom.profiles.group` (read by the config
loader), `oxbloom.main.banner-mode` and `oxbloom.main.conditions-report` (read by boot), and, read
only with the `logging` feature but listed with or without it, `oxbloom.logging.enabled` (default
`true`) and `application.name` (default `application`).

```rust
use oxbloom_boot::boot_properties;

let keys: Vec<_> = boot_properties().iter().map(|p| p.key).collect();
assert!(keys.contains(&"oxbloom.main.banner-mode"));
assert_eq!(keys.len(), 7);
```

#### `AutoConfiguration::properties()`

`fn properties() -> &'static [PropertyMetadata]`, a fourth method of `AutoConfiguration` next to
`conditions`, `after` and `before`. The default is the empty list. Implement it to list the keys the
configuration reads; the `#[auto_configuration]` macro has no option for it yet, so the method goes in
the hand-written `impl AutoConfiguration`. The list must equal the keys the code reads (the built-in
auto-configurations pin this with tests).

```rust
use oxbloom_boot::{AutoConfiguration, Configuration, PropertyMetadata};
use oxbloom_di::{component, ContainerBuilder};

#[component]
struct Mail;
impl Configuration for Mail {
    fn register(builder: ContainerBuilder) -> ContainerBuilder { builder }
}
impl AutoConfiguration for Mail {
    fn properties() -> &'static [PropertyMetadata] {
        static KEYS: [PropertyMetadata; 1] =
            [PropertyMetadata::new("mail.host", "String", None, "The SMTP host.")];
        &KEYS
    }
}
assert_eq!(Mail::properties()[0].key, "mail.host");
```

#### `OxbloomApplicationBuilder::property_metadata()`

`pub fn property_metadata(&self) -> Vec<PropertyMetadata>`. The keys this application reads: the boot
keys plus the keys of the registered auto-configurations that are not excluded, sorted by key. A key
declared more than once appears once, with its first declaration. It reads the builder only, so call
it after `auto_configurations(..)` and `exclude::<A>()` and before `run()`.

```rust
use oxbloom_boot::{OxbloomApplication, PropertyMetadata};

let list = OxbloomApplication::builder().property_metadata();
assert_eq!(list[0].key, "oxbloom.main.banner-mode");
assert!(PropertyMetadata::to_json(&list).starts_with("[{\"key\":"));
```

#### `auto_configurations()` (features `security` or `web`)

`pub fn auto_configurations() -> AutoConfigurations`. The list of the built-in auto-configurations
for the enabled features, in dependency order: `JwtDecoderAutoConfiguration`, `PasswordEncoderAutoConfiguration`,
`SessionAutoConfiguration` and `SecurityAutoConfiguration` with `security`, `SslAutoConfiguration` with `ssl`,
`ActuatorAutoConfiguration` with `actuator`, then
`RequestLogAutoConfiguration` and `WebServerAutoConfiguration` with `web`, and last
`TasksAutoConfiguration` and then `RetryAutoConfiguration` with `tasks`. Without
either feature the function does not exist. Remove one entry with `exclude::<A>()`.

```rust
use oxbloom_boot::auto_configurations;

// 4 with `security` only, 2 with `web` only, 6 with both.
assert!(auto_configurations().len() >= 1);
```

#### `JwtDecoderAutoConfiguration` (feature `security`)

Registers a `DefaultJwtDecoder`, provided as `dyn JwtDecoder`, when the `security.jwt` section exists
(`on_property("security.jwt")`, which for `Config` matches a table). The decoder comes from
`oxbloom_security::jwt_decoder_from_config`, with an optional `Arc<dyn JwksFetcher>` bean for a
`jwk-set-uri`. The bean has `on_missing_bean::<dyn JwtDecoder>()`, so a decoder of your own wins. A
present but invalid section fails startup in the `Container` phase with the `ConfigError` as the
typed cause, and a built-in analyzer for invalid `security.jwt` configuration explains it by key. Neither error text nor metadata contains the
secret. Keys described: `security.jwt.algorithms`, `audience`, `clock-skew-secs`, `issuer`,
`jwk-set-uri`, `jwks-ssl-bundle`, `public-key-pem` and `secret`. With the feature `ssl`,
`jwks-ssl-bundle` names an SSL bundle whose trust the JWKS client uses (built once at startup, no
reload). A bundle with a trust store replaces the platform roots; one without keeps the platform
verifier and only adds its client certificate. An unknown name is a `ConfigError::Invalid` on that key
listing the available bundle names, and the key together with a `JwksFetcher` bean is a
`ConfigError::Invalid` on the key as well.

```rust
use oxbloom_boot::{auto_configurations, Banner, JwtDecoderAutoConfiguration, OxbloomApplication};
use oxbloom_config::Config;

// Without a `security.jwt` section no decoder is built.
let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .banner(Banner::off())
    .output(std::io::sink())
    .auto_configurations(auto_configurations())
    .run()
    .await?;
assert_eq!(status.code(), 0);
let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<JwtDecoderAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert!(keys.contains(&"security.jwt.secret"));
```

#### `SecurityCustomizer` (feature `security`)

`pub trait SecurityCustomizer: Send + Sync { fn customize(&self, http: HttpSecurity) -> HttpSecurity; }`.
Contributes rules to the chain that `SecurityAutoConfiguration` builds. Register an implementation as
`dyn SecurityCustomizer` (`.provides(|c: Arc<X>| -> Arc<dyn SecurityCustomizer> { c })`). Customizers
run in container order (ascending `order`, then registration order), each on the result of the one
before. `HttpSecurity::authorize` can be called once in total, so one customizer owns the rules; a
second call is `DuplicateAuthorizeSection`.

```rust
use oxbloom_boot::SecurityCustomizer;
use oxbloom_security::HttpSecurity;

struct PublicHealth;
impl SecurityCustomizer for PublicHealth {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| a.get("/health").permit_all().any_request().authenticated())
    }
}
```

#### `SecurityAutoConfiguration` (feature `security`)

Registers the `SecurityFilterChain` bean, processed after `JwtDecoderAutoConfiguration`, with
`on_missing_bean::<SecurityFilterChain>()` so your own chain is the only one. It applies every
`dyn SecurityCustomizer`; with none it adds the single rule `any_request().authenticated()`, a closed
chain that answers 401. If a `dyn JwtDecoder` exists it then enables `oauth2_resource_server` with
it, and calls `build()`. A customizer list that never calls `authorize` fails with
`NoAuthorizeSection` (closed, not open). It runs after `PasswordEncoderAutoConfiguration`. With
`security.http-basic.enabled = true` it also calls `http_basic` with `security.http-basic.realm` and adds
every `dyn AuthenticationProvider` bean with `password_provider`; the default is `false`, and
configured users alone never open Basic. With Basic on and no provider bean the build fails with
`HttpBasicWithoutProvider`. With `security.form-login.enabled = true` it also calls `form_login` with the settings of
`FormLoginConfig::from_config`; that needs the table `security.session` (without it the startup
fails with an error that names `security.session`) and a password provider bean (without one the
build fails with `FormLoginWithoutProvider`, whose text names no key). `SecurityAutoConfiguration` also reads `CsrfProperties::from_config` with the form login switch: with form login on it calls `csrf` with the settings (the check is on unless `security.csrf.enabled` is `false`), and an invalid name or endpoint fails the startup with the key named, or with `SecurityBuildError::InvalidCsrfConfig` when the endpoint collides with a form login path. With form login off it never calls `csrf`, and `security.csrf.enabled = true` or any other `security.csrf` key fails the startup naming the key. Its metadata lists the ten `security.form-login.*` keys and the four `security.csrf.*` keys, and the two
`security.http-basic.*` keys it reads are in the metadata of `PasswordEncoderAutoConfiguration`.

`SecurityAutoConfiguration` also applies the slice 7a settings, before the `SecurityCustomizer` beans, so a customizer can override or compose them (form login and CSRF are applied after the customizers, so there the configuration wins). It calls `clock` with the `dyn Clock` bean when there is one, `headers` with `HeadersProperties::from_config`, `session_limit` with `SessionFilter::principal_sessions` when `security.session.max-sessions-per-user` is set, and `revalidate` with the `dyn UserDetailsService` bean. The default revalidation (5 minutes) applies only with form login from configuration and that bean, and stays off without them; an explicit `security.session.revalidate-interval` without the bean fails the startup naming the key, and `0s` is off and needs no bean. A limit with the cookie store, or with a store bean that has no index, fails the startup with an error that names `security.session.max-sessions-per-user` (a `SessionManagementAnalyzer` renders it). Its metadata lists nine more `security.headers.*` keys (23 in all) and the `SessionAutoConfiguration` metadata lists the two session keys (12 in all). The keys are applied only by this auto-configuration: with a user `SecurityFilterChain` bean they are ignored, as for form login and CSRF.

`SecurityAutoConfiguration` also reads `security.remember-me` with `RememberMeProperties::from_config`. It is off unless `enabled` is `true`, but its keys are checked either way: an unknown key or a bad value fails the startup even while it is off, and a secret on the command line is refused. When on, it calls `remember_me` with the secret, the old secrets, the lifetime, the field name, the cookie name, the `dyn UserDetailsService` bean (the one revalidation uses) and the same clock as form login and revalidation, before the `SecurityCustomizer` beans, so a customizer can change it with `http.remember_me(|r| ..)` (a customizer's `http.clock(..)` does not change the clock of remember-me). Enabled remember-me needs form login, a secret and the user service bean: without form login the startup fails with an error that names `security.form-login.enabled`, without a secret with one that names `security.remember-me.secret`, and without the bean with one that names `security.remember-me.enabled`. A cookie name equal to the session cookie name fails it with an error that names `security.remember-me.cookie-name`. No error, report or metadata entry shows a secret. The six `security.remember-me.*` keys are in the metadata of this auto-configuration.

With the feature `oauth2`, `SecurityAutoConfiguration` also reads `security.oauth2`. When `security.oauth2.client.registration` has an entry, it calls `oauth2_login` after the `SecurityCustomizer` beans with the registrations and providers of `OAuth2Settings::from_config`, the `dyn HttpClient` bean, `security.oauth2.base-url`, the optional `security.oauth2.failure-url` and the `dyn Clock` bean when there is one. A customizer that already called `oauth2_login` wins (`HttpSecurity::has_oauth2_login`), and then the base URL is not required. The keys are checked either way, and every failure names its key and never the value or the client secret: an unknown key under `security.oauth2`, a bad registration, a missing base URL (`ConfigError::Missing`), a base URL with a path, a query or a fragment, a `redirect-uri` that does not resolve to `{base-url}/login/oauth2/code/{id}` (reported on `security.oauth2.client.registration.<id>.redirect-uri`), and a failure URL that is not local or lies on an OAuth2 endpoint. Without form login the report says to set `security.form-login.enabled = true`, and without an `HttpClient` bean it says the bean is missing. Its metadata lists the 14 `security.oauth2.*` keys. See [section 4.15](#415-oauth2-login).

It also registers a `SessionRegistry` bean, unless the application has one, when there is a `SessionFilter` bean and `security.session.store` is `memory` or missing. The bean is lazy: it is built from `SessionFilter::principal_sessions` when first asked for. With a store that has no per-user index the request for it fails with `RegistryWithoutIndex` (a consumer of `Arc<SessionRegistry>` or `Option<Arc<SessionRegistry>>` fails the startup), and with the cookie store there is no bean.

```rust
use oxbloom_boot::{OxbloomApplication, SecurityAutoConfiguration};

let builder = OxbloomApplication::builder()
    .auto_configuration::<SecurityAutoConfiguration>()
    .exclude::<SecurityAutoConfiguration>();
assert!(builder.property_metadata().iter().all(|p| !p.key.starts_with("security.")));
```

#### `PasswordEncoderAutoConfiguration` (feature `security`)

Registers, in this order: a `DelegatingPasswordEncoder` from `security.password.*`, provided as
`dyn PasswordEncoder` (`on_missing_bean`); an `InMemoryUserDetailsService` with the users of
`security.users.<name>`, provided as `dyn UserDetailsService`, when the section `security.users` exists
and no `dyn UserDetailsService` is registered; and a `DaoAuthenticationProvider`, provided as
`dyn AuthenticationProvider`, when a `dyn UserDetailsService` and a `dyn PasswordEncoder` both exist. It has no
group conditions. The keys of `security.password` are not read when the application registered its own
encoder, and those of `security.users` are not read when it registered its own store. A bad value, an
unknown key or a plain-text password fails the build with a `ConfigError` that names the key, never the value.
The metadata lists ten keys: `security.http-basic.enabled` (`false`), `security.http-basic.realm` (`Oxbloom`),
`security.password.default-encoder` (`argon2`), `security.password.argon2.memory-kib` (`19456`),
`.iterations` (`2`), `.parallelism` (`1`) and `security.users.<name>.password`, `.roles`, `.enabled` and
`.locked`. The guide chapter is [Password login](guide/19-password-login.md).

```rust
use oxbloom_boot::{OxbloomApplication, PasswordEncoderAutoConfiguration};

let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<PasswordEncoderAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert!(keys.contains(&"security.http-basic.enabled"));
assert!(keys.contains(&"security.users.<name>.password"));
```

#### `SessionAutoConfiguration` (feature `security`)

Registers a `SessionFilter`, provided as `dyn Filter`, from the `security.session` keys. The group runs only when the table
`security.session` exists (`on_property`), so an application that never configures sessions gets no filter. It runs after
`PasswordEncoderAutoConfiguration` and before `SecurityAutoConfiguration`, and the bean has `on_missing_bean::<SessionFilter>()`.
`store = "memory"` uses the application's `dyn SessionStore` bean when there is one and otherwise an `InMemorySessionStore`;
`store = "cookie"` builds a `CookieSessionBackend` from `security.session.secret` and `old-secrets` and ignores a `dyn SessionStore`
bean. An optional `Arc<dyn Clock>` bean replaces the system clock in the filter, the store and the backend. A bad value, an unknown
key, a cookie store without a secret, a short secret or a secret on the command line fails the build with a `ConfigError` that names
the key, never the value. The metadata lists ten keys, the ones in the table of the guide chapter [Sessions](guide/20-sessions.md).

```rust
use oxbloom_boot::{OxbloomApplication, SessionAutoConfiguration};

let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<SessionAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert!(keys.contains(&"security.session.store"));
```

#### `WebServerAutoConfiguration` (feature `web`)

Registers a `WebServer`, provided as `dyn Service`, when at least one `dyn Controller` bean exists
(`on_bean::<dyn Controller>()` on the group). It builds an `App` from the `SecurityFilterChain` if
there is one, every `dyn Filter` bean (all added with `App::filter`; each filter's `order()` decides its place,
so the chain runs at `order::SECURITY` and plain filters after it) and the routes of every controller. It also
registers a `ProblemRenderer` as a `dyn ErrorRenderer` bean with `on_missing_bean`, so an application bean
replaces it, and the server uses whichever bean exists. The other `server.*` keys (below) are read when the
`WebServer` is built. A duplicate route fails startup in the `Container` phase with a `RouteError` cause; a
bad `server.port` or `server.address` fails with the key in a `ConfigError`. The bean has
`on_missing_bean::<WebServer>()`, so a `WebServer` of your own wins. It is processed after
`SecurityAutoConfiguration` when `security` is on. With no controller there is no server and no
`Service`, so `run()` returns after the runners. Keys described: `server.address` (default
`127.0.0.1`, an IP address), `server.port` (default `8080`) and the keys of the table in
[`WebServer::new`](#webservernewapp-config-and-webserverlocal_addr-feature-web).

```rust
use oxbloom_boot::{OxbloomApplication, WebServerAutoConfiguration};

let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<WebServerAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert!(keys.contains(&"server.port") && keys.contains(&"server.address"));
```

#### `RequestLogAutoConfiguration` (feature `web`)

Registers a `RequestLogFilter` as a `dyn Filter` bean when `logging.request.enabled` is `true`. With the key
missing or any other value the group is skipped and the conditions report says why. The filter declares the slot
`order::REQUEST_LOG`, so it runs outside the security chain and the body limit; requests that they reject (401,
403, 413) are in the request log. `WebServerAutoConfiguration` runs after this group, so the server collects the
filter. An application that registers its own `RequestLogFilter` bean replaces the
auto-configured one (`on_missing_bean`).

| Key | Default | Meaning |
|---|---|---|
| `logging.request.enabled` | `false` | Register the filter. |
| `logging.request.headers` | empty | The allowlist of request header names to print (entries are trimmed, empty ones dropped). |
| `logging.request.level` | `info` | `error`, `warn`, `info`, `debug` or `trace`. `off` or any other value fails the `Container` phase with a `ConfigError` that names the key and never the value. |

The three keys are listed by `RequestLogAutoConfiguration::properties()` and not in `oxbloom_logging::PROPERTIES`.
Events below the root level (or below `logging.level.oxbloom::web::request`) are not shown unless that level is
lowered to match. The redaction rules are those of `RequestLogFilter`.

```rust
use oxbloom_boot::{OxbloomApplication, RequestLogAutoConfiguration};

let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<RequestLogAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert!(keys.contains(&"logging.request.enabled") && keys.contains(&"logging.request.headers"));
```

#### `SslAutoConfiguration` (feature `ssl`)

Registers the `oxbloom_ssl::SslBundles` of the `ssl.*` keys when the configuration has any `ssl.bundle.*`
key (`on_property("ssl.bundle")`, which a `[ssl.bundle.pem.<name>]` or `[ssl.bundle.pkcs12.<name>]` section
satisfies), with `on_missing_bean::<SslBundles>()` so your own bundles win. An optional `dyn Clock` bean
replaces the system time in the validity checks; it is set before the container starts the reload task
through the `Lifecycle` hook of `SslBundles`, and the task stops at shutdown. A bean of your own gets no
lifecycle hook from boot, so you own its reload task. A bundle that cannot be loaded fails the `Container`
phase with `DiError::Creation` whose source is the `SslError`; the built-in SSL analyzer names the bundle
and the key to fix and never prints a private key, a password or a file path. `WebServerAutoConfiguration`
and `JwtDecoderAutoConfiguration` run after it and use the bean for `server.ssl.bundle` and
`security.jwt.jwks-ssl-bundle`. Its property metadata is `oxbloom_ssl::PROPERTIES`, so `exclude` drops the
`ssl.*` keys; `server.ssl.bundle` stays in `boot_properties()`. The keys are listed under
[`oxbloom-ssl`](#411-oxbloom-ssl).

```rust
use oxbloom_boot::{Banner, OxbloomApplication, SslAutoConfiguration, StartupPhase};
use oxbloom_ssl::SslError;

// A broken bundle fails the container with the `SslError` naming the key.
let err = OxbloomApplication::builder()
    .with_config(Config::from_str("[ssl.bundle.pem.web]\ntrust = \"no-such-file.pem\"")?)
    .banner(Banner::off())
    .output(std::io::sink())
    .auto_configuration::<SslAutoConfiguration>()
    .run()
    .await
    .unwrap_err();
assert_eq!(err.phase(), StartupPhase::Container);
assert!(matches!(err.find_cause::<SslError>(), Some(SslError::Io { .. })));
assert!(err.analysis().unwrap().to_string().contains("ssl.bundle.pem.web.trust"));
assert!(SslAutoConfiguration::properties().iter().any(|p| p.key == "ssl.reload-interval"));
```

#### `ActuatorAutoConfiguration` (feature `actuator`)

Registers the actuator endpoints of [`oxbloom-actuator`](#412-oxbloom-actuator) as a `dyn Controller`, with
the beans they read. The group runs only when the application registers a `dyn Controller` of its own (the
condition `on_bean`, like `WebServerAutoConfiguration`), so it never turns a run without a web application
into a server, and only when `management.endpoints.enabled` is missing or the text `true`
(`on_property_value_or_missing`); any other value skips the group silently, and the conditions report names
the reason. It registers the `ActuatorSettings` of the `management.*` keys (an unknown key or a bad value
fails the build with `DiError::Creation` whose source is the `ConfigError`); an `Availability` unless you
registered one; a `RunListener` that sets readiness to `AcceptingTraffic` at `ready` and to `RefusingTraffic`
at `stopping`, with the lowest `order`; a `HealthRunner` over every `dyn HealthIndicator` bean (an entry of
`management.health.readiness.include` that names no registered indicator fails the build with a
`ConfigError` on that key); an `Info` with your `BuildInfo` bean (all fields absent without one), the
detected `Platform` and every `dyn InfoContributor` bean; the `ActuatorController`, which also serves
`/loggers` with the feature `logging` and a `LoggingSystem` bean; with the feature `ssl` and an `SslBundles`
bean, the `ssl` indicator; and, with the feature `security` and `management.endpoints.health.public = true`,
a `SecurityCustomizer` that permits `GET <base-path>/health/**` and authenticates every other request. That
customizer adds its own `authorize` section, so an application customizer that also calls `authorize` fails
with `DuplicateAuthorizeSection`: open the health path in your own customizer instead. It runs after
`SslAutoConfiguration` and before `WebServerAutoConfiguration`. Its property metadata lists the nine
`management.*` keys, which are in the table under [`oxbloom-actuator`](#412-oxbloom-actuator).

```rust
use oxbloom_boot::{ActuatorAutoConfiguration, OxbloomApplication};

let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<ActuatorAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert!(keys.contains(&"management.endpoints.base-path") && keys.contains(&"management.health.timeout"));
```

#### `TasksAutoConfiguration` and `SchedulerService` (feature `tasks`)

Runs the tasks of every `dyn Scheduling` bean with the scheduler of [`oxbloom-tasks`](#413-oxbloom-tasks). The
group runs only when the application registers at least one `dyn Scheduling` bean (the condition `on_bean`) and
`tasks.scheduling.enabled` is missing or the text `true` (`on_property_value_or_missing`); any other value skips
the group silently, and the conditions report names the reason. It registers the `TasksSettings` of the
`tasks.scheduling.*` keys (an unknown key directly under `tasks` or `tasks.scheduling`, or a bad value, fails
the build with `DiError::Creation` whose source is the `ConfigError` naming the key); a `Scheduler`, unless the
application registered one, built with `Scheduler::try_new` from the tasks of every `dyn Scheduling` bean, the
settings and the `dyn Clock` bean (the system clock without one), where a bad cron expression, zone or
duration, a zero `fixed_rate` or `fixed_delay`, or two tasks with the same name fail the build with
`DiError::Creation` for `Scheduler` whose source is the `TaskError`; and a `SchedulerService` as a
`dyn Service`. The service has `phase()` `i32::MAX`, the phase of the web server, and its registration has the
DI order `i32::MAX`, so it starts after the server and stops before it. Boot fires `ready` after all services
started, so scheduled work can begin before readiness flips; use `initial_delay` for warm-up.
`SchedulerService::new(scheduler)` takes an `Arc<Scheduler>`, `scheduler()` returns it, `start` starts the
scheduler and `stop` stops it, and `name()` is `TaskScheduler`. The property metadata lists the five
`tasks.scheduling.*` keys, which are in the table under [`oxbloom-tasks`](#413-oxbloom-tasks).

```rust
use std::sync::Arc;
use oxbloom_boot::{OxbloomApplication, SchedulerService, Service, TasksAutoConfiguration};
use oxbloom_core::{Clock, SystemClock};
use oxbloom_tasks::{Scheduler, SchedulerConfig};

let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<TasksAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert!(keys.contains(&"tasks.scheduling.zone") && keys.contains(&"tasks.scheduling.max-concurrent"));

let clock: Arc<dyn Clock> = Arc::new(SystemClock);
let scheduler = Arc::new(Scheduler::try_new(Vec::new(), SchedulerConfig::default(), clock)?);
let service = SchedulerService::new(scheduler);
assert_eq!((service.phase(), service.name()), (i32::MAX, "TaskScheduler"));
```

#### `RetryAutoConfiguration` (feature `tasks`)

Makes the `tasks.retry.*` keys the process-wide defaults of every `#[retryable]` method of
[`oxbloom-tasks`](#413-oxbloom-tasks). It has no conditions, so it runs with or without a `dyn Scheduling` bean
and when `tasks.scheduling.enabled` is `false`. It registers the `RetryDefaults` read with
`RetryDefaults::from_config` and stores them with `set_defaults`. An unknown key directly under `tasks` or
`tasks.retry`, or a bad value, fails the build with `DiError::Creation` for `RetryDefaults` whose source is the
`ConfigError` naming the key, never the value, and the defaults stay as they were. The defaults are
process-wide: each context that runs this group overwrites them, a context without it keeps what the process
stored before, they stay set after the run ends, and `oxbloom_tasks::reset_defaults()` restores the built-in
values (`oxbloom-test` has no reset). The property metadata lists the six `tasks.retry.*` keys, so with
`TasksAutoConfiguration` there are eleven `tasks.*` keys in all; the retry keys are in the table under
[`oxbloom-tasks`](#413-oxbloom-tasks).

```rust
use oxbloom_boot::{OxbloomApplication, RetryAutoConfiguration};

let keys: Vec<_> = OxbloomApplication::builder()
    .auto_configuration::<RetryAutoConfiguration>()
    .property_metadata()
    .iter()
    .map(|p| p.key)
    .collect();
assert_eq!(keys.len(), 6);
assert!(keys.contains(&"tasks.retry.max-attempts") && keys.contains(&"tasks.retry.jitter"));
```

#### `OxbloomApplicationBuilder::listener(listener)`

Adds a listener that receives every event, including `starting` and `environment_prepared`,
which happen before the container exists. `RunListener` beans receive `context_ready` onward, after
the builder listeners.

```rust
struct Hello;
impl RunListener for Hello {
    fn starting(&self) {
        println!("starting");
    }
}

let builder = OxbloomApplication::builder().listener(Hello);
```

#### `OxbloomApplicationBuilder::shutdown_timeout(duration)`

Bounds the whole stop sequence. Default: 30 seconds. A `stop` still running at the deadline is
aborted and named in the output; `Duration::ZERO` gives no grace at all. The deadline cannot
interrupt a `stop` that blocks the thread, and it does not bound the `pre_destroy` hooks.

```rust
let builder = OxbloomApplication::builder().shutdown_timeout(Duration::from_secs(5));
```

#### `OxbloomApplicationBuilder::shutdown_handle()`

The `ShutdownHandle` of this run. Triggering it ends the run without a signal; a trigger before the
run waits is remembered.

```rust
let builder = OxbloomApplication::builder();
let handle = builder.shutdown_handle();
handle.trigger();
assert!(builder.shutdown_handle().is_triggered());
```

#### `OxbloomApplicationBuilder::output(writer)`

Where the banner and boot's diagnostics go. Default: stderr. Write errors are ignored. Use
`std::io::sink()` in tests.

```rust
let builder = OxbloomApplication::builder().output(std::io::sink());
```

#### `OxbloomApplicationBuilder::banner(banner)`

Sets the banner line. Default: `Banner::default()`. The property `oxbloom.main.banner-mode`
(`console` or `off`) overrides it; any other value fails the run in the `Config` phase.

```rust
let builder = OxbloomApplication::builder().banner(Banner::text("My service"));

let err = OxbloomApplication::builder()
    .with_config(Config::from_str("[oxbloom.main]\nbanner-mode = \"loud\"")?)
    .output(std::io::sink())
    .run()
    .await
    .unwrap_err();
assert_eq!(err.phase(), StartupPhase::Config);
```

#### `OxbloomApplicationBuilder::failure_analyzer(analyzer)`

Adds a `FailureAnalyzer` that is asked when startup fails, in any phase, also before a container
exists. Repeatable; builder analyzers are asked in call order, before `Arc<dyn FailureAnalyzer>`
beans and the built-in analyzers, and the first analysis wins. Use it for analyzers of your own
errors, or to replace a built-in text.

```rust
use oxbloom_di::DiError;

struct DuplicateBean;

impl FailureAnalyzer for DuplicateBean {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        match failure.find_cause::<DiError>()? {
            DiError::Duplicate { .. } => Some(FailureAnalysis::new(
                "A bean is registered twice.",
                "Remove one of the registrations.",
            )),
            _ => None,
        }
    }
}

// `Config` is registered by boot; registering it again fails.
let err = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .configure(|b| b.instance(Config::from_str("").unwrap()))
    .failure_analyzer(DuplicateBean)
    .output(std::io::sink())
    .run()
    .await
    .unwrap_err();
assert_eq!(err.analysis().unwrap().description(), "A bean is registered twice.");
```

An analyzer can also be a bean, which lets it use other beans; it is asked only when the container
was built. Eager bean creation and wiring failures happen inside `build()`, before any container
exists, so register analyzers meant for them on the builder with `failure_analyzer`. If creating the
analyzer beans fails, all bean analyzers are dropped silently; if it panics, boot writes the line
`failure analyzer beans panicked` and also goes on without them.

```rust
struct Always;
impl FailureAnalyzer for Always {
    fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
        Some(FailureAnalysis::with_no_action("A runner failed."))
    }
}
struct Fails;
#[async_trait]
impl ApplicationRunner for Fails {
    async fn run(&self, _: &ApplicationArguments) -> Result<(), RunnerError> {
        Err(RunnerError::new("boom"))
    }
}

let err = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .output(std::io::sink())
    .configure(|b| {
        b.instance(Always)
            .provides(|a: Arc<Always>| -> Arc<dyn FailureAnalyzer> { a })
            .instance(Fails)
            .provides(|r: Arc<Fails>| -> Arc<dyn ApplicationRunner> { r })
    })
    .run()
    .await
    .unwrap_err();
assert_eq!(err.analysis().unwrap().description(), "A runner failed.");
```

#### `OxbloomApplicationBuilder::failure_report(enabled)`

Sets whether the failure report is printed to the output target. `run()` prints it only with
`true`; `run_and_exit()` prints it unless this is `false`, in which case it writes the bare error
line instead. The report is printed once, after the `failed` listeners and cleanup, as the last
step before the error is returned. Turn it on for `run()` in tests that want to see the report, and
off for `run_and_exit()` when a log line is preferred.

```rust
use std::io::Write;
use std::sync::Mutex;

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

let out = Buf::default();
let err = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .configure(|b| b.instance(Config::from_str("").unwrap()))
    .banner(Banner::off())
    .output(out.clone())
    .failure_report(true)
    .run()
    .await
    .unwrap_err();
let text = String::from_utf8(out.0.lock().unwrap().clone())?;
assert!(text.starts_with("***************************\nAPPLICATION FAILED TO START\n"));
assert!(text.contains(&err.analysis().unwrap().to_string()));
```

#### `OxbloomApplicationBuilder::run()`

Runs the application and returns the `ExitStatus`, or a `StartupError` whose `source()` is the
`ConfigError`, `DiError`, `RunnerError` or `ServiceError`. On failure the analysis is attached
before the `failed` listeners run; the report is printed only with `failure_report(true)`. With at least one service it waits for
Ctrl-C, SIGTERM (Unix) or the `ShutdownHandle`. Whenever a service is registered, the runtime needs
the time and IO drivers (`#[tokio::main]` or `enable_all()`): boot installs its signal handlers just
before the first service starts.

```rust
let err = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .configure(|b| b.instance(Config::from_str("").unwrap()))
    .output(std::io::sink())
    .run()
    .await
    .unwrap_err();
let source = std::error::Error::source(&err).and_then(|s| s.downcast_ref::<oxbloom_di::DiError>());
assert!(matches!(source, Some(oxbloom_di::DiError::Duplicate { .. })));
```

#### `OxbloomApplicationBuilder::run_and_exit()`

Runs like `run()` and returns a `std::process::ExitCode`: the status code (already in 0..=255, see
`ExitCodeGenerator`), or 1 after a startup failure, for which it first writes the failure report to
the output target (or, with `failure_report(false)`, the error's message as one line; never both).
Use it as the tail of `main`.

```rust
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    OxbloomApplication::builder()
        .args(std::env::args().skip(1))
        .run_and_exit()
        .await
}
```

#### `RunListener`

Observes a run. Every method has an empty default: `starting()`, `environment_prepared(&Config)`,
`context_ready(&Container)`, `started(&Container)`, `ready(&Container)`, `stopping()`,
`stopped(&ExitStatus)`, `failed(&StartupError)`, and `name()` (default: the type name, used in
diagnostics). `stopping` fires on every successful run, also without services. Methods are
synchronous and must not panic: a panic is caught and reported as `listener {name} panicked`, but
the process-wide panic hook still prints it. Register beans with `provides(.. -> Arc<dyn RunListener>)`.

```rust
struct Report;

impl RunListener for Report {
    fn ready(&self, container: &Container) {
        let args = container.get::<ApplicationArguments>().expect("registered by boot");
        println!("ready with {} arguments", args.raw().len());
    }
    fn stopped(&self, status: &ExitStatus) {
        println!("exit code {}", status.code());
    }
    fn failed(&self, error: &StartupError) {
        println!("failed in the {} phase", error.phase());
    }
}

let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .output(std::io::sink())
    .configure(|b| b.instance(Report).provides(|r: Arc<Report>| -> Arc<dyn RunListener> { r }))
    .run()
    .await?;
assert_eq!(status.code(), 0);
assert!(Report.name().ends_with("Report"));
```

#### `ApplicationRunner`

Runs once with the parsed `ApplicationArguments`, after the container is built and before the
services start. Application runners run in DI order, before every `CommandLineRunner`. An error
fails the run in the `Runner` phase; a panic is not caught.

```rust
struct Migrate;

#[async_trait]
impl ApplicationRunner for Migrate {
    async fn run(&self, args: &ApplicationArguments) -> Result<(), RunnerError> {
        if args.option("dry-run").is_some() {
            return Err(RunnerError::new("dry run requested"));
        }
        Ok(())
    }
}

let err = OxbloomApplication::builder()
    .args(["--dry-run"])
    .with_config(Config::from_str("")?)
    .output(std::io::sink())
    .configure(|b| b.instance(Migrate).provides(|m: Arc<Migrate>| -> Arc<dyn ApplicationRunner> { m }))
    .run()
    .await
    .unwrap_err();
assert_eq!(err.phase(), StartupPhase::Runner);
```

#### `CommandLineRunner`

Runs once with the raw arguments, after every `ApplicationRunner`. Use it for command-line tools:
with no `Service`, `run()` returns right after the runners.

```rust
struct Count;

#[async_trait]
impl CommandLineRunner for Count {
    async fn run(&self, args: &[String]) -> Result<(), RunnerError> {
        println!("{} arguments", args.len());
        Ok(())
    }
}

let status = OxbloomApplication::builder()
    .args(["one", "two"])
    .with_config(Config::from_str("")?)
    .output(std::io::sink())
    .configure(|b| b.instance(Count).provides(|c: Arc<Count>| -> Arc<dyn CommandLineRunner> { c }))
    .run()
    .await?;
assert_eq!(status.code(), 0);
```

#### `Service`: `start`, `stop`, `phase`, `name`

A long-running part of the application. `start` returns once the service runs (spawn long-running
work); `stop` runs in its own task under the shared deadline, and a panic in it is reported while
the next service still stops. `phase()` (default 0) orders the start, lower first; stop is in
reverse. `name()` (default: the type name) is used in diagnostics. A start error stops the services
started so far and fails the run in the `Service` phase.

```rust
struct Worker;

#[async_trait]
impl Service for Worker {
    async fn start(&self) -> Result<(), ServiceError> {
        println!("worker started");
        Ok(())
    }
    async fn stop(&self) {
        println!("worker stopped");
    }
    fn phase(&self) -> i32 {
        -10 // start before the web server, stop after it
    }
}

let builder = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .output(std::io::sink())
    .configure(|b| b.instance(Worker).provides(|w: Arc<Worker>| -> Arc<dyn Service> { w }));
builder.shutdown_handle().trigger(); // remembered: start, then stop right away
assert_eq!(builder.run().await?.code(), 0);
```

#### `ExitCodeGenerator`

Contributes to the exit code. The run finishes with the highest `exit_code()` among the beans that
provide `dyn ExitCodeGenerator` (0 with none), evaluated after the services stopped. **Each code is
mapped before the maximum is taken: 0..=255 stays, a negative code or one above 255 becomes 255**,
so -1 next to 0 exits with 255. The beans are
collected before the services start, so a lazy generator is created then. A forced exit by a
signal keeps its fixed code (130 or 143).

```rust
struct Degraded;

impl ExitCodeGenerator for Degraded {
    fn exit_code(&self) -> i32 {
        3
    }
}

let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .output(std::io::sink())
    .configure(|b| b.instance(Degraded).provides(|d: Arc<Degraded>| -> Arc<dyn ExitCodeGenerator> { d }))
    .run()
    .await?;
assert_eq!(status.code(), 3);
```

#### `ShutdownHandle`: `new`, `trigger`, `is_triggered`, `wait`

Requests shutdown without a signal. Clones share one state; triggering is idempotent and
remembered. Boot registers the run's handle, so a runner or service can inject
`Arc<ShutdownHandle>` and end the application.

```rust
let handle = ShutdownHandle::new();
let waiter = handle.clone();
let task = tokio::spawn(async move { waiter.wait().await });
assert!(!handle.is_triggered());
handle.trigger();
handle.trigger(); // no further effect
task.await?;
assert!(handle.is_triggered());
```

#### `StartupError` and `StartupPhase`

`StartupError` is the error of a failed run. `Display` is `startup failed in the {phase} phase:
{source}`; `phase()` returns the `StartupPhase` (`Config`, `Container`, `Runner`, `Service`, displayed
in lowercase); `source()` returns the underlying error itself, so it can be downcast. Both types are
`#[non_exhaustive]`.

```rust
let e = StartupError::new(StartupPhase::Config, "missing file");
assert_eq!(e.phase(), StartupPhase::Config);
assert_eq!(e.to_string(), "startup failed in the config phase: missing file");
assert_eq!(StartupPhase::Container.to_string(), "container");
```

#### `StartupError::find_cause::<E>()`

Returns the first error of type `E` in the cause chain: the source itself, then its `source()`, and
so on, for at most 64 levels, so a cyclic chain cannot loop. This is how an analyzer reaches the
typed error behind a failure, for example a `ConfigError` kept by `try_bean` or your own error
returned with `DiError::creation_from`.

```rust
let e = StartupError::new(
    StartupPhase::Runner,
    RunnerError::new(std::io::Error::other("disk full")),
);
assert!(e.find_cause::<RunnerError>().is_some());
assert_eq!(e.find_cause::<std::io::Error>().unwrap().to_string(), "disk full");
assert!(e.find_cause::<std::fmt::Error>().is_none());
```

#### `StartupError::analysis()`

The `FailureAnalysis` that boot attached before the `failed` listeners ran, or `None` when no
analyzer recognised the failure or the error was not produced by a run. `Debug` of the error does
not print it.

```rust
let err = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .configure(|b| b.instance(Config::from_str("").unwrap()))
    .output(std::io::sink())
    .run()
    .await
    .unwrap_err();
// The built-in bean-wiring analyzer recognises the duplicate.
let analysis = err.analysis().expect("analyzed");
assert!(analysis.description().contains("is registered more than once"));
assert_eq!(analysis.action(), Some("Register the type only once."));
assert!(StartupError::new(StartupPhase::Config, "x").analysis().is_none());
```

#### `FailureAnalysis`: `new`, `with_no_action`, `description`, `action`

A diagnosis of a startup failure: what went wrong and, optionally, what to do about it. `Display`
renders the report block without a trailing line break. The text is printed as given, so it must
not contain the block markers (the line of 27 asterisks or `APPLICATION FAILED TO START`), nor
configuration values, argument values or secrets.

```rust
let analysis = FailureAnalysis::new(
    "The required configuration key `db.url` is not set.",
    "Set `db.url` in `application.toml`.",
);
assert_eq!(analysis.description(), "The required configuration key `db.url` is not set.");
assert_eq!(analysis.action(), Some("Set `db.url` in `application.toml`."));
assert_eq!(
    analysis.to_string(),
    "***************************\nAPPLICATION FAILED TO START\n***************************\n\n\
     Description:\n\nThe required configuration key `db.url` is not set.\n\n\
     Action:\n\nSet `db.url` in `application.toml`."
);

// Without an action the report has no Action section.
let down = FailureAnalysis::with_no_action("The database is down.");
assert_eq!(down.action(), None);
assert!(down.to_string().ends_with("Description:\n\nThe database is down."));
```

#### `FailureAnalyzer`: `analyze`, `name`

Turns a `StartupError` into a `FailureAnalysis` when it recognises the cause; return `None`
otherwise. Register it with `failure_analyzer(..)` or as an `Arc<dyn FailureAnalyzer>` bean. Calls
are panic-isolated: a panicking analyzer is skipped and reported as `analyzer {name} panicked`
(`name()` defaults to the type name); the default panic hook still prints the payload.

```rust
#[derive(Debug)]
struct QuotaExceeded;
impl std::fmt::Display for QuotaExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("quota exceeded")
    }
}
impl std::error::Error for QuotaExceeded {}

struct QuotaAnalyzer;

impl FailureAnalyzer for QuotaAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        failure.find_cause::<QuotaExceeded>()?;
        Some(FailureAnalysis::new("The API quota is used up.", "Raise the quota."))
    }
}

let error = StartupError::new(StartupPhase::Service, ServiceError::new(QuotaExceeded));
assert_eq!(QuotaAnalyzer.analyze(&error).unwrap().description(), "The API quota is used up.");
assert!(QuotaAnalyzer.analyze(&StartupError::new(StartupPhase::Config, "x")).is_none());
assert!(QuotaAnalyzer.name().ends_with("QuotaAnalyzer"));
```

#### `ServiceError` and `RunnerError`

The errors a `Service::start` and a runner return. `Display` forwards to the wrapped error and
`source()` returns the wrapped error itself, so you can downcast it to your own type.

```rust
let e = ServiceError::new(std::io::Error::other("port in use"));
assert_eq!(e.to_string(), "port in use");
assert!(std::error::Error::source(&e).and_then(|s| s.downcast_ref::<std::io::Error>()).is_some());

let r = RunnerError::new("bad input");
assert_eq!(r.to_string(), "bad input");
```

#### `ExitStatus`

The exit status of a finished run: `ExitStatus::new(code)` and `code()`.

```rust
let status = ExitStatus::new(2);
assert_eq!(status.code(), 2);
assert_eq!(status, ExitStatus::new(2));
```

#### `Banner`: `default`, `off`, `text`

The banner written before the container is built. `Banner::default()` is the
Oxbloom ASCII art with `:: Oxbloom ::` and `(v{version})` on the last line,
`Banner::off()` prints nothing, `Banner::text(..)` prints your own text.

```rust
assert_ne!(Banner::off(), Banner::default());
let builder = OxbloomApplication::builder().banner(Banner::off());
```

#### `WebServer::new(app, &config)` and `WebServer::local_addr()` (feature `web`)

A `Service` that serves an `oxbloom_web::App`. It reads `server.address` (default `127.0.0.1`) and
`server.port` (default `8080`, `0` picks a free port); an unusable value is a `ConfigError` that names
the key. `phase()` is `i32::MAX`: it starts last and stops first, draining open connections.
`local_addr()` is `None` before `start`. Clones share one server, so keep a clone to read the port.
The other `server.*` keys are read by `WebServer::new` and passed to the `App` when they are present. A setting
made in code wins when its key is absent: `WebServer::new(App::new().max_body_bytes(16), &config)` keeps the
16 bytes unless `config` has `server.max-body-bytes`. The defaults below are the `App`'s own. `strategy` and
`trusted-proxies` each replace only their own part of `App::forwarded`, so `trusted-proxies` alone keeps the
strategy of the `App`.

| Key | Default | Meaning |
|---|---|---|
| `server.shutdown-timeout` | `30s` | `stop` waits this long for open connections, then they are dropped and a `warn` event is logged on target `oxbloom::web`. `0` drops them at once. |
| `server.max-body-bytes` | `1MiB` | Request body limit (a size such as `64KiB`, or plain bytes); larger bodies get 413. `0` rejects every non-empty body. |
| `server.header-read-timeout` | `10s` | Time to read the request headers; must be above 0 and at most 1 day. |
| `server.forward-headers.strategy` | `none` | `none` or `framework` (see `ForwardedConfig`). |
| `server.forward-headers.trusted-proxies` | loopback only | List of IP addresses and CIDR networks; an empty list trusts nobody. |
| `server.error.include-message` | `never` | `never` or `always`: whether the framework's internal message (the panic message) becomes the `detail` of the 500 answer. An author's `detail` is always shown. It only configures the `ProblemRenderer` that the auto-configuration registers: `WebServer::new` validates it but sets no renderer (use `App::error_renderer`). |
| `server.cors.allowed-origins` | unset | List: `*` or `scheme://host[:port]` entries. Setting it adds a `CorsFilter` (CORS is off while it is absent). A present key adds a filter next to one set with `App::cors`, so both run. An explicitly empty list fails startup. |
| `server.cors.allowed-methods` | `GET`, `HEAD`, `POST` | Upper-case method names, or `*`. |
| `server.cors.allowed-headers` | none | Header names, or `*`. |
| `server.cors.exposed-headers` | none | Header names. |
| `server.cors.allow-credentials` | `false` | Not allowed with `*` in origins, methods or headers; the error names this key. |
| `server.cors.max-age` | `30m` | Preflight cache time. |
| `server.compression.enabled` | `false` | `true` adds a `CompressionFilter`; a startup error naming this key when `oxbloom-web` has no codec feature. |
| `server.compression.min-size`, `max-body-bytes` | `1KiB`, `8MiB` | Size window of compressed bodies. |
| `server.compression.mime-types` | the six default types | List; `family/*` matches a family. |
| `server.compression.level` | `default` | `fast`, `default` or `best`. |
| `server.static.enabled` | `false` | `true` adds a `StaticResources` over a `DiskResources`; a conflicting route fails startup naming `server.static.path`. |
| `server.static.directory` | `static` | Must be an existing directory when enabled. |
| `server.static.path`, `index` | `/`, `index.html` | Prefix and directory file; `index` must be a plain file name (1 to 255 bytes, not starting with `.`, without `/`, `\`, `%` or NUL) or startup fails. |
| `server.static.cache-max-age`, `max-file-bytes` | `0s`, `10MiB` | `Cache-Control` max age and file size cap. |
| `server.ssl.bundle` | unset | The SSL bundle to serve HTTPS with, read by `WebServer::with_ssl` (feature `ssl`). `WebServer::new` refuses a set key with a `ConfigError::Invalid` on it instead of serving plain HTTP. |

The sub-keys (`server.compression.min-size`, `mime-types`, `level`, `max-body-bytes`; `server.cors.*` other than
`allowed-origins`; `server.static.*` other than `enabled`) are validated but have no effect without their switch key
(`server.compression.enabled = true`, `server.cors.allowed-origins`, `server.static.enabled = true`), and they never
tune a filter or route set in code.

An invalid value fails with a `ConfigError` that names the key and never the value; in an application the
failure report is the generic container report, because the built-in analyzer explains only missing keys.
A non-table parent of a `server.*` key is ignored and the `App`'s value applies. With `framework`, follow the
deployment requirement of `ForwardedConfig`.

To read `server.*` from the configuration boot builds, create it in a `try_bean` that injects
`Arc<Config>`, as `examples/hello` does.

```rust
use oxbloom_web::{App, HttpMethod, Route};

let config = Config::from_str("[server]\nport = 0")?;
let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
let server = WebServer::new(app, &config)?;
assert_eq!(server.local_addr(), None);

server.start().await?;
assert_ne!(server.local_addr().expect("started").port(), 0);
server.stop().await;

// Inside an application: register it as a `dyn Service`.
let server = WebServer::new(App::new(), &config)?;
let builder = OxbloomApplication::builder()
    .with_config(config)
    .output(std::io::sink())
    .configure(move |b| b.instance(server).provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s }));
builder.shutdown_handle().trigger();
assert_eq!(builder.run().await?.code(), 0);

let bad = Config::from_str("[server]\naddress = \"localhost\"")?;
assert!(WebServer::new(App::new(), &bad).is_err());
```

#### `WebServer::with_ssl(app, &config, bundles)` (feature `ssl`)

`WebServer::new` that serves HTTPS with the identity of the bundle that `server.ssl.bundle` names in
`bundles: Option<&SslBundles>`. Without the key it is exactly `WebServer::new`, plain HTTP. With it the port
speaks only HTTPS: there is no HTTP listener and no redirect. The TLS configuration is the bundle's
`server_config()` (ALPN `http/1.1` only), which resolves the certificate at every handshake, so a reloaded
bundle reaches the next connection without a restart. `WebServerAutoConfiguration` calls it with the
`SslBundles` bean; call it yourself when you build the `WebServer` by hand. A `ConfigError::Invalid` on
`server.ssl.bundle` when `bundles` is `None`, has no bundle of that name, or the bundle has no certificate
and private key; the message lists the available bundle names and never a path or file content.
`WebServer::new` refuses a set `server.ssl.bundle` for the same reason: a silent fallback to plain HTTP
would break "HTTPS only when the key is set" ([ADR 0076](adr/0076-web-server-new-refuses-ssl-bundle.md)).
The `Web server listening` event does not show the scheme.

```rust
use oxbloom_ssl::SslBundles;
use oxbloom_web::App;

// `cert` and `key` hold PEM text; a file path works the same way.
let config = Config::from_str(&format!(
    "[server]\nport = 0\n[server.ssl]\nbundle = \"web\"\n\
     [ssl.bundle.pem.web]\ncertificate = '''\n{cert}'''\nprivate-key = '''\n{key}'''"
))?;
let bundles = SslBundles::from_config(&config)?;
let server = WebServer::with_ssl(App::new(), &config, Some(&bundles))?;

// An unknown name is an error on the key that lists the bundles.
let typo = Config::from_str("[server.ssl]\nbundle = \"wbe\"")?;
let err = WebServer::with_ssl(App::new(), &typo, Some(&bundles)).unwrap_err();
assert!(matches!(err, ConfigError::Invalid { ref key, .. } if key == "server.ssl.bundle"));
assert!(err.to_string().contains("available bundles: web"));
```

#### `BindError`: `address`, `kind` (feature `web`)

The error behind a failed `WebServer::start`: the source of the `ServiceError`, found with
`find_cause::<BindError>()`. `address()` is the configured address and port it tried, `kind()` the
`std::io::ErrorKind`. `Display` (`the web server cannot bind its listener: <OS error text>`) and
`Debug` show neither the address nor the port; the built-in port-in-use report is the one place that
shows the address.

```rust
use oxbloom_web::App;

// Hold a port so that the server cannot bind it.
let taken = std::net::TcpListener::bind("127.0.0.1:0")?;
let port = taken.local_addr()?.port();
let config = Config::from_str(&format!("[server]\nport = {port}"))?;
let server = WebServer::new(App::new(), &config)?;

let error = OxbloomApplication::builder()
    .with_config(config)
    .output(std::io::sink())
    .configure(move |b| b.instance(server).provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s }))
    .run()
    .await
    .unwrap_err();
let bind = error.find_cause::<BindError>().expect("a bind error");
assert_eq!(bind.address().port(), port);
assert_eq!(bind.kind(), std::io::ErrorKind::AddrInUse);
assert!(!bind.to_string().contains(&port.to_string()));
assert!(error.analysis().unwrap().description().contains(&format!("127.0.0.1:{port}")));
```

#### `async_trait`

The attribute for `impl` blocks of `ApplicationRunner`, `CommandLineRunner` and `Service`,
re-exported from the `async-trait` crate so you need no dependency of your own.

```rust
use oxbloom_boot::async_trait;

struct Noop;

#[async_trait]
impl CommandLineRunner for Noop {
    async fn run(&self, _args: &[String]) -> Result<(), RunnerError> {
        Ok(())
    }
}
```

### 4.9 `oxbloom-logging`

A `tracing` subscriber built from the `logging.*` configuration keys, and the handle that changes its
levels while the application runs. With `oxbloom-boot` and its `logging` feature the application
needs no setup code; the README section "Logging" describes the keys and the rules for targets and
groups in one place. Rules to remember:

- A library crate must never call `LoggingSystem::init`. Only boot or the application does, once per
  process. A second call, or any other installed global subscriber, gives `LoggingError::AlreadyInstalled`.
- Invalid values are a `ConfigError::Invalid` that names the key and never contains the value.
- A target name may contain only letters, digits, `_`, `-` and `:`, because it becomes part of a filter
  directive. A group name matches like a configuration key (case, `-` and `_` are ignored); `root` cannot be a group.
- An own target level beats its group, and a group named like a target beats that target.
- `log` facade events reach the same subscriber and obey the same levels.
- `Settings` (the parsed keys) is internal. `oxbloom_logging::__private`, `LoggingSystem::init_with_writer`
  and boot's `logging_writer` are `#[doc(hidden)]` test hooks, not API.

Imports:

```rust
use oxbloom_config::Config;
use oxbloom_logging::{ConsoleColor, ConsoleFormat, FileFormat, FileRotation, JsonField, Level,
                      LoggingError, LoggingSystem, ParseLevelError, PROPERTIES};
```

#### `Level`

`Off`, `Error`, `Warn`, `Info`, `Debug`, `Trace`, ordered from quiet to verbose. `FromStr` is
case-insensitive, `Display` is lowercase, and `Deserialize` reads the same names. In every serde format, a
wrong name is an error that lists the level names and never contains the value.

```rust
let level: Level = "DEBUG".parse()?;
assert_eq!(level, Level::Debug);
assert_eq!(level.to_string(), "debug");
assert!(Level::Debug > Level::Info); // more verbose compares greater
```

#### `ParseLevelError`

The text is not a level name. The error carries no text, so a mistyped secret is never echoed.

```rust
let err = "loud".parse::<Level>().unwrap_err();
assert_eq!(err, ParseLevelError);
assert_eq!(err.to_string(), "expected one of off, error, warn, info, debug, trace");
```

#### `ConsoleFormat` and `ConsoleColor`

`logging.console.format` is `Full` (the default), `Compact`, `Pretty` or `Json`; `logging.console.color` is
`Auto` (the default, colors only for a terminal), `Always` or `Never`. Both are read case-insensitively.

```rust
let config = Config::from_str(
    "[logging.level]\nroot = \"warn\"\n\"oxbloom_web::router\" = \"debug\"\n\
     [logging.console]\nformat = \"compact\"",
)?;
assert_eq!(config.get::<Level>("logging.level.root")?, Level::Warn);
assert_eq!(config.get::<ConsoleFormat>("logging.console.format")?, ConsoleFormat::Compact);
assert_eq!(ConsoleColor::default(), ConsoleColor::Auto);
```

#### `FileRotation`, `FileFormat` and `JsonField`

`FileRotation` is `Hourly`, `Daily` (the default) or `Never`, read from `logging.file.rotation`.
`FileFormat` is `Full` (the default), `Compact` or `Json`, read from `logging.file.format`; file output
is never colored. Both are read case-insensitively. `JsonField` is `Timestamp`, `Level`, `Target`,
`Message`, `Span` or `Thread`, the fields that `logging.json.fields.<field>` and
`logging.json.names.<field>` accept; `JsonField::ALL` lists them in declaration order. The order of the keys inside a JSON line is alphabetical and not guaranteed.

```rust
let config = Config::from_str(
    "[logging.file]\nenabled = true\nrotation = \"hourly\"\nformat = \"json\"",
)?;
assert_eq!(config.get::<FileRotation>("logging.file.rotation")?, FileRotation::Hourly);
assert_eq!(config.get::<FileFormat>("logging.file.format")?, FileFormat::Json);
assert_eq!(FileRotation::default(), FileRotation::Daily);
assert_eq!(JsonField::ALL.len(), 6);
```

The keys of the file and JSON output:

| Key | Default | Meaning |
|---|---|---|
| `logging.file.enabled` | `false` | Write a log file in addition to the console. |
| `logging.file.path` | `logs` | The directory; created when missing. |
| `logging.file.name` | `application.name`, else `oxbloom` | The file name prefix. |
| `logging.file.rotation` | `daily` | `hourly`, `daily` or `never`. |
| `logging.file.max-history` | unset (keep all) | The number of files to keep, at least 1. |
| `logging.file.format` | `full` | `full`, `compact` or `json`. |
| `logging.json.fields.<field>` | `true` | Include the field in JSON lines. |
| `logging.json.names.<field>` | the field name | Rename the field; names must be non-empty and differ between included fields. |

Rules to remember: pruning with `max-history` considers every file in the directory whose name starts
with the prefix (use a dedicated directory and a distinctive prefix; with `rotation = "never"` and
`max-history = 1` the existing file is deleted at start); the file writer is non-blocking and drops
events when its buffer is full; buffered lines are flushed when the last `LoggingSystem` clone drops,
which boot does when `run()` returns (events logged after that are not in the file); the directory and file
are created and the writer started before the subscriber is installed, so an `AlreadyInstalled` init leaves
an empty log file; `logging.file.enabled = true` is ignored when logging is disabled
(`oxbloom.logging.enabled = false` or `.logging(false)`); two systems in one process on the same directory
run two writers on one file that both prune it (only matters in tests); output layers are fixed at install time, so `set_level`, `reset` and `reload` change levels only. A JSON
line has the configured names of the enabled fields as keys, event fields other than `message` as extra
keys (a built-in key wins a name clash), `span` as the innermost span's fields plus `name` (the span name wins over a span field called `name`), an RFC 3339
UTC `timestamp` with millisecond precision and a lowercase `level`. The README section "Logging" has the
full text and TOML samples.

#### `PROPERTIES`

`pub const PROPERTIES: &[(&str, &str, Option<&str>, &str)]`: key, type name, default and description of
the thirteen keys the crate reads (`logging.level.root`, `logging.level.<target>`, `logging.group.<name>`,
`logging.console.format`, `logging.console.color`, the six `logging.file.*` keys and
`logging.json.fields.<field>` and `logging.json.names.<field>`). `oxbloom-boot` turns the list into
`PropertyMetadata`. The `logging.request.*` keys are not in this list; they belong to
`RequestLogAutoConfiguration` and are listed through its `properties()`.

```rust
assert_eq!(PROPERTIES.len(), 13);
assert!(PROPERTIES.iter().any(|p| p.0 == "logging.group.<name>"));
```

#### `LoggingError`

`#[non_exhaustive]`: `AlreadyInstalled`, `Config(ConfigError)`, `Filter(String)` and
`Io { path, source }`. `Io` means a log directory or file could not be created or opened; the message names
the path and `source()` is the `std::io::Error`. A `match` outside the crate needs a wildcard arm. Messages
name keys or paths, never configuration values.

```rust
use oxbloom_config::ConfigError;

let err = LoggingError::from(ConfigError::invalid("logging.level.app", "not a level"));
assert!(matches!(err, LoggingError::Config(ConfigError::Invalid { .. })));
assert_eq!(
    err.to_string(),
    "invalid configuration key `logging.level.app`: not a level"
);
```

```rust
assert_eq!(
    LoggingError::AlreadyInstalled.to_string(),
    "a global tracing subscriber or `log` logger is already installed"
);
assert_eq!(
    LoggingError::Filter("reload failed".into()).to_string(),
    "invalid logging filter: reload failed"
);
```

#### `LoggingSystem::init(&config)`

Installs the global subscriber, with console output on standard output, and returns the handle. The
handle is cheap to clone; clones share the same levels. Errors: `LoggingError::Config` for a bad
`logging.*` value, `LoggingError::AlreadyInstalled` when the process already has a global subscriber
or `log` logger.

```rust
let config = Config::from_str("[logging.console]\nformat = \"compact\"")?;
let logging = LoggingSystem::init(&config)?;
tracing::info!("started");

// The subscriber is per process.
assert!(matches!(LoggingSystem::init(&config), Err(LoggingError::AlreadyInstalled)));
```

#### `LoggingSystem::set_level(target, level)`

Changes one target at runtime. `root` is the default level. A group name sets the level of the group,
which applies to each member without a level of its own. A target with a character outside letters,
digits, `_`, `-` and `:` is a `LoggingError::Config` naming `logging.level.<target>`.

```rust
let logging = LoggingSystem::init(&Config::from_str("")?)?;
logging.set_level("root", "warn".parse()?)?;
logging.set_level("oxbloom_web::router", Level::Trace)?;
assert_eq!(logging.level("root"), Level::Warn);
assert_eq!(logging.level("oxbloom_web::router"), Level::Trace);
assert!(logging.set_level("not,a,target", Level::Trace).is_err());
```

#### `LoggingSystem::validate_target(target)`

Checks a target against the rule of `set_level` without changing anything and without an installed
subscriber: an empty name or one with a character outside letters, digits, `_`, `-` and `:` (a dotted name
such as `a.b` included) is a `LoggingError::Config` naming `logging.level.<target>`. The actuator uses it
to reject an invalid target on `GET {base}/loggers/{target}` as well as on `POST`.

```rust
assert!(LoggingSystem::validate_target("oxbloom_web::router").is_ok());
assert!(LoggingSystem::validate_target("a.b").is_err());
```

#### `LoggingSystem::reset(target)`

Returns the target to its level from the last `init` or `reload`. For a group it restores the group
level and the entry of each member.

```rust
let logging = LoggingSystem::init(&Config::from_str("[logging.level]\napp = \"warn\"")?)?;
logging.set_level("app", Level::Trace)?;
logging.reset("app")?;
assert_eq!(logging.level("app"), Level::Warn);
```

#### `LoggingSystem::level(target)`

The level in force. `root` gives the default, a group name gives the group's level (root when it has
none), and any other target gets the level of the longest configured target it starts with, as the
filter matches, falling back to root.

```rust
let config = Config::from_str("[logging.level]\nroot = \"error\"\noxbloom_web = \"debug\"")?;
let logging = LoggingSystem::init(&config)?;
assert_eq!(logging.level("root"), Level::Error);
assert_eq!(logging.level("oxbloom_web::router"), Level::Debug);
assert_eq!(logging.level("oxbloom_security"), Level::Error);
```

#### `LoggingSystem::reload(&config)`

Re-reads `logging.level.*` and `logging.group.*` from the `Config` you pass. Keys that are gone are
removed and earlier `set_level` changes are dropped; the result is the new baseline for `reset`.
`logging.console.*` is validated but not applied, because the output layer is fixed at install time.
On an invalid value the current levels stay. Nothing calls `reload` for you: there is no file watcher
and no SIGHUP handler.

```rust
let logging = LoggingSystem::init(&Config::from_str("[logging.level]\napp = \"debug\"")?)?;
logging.reload(&Config::from_str("[logging.level]\nroot = \"warn\"")?)?;
assert_eq!(logging.level("app"), Level::Warn);
```

#### `OxbloomApplicationBuilder::logging(enabled)` (feature `logging` of `oxbloom-boot`)

`pub fn logging(self, enabled: bool) -> Self`, default `true`. When enabled and the property
`oxbloom.logging.enabled` is not `false`, boot calls `LoggingSystem::init` right after the configuration
is loaded and before the banner and the container, and registers the `LoggingSystem` as a bean. An
invalid `logging.*` value fails the run in the `Config` phase with a report that names the key. When the
process already has a global subscriber, boot keeps it, registers no bean and emits one `warn` event with
the target `oxbloom::boot` through it; nothing goes to the output target. `oxbloom.logging.enabled` must be a
boolean even when `.logging(false)` is set.

```rust
let status = OxbloomApplication::builder()
    .with_config(Config::from_str("")?)
    .logging(false)
    .output(std::io::sink())
    .run()
    .await?;
assert_eq!(status.code(), 0);
```

#### Startup events and `application.name` (feature `logging` of `oxbloom-boot`)

Boot emits events at the target `oxbloom::boot`: `info` for `Starting {app}` (with the active profiles) and
`Started {app} in {secs}s`, for each service start and stop and for the address the web server listens
on; `warn` for configuration warnings; `debug` for each condition decision, emitted together after the
container is built; `error` for the failure summary. `{app}` is the value of `application.name`, or
`application`. A failure before the subscriber exists, such as an invalid `logging.*` value, logs no `error`
event. The banner and the conditions and failure reports stay on the output target. The built-in
analyzer for invalid `logging` configuration names the key and a fixed text, never the value; it is
not a public item.

### 4.10 `oxbloom-test`

Add the crate as a dev-dependency: `oxbloom-test = { path = "../oxbloom-test" }` (or its published version). It
depends on `oxbloom-boot`; its features `web`, `security` and `logging` forward to the same features of
`oxbloom-boot`. The runner keeps logging off in every build (it passes `--oxbloom.logging.enabled=false`),
so the `logging` feature of `oxbloom-test` is needed only for `ContextRunner::clock` to reach
`OxbloomApplicationBuilder::logging_clock`.

#### The `Clock` trait

`oxbloom_core::Clock` has one method, `fn now(&self) -> SystemTime`, and requires `Send + Sync + 'static`.
`SystemClock` is the default. Today the JWT decoder and the JSON log timestamp read it. With no clock set
the decoder behaves as before; with a clock set it turns off the `exp` and `nbf` checks of `jsonwebtoken`
and makes the checks itself against `clock.now()`, after the issuer and audience checks. An expired token
gives `AuthenticationError::Expired`; a token that is not yet valid or has a non-numeric `exp` or `nbf`
gives `AuthenticationError::InvalidToken`.

```rust
use std::sync::Arc;
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_security::{Algorithm, JwtDecoderBuilder};

let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec()))
    .clock(Arc::new(my_clock))
    .build()?;
```

From configuration, call `jwt_decoder_from_config_with_clock(&config, fetcher, Some(clock))`;
`jwt_decoder_from_config(&config, fetcher)` keeps its signature and uses no clock.

To make boot use a clock, register it as a bean of type `Arc<dyn Clock>`. `JwtDecoderAutoConfiguration`
(feature `security`) takes it when present. For the JSON log timestamp (feature `logging`), call
`OxbloomApplicationBuilder::logging_clock(clock)` on the builder; this is separate from the bean.

```rust
use std::sync::Arc;
use oxbloom_core::Clock;
use oxbloom_di::ContainerBuilder;

// Register a concrete clock and expose it as the trait object that boot looks for.
let builder = ContainerBuilder::new()
    .component::<MyClock>()
    .provides(|c: Arc<MyClock>| -> Arc<dyn Clock> { c });
```

#### `ContextRunner`

`ContextRunner` wraps `OxbloomApplicationBuilder` and runs the real boot lifecycle: it builds the
container, runs the auto-configurations, starts the services, waits for the ready event, calls your check,
and shuts down. It is hermetic: no environment variables are read (`env_from([])`), the banner and logging
are off, output is captured, and `server.port` is `0`. Configuration comes only from what you give it.

```rust
use oxbloom_test::ContextRunner;

#[tokio::test]
async fn the_greeting_is_configured() {
    ContextRunner::new()
        .config_str("[app]\ngreeting = \"Hi\"\n")
        .property("app.name", "demo")
        .configure(|b| b.instance(7u32))
        .run(|view| {
            assert_eq!(view.config().get::<String>("app.greeting").unwrap(), "Hi");
            assert_eq!(*view.get::<u32>().unwrap(), 7);
            assert!(!view.contains::<String>());
        })
        .await;
}
```

Builder methods: `property(key, value)` (a command-line override, so it wins over files),
`config_str(toml)` (the `application.toml` of the run), `config_dir(path)`, `profile(name)`,
`configure(|b| ..)` (container wiring), `auto_configuration::<A>()`, `auto_configurations(list)`,
`exclude::<A>()` and `clock(Arc<dyn Clock>)`. The `ContextView` handed to the check has `get::<T>()`,
`contains::<T>()`, `config()`, `conditions()` (the conditions report) and `output()` (the `Captured`
output).

`property` refuses secrets in the same way production does: `property("security.jwt.secret", ..)` fails
startup, because a secret on the command line is rejected. Put secrets in `config_str` or in a
`TempConfigDir`.

The `ready` check is synchronous: it runs inside boot's `ready` listener callback, so it cannot `.await`.
A panic or failed assertion in the check is caught there, the application is shut down, and the panic is
re-thrown with its original payload, so the test fails. The whole run (startup and shutdown) must end
within 10 seconds, or `run` panics. Shutdown of services is bounded by boot's 5 second timeout; a service
that panics or misses it fails `run` with `shutdown failed` and the captured output.

To expect a startup failure, use `run_failing`. Its `FailureView` has `error()` (the `StartupError`) and
`output()` (the failure report). It does not check the shutdown lines.

```rust
use oxbloom_boot::StartupPhase;
use oxbloom_test::ContextRunner;

#[tokio::test]
async fn a_missing_key_fails_startup() {
    ContextRunner::new()
        .configure(|b| b.component::<NeedsGreeting>())
        .run_failing(|failure| {
            assert_eq!(failure.error().phase(), StartupPhase::Container);
            assert!(failure.output().contains("APPLICATION FAILED TO START"));
        })
        .await;
}
```

The runner does not disable the signal handlers of boot, and the port default `server.port = 0` is passed as
the first command-line argument, so a `property("server.port", ..)` wins.

#### Temporary config directories

`TempConfigDir::new().file(name, text)` creates a directory with the named files and removes it on drop;
`path()` gives the directory for `ContextRunner::config_dir` or for `Config` loading. Dropping it after
the directory has already vanished is harmless.

```rust
use oxbloom_test::{ContextRunner, TempConfigDir};

let dir = TempConfigDir::new()
    .file("application.toml", "mode = \"base\"")
    .file("application-dev.toml", "mode = \"dev\"");
ContextRunner::new().config_dir(dir.path()).profile("dev");
```

#### Captured output

`Captured` is a cloneable in-memory `Write` target: `contents()`, `lines()`, `contains(needle)` and
`clear()`. `Log` is a shared list of strings for recording the order of events: `push(entry)` and
`entries()`. Both can be cloned into components and listeners.

#### Clocks

`FixedClock::at(time)` always returns the same time. `ManualClock::at(time)` moves only on
`advance(duration)`, and clones share one time, so a test can keep a handle while the application holds
another. A JWT example:

```rust
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use oxbloom_config::Secret;
use oxbloom_security::{Algorithm, AuthenticationError, JwtDecoder, JwtDecoderBuilder};
use oxbloom_test::ManualClock;

let clock = ManualClock::at(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000));
let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec()))
    .clock(Arc::new(clock.clone()))
    // No tolerance, so the token expires exactly at its `exp`.
    .clock_skew(Duration::ZERO)
    .build()?;
// A token with exp = 1_060 is accepted now ...
assert!(decoder.decode(&token).await.is_ok());
// ... and expired after the clock moves past it.
clock.advance(Duration::from_secs(120));
assert_eq!(decoder.decode(&token).await.unwrap_err(), AuthenticationError::Expired);
```

`ContextRunner::clock(Arc<dyn Clock>)` registers the clock as the `Arc<dyn Clock>` bean for the run (and,
with the feature `logging`, as the clock of the JSON log timestamp), so `view.get::<dyn Clock>()` and the
JWT auto-configuration see it.

#### Testing web applications

The feature `web` of `oxbloom-test` (off by default) adds `TestClient`, `TestResponse` and `TestApp`. The
feature `json` (it turns `web` on) adds `RequestBuilder::json` and `TestResponse::json`. The features
`security` and `logging` forward to boot, as above, and `TestApp::secured()` needs `security`. The feature
`tls` (it turns `web` and boot's `ssl` on) adds `TestClient::connect_tls` and `RunningApp::client_tls`.

##### `TestClient`

`TestClient` sends a request to an `oxbloom_web::App` in one of three modes and always answers with a
`TestResponse`, so one assertion style works in every mode. The request builder is the same too: `get`,
`post`, `put`, `patch`, `delete`, `head` and `options` take a path (with an optional `?query`), then
`.header(name, value)`, `.body(impl Into<Bytes>)`, `.json(&value)` (feature `json`) and `.send().await`.
`TestResponse` has `status()`, `header(name) -> Option<&str>` (`None` for a missing header or a value that
is not UTF-8), `bytes()`, `text()`, `json::<T>()` and `into_inner()` (the `oxbloom_web::Response`).

In memory, `TestClient::in_memory(app)` calls `App::handle_from` with the fake peer `127.0.0.1:50000`;
`.peer(addr)` changes it. A handler panic comes back as the 500 problem+json that `App` renders.

```rust
use oxbloom_test::TestClient;
use oxbloom_web::{App, HttpMethod, Route};

let app = App::new().route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
let client = TestClient::in_memory(app);
assert_eq!(client.get("/ping").send().await.text(), "pong");
assert_eq!(client.get("/nothing").send().await.status(), 404);
```

Served, `TestClient::serve(app).await` binds `127.0.0.1:0`, runs `App::serve` in a task and sends real
HTTP/1.1 to it. `addr()` gives the address (`None` in memory). `shutdown().await` stops the server and waits
for it; dropping the client signals the stop without waiting.

```rust
let client = TestClient::serve(app).await;
assert_eq!(client.get("/ping").send().await.text(), "pong");
client.shutdown().await;
```

Connected, `TestClient::connect(addr)` sends real HTTP/1.1 to a server that already runs and that the client
does not own. Nothing connects until the first request. `TestApp` uses it.

```rust
let client = TestClient::connect("127.0.0.1:8080".parse()?);
let res = client.post("/items").json(&serde_json::json!({ "name": "tea" })).send().await;
assert_eq!(res.status(), 201);
```

HTTPS, `TestClient::connect_tls(addr, ca_pem)` (feature `tls`) sends real HTTP/1.1 over TLS to a server
that already runs, and trusts only the CA certificates in `ca_pem`. Use it for a server that serves
`server.ssl.bundle` or `App::serve_tls`. Every connection sends and verifies the server name `localhost`,
whatever the IP of `addr` is, so the test certificate needs the DNS name `localhost`. The client offers
ALPN `http/1.1` and uses the `aws-lc-rs` provider. A server certificate the CA did not sign makes `send`
panic with the method, the path and the handshake error, for example
`GET /hello failed: tls handshake error: invalid peer certificate: UnknownIssuer`. A `ca_pem` without a
usable certificate panics at once with the fixed message
`connect_tls: the CA PEM holds no valid certificate`; the PEM text is never part of a message.

```rust
// `ca_pem` is the PEM text of the test CA, generated with `rcgen` when the test runs.
let client = TestClient::connect_tls(addr, &ca_pem);
assert_eq!(client.get("/ping").send().await.text(), "pong");
```

Each socket client keeps one HTTP/1.1 connection and reuses it. The socket transports add the headers
that a real client and server add: requests get `Host`, and `Content-Length` when they carry a body;
answers get `Date` and `Content-Length`. They refuse a path that does not start with `/`; the in-memory
mode adds none of that, because no HTTP message is built. Test a `413` for a body over the limit in
memory: over a socket the server answers `413` and closes the connection before it has read the whole
body, which can reset the connection while the client is still sending.

##### `handle_from` versus `handle`

`App::handle(req)` runs only the filters and the router. `App::handle_from(req, peer)` runs the steps of
the server path on an already buffered `Request`: the body limit (`App::max_body_bytes`, answered with
413), the resolution of the client address, scheme and host from `peer` and the forwarded headers of
trusted proxies (`App::forwarded`), the filters and the router. `handle` is unchanged. `peer: None` means
that the client address is unknown. `TestClient::in_memory` uses `handle_from`, so a request gives the same
answer in memory and over a socket.

```rust
let mut req = Request::new(HttpMethod::Post, "/echo");
req.body = "too long".into();
let res = app.handle_from(req, "192.0.2.1:40000".parse().ok()).await;
assert_eq!(res.status(), 413);
```

##### `TestApp`

`TestApp` starts a whole boot application with a real web server, through the same lifecycle as
`ContextRunner`, and has its configuration methods (`property`, `config_str`, `config_dir`, `profile`,
`configure`, `auto_configuration`, `auto_configurations`, `exclude`, `clock`) and its hermetic defaults:
no environment variables, no banner, logging off, captured output and `server.port = 0`. It also has
`listener(impl RunListener)` to observe lifecycle events. `TestApp` is real-server only; for an in-memory
call without DI, use `TestClient::in_memory`.

`async start()` returns a `RunningApp` once the `WebServer` is bound. It panics with the failure report and
the captured output when startup fails, and at once, not after 10 seconds, when the application has no
web server (no `dyn Controller` bean). `async start_failing()` is the mirror: it returns the
`StartupError` of a bad `server.*` value or a port clash and panics when startup succeeds.
`RunningApp` has `client()` (a connected `TestClient`), `addr()`, `config()` (the `Config` captured at
`context_ready`), `output()` and `async stop()`, which returns `Stopped` with `status()` and `output()`.
`stop` applies the C10a rules: a 10 second bound and the shutdown-failure guard. Dropping a `RunningApp`
triggers shutdown without waiting.

Three presets choose the auto-configurations; `.exclude::<A>()` still works after them.

- `TestApp::web()`: `RequestLogAutoConfiguration` and `WebServerAutoConfiguration`. Controllers and
  filters come from `configure`. There is no security chain and no JWT decoder.
- `TestApp::secured()` (feature `security`): the web slice plus `JwtDecoderAutoConfiguration` and
  `SecurityAutoConfiguration`.
- `TestApp::full()`: `oxbloom_boot::auto_configurations()` for every feature `oxbloom-test` was built with.

`TestApp::new()` registers nothing, like `ContextRunner::new()`, which is also the config slice.

```rust
use std::sync::Arc;
use oxbloom_test::TestApp;
use oxbloom_web::{Controller, HttpMethod, Route};

struct Ping;
impl Controller for Ping {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    }
}

let app = TestApp::web()
    .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    .start()
    .await;
assert_eq!(app.client().get("/ping").send().await.text(), "pong");
app.stop().await;
```

A secured application with a `ManualClock` and a bearer token. The `security.jwt` section goes in
`config_str`, because a secret is refused as a property. The token is made here with the `jsonwebtoken`
crate; the clock decides whether its `exp` has passed.

```rust
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use oxbloom_test::{ManualClock, TestApp};

const SECRET: &str = "an-example-secret-of-32-bytes!!!";
let now = UNIX_EPOCH + Duration::from_secs(1_900_000_000);
let clock = Arc::new(ManualClock::at(now));
let app = TestApp::secured()
    .config_str(format!(
        "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{SECRET}\"\nclock-skew-secs = 0\n"
    ))
    .clock(clock.clone())
    .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    .start()
    .await;

let claims = serde_json::json!({ "sub": "alice", "exp": 1_900_000_060_u64 });
let token = encode(
    &Header::new(Algorithm::HS256),
    &claims,
    &EncodingKey::from_secret(SECRET.as_bytes())?,
)?;
let bearer = format!("Bearer {token}");

assert_eq!(app.client().get("/ping").send().await.status(), 401);
let ok = app.client().get("/ping").header("authorization", bearer.as_str()).send().await;
assert_eq!(ok.status(), 200);
clock.advance(Duration::from_secs(120));
let expired = app.client().get("/ping").header("authorization", bearer.as_str()).send().await;
assert_eq!(expired.status(), 401);
app.stop().await;
```

An application that serves HTTPS answers only TLS on its port, so `client()` cannot reach it.
`client_tls(ca_pem)` (feature `tls`) returns a new, owned `TestClient::connect_tls` client to the same
address. The certificate and key below are generated with `rcgen` at test time; a self-signed
`localhost` certificate is its own CA.

```rust
use oxbloom_boot::SslAutoConfiguration;

let key = rcgen::KeyPair::generate()?;
let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])?.self_signed(&key)?;
let app = TestApp::web()
    .auto_configuration::<SslAutoConfiguration>()
    .config_str(format!(
        "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
         [server.ssl]\nbundle = \"web\"\n",
        cert.pem(),
        key.serialize_pem()
    ))
    .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    .start()
    .await;
let client = app.client_tls(&cert.pem());
assert_eq!(client.get("/ping").send().await.text(), "pong");
drop(client);
app.stop().await;
```

A bearer token, a header value, a body or a CA PEM is never part of a panic message from the client.

##### Bounds and limits

`send()`, `shutdown()`, `TestApp::start` and `RunningApp::stop` are each bounded at 10 seconds. A
connection error, a bad path or a timeout panics with the method, the path (without the query string) and
the cause. The bounds are Tokio timers, so two limits of the C10a runner apply:

- A call that blocks the thread of a current-thread runtime (a blocking handler, a `std` sleep) stops the
  timer from firing, so the bound cannot interrupt it. Use a multi-thread runtime
  (`#[tokio::test(flavor = "multi_thread")]`) for such a test.
- Under `#[tokio::test(start_paused = true)]` Tokio advances paused time on its own while the server waits
  on I/O, so a bound can fire early. Run these tests without paused time.

##### What `TestApp` binds and what it does not guarantee

- `TestClient::serve` and `TestApp` never read the process environment. They bind loopback: `serve` always
  binds `127.0.0.1`; `TestApp` binds it because boot's default `server.address` is loopback, and a user
  property or configuration file can override that. When the server listens on every interface
  (`server.address` is `0.0.0.0` or `::`), `client()` dials the loopback address of the same family, and
  `addr()` still returns the bound address.
- A `.listener(..)` given to the builder has seen `ready` when `start` returns. A `RunListener` that is a
  bean can still be inside its `ready` callback when `start` returns; only builder listeners are
  guaranteed to have seen `ready`.
- With several `WebServer` beans, the first one that is bound wins: `addr()` and `client()` belong to it.

##### What stays on raw sockets

Tests that need a malformed request, slow headers, a body that a real client refuses to send, or any other
control of the bytes keep their `TcpStream` and say why in a comment. `oxbloom-web`'s own tests, which
exercise the crate itself, do the same. An in-`src` unit test of a crate that `oxbloom-test` depends on
cannot use `oxbloom-test` (a dev-dependency cycle); only the integration tests under `tests/` can. The
examples keep their own start harness because `TestApp` cannot take an `OxbloomApplicationBuilder`; they use
`TestClient::connect` for their requests.

##### Features and dev-dependencies

A crate that gives its `oxbloom-test` dev-dependency the features `web`, `security` or `tls` turns the
same features of `oxbloom-boot` on for the crate's own test builds (`tls` turns on boot's `ssl`), because
Cargo unifies the features of one build. So `cargo test -p <crate> --no-default-features` is no longer
feature-off for the code it tests, and `cargo test --no-default-features --lib` does not avoid the
unification either.

Boot's own dev-dependency has `web`, `security` and `tls`, so every `cargo test -p oxbloom-boot` run builds
boot with `web`, `security` and `ssl`. No boot test run builds boot without `security` or without `ssl`.
Feature-off coverage of boot comes from two places:

- The compile-only builds `cargo build -p oxbloom-boot --no-default-features`, `--no-default-features
  --features logging`, `--no-default-features --features web`, `--no-default-features --features security`
  and `--no-default-features --features ssl`. They do not see dev-dependencies, so they compile each
  feature set, but they run no test.
- `cargo test -p oxbloom-test --features web`. It builds boot with `web` and without `security` and `ssl`,
  and runs `TestApp::web()`, `TestApp::full()` and the stop and drop tests against it. It is the only test
  run that executes boot without `ssl`.

The counts of `cargo test -p oxbloom-boot` show the effect. With `oxbloom-test` features `web` and
`security` (C10b) they moved from 298 to 420 (`--no-default-features`), 370 to 420 (`--features web`), 317
to 440 (`--features logging`) and 329 to 420 (`--features security`), with 440 for `--all-features`
unchanged. With `tls` (C7) they moved from 420 to 443 and from 440 to 463, and the new tests of C7 brought
them to 447 for `--no-default-features`, `--features web`, `--features security` and `--features ssl`, and
467 for `--features logging` and `--all-features`
([ADR 0075](adr/0075-boot-tests-enable-oxbloom-test-tls.md)).

#### `with_authentication` (feature `security`)

`with_authentication(authentication: Authentication, future) -> F::Output` is an `async fn` that runs `future` with `authentication` as the current caller. It is a thin wrapper over `SecurityContext::scope(SecurityContext::authenticated(..), ..)` and exists so a secured service can be tested without HTTP (see [Method security](guide/25-method-security.md)). Code that runs outside the future, or in a task started with a bare `tokio::spawn`, does not see the caller; use `oxbloom_security::spawn` for that. The feature `security` also enables the optional dependency on `oxbloom-security`.

```rust
use oxbloom_security::{Authentication, GrantedAuthority, SecurityContext};
use oxbloom_test::with_authentication;

# tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
let admin = Authentication::new("ada", [GrantedAuthority::new("ROLE_ADMIN")]);
let name = with_authentication(admin, async {
    SecurityContext::current().authentication().map(|a| a.principal().to_owned())
})
.await;
assert_eq!(name.as_deref(), Some("ada"));
# });
# Ok::<(), std::io::Error>(())
```

### 4.11 `oxbloom-ssl`

Named SSL bundles loaded from the `ssl.*` keys. A bundle has an identity (a certificate chain and its
private key), a trust store (CA certificates), or both, from PEM values or files under
`ssl.bundle.pem.<name>` or from a PKCS#12 file under `ssl.bundle.pkcs12.<name>`. A bundle turns into rustls
server and client configurations that use the `aws-lc-rs` provider. A polling task swaps a bundle in when
its files change, and certificates that are expired, not valid yet or expiring soon log warnings. In an
application, `SslAutoConfiguration` (feature `ssl` of `oxbloom-boot`) does all of this; use the crate
directly when you build TLS configurations yourself. The guide chapter is
[SSL bundles](guide/16-ssl-bundles.md#ssl-bundles). Rules to remember:

- A value that starts with `-----BEGIN` (after leading whitespace) is inline PEM; any other value is a file
  path, relative to the process's current directory.
- Errors, `Debug` output and log events name the bundle and the key to fix, never a private key, a
  password, a file path or file contents.
- Bundle names are unique across `pem` and `pkcs12` and match like configuration keys (case-insensitive,
  ignoring `-` and `_`). An unknown key under `ssl` or under a bundle is a configuration error that names the key
  and lists the valid keys.
- Validity problems only warn; they never stop the startup.
- The feature `di` implements `oxbloom_di::Lifecycle` for `SslBundles`.

| Key | Default | Meaning |
|---|---|---|
| `ssl.reload-interval` | `30s` | How often the files of every bundle are checked for changes; `0s` turns polling off (and with it the 24 hour repeat of validity warnings). |
| `ssl.expiry-warning` | `14d` | A certificate that expires within this time produces a warning. |
| `ssl.bundle.pem.<name>.certificate` | unset | The certificate chain of the identity, leaf first: a file path or inline PEM. Requires `private-key`. |
| `ssl.bundle.pem.<name>.private-key` | unset | `Secret`. The unencrypted private key (PKCS#8, PKCS#1 or SEC1): a file path or inline PEM. Requires `certificate`. |
| `ssl.bundle.pem.<name>.trust` | unset | The CA certificates the bundle trusts: a file path or inline PEM. |
| `ssl.bundle.pkcs12.<name>.path` | unset | The PKCS#12 file. Its key and certificate chain are the identity; every other certificate is the trust store. |
| `ssl.bundle.pkcs12.<name>.password` | empty | `Secret`. The password of the PKCS#12 file. |
| `server.ssl.bundle` | unset | Boot (feature `ssl`): the bundle the web server serves HTTPS with. |
| `security.jwt.jwks-ssl-bundle` | unset | Boot (features `ssl` and `security`): the bundle whose client configuration the JWKS client uses. |

Imports for the examples below:

```rust
use std::sync::Arc;
use oxbloom_config::{Config, ConfigError};
use oxbloom_ssl::{CertValidity, SslBundles, SslError};
// `cert` and `key` hold PEM text of a certificate and its key, `ca` the PEM of a CA certificate.
```

#### `SslBundles::from_config(&config)`

Reads `ssl.*` and loads every bundle, reading the files they name. Without any `ssl` key the set is empty.
The certificates are checked against the system time once, and each problem logs a warning. Errors:
`SslError::Config` for an invalid value, an unknown key, or `ConfigError::Missing` when `certificate` is set
without `private-key` (or the reverse); `DuplicateName`; `Io`, `Pem`, `Pkcs12`, `KeyMismatch` and
`EmptyBundle` when a bundle's material cannot be loaded.

```rust
let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca}'''"))?;
let bundles = SslBundles::from_config(&config)?;
assert_eq!(bundles.names(), ["client"]);

let half = Config::from_str("[ssl.bundle.pem.web]\ncertificate = \"web.crt\"")?;
assert!(matches!(
    SslBundles::from_config(&half),
    Err(SslError::Config(ConfigError::Missing { key })) if key == "ssl.bundle.pem.web.private-key"
));
```

A PKCS#12 bundle (`ssl.bundle.pkcs12.<name>`) is read the same way. A wrong password or a damaged file is
`SslError::Pkcs12`. The `p12-keystore` reader is used with its `pbes1` feature enabled for older files; only
files written by the reader's own writer are covered by tests.

```rust
let config = Config::from_str(
    "[ssl.bundle.pkcs12.p12]\npath = \"config/server.p12\"\npassword = \"${P12_PASSWORD}\"",
)?;
let p12 = SslBundles::from_config(&config)?.get("p12")?;
assert!(p12.has_identity());
```

#### `SslBundles::with_clock(clock)`

Uses `clock` (`oxbloom_core::Clock`) instead of the system time for the validity checks and checks every
certificate against it at once. Call it before `start_reload`; the task checks with this clock on every
tick. A warning for the same certificate and condition repeats only when the clock is 24 hours away from the
last one, and a changed condition (expiring, then expired) warns at once. Boot passes the `dyn Clock` bean.

```rust
let in_a_week = SystemTime::now() + Duration::from_secs(7 * 24 * 60 * 60);
let bundles = SslBundles::from_config(&config)?.with_clock(Arc::new(FixedClock(in_a_week)));
let task = bundles.start_reload(); // checks with `FixedClock` on every tick
task.stop();
```

#### `SslBundles::get(name)` and `SslBundles::names()`

`get` returns the bundle named `name`, matched like configuration keys, or `SslError::UnknownBundle` with the
sorted names. `names` lists every bundle name as written, sorted.

```rust
let bundles = SslBundles::from_config(&config)?;
assert_eq!(bundles.get("client")?.name(), "client");

let err = bundles.get("web").unwrap_err();
assert_eq!(err.to_string(), "unknown SSL bundle `web`; available bundles: client");
```

#### `SslBundles::reload_interval()` and `SslBundles::expiry_warning()`

The parsed `ssl.reload-interval` (default 30 seconds, zero means off) and `ssl.expiry-warning` (default
14 days).

```rust
let bundles = SslBundles::from_config(&Config::from_str("[ssl]\nreload-interval = \"5s\"")?)?;
assert_eq!(bundles.reload_interval(), Duration::from_secs(5));
assert_eq!(bundles.expiry_warning(), Duration::from_secs(14 * 24 * 60 * 60));
```

#### `SslBundles::reload()`

Rebuilds every bundle now from all its sources, inline PEM included, and returns the result per bundle in
name order. A bundle that loads is swapped in for the next handshake and checked against the clock; a bundle
that fails keeps its old material. Nothing calls it automatically. It does blocking file I/O, so in async
code run it on `tokio::task::spawn_blocking`. A file that keeps changing during three reads gives
`SslError::Io` with kind `Interrupted`. A reload never changes the parts a bundle has: new files without its
identity (a certificate-only PKCS#12 export) fail with `NoIdentity`, new files without its trust store with
`EmptyBundle`, and new files that add an identity or a trust store with `ShapeChanged`.

```rust
// `ca_path` names a PEM file with the CA certificate.
let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = \"{ca_path}\""))?;
let bundles = SslBundles::from_config(&config)?;
assert!(bundles.reload()[0].1.is_ok());

// After the file was broken: the error is reported and the bundle keeps its trust store.
assert!(matches!(bundles.reload()[0].1, Err(SslError::Pem { .. })));
assert!(bundles.get("client")?.has_trust());
```

#### `SslBundles::start_reload()` and `ReloadTask`

Starts the task that checks the size and modification time of every bundle file every `reload_interval`,
following symlinks, so a Kubernetes secret mount that swaps a symlink is seen. Inline PEM and the PKCS#12
password are not watched. A changed bundle is rebuilt whole and swapped in for the next handshake; a file
that changes during the read is tried again on the next tick. A failed reload keeps the old material and
logs one `warn` on target `oxbloom::ssl` with the bundle name and an error kind (`pem`, `key-mismatch`,
`no-identity`, `io (...)`, ...), and logs again only after the files change again. Each tick also runs the validity check.
With an interval of zero the task does nothing. Inside a Tokio runtime the task runs on it, outside one on
its own thread. `ReloadTask::stop` ends it (it may wait for a pass that is reading files), dropping it does
the same, and `is_running` says whether it polls. With the feature `di`, register the bundles with
`ContainerBuilder::lifecycle::<SslBundles>()` and the container starts and stops the task.

```rust
let config = Config::from_str(&format!(
    "[ssl]\nreload-interval = \"5s\"\n[ssl.bundle.pem.client]\ntrust = '''\n{ca}'''"
))?;
let bundles = SslBundles::from_config(&config)?;
let task = bundles.start_reload();
assert!(task.is_running());
task.stop();

let container = oxbloom_di::ContainerBuilder::new()
    .instance(SslBundles::from_config(&config)?)
    .lifecycle::<SslBundles>()
    .build()?;
container.shutdown(); // stops the reload task
```

Replace a certificate file and its key file together (a directory or symlink swap): replaced one after the
other, the task can see the first write alone and log one `key-mismatch` warning.

#### `SslBundle`: `name`, `has_identity`, `has_trust`, `validity`

One named bundle. Cloning is cheap and clones share the material. `validity()` lists a `CertValidity`
(`subject`, `not_before`, `not_after`) for every distinct certificate: the identity chain, leaf first, then
the trust certificates. The subject is the common name, else the first DNS name, else `(no subject)`. A later
health or info endpoint can read it. `Debug` shows only the name and which parts exist.

```rust
let web = SslBundles::from_config(&Config::from_str(&format!(
    "[ssl.bundle.pem.web]\ncertificate = '''\n{cert}'''\nprivate-key = '''\n{key}'''"
))?)?
.get("web")?;
assert!(web.has_identity() && !web.has_trust());
for cert in web.validity() {
    println!("{} is valid until {:?}", cert.subject, cert.not_after);
}
assert_eq!(format!("{web:?}"), "SslBundle { name: \"web\", identity: true, trust: false }");
```

#### `SslBundle::server_config()`

A `rustls::ServerConfig` that presents the bundle's identity, without client authentication, with ALPN
`http/1.1` only. The certificate is resolved at every handshake from the current material, so the
configuration stays valid for the life of the bundle and serves a reloaded certificate. Use it with
`App::serve_tls` through `tokio_rustls::TlsAcceptor::from`. `SslError::NoIdentity` for a trust-only bundle.

```rust
let server = bundles.get("web")?.server_config()?;
assert_eq!(server.alpn_protocols, [b"http/1.1".to_vec()]);
let acceptor = tokio_rustls::TlsAcceptor::from(server);

let err = bundles.get("client")?.server_config().unwrap_err();
assert!(matches!(err, SslError::NoIdentity { .. }));
```

#### `SslBundle::client_config()`

A `rustls::ClientConfig` that verifies servers against the bundle's trust store, or with the platform
verifier when the bundle has none, and presents the identity as a client certificate when the bundle has
one. The client certificate is resolved at every handshake; the trust store is fixed when the method is
called, so after a reload only configurations built afterwards see a new trust store. Every certificate of a
PKCS#12 file other than the leaf becomes a trust anchor. `SslError::Io` naming the `trust` key (`path` for
PKCS#12) when the bundle has no trust store and the platform verifier cannot be set up. Use it with
`ReqwestJwksFetcher::with_tls`.

```rust
let client = bundles.get("client")?.client_config()?;
assert!(!client.client_auth_cert_resolver.has_certs()); // trust only, no client certificate
```

#### `SslError`

The error of loading a bundle or building a configuration from it: `#[non_exhaustive]`, `Send + Sync +
'static`, with fixed texts. Variants: `Io { bundle, key, kind }` (names the key, not the path), `Pem {
bundle, key }` (empty, malformed or encrypted PEM, or an unsupported key type), `NoIdentity { bundle }`,
`KeyMismatch { bundle }`, `Pkcs12 { bundle }`, `EmptyBundle { bundle }`, `ShapeChanged { bundle }` (a
reload would add an identity or a trust store), `DuplicateName { name }`,
`UnknownBundle { name, available }` and `Config(ConfigError)` (shown unchanged; also `From<ConfigError>`).

```rust
let err = SslError::KeyMismatch { bundle: "web".into() };
assert_eq!(err.to_string(), "the private key of SSL bundle `web` does not match its certificate");

fn bundle_name(err: &SslError) -> Option<&str> {
    match err {
        SslError::NoIdentity { bundle } | SslError::KeyMismatch { bundle } => Some(bundle),
        _ => None, // the enum is non-exhaustive
    }
}
```

#### `PROPERTIES`

The `ssl.*` keys as `(key, type, default, description)` tuples, the same shape as
`oxbloom_logging::PROPERTIES`. `SslAutoConfiguration::properties()` turns them into `PropertyMetadata`.

```rust
use oxbloom_ssl::PROPERTIES;

let (key, _, default, _) = PROPERTIES[0];
assert_eq!((key, default), ("ssl.reload-interval", Some("30s")));
assert!(PROPERTIES.iter().any(|p| p.0 == "ssl.bundle.pem.<name>.private-key"));
```

### 4.12 `oxbloom-actuator`

Operational endpoints for a web application: the liveness and readiness state, health indicators, build and
platform information and runtime log levels. In an application, `ActuatorAutoConfiguration` (feature
`actuator` of `oxbloom-boot`) wires everything; use the crate directly to build the controller yourself. The
guide chapter is [Actuator](guide/17-actuator.md#actuator). The crate has the features `web` (the
`ActuatorController`), `logging` (the `/loggers` endpoint, implies `web`) and `ssl` (the
`SslHealthIndicator`); the other types are always available. Rules to remember:

- The endpoints are under the base path, `/actuator` by default. `/health*` answers 200 only for `UP` and
  503 otherwise; an endpoint that is switched off answers 404.
- `/health` shows the components only with `management.health.show-details = "always"`.
- Without the `security` feature of `oxbloom-boot` every actuator endpoint is open to anyone, the write
  endpoint `POST <base-path>/loggers/{target}` included; boot logs a `warn` event at `oxbloom::boot` when it
  registers the loggers routes in that case. With security, `management.endpoints.health.public = true`
  together with `show-details = "always"` gives the indicator details to anonymous callers, because this
  slice has no `when-authorized` value.
- Liveness never runs an indicator. Readiness runs only the indicators named in
  `management.health.readiness.include`.
- An indicator that exceeds `management.health.timeout` or panics is `DOWN` with an `error` detail, never a
  500. Indicators must not block the thread, because the timeout cannot interrupt them.
- Log targets use `::`; a dotted target is a 400 on `GET` and `POST`, and `configuredLevel: null` is a 400.
  `POST` needs `Content-Type: application/json`; any other type is a 415 that changes nothing.

| Key | Default | Meaning |
|---|---|---|
| `management.endpoints.enabled` | `true` | Only `true` (or a missing key) switches the group on; any other value skips it silently. |
| `management.endpoints.base-path` | `/actuator` | The path the endpoints are served under; `/`, empty segments and characters other than ASCII letters, digits, `-`, `_`, `.` and `~` are refused. |
| `management.endpoints.health.public` | `false` | With security, permits `GET <base-path>/health/**` without authentication. |
| `management.endpoint.health.enabled` | `true` | `false` removes the three health routes. |
| `management.endpoint.info.enabled` | `true` | `false` removes `/info`. |
| `management.endpoint.loggers.enabled` | `true` | `false` removes `/loggers`. |
| `management.health.show-details` | `never` | `never` or `always`. |
| `management.health.timeout` | `5s` | The time limit of each health indicator. |
| `management.health.readiness.include` | empty | The indicators that also decide readiness; each must be registered. |

Imports for the examples below:

```rust
use std::sync::Arc;
use std::time::Duration;
use async_trait::async_trait;
use oxbloom_actuator::{
    ActuatorController, ActuatorSettings, Availability, BuildInfo, Health, HealthIndicator, HealthRunner,
    Info, InfoContributor, Liveness, Platform, Readiness, Status,
};
use oxbloom_config::Config;
```

#### `Availability`, `Liveness` and `Readiness`

`Availability::new()` starts as `Liveness::Correct` and `Readiness::RefusingTraffic` and holds both in
atomics, so it is shared as `Arc<Availability>` without locks. `liveness()` and `readiness()` read the
states, `set_readiness(r)` changes readiness and `break_liveness()` marks the application broken, with no
way back. `ActuatorAutoConfiguration` registers one (unless you did) and a `RunListener` that sets
`AcceptingTraffic` at `ready` and `RefusingTraffic` at `stopping`.

```rust
let availability = Availability::new();
assert_eq!(availability.readiness(), Readiness::RefusingTraffic);
availability.set_readiness(Readiness::AcceptingTraffic);
availability.break_liveness();
assert_eq!(availability.liveness(), Liveness::Broken);
```

#### `Status`, `Health` and `HealthIndicator`

`Status` is `Up`, `Down`, `OutOfService` or `Unknown` (the enum is `#[non_exhaustive]`). `Status::worst`
returns the worst of several (order `Down`, `OutOfService`, `Unknown`, `Up`; `Up` for none), `as_str()`
gives `UP`, `DOWN`, `OUT_OF_SERVICE` or `UNKNOWN`, and `is_up()` tests for `Up`. `Health` has a `status` and
`details` (a `BTreeMap<String, serde_json::Value>`); build it with `Health::up()`, `Health::down()` or
`Health::new(status)` and add details with `detail(key, value)`. A `HealthIndicator` has
`fn name(&self) -> &str` and `async fn health(&self) -> Health` (declared with `async_trait`). Register it
as a `dyn HealthIndicator` bean.

```rust
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

assert_eq!(Status::worst([Status::Up, Status::OutOfService]), Status::OutOfService);
assert_eq!(Health::down().detail("error", "refused").details["error"], "refused");
```

#### `HealthRunner::new(indicators, timeout)` and `run(only)`

`run(None).await` runs every indicator at the same time, each in its own task and under the timeout, and
returns the worst status and each indicator's `Health` by name (`(Status, BTreeMap<String, Health>)`).
`run(Some(&names))` runs only the indicators with those names. An indicator that times out or panics is
`DOWN` with the `error` detail `timed out` or `panicked`. Two indicators with one name get the keys `name`,
`name#2`, `name#3` and so on, and all count towards the status.

```rust
# async fn demo() {
let runner = HealthRunner::new(vec![Arc::new(Database)], Duration::from_secs(1));
let (status, parts) = runner.run(None).await;
assert_eq!(status, Status::Up);
assert_eq!(parts["db"].status, Status::Up);
let (status, parts) = runner.run(Some(&["other".to_string()])).await;
assert_eq!((status, parts.len()), (Status::Up, 0));
# }
```

#### `BuildInfo`, `build_info!()` and `build::emit()`

The application's `build.rs` calls `oxbloom_actuator::build::emit()` (with `oxbloom-actuator` as a build
dependency). It prints the package name and version, the short git sha, the git branch and the build time (in
seconds since the Unix epoch), leaves the git facts out when `git` fails and never fails the build. The branch is
left out on a detached checkout. The script reruns when `build.rs`, the git `HEAD`, the current branch ref or
`packed-refs` changes (the git directory comes from `git rev-parse --absolute-git-dir`), so a new commit
refreshes the sha. The macro `build_info!()`
expands in the application's crate and reads those variables with `option_env!`, which a library function
could not do; it returns a `BuildInfo`. `BuildInfo` has the `Option<String>` fields `name`, `version`,
`git_sha`, `git_branch` and `build_time`, derives `Default`, and leaves absent fields out of the JSON.
Register it as a bean.

```rust
let info = BuildInfo::new(Some("app"), Some("1.2.3"), None, None, None);
assert_eq!(info.version.as_deref(), Some("1.2.3"));
assert_eq!(serde_json::to_value(&info).unwrap()["name"], "app");
let from_macro = oxbloom_actuator::build_info!(); // without a build script every fact is absent
assert!(from_macro.git_sha.is_none());
```

#### `Platform`, `Info` and `InfoContributor`

`Platform::detect()` is `Kubernetes` when `KUBERNETES_SERVICE_HOST` is set and `None` otherwise
(`detect_from(get)` takes the lookup as a closure and `as_str()` gives `kubernetes` or `none`; the enum is
`#[non_exhaustive]`). `Info` has the public fields `build`, `platform` and `contributors`, and `render()`
returns the `/info` JSON: `build`, `platform`, then every contributor's entries (a contributor can overwrite
`build` and `platform`). An `InfoContributor` has
`fn contribute(&self, out: &mut serde_json::Map<String, serde_json::Value>)`.

```rust
struct Owner;

impl InfoContributor for Owner {
    fn contribute(&self, out: &mut serde_json::Map<String, serde_json::Value>) {
        out.insert("owner".into(), "platform-team".into());
    }
}

let info = Info {
    build: BuildInfo::default(),
    platform: Platform::None,
    contributors: vec![Arc::new(Owner)],
};
assert_eq!(info.render()["owner"], "platform-team");
assert_eq!(info.render()["platform"], "none");
```

#### `ActuatorSettings`

`ActuatorSettings::from_config(&config)` reads the `management.*` keys, and `ActuatorSettings::default()`
is the table above. The public fields are `enabled`, `base_path` (normalized: a leading `/` and no trailing
`/`), `health_public`, `show_details`, `timeout`, `readiness_include`, `health_enabled`, `info_enabled` and
`loggers_enabled`. `enabled` is read by boot's auto-configuration; `ActuatorController` ignores it. An
unknown key under `management`, `management.endpoints`, `management.endpoint` or `management.health`, a
`base-path` of `/`, an empty one or one with an empty segment or a character outside ASCII letters, digits,
`-`, `_`, `.` and `~`, and a `show-details` other than `never` or
`always` are `ConfigError::Invalid`, which never shows a configured value.

```rust
let config = Config::from_str("[management.endpoints]\nbase-path = \"ops/\"")?;
let settings = ActuatorSettings::from_config(&config)?;
assert_eq!(settings.base_path, "/ops");
assert!(!settings.show_details);
let root = Config::from_str("[management.endpoints]\nbase-path = \"/\"")?;
assert!(ActuatorSettings::from_config(&root).is_err());
```

#### `ActuatorController` (feature `web`)

`ActuatorController::new(settings, availability, runner, info)` takes the settings, an `Arc<Availability>`,
an `Arc<HealthRunner>` and an `Arc<Info>`. It implements `oxbloom_web::Controller`, and `routes()` serves
`GET {base}/health`, `{base}/health/liveness`, `{base}/health/readiness` and `{base}/info`. With the feature
`logging`, `with_loggers(Arc<LoggingSystem>)` adds `GET {base}/loggers`, `GET {base}/loggers/{target}` and
`POST {base}/loggers/{target}` with the body `{"configuredLevel":"DEBUG"}` (204; 400 for a bad body, a body
over 1 KiB, an unknown level, an empty or dotted target or `null`; 415 when the `Content-Type` is not
`application/json` or a `+json` type). `GET {base}/loggers/{target}` answers 400 for an empty or dotted target
too.

```rust
use oxbloom_web::{App, Controller, HttpMethod, Request};

# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
let controller = Arc::new(ActuatorController::new(
    ActuatorSettings::default(),
    Arc::new(Availability::new()),
    Arc::new(HealthRunner::new(vec![], Duration::from_secs(1))),
    Arc::new(Info { build: BuildInfo::default(), platform: Platform::None, contributors: vec![] }),
));
let app = App::new().routes(controller.routes())?;
let res = app.handle(Request::new(HttpMethod::Get, "/actuator/health")).await;
assert_eq!(res.status(), 200);
assert_eq!(res.body(), r#"{"status":"UP"}"#);
# Ok(()) }
```

#### `SslHealthIndicator` (feature `ssl`)

`SslHealthIndicator::new(Arc<SslBundles>, Arc<dyn Clock>)` is the indicator named `ssl`. It is `Down` when a
certificate of any bundle has expired and `Up` otherwise. Its details list `bundles`, `expired` and
`warning` by bundle name only (`warning` covers a certificate that is not valid yet or expires within
`SslBundles::expiry_warning`). Boot registers it for you with the feature `ssl` and an `SslBundles` bean.

```rust
use oxbloom_core::SystemClock;
use oxbloom_actuator::SslHealthIndicator;
use oxbloom_ssl::SslBundles;

# async fn demo() {
let bundles = SslBundles::from_config(&Config::from_str("").unwrap()).unwrap();
let indicator = SslHealthIndicator::new(Arc::new(bundles), Arc::new(SystemClock));
assert_eq!(indicator.name(), "ssl");
assert_eq!(indicator.health().await.status, Status::Up);
# }
```

### 4.13 `oxbloom-tasks`

Scheduled tasks and retry: fixed-rate, fixed-delay and cron triggers, the `Scheduler` that runs them on tokio, the
macros `#[scheduling]` and `#[scheduled]`, and `RetryPolicy` with the macro `#[retryable]`. In an application, `TasksAutoConfiguration` (feature `tasks` of
`oxbloom-boot`) wires everything; use the crate directly to build a `Scheduler` yourself. The guide chapter is
[Task scheduling](guide/18-tasks.md#task-scheduling). The macros live in `oxbloom-macros` without a feature
flag and are re-exported by this crate. Rules to remember:

- A cron expression has six fields, seconds first. Days of the week follow Spring: `0` and `7` are Sunday, `1`
  is Monday, and the names `MON` to `SUN` work in any case. Names and numbers can be mixed, also in one range
  (`MON-3`), and a named range may end at Sunday (`SAT-SUN`). Cron evaluation ends at the year 2100.
- A cron trigger without a zone uses the zone of the scheduler (`tasks.scheduling.zone`, default UTC). A local
  time skipped by daylight saving time does not fire, and a repeated one fires once; sub-hourly expressions have
  a gap of about 75 minutes on a fall-back day.
- A task never runs concurrently with itself: a trigger that finds the previous run active is skipped, counted
  and logged at `warn`. An `Err` or a panic is logged at `error` and the schedule continues.
- The attribute literals and `parse_duration` accept a whole number with `ms`, `s`, `m`, `h` or `d`; the
  configuration binder accepts more forms, such as `1.5s`.
- A plain `fn` runs in `spawn_blocking` and cannot be aborted: `stop` returns at the deadline and leaves the
  thread to finish.
- Retry is described in [Retry](#retry-oxbloom-tasks) below. Events of the scheduler and of retry use the log target `oxbloom::tasks`.

| Key | Default | Meaning |
|---|---|---|
| `tasks.scheduling.enabled` | `true` | Read by boot; only `true` (or a missing key) switches the group on. |
| `tasks.scheduling.max-concurrent` | `8` | How many runs execute at the same time across all tasks; at least 1. |
| `tasks.scheduling.shutdown-timeout` | `10s` | How long `stop` waits for runs in flight before it aborts them. |
| `tasks.scheduling.zone` | `UTC` | The zone of cron tasks without a `zone` of their own. |
| `tasks.scheduling.initial-delay` | `0s` | The initial delay of tasks without an `initial_delay` of their own. |

The `tasks.retry.*` keys are in the table of the [retry subsection](#retry-oxbloom-tasks).

Imports for the examples below:

```rust
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};
use oxbloom_config::Config;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_tasks::{
    CronExpr, Outcome, Scheduler, SchedulerConfig, Scheduling, Task, TasksSettings, Trigger, Zone,
    parse_duration, scheduling,
};
```

#### `#[scheduling]` and `#[scheduled(..)]`

`#[scheduling]` goes on an inherent `impl` block and generates `impl ::oxbloom_tasks::Scheduling` with one
`Task` per `#[scheduled]` method, in declaration order. The generated code names the crate `oxbloom_tasks`, so
the dependency must not be renamed. The keys of `#[scheduled(..)]` are string literals: exactly one of
`cron = ".."`, `fixed_rate = ".."` and `fixed_delay = ".."`, and optionally `initial_delay = ".."`,
`zone = ".."` (with `cron` only) and `name = ".."` (default `Type::method`). A method takes only `&self` and
returns `()` or `Result<(), E>` where `E: Display`; an `async fn` is awaited and a plain `fn` runs in
`tokio::task::spawn_blocking`. Trait impls, two triggers or none, unknown or repeated keys, an empty `name`,
other arguments, generic methods, other return types and `#[scheduled]` outside the impl are compile errors.
A literal that does not parse gives `Task::invalid`, so `Scheduler::try_new` fails with a `TaskError` naming the
task and the key. Not supported: a `#[cfg]`-gated `#[scheduled]` method and `unsafe fn`. A method named
`clone`, `to_string` or `scheduled` can collide with methods of `Arc` or of a trait. The application
registers the bean with `.provides(.. -> Arc<dyn Scheduling>)`.

```rust
struct Reports;

#[scheduling]
impl Reports {
    #[scheduled(cron = "0 0 3 * * *", zone = "Europe/Amsterdam")]
    async fn nightly(&self) -> Result<(), std::io::Error> {
        Ok(())
    }

    #[scheduled(fixed_rate = "30s", initial_delay = "5s", name = "refresh")]
    fn refresh(&self) {}
}

let tasks = Arc::new(Reports).scheduled();
assert_eq!(tasks[0].name(), "Reports::nightly");
assert_eq!(tasks[1].name(), "refresh");
assert_eq!(tasks[1].delay(), Some(Duration::from_secs(5)));
```

#### `Scheduling`

`pub trait Scheduling: Send + Sync + 'static` with `fn scheduled(self: Arc<Self>) -> Vec<Task>`. The macro
implements it; implement it by hand to build tasks from data.

```rust
struct Heartbeat;

impl Scheduling for Heartbeat {
    fn scheduled(self: Arc<Self>) -> Vec<Task> {
        let trigger = Trigger::fixed_rate(Duration::from_secs(10)).expect("non-zero period");
        vec![Task::new("heartbeat", trigger, || Box::pin(async { Ok(()) }))]
    }
}

let source: Arc<dyn Scheduling> = Arc::new(Heartbeat);
assert_eq!(source.scheduled()[0].name(), "heartbeat");
```

#### `Trigger`

`Trigger` is `FixedRate(Duration)`, `FixedDelay(Duration)` or `Cron(CronExpr, Option<Zone>)`. The constructors
are `Trigger::fixed_rate(period: Duration)` and `Trigger::fixed_delay(delay: Duration)` (zero is a
`TriggerError`), `Trigger::cron(expression: &str, zone: Zone)` and
`Trigger::cron_in_default_zone(expression: &str)`, all returning `Result<Trigger, TriggerError>`. The second
cron form leaves the zone open for the scheduler to fill, so it is not equal under `PartialEq` to
`Trigger::cron(x, Zone::Utc)`. `describe(&self) -> String` gives text such as `fixed rate 5s` or
`cron 0 0 3 * * * (UTC)`. `next(&self, last_start: Option<SystemTime>, last_end: Option<SystemTime>, now: SystemTime) -> Option<SystemTime>`
is a pure function: a fixed rate is `last_start + period`, a fixed delay is `last_end + delay`, both are `now`
without history and never earlier than `now`; a cron trigger ignores the history and returns the first match
strictly after `now`. Missed instants are not caught up. A backward clock delays a fixed rate or fixed delay
by the jump here; the `Scheduler` clamps that wait to one period.

```rust
let rate = Trigger::fixed_rate(Duration::from_secs(10))?;
let start = UNIX_EPOCH + Duration::from_secs(1_000);
assert_eq!(rate.next(Some(start), None, start), Some(start + Duration::from_secs(10)));
assert!(Trigger::fixed_delay(Duration::ZERO).is_err());

let nightly = Trigger::cron("0 0 3 * * *", Zone::parse("Europe/Amsterdam")?)?;
assert_eq!(nightly.describe(), "cron 0 0 3 * * * (Europe/Amsterdam)");
assert_ne!(Trigger::cron_in_default_zone("0 0 3 * * *")?, Trigger::cron("0 0 3 * * *", Zone::Utc)?);
```

#### `CronExpr`, `Zone`, `TriggerError` and `parse_duration`

`CronExpr::parse(expression: &str) -> Result<CronExpr, TriggerError>` checks the six fields and translates the
numeric days of the week; `as_str()` returns the expression with single spaces, not the translation, and
`Display` writes the same. `Zone` is `Utc` (the default) or `Named(chrono_tz::Tz)`; `Zone::parse(name: &str)`
accepts `UTC` or an IANA name and its `Display` writes it back. `TriggerError` has a message that never repeats
the offending value. `parse_duration(text: &str) -> Result<Duration, TriggerError>` takes a whole number with
one of `ms`, `s`, `m`, `h` or `d`; a fraction, a sign, a missing unit or an unknown unit is an error.

```rust
assert!(CronExpr::parse("0 0 9 * * MON-FRI").is_ok());
assert!(CronExpr::parse("*/5 * * * *").is_err()); // five fields
assert_eq!(Zone::parse("Europe/Amsterdam")?.to_string(), "Europe/Amsterdam");
assert!(Zone::parse("Mars/Olympus_Mons").is_err());
assert_eq!(parse_duration("250ms")?, Duration::from_millis(250));
assert!(parse_duration("1.5s").is_err());
```

#### `Task`, `BoxFuture`, `RunFn` and `Outcome`

`Task::new(name: impl Into<String>, trigger: Trigger, run: F)` takes a closure
`Fn() -> BoxFuture<'static, Result<(), String>> + Send + Sync + 'static`; the error is the `Display` text of
the task's own error. `initial_delay(self, delay: Duration) -> Task` postpones the first run, `name()`,
`trigger() -> Option<&Trigger>` and `delay() -> Option<Duration>` read it back, and
`Task::invalid(name, key: &'static str, error: TriggerError)` makes a task that never runs and makes
`Scheduler::try_new` fail naming the key. `BoxFuture<'a, T>` is `Pin<Box<dyn Future<Output = T> + Send + 'a>>`
and `RunFn` is the boxed closure type. `Outcome` is `Ok`, `Failed`, `Panicked` or `Skipped`.

```rust
let task = Task::new("cleanup", Trigger::fixed_delay(Duration::from_secs(60))?, || {
    Box::pin(async { Err("disk full".to_owned()) })
})
.initial_delay(Duration::from_secs(5));
assert_eq!(task.trigger().map(Trigger::describe).as_deref(), Some("fixed delay 60s"));
assert_eq!(task.delay(), Some(Duration::from_secs(5)));
assert_ne!(Outcome::Skipped, Outcome::Ok);
```

#### `Scheduler`, `SchedulerConfig` and `TaskStatus`

`Scheduler::try_new(tasks: Vec<Task>, config: SchedulerConfig, clock: Arc<dyn Clock>) -> Result<Scheduler, TaskError>`
builds a scheduler; nothing runs until `start`. It fails for the first `Task::invalid` and for two tasks with
the same name. `SchedulerConfig` has the public fields `max_concurrent` (zero counts as one),
`shutdown_timeout`, `zone` and `initial_delay`, and its default is 8, ten seconds, UTC and zero.
`async fn start(&self)` spawns one loop per task on the current tokio runtime (it panics outside a runtime) and
does nothing when called twice or after `stop`. `async fn stop(&self)` stops the triggers, waits up to
`shutdown_timeout` for runs in flight, aborts the rest with a `warn` event per task and does nothing when
called again. `is_running(&self) -> bool` is true between `start` and `stop`. `tasks(&self) -> Vec<TaskStatus>`
is a snapshot in the order the tasks were given; `TaskStatus` has `name`, `trigger` (as `Trigger::describe`),
`last_start`, `last_end`, `last_outcome` and `skipped`. The scheduler reads time only from the `Clock`, sleeps
in chunks of at most one second and re-reads it, so a forward jump gives one run and never a burst.

```rust
let task = Task::new("ping", Trigger::fixed_rate(Duration::from_secs(60))?, || Box::pin(async { Ok(()) }));
let clock: Arc<dyn Clock> = Arc::new(SystemClock);
let scheduler = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock)?;
assert!(!scheduler.is_running());
assert_eq!(scheduler.tasks()[0].trigger, "fixed rate 60s");

# async fn demo(scheduler: Scheduler) {
scheduler.start().await;
assert!(scheduler.is_running());
scheduler.stop().await;
# }
```

#### `TaskError`

`TaskError` names the task (`task()`) and the key (`key()`: `cron`, `zone`, `fixed_rate`, `fixed_delay`,
`initial_delay` or `name`) and its `Display` reads
``scheduled task `Jobs::poll`, key `fixed_delay`: fixed delay must be greater than zero``. It never shows the value.

```rust
let error = Trigger::fixed_delay(Duration::ZERO).unwrap_err();
let task = Task::invalid("Jobs::poll", "fixed_delay", error);
let clock: Arc<dyn Clock> = Arc::new(SystemClock);
let err = Scheduler::try_new(vec![task], SchedulerConfig::default(), clock).unwrap_err();
assert_eq!((err.task(), err.key()), ("Jobs::poll", "fixed_delay"));
```

#### `TasksSettings`

`TasksSettings::from_config(&config)` reads the `tasks.scheduling.*` keys, and `TasksSettings::default()` is
the table above. The public fields are `enabled`, `max_concurrent`, `shutdown_timeout`, `zone` and
`initial_delay`; `scheduler_config()` returns the `SchedulerConfig`. `enabled` is read by boot's
auto-configuration only. An unknown key directly under `tasks` or `tasks.scheduling`, a wrong type or bad
duration, a `max-concurrent` of 0 and an unknown zone are a `ConfigError` that names the key and never shows
the value. The configuration's duration form is richer than `parse_duration`, for example `1.5s`.

```rust
let config = Config::from_str("[tasks.scheduling]\nmax-concurrent = 2\ninitial-delay = \"5s\"")?;
let settings = TasksSettings::from_config(&config)?;
assert_eq!(settings.max_concurrent, 2);
assert_eq!(settings.initial_delay, Duration::from_secs(5));
assert!(settings.enabled);
let typo = Config::from_str("[tasks]\nschedule = true")?;
assert!(TasksSettings::from_config(&typo).is_err());
```

### Retry (`oxbloom-tasks`)

`RetryPolicy` runs an async operation again when it returns `Err`, and `#[retryable(..)]` applies a policy to one
method. Both need no scheduler. The guide chapter is [Retry](guide/18-tasks.md#retry). Rules to remember:

- `RetryPolicy::run` calls the operation up to `max_attempts` times, counting the first call. Before retry *n*
  (from 1) it sleeps `delay(n, ..)` with `tokio::time::sleep`. When the attempts run out the last error is
  returned. A panic is never retried and is not caught. Dropping the future stops the retries.
- Exponential backoff waits `initial * multiplier^(n-1)`, fixed backoff `initial`, both capped at `max_delay`
  and then spread by the jitter. With the built-in policy the delays before jitter are 100, 200, 400 and
  800 ms.
- The built-in default jitter is `full` (a random time from zero to the delay); Spring's default is none.
- `#[retryable]` needs an `async fn` that returns `Result<T, E>` with `E: Display` and takes `&self` or no
  receiver. `&mut self`, `self` and `mut` arguments are compile errors. The body runs again for every attempt,
  so it may borrow its arguments but not consume them; clone what it must own. A free `async fn` is accepted.
- A key that the attribute omits is read from `defaults()` at call time; a key that it writes is fixed at
  compile time. The defaults are process-wide state (a `static RwLock`).
- With `max_attempts = 1` the first failure logs the `error` event; an error that `retry_if` rejects logs
  nothing.
- Events (target `oxbloom::tasks`): a `warn` event `attempt failed, retrying` per retry with `method`,
  `attempt`, `delay_ms` and `error`, and one `error` event `retry attempts exhausted` with `method`,
  `attempts` and `error`.

| Key | Default | Meaning |
|---|---|---|
| `tasks.retry.max-attempts` | `3` | At least 1. |
| `tasks.retry.backoff` | `exponential` | `fixed` or `exponential`. |
| `tasks.retry.initial` | `100ms` | The delay before the first retry; not above `max-delay`. |
| `tasks.retry.multiplier` | `2.0` | Finite and at least 1.0. |
| `tasks.retry.max-delay` | `30s` | The cap on every delay. |
| `tasks.retry.jitter` | `full` | `none`, `full` or `equal`. |

Imports for the examples below:

```rust
use std::time::Duration;
use oxbloom_config::Config;
use oxbloom_tasks::{
    Backoff, Jitter, RetryDefaults, RetryError, RetryPolicy, defaults, reset_defaults, retryable, set_defaults,
};
```

#### `#[retryable(..)]`

The keys are `max_attempts = 3` (an integer, at least 1), `backoff = "exponential"` or `"fixed"`,
`initial = ".."` and `max_delay = ".."` (a whole number with `ms`, `s`, `m`, `h` or `d`; `initial` must not
exceed `max_delay`), `multiplier = 2.0` (at least 1, finite), `jitter = "full"`, `"equal"` or `"none"`, and
`retry_if = "path::to::fn"`, a `fn(&E) -> bool` in a string. All are optional. The macro keeps its own copy of the
duration grammar, because it cannot call `parse_duration`; a test keeps the two equal. Compile errors, each
naming the method: a plain `fn`, a return type that is not a `Result`, a receiver other than `&self`, a `mut`
argument, generic parameters, `max_attempts = 0`, an unknown `backoff` or `jitter`, a bad duration, `initial`
above `max_delay`, a `multiplier` below 1, a `retry_if` that is not a path in a string, and an unknown or
repeated key. The attribute combines with `#[scheduled]` on a method of a `#[scheduling]` impl, in either order:
one scheduled run then includes all its attempts and counts as the same run for the overlap rule. The generated
code names the crate `oxbloom_tasks`, so the dependency must not be renamed.

```rust
#[derive(Debug, PartialEq)]
enum PaymentError {
    Unavailable,
    Declined,
}

impl std::fmt::Display for PaymentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PaymentError::Unavailable => "payment service unavailable",
            PaymentError::Declined => "card declined",
        })
    }
}

fn is_transient(error: &PaymentError) -> bool {
    *error == PaymentError::Unavailable
}

struct Payments;

impl Payments {
    #[retryable(max_attempts = 4, initial = "10ms", jitter = "none", retry_if = "is_transient")]
    async fn charge(&self, id: u64) -> Result<u64, PaymentError> {
        if id == 0 { Err(PaymentError::Declined) } else { Ok(id) }
    }
}

assert_eq!(Payments.charge(7).await, Ok(7));
assert_eq!(Payments.charge(0).await, Err(PaymentError::Declined));
```

#### `RetryPolicy`

A struct with the public fields `max_attempts: u32`, `backoff: Backoff`, `initial: Duration`,
`multiplier: f64`, `max_delay: Duration` and `jitter: Jitter`. `RetryPolicy::default()` holds the built-in values
(3, exponential, 100 ms, 2.0, 30 s, full), not the process-wide defaults; use `defaults().policy()` for those.
`delay(retry_number: u32, unit_random: f64) -> Duration` is pure: a `retry_number` of 0 counts as 1, the
backoff is computed in floating point and capped before it becomes a `Duration`, full jitter multiplies by
`unit_random`, equal jitter takes half plus `unit_random` times the other half, and `unit_random` outside `[0, 1]`
is clamped and NaN counts as zero.
`async fn run<T, E, Fut, Op, If>(&self, label: &str, op: Op, retry_if: If) -> Result<T, E>` takes
`Op: FnMut() -> Fut`, `E: Display` and `If: Fn(&E) -> bool`; it returns the last error, or the first error that
`retry_if` rejects. `label` is the `method` field of the events. `run_with_random` takes the random source as
a fourth argument and is `#[doc(hidden)]`, for tests.

```rust
let policy = RetryPolicy { max_attempts: 4, jitter: Jitter::None, ..RetryPolicy::default() };
assert_eq!(policy.delay(1, 0.5), Duration::from_millis(100));
assert_eq!(policy.delay(4, 0.5), Duration::from_millis(800));
let full = RetryPolicy { jitter: Jitter::Full, ..policy.clone() };
assert_eq!(full.delay(2, 0.25), Duration::from_millis(50));

let calls = std::cell::Cell::new(0);
let result = policy
    .run(
        "fetch",
        || {
            calls.set(calls.get() + 1);
            let n = calls.get();
            async move { if n < 3 { Err("unavailable") } else { Ok(n) } }
        },
        |_| true,
    )
    .await;
assert_eq!(result, Ok(3));
```

#### `Backoff`, `Jitter` and `RetryError`

`Backoff` is `Fixed` or `Exponential`; `Jitter` is `None`, `Full` or `Equal`. Both are `Copy`, implement
`Display` (the lowercase name) and `FromStr` (lowercase, surrounding whitespace ignored). A parse failure is a
`RetryError`, whose message lists the accepted names and never repeats the input.

```rust
assert_eq!("fixed".parse::<Backoff>()?, Backoff::Fixed);
assert_eq!(Jitter::Equal.to_string(), "equal");
let error: RetryError = "linear".parse::<Backoff>().unwrap_err();
assert_eq!(error.to_string(), "unknown backoff, expected fixed or exponential");
```

#### `RetryDefaults`, `defaults`, `set_defaults` and `reset_defaults`

`RetryDefaults` has the fields of `RetryPolicy`, `Default` is the built-in values, and `policy()` returns a
`RetryPolicy`. `RetryDefaults::from_config(&config)` reads the `tasks.retry.*` keys: an unknown key directly
under `tasks` or `tasks.retry`, a wrong type or bad duration, a `max-attempts` of 0, a `multiplier` below 1 or
not finite, an `initial` greater than `max-delay` and an unknown `backoff` or `jitter` name are a `ConfigError`
that names the key and never shows the value. `set_defaults(value)` stores the process-wide defaults,
`defaults()` returns the last stored value (the built-in values at first), and `reset_defaults()` restores the
built-in values. In an application, boot's `RetryAutoConfiguration` calls `set_defaults`.

```rust
let config = Config::from_str("[tasks.retry]\nmax-attempts = 5\nbackoff = \"fixed\"\ninitial = \"250ms\"")?;
let from_config = RetryDefaults::from_config(&config)?;
assert_eq!((from_config.max_attempts, from_config.backoff), (5, Backoff::Fixed));
assert_eq!(from_config.initial, Duration::from_millis(250));

let zero = Config::from_str("[tasks.retry]\nmax-attempts = 0")?;
assert!(RetryDefaults::from_config(&zero).unwrap_err().to_string().contains("tasks.retry.max-attempts"));

set_defaults(from_config);
assert_eq!(defaults().max_attempts, 5);
reset_defaults();
assert_eq!(defaults(), RetryDefaults::default());
```

Not covered: a circuit breaker, rate limiting, retry of a plain `fn` or on a result value, per-error backoff,
named policies in configuration, metrics, an actuator view and a `recover` fallback method.

### 4.14 `oxbloom-http-client`

Outbound HTTP: the `HttpClient` trait, `Request` and `Response`, a `reqwest` client with safe defaults, retry for idempotent requests, interceptors, the `http.client.*` settings, the `HttpClientAutoConfiguration` bean in `oxbloom-boot` (feature `http-client`) and `MockHttpClient` in `oxbloom-test` (feature `http-client`). The guide chapter is [HTTP client](guide/26-http-client.md#http-client) and the decisions are in [ADR 0093](adr/0093-http-client.md). Rules to remember:

- A status that is not 2xx is a normal `Response`. `Response::error_for_status` turns it into `HttpClientError::BadStatus`.
- The text of every `HttpClientError` is fixed. No error and no `Debug` output holds a URL, a header value or a body.
- A request that `reqwest` cannot build (for example a bad URL, or a scheme other than `http` or `https` such as `file:` or `ftp:`) is `InvalidRequest`, not `Network`. With redirects on, a server-controlled redirect whose `Location` is malformed or not `http`/`https` is `InvalidRequest` too, so the variant does not always mean the request must be fixed. Credentials in the URL (`https://user:password@host/`) become an `Authorization: Basic` header; prefer headers.
- `Request::header` cannot fail: a bad header name or value marks the request invalid. Every client calls `Request::validate()` before it sends, and so should an `HttpClient` you write.
- A `ReqwestClient` has a 10 second total timeout, a 1 MiB body limit, no redirects and no `Referer`, and speaks HTTP/1.1 only. Proxies come from `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`, read when the client is built; there is no setter.
- When a request needs more than `follow_redirects(n)` redirects, the last 3xx response is returned. A 307 or 308 to another scheme, host or port, which would send the method and the body again, and any redirect from `https` to `http` are not followed either: the 3xx response is returned. A 307 or 308 upgrade from `http` to `https` is stopped too, because the scheme differs. A `POST` answered by a cross-origin 301 or 302 becomes a `GET` without a body, but a `PUT`, `PATCH` or `DELETE` keeps its method and body on a 301 or 302, so leave redirects off (the default) for requests other than `POST` with a secret in the body. On a followed redirect to another scheme, host or port, `Authorization`, `Cookie`, `Cookie2`, `Proxy-Authorization` and `WWW-Authenticate` are dropped, but other headers such as `X-Api-Key` are kept.
- `RetryingClient` retries only `GET`, `HEAD`, `PUT`, `DELETE` and `OPTIONS`, on `Network`, `Timeout` and the statuses 502, 503 and 504, and returns the last response when the attempts run out. Each attempt gets the full timeout.
- The retry settings apply only when `http.client.retry.max-attempts` is set, and the client is wrapped only when it is above 1.

Imports for the examples below:

```rust
use std::sync::Arc;
use http::Method;
use oxbloom_config::Config;
use oxbloom_http_client::{
    Body, ClientBuilder, HttpClient, HttpClientError, HttpClientSettings, InterceptedClient, Interceptor,
    Request, Response, RetryingClient,
};
use oxbloom_test::{MockHttpClient, MockResponse};
```

#### `HttpClient`

```rust
#[async_trait::async_trait]
pub trait HttpClient: Send + Sync {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError>;
}
```

Object safe; hold an `Arc<dyn HttpClient>`. Implemented by `ReqwestClient`, `RetryingClient`, `InterceptedClient` and `MockHttpClient`.

```rust
async fn ping(client: Arc<dyn HttpClient>) -> Result<String, HttpClientError> {
    client.execute(Request::get("https://api.example.test/ping")).await?.text()
}
```

#### `Request` and `Body`

`Request::new(method, url)`, `Request::get(url)` and `Request::post(url)` start a request. The builder methods are `header(name, value)` (replaces an existing value), `body(Body)` and `timeout(Duration)`. `set_header(&mut self, name, value) -> Result<(), HttpClientError>` is for interceptors. The getters are `method()` (an owned `Method`), `url()`, `headers()`, `body_ref()`, `timeout_value()` and `validate()`. `Body` is `Empty`, `Bytes(Vec<u8>)`, `Form(Vec<(String, String)>)` and, with the feature `json`, `Json(serde_json::Value)`; the matching `Content-Type` is set when you did not set one.

```rust
let request = Request::post("https://idp.example.test/token")
    .header("accept", "application/json")
    .body(Body::Form(vec![("grant_type".into(), "client_credentials".into())]))
    .timeout(std::time::Duration::from_secs(3));
assert_eq!(request.url(), "https://idp.example.test/token");
assert!(request.validate().is_ok());
assert_eq!(Request::get("https://x.test/").header("bad name", "v").validate(), Err(HttpClientError::InvalidRequest));
```

#### `Response`

`Response::new(status, headers, body)`, `status()`, `headers()`, `bytes()`, `text()` (`Decode` for non-UTF-8), `json::<T>()` (`Decode` on bad JSON) and `error_for_status()`.

```rust
let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), br#"{"n":1}"#.to_vec());
let value: serde_json::Value = response.json()?;
assert_eq!(value["n"], 1);
```

#### `HttpClientError`

`Timeout`, `Network`, `TooLarge`, `BadStatus(StatusCode)`, `Build`, `Decode` and `InvalidRequest`, a `#[non_exhaustive]` enum with fixed text, for example `"the HTTP request timed out"`. `InvalidRequest` is a bad header, a bad URL or a scheme other than `http`/`https`, and also a server-controlled redirect with a malformed or non-http `Location`.

#### `ClientBuilder` and `ReqwestClient` (feature `reqwest`, default)

`ClientBuilder::new()` has the defaults above. `timeout(Duration)`, `connect_timeout(Duration)`, `max_body_bytes(usize)`, `follow_redirects(usize)`, `user_agent(&str)` and `tls(Arc<rustls::ClientConfig>)` change them, and `build()` returns `Result<ReqwestClient, HttpClientError>` (`Build` for a bad `User-Agent` or a failed TLS setup). `ReqwestClient` is `Clone` and the clones share the settings. `tls` takes a configuration built on `aws-lc-rs`, for example `SslBundle::client_config`; a configuration whose ALPN offers `h2` makes calls fail with `Network`, because only HTTP/1.1 is built in.

```rust
let client = ClientBuilder::new()
    .timeout(std::time::Duration::from_secs(5))
    .max_body_bytes(64 * 1024)
    .follow_redirects(2)
    .user_agent("billing/2.0")
    .build()?;
```

#### `RetryingClient` (feature `retry`)

`RetryingClient::new(inner: Arc<dyn HttpClient>, policy: oxbloom_tasks::RetryPolicy)`.

```rust
let policy = oxbloom_tasks::RetryPolicy { max_attempts: 3, ..Default::default() };
let client = RetryingClient::new(Arc::new(ClientBuilder::new().build()?), policy);
```

#### `Interceptor` and `InterceptedClient`

`Interceptor::before(&self, request: &mut Request) -> Result<(), HttpClientError>` runs before the send. `InterceptedClient::new(inner)` and `.with(Arc<dyn Interceptor>)` register interceptors, which run in registration order; an error stops the request. `execute` validates the request before the interceptors and again after them.

```rust
struct Trace;

#[async_trait::async_trait]
impl Interceptor for Trace {
    async fn before(&self, request: &mut Request) -> Result<(), HttpClientError> {
        request.set_header("x-trace", "on")
    }
}

let client = InterceptedClient::new(Arc::new(ClientBuilder::new().build()?)).with(Arc::new(Trace));
```

#### `HttpClientSettings` (feature `config`)

`HttpClientSettings::from_config(&Config) -> Result<_, ConfigError>`, `Default` and `build(tls: Option<Arc<rustls::ClientConfig>>) -> Result<Arc<dyn HttpClient>, HttpClientError>`. The public fields are `timeout`, `connect_timeout`, `max_body_bytes`, `follow_redirects`, `user_agent`, `ssl_bundle` and `retry: Option<RetryPolicy>`. `build` does not resolve `ssl_bundle`; the caller does and passes the result as `tls`. A `user_agent` that is not a valid header value is a `ConfigError::Invalid` on `http.client.user-agent` from `from_config`. An error names the full key and never the value.

```rust
let config = Config::from_str("[http.client]\ntimeout = \"3s\"\n[http.client.retry]\nmax-attempts = 2")?;
let settings = HttpClientSettings::from_config(&config)?;
let client: Arc<dyn HttpClient> = settings.build(None)?;
```

| Key | Default | Meaning |
|---|---|---|
| `http.client.timeout` | `10s` | The total time of a request, connect included. Above zero. |
| `http.client.connect-timeout` | unset | The time to connect. Above zero. |
| `http.client.max-body-bytes` | `1048576` | The response body limit. At least 1. |
| `http.client.follow-redirects` | `0` | How many redirects are followed. |
| `http.client.user-agent` | unset | The `User-Agent` header. Not empty, and a valid header value (no line breaks). |
| `http.client.ssl-bundle` | unset | Boot (feature `ssl`): the bundle whose client configuration the client uses. Not empty; unknown names list the bundles. |
| `http.client.retry.max-attempts` | unset | Turns the retry settings on; the client retries when it is above 1. |
| `http.client.retry.backoff` | `exponential` | `fixed` or `exponential`. |
| `http.client.retry.initial` | `100ms` | The delay before the first retry; not above `max-delay`. |
| `http.client.retry.multiplier` | `2.0` | Finite and at least 1.0. |
| `http.client.retry.max-delay` | `30s` | The cap on every delay. |
| `http.client.retry.jitter` | `full` | `none`, `full` or `equal`. |

An unknown key under `http.client` or `http.client.retry` is an error. The `retry` keys other than `max-attempts` are checked and then ignored when `max-attempts` is not set.

#### `HttpClientAutoConfiguration` (feature `http-client` of `oxbloom-boot`)

Registers one `Arc<dyn HttpClient>` bean built from `HttpClientSettings`, unless the application registers a `dyn HttpClient` bean (`on_missing_bean`). It has no conditions, so the bean exists with the defaults when the section is missing. `auto_configurations()` lists it last. With the feature `ssl`, `http.client.ssl-bundle` resolves a bundle of the `SslBundles` bean; without it the key is an error. A bad value fails the start with `DiError::Creation` whose source is the `ConfigError` for the key. The startup failure report names the key; for an unknown `http.client.ssl-bundle` it also shows the available bundle names (for example "available bundles: api, idp"), and any other client build failure is reported on the section `http.client`. `HttpClientAutoConfiguration::properties()` lists the twelve keys as property metadata.

```rust
// In a bean: `client: Arc<dyn HttpClient>` is the configured client.
// OxbloomApplication::builder().auto_configuration::<HttpClientAutoConfiguration>()
```

#### `MockHttpClient` and `MockResponse` (feature `http-client` of `oxbloom-test`)

`MockResponse::status(code)` (panics for an invalid code) with `header(name, value)`, `json(&value)`, `text(&str)` and `bytes(Vec<u8>)`; a `Response` converts into one. `MockHttpClient::new()`, `on(method, url, response) -> &Self`, `on_fn(f) -> &Self`, `requests()`, `request_count()` and `unmatched_requests()` (the requests no rule answered; a test of a failure path should assert it is empty, so a mistyped URL cannot pass a negative test). `on` matches the method and the exact URL, a second `on` for the same pair adds a later answer, answers are served in order and the last repeats. `on_fn` closures and `on` rules are tried in the order they were added, and a closure returns `None` to pass. A request that no rule answers is `Network`. Every valid request is recorded; an invalid one is an error and is not recorded. The closure of `on_fn` runs while the mock's state lock is held, so it must not call the same mock.

```rust
let mock = Arc::new(MockHttpClient::new());
mock.on(Method::GET, "https://idp.example.test/jwks", MockResponse::status(200).text("{}"));
let fetcher = oxbloom_security::ReqwestJwksFetcher::with_client(mock.clone());
```

### 4.15 OAuth2 login

OAuth2 and OpenID Connect login with the authorization code flow and PKCE, in `oxbloom-security` behind the feature `oauth2` (it implies `jwks`), wired by `SecurityAutoConfiguration` with the feature `oauth2` of `oxbloom-boot`, and tested with `FakeProvider` from `oxbloom-test` (feature `oauth2`). The guide chapter is [OAuth2 login](guide/27-oauth2-login.md#oauth2-login) and the decisions are in [ADR 0094](adr/0094-oauth2-login.md). Rules to remember:

- `GET /oauth2/authorization/{id}` starts a login and `GET /login/oauth2/code/{id}` is the callback. Both are open, answered before the rules; any other method, HEAD included, is a 405 with `Allow: GET`. The callback path is fixed.
- OAuth2 login needs `form_login` and a `SessionFilter`; without the filter every login fails and the fixed error "OAuth2 login has no session: add a SessionFilter to the application" is logged once per `OAuth2Login`. A successful login is a form login: new session id, new CSRF token, the `Authentication` in the session, the session limit, then the saved request or the default success URL. Every failure redirects to the failure URL with an empty body and logs a fixed text at debug on `oxbloom::security`.
- The principal is the `user-name-attribute` value (default `sub`; a JSON integer becomes a string). The authorities are `OIDC_USER` or `OAUTH2_USER` plus `SCOPE_<scope>` for each granted scope. The details hold the user attributes (without those whose names start with `oxbloom.` or `oauth2.`), `oauth2.registration` and `oauth2.principal = true`; revalidation skips a principal with both markers.
- Provider calls (discovery, the key set, the token exchange, the user info) go through the `HttpClient` given to `OAuth2Login::new`, so its settings apply (in boot the `http.client.*` keys: `follow-redirects`, default 0, `max-body-bytes`, the proxy and the SSL bundle), and each discovery, token and user info request carries a timeout of 10 seconds per attempt that replaces a shorter `http.client.timeout` (the key set fetch waits for the shorter of the two).
- The tokens are stored in the session under `oxbloom.security.oauth2.tokens.<registration>` and removed at every other login. The session limit and the session index count an OAuth2 user as `oauth2:<registration>:<principal>`, so usernames that start with `oauth2:` are reserved: with the feature `oauth2` a password or remember-me login whose principal starts with `oauth2:` fails like a wrong password.
- Id tokens are accepted with RS256, RS384, RS512, ES256 or ES384 only (default RS256 and ES256, narrowed by discovery); `none`, symmetric algorithms and PS256 are refused, and a `kid` is required.
- No error, log line or `Debug` output holds a token, a code, a state, a nonce, a verifier, a secret, a URL or a response body.

```rust
use std::sync::Arc;
use std::time::Duration;
use oxbloom_config::{Config, Secret};
use oxbloom_security::{
    ClientAuthentication, ClientRegistration, HttpSecurity, OAuth2Login, OAuth2Settings,
    ProviderConfig, SecurityBuildError,
};
use oxbloom_test::{FakeProvider, FakeUser, Fault, MockHttpClient};
```

#### `HttpSecurity::oauth2_login(f)` and `HttpSecurity::has_oauth2_login()` (feature `oauth2`)

`oauth2_login(f: impl FnOnce(OAuth2Login) -> OAuth2Login)` turns OAuth2 login on. On the first call `f` receives settings with no client, so start from `OAuth2Login::new(client)` or call `client`; a later call tunes the result of the earlier one. `has_oauth2_login()` says whether `oauth2_login` was called; boot reads it to step aside when a customizer configured OAuth2 login. `build()` fails with `OAuth2LoginWithoutFormLogin`, `OAuth2LoginWithoutClient`, `OAuth2LoginWithoutRegistration` (none, an unknown provider, or a registration or provider that fails the shape checks of the `security.oauth2.client.*` keys: an id or provider name outside `[a-z0-9-]+`, an empty client id, a scope outside RFC 6749, no non-empty secret unless the authentication is `ClientAuthentication::None`, neither an issuer nor both an authorization and a token endpoint, an endpoint that is not `https` (or `http` for a loopback host), or a plain provider without both a user-info endpoint and a user name attribute), `OAuth2LoginWithoutBaseUrl` (missing, not absolute, credentials in the URL, an invalid port, plain `http` for a host that is not loopback, another scheme, a path beyond an optional `/`, or a query or a fragment), `InvalidOAuth2Login` (a redirect URI that is not `{base-url}/login/oauth2/code/{id}`, or a zero pending max age) and `InvalidFormLoginUrl` (an unsafe failure URL, or one under `/oauth2/authorization/` or `/login/oauth2/code/`). With OAuth2 login on, form login needs no password provider.

```rust
let fake = FakeProvider::new("https://idp.example.test");
let chain = HttpSecurity::new()
    .form_login(|f| f)
    .oauth2_login(|_| {
        OAuth2Login::new(fake.client())
            .registration(ClientRegistration {
                id: "idp".into(),
                client_id: "app".into(),
                client_secret: Some(Secret::new("app-secret".into())),
                scopes: vec!["email".into()],
                redirect_uri: "{base-url}/login/oauth2/code/{id}".into(),
                client_authentication: ClientAuthentication::Basic,
                provider: "idp".into(),
            })
            .provider("idp", ProviderConfig {
                issuer_uri: Some("https://idp.example.test".into()),
                ..ProviderConfig::default()
            })
            .base_url("https://app.example.test")
    })
    .authorize(|a| a.any_request().authenticated())
    .build()?;

assert!(!HttpSecurity::new().has_oauth2_login());
let err = HttpSecurity::new()
    .oauth2_login(|_| OAuth2Login::new(Arc::new(MockHttpClient::new())))
    .authorize(|a| a.any_request().authenticated())
    .build()
    .err();
assert_eq!(err, Some(SecurityBuildError::OAuth2LoginWithoutFormLogin));
```

#### `OAuth2Login`

The settings inside `oauth2_login`. `OAuth2Login::new(client: Arc<dyn HttpClient>)`, then `client(..)` (replace the client), `registration(ClientRegistration)` (the same id replaces it), `provider(name, ProviderConfig)`, `base_url(&str)` (the `{base-url}` of the redirect URI; a trailing slash is dropped; never taken from the `Host` header), `failure_url(&str)` (default: the failure URL of form login), `clock(Arc<dyn Clock>)` (default: the `HttpSecurity` clock) and `pending_max_age(Duration)` (default 10 minutes). A provider with `issuer_uri` or `jwk_set_uri` is an OpenID Connect provider (the login asks for `openid`, sends a `nonce` and validates the id token); any other uses `user_info_uri`. `Debug` shows the registration ids and provider names, never a secret. A provider with `jwk_set_uri` and no `issuer_uri` makes the build log one warning naming the registration.

```rust
let login = OAuth2Login::new(Arc::new(MockHttpClient::new()))
    .base_url("https://app.example.test")
    .failure_url("/signin?failed")
    .pending_max_age(Duration::from_secs(300));
assert!(format!("{login:?}").contains("/signin?failed"));
```

#### `OAuth2Settings`, `ClientRegistration`, `ProviderConfig`, `ClientAuthentication`

`OAuth2Settings::from_config(&Config) -> Result<OAuth2Settings, ConfigError>` reads `security.oauth2.client.registration.<id>.*` (`client-id`, `client-secret`, `scope`, `redirect-uri`, `client-authentication`, `provider`) and `security.oauth2.client.provider.<id>.*` (`issuer-uri`, `authorization-uri`, `token-uri`, `user-info-uri`, `jwk-set-uri`, `user-name-attribute`). The fields are `registrations: Vec<ClientRegistration>` (ordered by id) and `providers: BTreeMap<String, ProviderConfig>`; without the section both are empty. An error is a `ConfigError::Invalid` that names the full key and never the value: a missing `client-id`, a missing `client-secret` unless `client-authentication` is `none`, an unknown provider, a provider with neither an `issuer-uri` nor both an `authorization-uri` and a `token-uri`, a plain provider without both `user-info-uri` and `user-name-attribute`, a non-`https` endpoint (loopback hosts excepted), an id outside `[a-z0-9-]+`, a bad scope or redirect template, an unknown key. `ClientRegistration` has the public fields `id`, `client_id`, `client_secret: Option<Secret<String>>`, `scopes`, `redirect_uri`, `client_authentication` and `provider`, and `resolved_redirect_uri(base_url)`; its `Debug` hides the secret. `ProviderConfig` has six `Option<String>` fields and `Default`. `ClientAuthentication` is `Basic` (default, HTTP Basic with form-URL-encoded id and secret), `Post` or `None`.

```rust
let config = Config::from_str(
    "[security.oauth2.client.registration.idp]\nclient-id = \"app\"\nclient-secret = \"s3cret-value\"\n\
     [security.oauth2.client.provider.idp]\nissuer-uri = \"https://idp.example\"\n",
)?;
let settings = OAuth2Settings::from_config(&config)?;
let idp = &settings.registrations[0];
assert_eq!(idp.client_authentication, ClientAuthentication::Basic);
assert_eq!(idp.resolved_redirect_uri("https://app.example/"), "https://app.example/login/oauth2/code/idp");
assert!(!format!("{idp:?}").contains("s3cret-value"));
```

#### `OAuth2Client`, `AccessToken` and `TokenError`

`OAuth2Client::new(settings, client)` gets access tokens for the application itself, with the client credentials grant (RFC 6749 section 4.4), at the providers of the registrations in an `OAuth2Settings`. `client_credentials(registration_id)` posts `grant_type=client_credentials` and the registration's scopes (no `scope` field when there are none) with the client authentication of the registration, and returns an `AccessToken { value: Secret<String>, expires_at: SystemTime }`. The token is cached per registration until `skew` (default 30 seconds, `.skew(Duration)`) before it expires. A token without `expires_in` counts as valid for 60 seconds, and a token that is within the skew of its expiry when it arrives (for example `expires_in = 0`) is returned but not kept, so the next call asks again. Callers that arrive while a request is in flight wait for it and take its outcome, so there is one request at a time per registration; a failed token request is not cached and has no backoff, so the next call tries again at once; provider discovery keeps the 5 second failure backoff of its metadata cache, so within 5 seconds of a failed discovery a call can return `TokenError::Discovery` or `Http` without a request. `invalidate_token(registration_id, &refused)` drops the cached token only if it is the refused one (same expiry, constant-time equal value): call it when a service answered 401 to a request that carried `refused`; a late 401 for an old token does not discard a fresh one, and many callers refused with the same token cause one refetch (the interceptor of Task 12 uses it). `invalidate(registration_id)` is the unconditional reset. `AccessToken` is `#[non_exhaustive]`: read `value` and `expires_at`. The client has its own metadata cache, not the one of the login. A registration with `client-authentication = none` is refused with `TokenError::Invalid`, because a public client has nothing to authenticate with. `.clock(Arc<dyn Clock>)` replaces the system clock (tests pass a `ManualClock`). The client is `Send + Sync` and cheap to clone; the clones share the cache. Provider calls follow the same rules as the login: they go through the given `HttpClient` and carry the 10 second timeout described above.

`TokenError` is non-exhaustive with fixed text and no URL, body or token: `UnknownRegistration`, `Provider(&'static str)` (an RFC 6749 error code from an allow-list, or `unknown`), `Http`, `Discovery`, `ReauthenticationRequired` (reserved for the refresh of a user token) and `Invalid`. `AccessToken`'s `Debug` shows the expiry and hides the token.

```rust
let fake = FakeProvider::new("https://idp.example.test");
let mut settings = OAuth2Settings::default();
settings.providers.insert("idp".into(), ProviderConfig {
    issuer_uri: Some("https://idp.example.test".into()),
    ..ProviderConfig::default()
});
settings.registrations.push(ClientRegistration {
    id: "billing".into(),
    client_id: "orders-service".into(),
    client_secret: Some(Secret::new("orders-secret".into())),
    scopes: vec!["invoices.read".into()],
    redirect_uri: String::new(),
    client_authentication: ClientAuthentication::Basic,
    provider: "idp".into(),
});
let client = OAuth2Client::new(settings, fake.client());
let token = client.client_credentials("billing").await?;
client.client_credentials("billing").await?; // from the cache
assert_eq!(fake.token_requests().len(), 1);
assert!(!format!("{token:?}").contains(token.value.expose()));
client.invalidate_token("billing", &token); // after a 401 for `token`
```

#### The boot keys and the build errors in boot

With the feature `oauth2` of `oxbloom-boot`, `SecurityAutoConfiguration` configures OAuth2 login when `security.oauth2.client.registration` has an entry (see `SecurityAutoConfiguration` in section 4.8). The two boot keys are `security.oauth2.base-url` (required then; an absolute `https` URL, `http` only for a loopback host, with no path beyond an optional `/`, no query and no fragment) and `security.oauth2.failure-url` (optional; a local path outside the two endpoints). The startup failure report names the key for every configuration error (for example `security.oauth2.client.registration.idp.redirect-uri` for a redirect URI that does not lead to the fixed callback path) and leaves out the value; `OAuth2LoginWithoutFormLogin` becomes a report that says to set `security.form-login.enabled = true`, and a missing `HttpClient` bean a report that names `HttpClientAutoConfiguration`. `SecurityAutoConfiguration::properties()` lists 14 `security.oauth2.*` keys with the feature.

```toml
[security.form-login]
enabled = true
[security.session]
[security.oauth2]
base-url = "https://app.example"
[security.oauth2.client.registration.github]
client-id = "Iv1.0123456789abcdef"
client-secret = "${GITHUB_CLIENT_SECRET}"
scope = ["read:user"]
[security.oauth2.client.provider.github]
authorization-uri = "https://github.com/login/oauth/authorize"
token-uri = "https://github.com/login/oauth/access_token"
user-info-uri = "https://api.github.com/user"
user-name-attribute = "id"
```

#### `FakeProvider`, `FakeUser`, `Fault` and `TokenRequest` (feature `oauth2` of `oxbloom-test`)

`FakeProvider::new(issuer)` is an OpenID Connect provider over a `MockHttpClient` with a fresh ES256 key: `/.well-known/openid-configuration`, `/jwks`, `POST /token` and `/userinfo` under the issuer (not `/authorize`, where only the browser goes). `client()` returns its `Arc<MockHttpClient>` (always the same one), `issuer()`, `clock(Arc<dyn Clock>)` takes the token times from a test clock, `issue_code(FakeUser) -> String` registers a code that works once, and `token_requests()` returns the token requests (`TokenRequest::field`, `fields`, `authorization`; `Debug` hides the values). By default the token endpoint checks nothing but the code; the expectations turn on checks: `expect_nonce(nonce)` (the nonce of the id tokens of codes issued from now on), `expect_pkce(challenge)` (the S256 of `code_verifier` must equal it, else `invalid_grant`), `expect_redirect_uri(uri)` (exact match, else `invalid_grant`) and `expect_client_secret(secret)` (from Basic or `client_secret`, else `invalid_client`). `misbehave(Fault)` adds a fault that stays in force: `WrongIssuer`, `WrongAudience`, `WrongAzp`, `Expired`, `NotYetValid`, `WrongNonce`, `AlgNone`, `Hs256WithPublicKey`, `MissingSub`, `TokenEndpointError(code)`, `TokenEndpointStatus(status)`, `OversizedBody`, `DiscoveryIssuerMismatch` and `UserinfoError`. `FakeUser::new(sub)` with `email`, `name`, `attr(key, value)` and `scope`; its user info is the attributes plus `sub`. The token endpoint serves the grants `authorization_code`, `client_credentials` (an access token only, with the requested `scope` echoed) and `refresh_token` (an unknown refresh token is `invalid_grant`). `access_token_lifetime(Option<u64>)` sets the `expires_in` of every access token (default 3600, `None` leaves it out) and `rotate_refresh_tokens(bool)` chooses whether a refresh issues a new refresh token (default) or keeps the old one valid; every access token is new and random.

```rust
let fake = FakeProvider::new("https://idp.example.test");
fake.expect_client_secret("app-secret")
    .expect_redirect_uri("https://app.example.test/login/oauth2/code/idp");
fake.misbehave(Fault::WrongNonce);
let code = fake.issue_code(FakeUser::new("alice").email("alice@example.test").scope("openid email"));
assert!(!code.is_empty() && fake.token_requests().is_empty());
```

## 5. Recipe: a complete small service

A configured greeting, a controller with an injected bean, and a web server started through
`OxbloomApplication`. `examples/hello` is a larger version of the same shape.

```rust
use std::process::ExitCode;
use std::sync::Arc;
use oxbloom_boot::{OxbloomApplication, Service, WebServer};
use oxbloom_config::Config;
use oxbloom_di::{DiError, component};
use oxbloom_web::{App, IntoResponse, Request, controller};

#[component]
struct Greeter { config: Arc<Config> }

impl Greeter {
    fn greeting(&self) -> String {
        self.config.get::<String>("app.greeting").unwrap_or_else(|_| "Hello".into())
    }
}

#[component]
struct HelloController { greeter: Arc<Greeter> }

#[controller("/hello")]
impl HelloController {
    #[get("/{name}")]
    async fn greet(&self, req: Request) -> impl IntoResponse {
        format!("{}, {}!", self.greeter.greeting(), req.param("name").unwrap_or("stranger"))
    }
}

/// Reads `server.port` from the configuration boot built; `?` keeps typed errors.
fn web_server(config: Arc<Config>, hello: Arc<HelloController>) -> Result<WebServer, DiError> {
    let app = App::new().routes(hello.routes()).map_err(DiError::creation_from::<WebServer>)?;
    Ok(WebServer::new(app, &config)?)
}

#[tokio::main]
async fn main() -> ExitCode {
    OxbloomApplication::builder()
        .args(std::env::args().skip(1))          // --server.port=9000, --app.greeting=Hi
        .config(|b| b.dir("config"))             // config/application.toml
        .configure(|b| {
            b.component::<Greeter>()
                .component::<HelloController>()
                .try_bean(web_server)
                .provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })
        })
        .run_and_exit()                          // Ctrl-C stops it; a startup failure prints a report
        .await
}
```

Needs `oxbloom-boot` with feature `web`. Boot registers `Config` itself, so components can take
`Arc<Config>` without `ContainerBuilderExt::config`.

## 6. Testing pattern

Test handlers without a network, wiring included:

```rust
#[tokio::test]
async fn greets() {
    let config = Config::from_str("[app]\ngreeting = \"Hi\"").unwrap();
    let container = ContainerBuilder::new()
        .config(config)
        .component::<Greeter>()
        .component::<HelloController>()
        .build()
        .unwrap();
    let app = App::new().controller::<HelloController>(&container).unwrap();

    let res = app.handle(Request::new(HttpMethod::Get, "/hello/Ada")).await;
    assert_eq!(res.body(), "Hi, Ada!");

    let res = app.handle(Request::new(HttpMethod::Post, "/hello/Ada")).await;
    assert_eq!(res.status(), 405);
}
```

This test needs `oxbloom-web` and `oxbloom-config` with feature `di`. To test the whole
application without a server, use `oxbloom-test` (see [`oxbloom-test`](#410-oxbloom-test)). To test a real server, run it in-process with `server.port = 0`, read the address in a `RunListener::ready`
from `container.get::<WebServer>()?.local_addr()`, send requests over a `tokio::net::TcpStream` and end
the run with the builder's `ShutdownHandle`; `examples/hello/tests/app.rs` does exactly this.

Compile-fail tests use `trybuild` (`tests/ui/*.rs` with `.stderr` snapshots). Regenerate snapshots with
`TRYBUILD=overwrite cargo test -p <crate>` and review the diff before keeping it.

## Not implemented

The OAuth2 client (client credentials and the user's token refresh), RP-initiated logout and OAuth2 provider presets, login rate limiting, Redis and SQL session stores, database user stores, database, jobs and queue, metrics and
other observability beyond the actuator, React integration, `oxbloom-ai`, `oxbloom add`, MVC web style. In
boot: analysis of failures after startup, a report format other than text. In DI: transient and request scopes,
injection of single configuration properties, qualifiers by name, async lifecycle hooks and
automatic shutdown on drop. For conditions and auto-configuration: condition options on
`#[component(..)]`, discovery of auto-configurations at link time (only explicit lists exist),
exclusion through configuration keys, a runtime `on_feature` condition (use `#[cfg(feature = "..")]`),
and naming condition-skipped registrations in the missing-bean failure report. For TLS and SSL bundles: mutual
TLS and `client-auth`, an HTTP to HTTPS redirect, ALPN `h2` (HTTP/2), hot reload of the JWKS client (it is built
once at startup), OCSP stapling, revocation checks, JKS keystores, encrypted PEM private keys, a file-system
watcher (reload polls), SSL bundles for database clients (the HTTP client takes `http.client.ssl-bundle`), and a limit on concurrent TLS handshakes.
For the actuator: a separate management port, roles per endpoint, `/metrics`, `/env`, `/beans` and
`/threaddump` endpoints, a `diskSpace` indicator, and resetting a log level through `configuredLevel: null`.
For the HTTP client: HTTP/2, a proxy setter (proxies come from the environment), per-host clients, a request-level retry override, streaming bodies, response interceptors and metrics. For task scheduling: retry, backoff and jitter (C9b), an actuator `/scheduledtasks` endpoint, persistence and
catch-up of missed runs, distributed locking, adding or removing tasks at run time, per-task concurrency limits,
a backward clock clamp for cron tasks, and one duration grammar for configuration and attributes.
Do not document these as available.
When you build one, start with a spec in `docs/superpowers/specs`.
