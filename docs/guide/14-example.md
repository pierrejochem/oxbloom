# Example

`examples/hello` is a small application that uses the features that exist today. Each file says which features it shows:

| File | Shows |
|---|---|
| `src/greeting.rs` | a `#[configuration]` with `#[bean]` methods, typed configuration from `hello.*`, two `dyn Greeter` beans with `primary` and `order`, a fallible bean that keeps its typed error |
| `src/web.rs` | a `#[component]` controller that injects `Arc<dyn Greeter>` and `Vec<Arc<dyn Greeter>>`, a `SecurityCustomizer` (the filter chain and the `WebServer` are auto-configured) |
| `src/lifecycle.rs` | a lazy bean with a `pre_destroy` hook, an `ApplicationRunner`, a `RunListener`, an `ExitCodeGenerator` |
| `src/failure.rs` | a custom `FailureAnalyzer` |
| `src/auto.rs` | an `#[auto_configuration]` with a class-level `on_property`, a default `dyn GuestGreeter` with `on_missing_bean`, and the application's own guest greeter behind `hello.custom-greeter = true` |
| `application.toml`, `application-dev.toml` | `hello.greeting`, `hello.exit-code`, `server.port`, the `logging.level` table and the `dev` profile |
| `tests/app.rs` | the application in-process on port 0, driven over TCP and ended by a `ShutdownHandle`; the custom report for an empty greeting and the built-in security report for a bad matcher pattern (the example enables boot's features `web`, `security` and `logging`) |
| `tests/auto.rs` | the auto-configured guest greeter by default, the application's own with `--hello.custom-greeter=true`, and the conditions report for both |

The wiring, from `src/lib.rs`:

```rust
OxbloomApplication::builder()
    .config(move |b| b.dir(config_dir).secret_key(SECRET_KEY))
    .listener(Announce)
    .failure_analyzer(EmptyGreetingAnalyzer)
    .auto_configuration::<GuestAutoConfig>()
    .configure(|b| {
        b.instance(GreetingConfig)
            .configuration::<GreetingConfig>()
            .bean(VisitCounter::default)
            .lazy()
            .lifecycle::<VisitCounter>()
            .component::<HelloController>()
            .provides(|c: Arc<HelloController>| -> Arc<dyn Controller> { c })
            .component::<StartupSummary>()
            .bean(ConfiguredExitCode::new)
            .provides(|c: Arc<ConfiguredExitCode>| -> Arc<dyn ExitCodeGenerator> { c })
            .bean(|| PublicPaths)
            .provides(|p: Arc<PublicPaths>| -> Arc<dyn SecurityCustomizer> { p })
    })
    .auto_configurations(oxbloom_boot::auto_configurations())
    .configure(auto::custom_guest_greeter)
```

Run it with `cargo run -p hello -- --server.port=0` (the default port is 8080, 8081 with the `dev` profile; 0 picks a free one). It reads its configuration files from `examples/hello` whatever the working directory. Ctrl-C stops it gracefully, and the process then exits with `hello.exit-code` (default 0).

- `GET /api/hello` returns `Hello, world!`, `GET /api/hello/Ada` returns `Hello, Ada!`, and `GET /api/greetings/Ada` lists every greeter in `order`.
- `GET /api/admin/visits` needs a bearer token and is 401 without one; so is every unknown path. Tokens are verified only when a JWT key is configured in the environment: `APP_SECURITY__JWT__ALGORITHMS=HS256` and `APP_SECURITY__JWT__SECRET=<at least 32 bytes>`.
- `--oxbloom.profiles.active=dev` adds `application-dev.toml`, which changes the greeting to `Hi from dev`.
- `--hello.greeting=` (an empty greeting) fails startup with the example's own failure report.
- At startup the runner prints `guest greeter: Welcome, guest!`, the auto-configured default. `--hello.custom-greeter=true` registers the application's own guest greeter instead, and the runner prints `guest greeter: Make yourself at home, guest!`. Any other value keeps the default.
- `--oxbloom.main.conditions-report=console` prints the conditions evaluation report shown in [The conditions report](06-conditions-and-auto-configuration.md#the-conditions-report); with the override flag, the `on_property_value` record moves to `Matched:` and the default's `on_missing_bean` record to `Skipped:`.

## The todos example

`examples/todos` is a CRUD REST API on the web, security and logging stack. It shows `#[controller("/api/todos")]` with `#[post]`, `#[get]`, `#[put]`, `#[patch]` and `#[delete]`, path parameters, a query filter (`?done=true`), status codes (201 with a `Location` header, 204, 400, 404, 415, 422), a thread-safe in-memory store, a `SecurityCustomizer`, and `tracing` events at the target `todos` that boot's logging subscriber prints. Bodies are read with `Request::json` and answers are built with `Json<T>`; a body without `Content-Type: application/json` is 415. Run it with `make run-todos ARGS="--server.port=0"` (or `cargo run -p todos -- --server.port=0`); it prints the address. The handlers log at `info`, and `--logging.level.todos=warn` hides their events. The example also enables the [actuator](17-actuator.md#actuator): its `build.rs` calls `oxbloom_actuator::build::emit()`, its security rules open `GET /actuator/health/**`, and `examples/todos/README.md` has the `curl` calls.

```
curl -X POST -H 'content-type: application/json' -d '{"title":"try it"}' http://127.0.0.1:<port>/api/todos
curl "http://127.0.0.1:<port>/api/todos?done=false"
```

`PUT` and `PATCH` on an unknown id are 404 (they never create), and concurrent `POST`s never share an id. The example's security rules open `/api/todos/**` so that it runs without a token; the doc comment of `PublicPaths` shows how to protect the writes. This opening is intentional for the example, and the example must not be deployed as it is.
