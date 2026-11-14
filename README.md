<p align="center"><img src="branding/logos/oxbloom-lockup.svg" alt="Oxbloom" width="360"></p>

# Oxbloom

**Enterprise Rust, in bloom.**

Modular, Spring-Boot-inspired MVC framework for Rust. See `IDEA.md` for the roadmap and
`docs/ROADMAP-core.md` for the planned core features. Brand assets and naming rules are in `branding/`.

## Status

Implemented and tested:

- `oxbloom-core`: `HttpMethod`.
- `oxbloom-macros`: `#[controller]`, `#[get]`, `#[post]`, `#[put]`, `#[patch]`, `#[delete]`, `#[head]`, `#[options]`, `#[component]`, `#[configuration]`, `#[bean]` (with condition options), `#[auto_configuration]`.
- `oxbloom-web`: router, Hyper server, graceful shutdown with a timeout, the ordered `Filter` layer, `HttpError` and problem+json error answers, forwarded-header handling, and with the default feature `json` the `Json<T>` response, `Request::json` and `Accept` negotiation, and with no feature a CORS filter, static resources (disk and embedded) and, with the default feature `compression` (gzip, brotli, zstd), a compression filter; see [Errors and filter order](#errors-and-filter-order), [JSON and content negotiation](#json-and-content-negotiation), [CORS](#cors), [Compression](#compression) and [Static resources](#static-resources).
- `oxbloom-di`: singleton container with trait and collection injection, `primary`/`order`, lifecycle hooks and shutdown, lazy beans, `bean` factories, `#[configuration]`/`#[bean]`, profile conditions, and conditions on beans and groups with a conditions report.
- `oxbloom-config`: TOML files with imports, profiles and groups, environment and command-line sources, relaxed keys, lists, `Duration`/`DataSize`, placeholders, key migration, origins, serde binding, `Secret`, and `Config` as the property source of conditions.
- `oxbloom-security`: filter chain, deny-by-default authorization, JWT resource server with static keys and JWKS.
- `oxbloom-boot`: `OxbloomApplication` with lifecycle events, `ApplicationArguments`, application and command-line runners, ordered services with a shared stop deadline, Ctrl-C and SIGTERM handling, `ShutdownHandle`, exit codes, a banner, failure analyzers with a startup failure report, and auto-configuration with ordering, `exclude` and the conditions report. Feature `web` adds `WebServer`, the port-in-use analyzer and `WebServerAutoConfiguration`; feature `security` adds the security analyzer, a built-in analyzer for invalid `security.jwt` configuration, `JwtDecoderAutoConfiguration`, `SecurityAutoConfiguration` and the `SecurityCustomizer` trait; feature `logging` installs the `oxbloom-logging` subscriber, registers the `LoggingSystem` bean, logs startup events and adds a built-in analyzer for invalid `logging` configuration. `PropertyMetadata` and `boot_properties()` describe the configuration keys, and `auto_configurations()` lists the auto-configurations of the enabled features.
- `oxbloom-logging`: a `tracing` subscriber built from the `logging.*` keys, with levels, groups, console format and color, rotating file output, JSON lines with configurable fields, the `log` bridge and `LoggingSystem` for changing levels at runtime (the request logging filter lives in `oxbloom-web`); see [Logging](#logging).
- `oxbloom-test`: test support for applications: `ContextRunner` runs a context through the real boot lifecycle, with captured output, temporary config directories and controllable clocks (`FixedClock`, `ManualClock`); the `Clock` trait in `oxbloom-core` reaches JWT `exp`/`nbf` checks and the JSON log timestamp. With the feature `web`, `TestClient` sends requests to an `App` in memory or over HTTP/1.1 and `TestApp` starts a whole application with a real web server on port 0. See [`oxbloom-test`](docs/API.md#410-oxbloom-test).
- `oxbloom-cli`: `oxbloom new` (a project started through `OxbloomApplication`), `oxbloom list`.
- `examples/hello`: a small application that uses the features above; see [Example](#example).
- `examples/todos`: a CRUD REST API on the web, security and logging stack; see [Example](#example).

Everything else in `IDEA.md` is not started. See [Roadmap](#roadmap).

## CLI

```
cargo run -p oxbloom-cli -- new my-app --dir /some/parent   # creates /some/parent/my-app
cargo run -p oxbloom-cli -- list
```

`new` writes `.gitignore`, `oxbloom.toml`, `Cargo.toml`, `application.toml`, `src/lib.rs`, `src/main.rs` and `tests/hello.rs`. The project depends on `oxbloom-boot` (feature `web`), `oxbloom-config`, `oxbloom-di` and `oxbloom-web` by path (`--oxbloom-path`, default: this workspace); a path that lacks the `Cargo.toml` of one of these four crates is rejected with `not an oxbloom workspace: missing <path>` (exit code 2). The generated project has been verified to pass `cargo check` and `cargo test`, to serve requests and to stop with exit code 0 on Ctrl-C. `--security`, `--database`, `--jobs`, `--ai`, `--tests` and `--frontend` exit with `<feature> is not implemented yet`. Exit code 2 = bad input, 1 = filesystem error.

The generated `application.toml` sets the port; `--server.port=9000` or `APP_SERVER__PORT=9000` overrides it, and port 0 picks a free one. Boot reads it from the working directory:

```toml
[server]
port = 8080
```

The generated `src/main.rs` builds the web server from the routes of `src/lib.rs` and runs it through `OxbloomApplication`, so a startup failure prints a failure report and exits with code 1 (`my_app` stands for the crate name):

```rust
fn web_server(config: Arc<Config>) -> Result<WebServer, DiError> {
    Ok(WebServer::new(my_app::app(), &config)?)
}

#[tokio::main]
async fn main() -> ExitCode {
    OxbloomApplication::builder()
        .args(std::env::args().skip(1))
        .listener(Announce) // prints `listening on http://..` when ready
        .configure(|b| {
            b.try_bean(web_server)
                .provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s })
        })
        .run_and_exit()
        .await
}
```

## Dependency injection

`oxbloom-di` is a singleton container. You register types on a `ContainerBuilder`, and `build()` creates every eager bean in dependency order. Wiring errors of eager beans are returned by `build()`, not at first use.

```rust
#[component]
struct UserService { repo: Arc<UserRepo> }

let container = ContainerBuilder::new()
    .component::<UserRepo>()
    .component::<UserService>()
    .build()?;                       // wiring errors (missing, cycle, duplicate) fail here
let app = App::new().controller::<UserController>(&container)?; // needs `--features di`
```

`oxbloom-web` builds without `oxbloom-di` unless the `di` feature is on.

### Registrations

- `#[component]` on a struct (or a unit struct) whose fields are injectable shapes; register it with `.component::<T>()`.
- `.instance(value)` for a value you create yourself.
- `.factory(|r: &Resolver| ...)` for a closure that asks the `Resolver` for its dependencies.
- `.bean(f)` for a closure or function whose parameters are injected, with 0 to 12 parameters. The bean is the return type: a function returning `Arc<X>` registers the bean `Arc<X>`, and one returning `Result<T, DiError>` registers that `Result`. Use `.try_bean(f)` for a fallible function; its `Err` becomes a `Creation` error.
- `#[configuration]` on an inherent, non-generic `impl` block whose `#[bean]` methods take `&self` and create beans; register it with `.configuration::<T>()`, and register the configuration type itself too (as a `#[component]` or an `.instance(..)`). `#[bean]` takes the options `fallible` (the method returns `Result<T, DiError>` and the bean is `T`), `provides(dyn A, ..)`, `primary`, `order = n`, `lazy` and `profile = ..`. A `#[cfg(..)]` on a bean method also removes its registration. Use one `#[configuration]` impl per type. `#[bean]` methods cannot take lifecycle hooks (the `lifecycle` option is rejected); register the resource as an `instance` or `factory` followed by `.lifecycle::<T>()`, or add `.pre_destroy(..)` right after `.bean(..)`. The macros refer to the crate as `oxbloom_di`, so do not rename the dependency in `Cargo.toml`.

```rust
#[component]
struct Persistence;

#[configuration]
impl Persistence {
    #[bean]
    fn pool(&self) -> Pool { Pool::connect("postgres://db/app") }

    #[bean(fallible, provides(dyn Store))]
    fn store(&self, pool: Arc<Pool>) -> Result<PgStore, DiError> { PgStore::new(pool) }
}

let container = ContainerBuilder::new()
    .component::<Persistence>()
    .configuration::<Persistence>()
    .bean(|store: Arc<dyn Store>, clock: Option<Arc<dyn Clock>>| Audit { store, clock })
    .build()?;
```

### Injectable shapes

The same shapes work as `#[component]` fields, as `bean` parameters and through `Resolver::get`, `get_opt`, `get_all` and the matching `Container` methods. The set of shapes is closed: the `Inject` trait is sealed.

| Shape | Meaning |
|---|---|
| `Arc<T>`, `Arc<dyn Trait>` | exactly one provider, or the single `primary` provider; otherwise `Missing` or `Ambiguous` |
| `Option<Arc<..>>` | `None` when nothing provides the key; a provider that fails to create is still an error |
| `Vec<Arc<..>>` | every provider, sorted by `order` and then by registration order; empty when there is none |

### Modifiers

A modifier applies to the registration just before it. A modifier before any registration, or one whose type does not match the last registration, makes `build()` return `Invalid`; the builder methods themselves never fail.

- `.provides(|a: Arc<Impl>| -> Arc<dyn Trait> { a })` also registers the bean as a trait object. The trait must be `Send + Sync`. `#[component(provides(dyn Trait))]` does the same.
- `.primary()` picks this provider when a single `Arc<dyn Trait>` has several providers. Without exactly one `primary` provider, a single request is `Ambiguous`.
- `.order(n)` sets the position in `Vec` requests (ascending, default `0`).
- `.lazy()` creates the bean on its first request instead of during `build()`. An eager bean that depends on a lazy one creates it during `build()`. A lazy bean's errors appear at first use.
- `.post_construct(|t: &T| ...)`, `.pre_destroy(|t: &T| ...)` and `.lifecycle::<T>()` (for `impl Lifecycle for T`) add lifecycle hooks.
- `.profile("dev")`, `.profile("!prod")` and `.profile_not("prod")` add profile conditions.
- `.on_bean::<T>()`, `.on_missing_bean::<T>()`, `.on_property(..)`, `.on_property_value(..)`, `.on_property_value_or_missing(..)` and `.condition(..)` add conditions; see [Conditions and auto-configuration](#conditions-and-auto-configuration).

`#[component(..)]` takes the options `provides(dyn A, ..)`, `primary`, `order = n`, `lazy`, `lifecycle` and `profile = "dev" | "!prod" | ["dev", "test"]`. They are applied through `Component::configure`, which a hand-written `Component` can implement too.

### Profiles

Pass the active profile names to the builder; the container does not read configuration itself. With `oxbloom-config`:

```rust
let container = ContainerBuilder::new()
    .active_profiles(config.active_profiles().iter().cloned())
    .config(config)                                   // does not forward the profiles
    .instance(Console).provides(|m: Arc<Console>| -> Arc<dyn Mailer> { m }).profile("dev")
    .instance(Smtp).provides(|m: Arc<Smtp>| -> Arc<dyn Mailer> { m }).profile("!dev")
    .build()?;
```

A registration with conditions is kept when at least one of them holds: a name is active, or a `!name` (or `profile_not`) names an inactive profile. A skipped registration is invisible: it is not created, it is not a candidate for any key, it never causes `Duplicate`, and a dependency on it is `Missing`. An empty profile name (also `"!"`) is `Invalid`.

### Lifecycle and shutdown

`post_construct` runs once, right after the bean is created and after the hooks of its dependencies; no dependent receives the bean before its hook succeeded. A failing hook is a `Creation` error. If `build()` fails, the beans that were already created are destroyed in reverse creation order before the error is returned.

`container.shutdown()` runs the `pre_destroy` hooks of every created bean in reverse creation order and returns a `ShutdownReport` with the fields `destroyed` and `failures` (both report structs are `#[non_exhaustive]`). A panicking `pre_destroy` is caught and reported as a `DestroyFailure` with fixed text. A second call returns an empty report. After shutdown every `get*` returns `ShutDown`; `Arc` handles handed out earlier stay valid. Dropping a container does not shut it down, and hooks are not async.

Lazy creation and shutdown share one container-wide lock that is reentrant for the creating thread. Concurrent first requests create a lazy bean once, and two threads never deadlock on each other's lazy chains. A lazy provider must not wait for another thread that itself creates a lazy bean of the same container. `shutdown` waits for a lazy creation in flight and destroys that bean too. A `shutdown` called from inside a provider or `post_construct` hook on the creating thread closes the container at once: the rest of that chain creates nothing and gets `ShutDown`, and nothing created is left undestroyed.

### Errors

`DiError` is `#[non_exhaustive]`; a `match` on it needs a wildcard arm. Messages carry type names, source labels and fixed text, never values; only `Creation.message` is the developer's own text.

| Variant | Meaning |
|---|---|
| `Missing { requested, required_by }` | nothing provides a required key |
| `Ambiguous { requested, candidates }` | several providers and no single `primary` |
| `Cycle { path }` | circular dependency, e.g. `A -> B -> A` |
| `Duplicate { type_name, sources }` | the same concrete type registered twice; `sources` names both registrations |
| `Creation { type_name, message, source }` | a provider or `post_construct` failed; `source` is the typed cause, if any |
| `Invalid { message }` | builder misuse, such as a modifier before any registration |
| `ShutDown` | a request after `shutdown()` |
| `TooDeep { limit, path }` | a dependency chain deeper than 256; `path` holds its first 8 entries |

`Creation.source` is an `ErrorSource`: empty for `DiError::creation::<T>(message)`, and holding the original error for `DiError::creation_from::<T>(error)`, whose message is the error's `Display`. `std::error::Error::source` on the `DiError` returns it, so a caller can `downcast_ref` it to the original type. `try_bean` and `#[bean(fallible)]` replace the type name with the failing bean's type and keep the source. With the `di` feature of `oxbloom-config` and `oxbloom-security`, `?` turns a `ConfigError` or a `SecurityBuildError` into such a `Creation` error, so `try_bean(|config: Arc<Config>| -> Result<T, DiError> { Ok(config.get("x")?) })` keeps the typed error. The source is shared behind an `Arc`, so cloning a cached error stays cheap, and it is not part of equality: two `Creation` errors with the same type name and message are equal whatever their sources, held or empty. `Display` and `Debug` never print it.

Adding the `source` field is a breaking change for code outside the workspace that builds `DiError::Creation { type_name, message }` literally or destructures it without `..`: the enum-level `#[non_exhaustive]` does not cover the fields of a variant. Use `DiError::creation` or `creation_from` to build the error, and `Creation { type_name, message, .. }` to match it.

Source labels are `component Type`, `instance Type`, `factory Type`, `Type::method` for `#[bean]` methods and `closure at file:line` for `bean` and `try_bean`. A failed creation is remembered: every later request for that bean gets the same error and the provider does not run again. `Cycle` and `TooDeep` are the exception; they describe the requesting stack, so they are never cached. A panicking provider or `post_construct` hook is not caught; the panic propagates out of `build()`.

### Limits

- Dependency depth is capped at 256, and `bean` / `try_bean` take at most 12 parameters.
- Creation is recursive, so a deep graph needs stack. A 256-deep chain on x86_64 Linux needs about 206 KB of stack in a debug build for simple closures, 278 KB for `#[component]` structs with four fields, 298 KB for `.bean` functions with four parameters and 322 KB for `#[bean]` methods with four parameters. `DiError` stayed at 56 bytes when `Creation` gained its `source` field, so these figures did not change. Release builds need 79 to 119 KB. The main thread has 8 MB; run very deep graphs on a thread with enough stack.
- Only singletons: transient and request scopes, injection of single configuration properties, and qualifiers by name are not implemented.

### Compatibility with the first container

- Unchanged: `Component`, `ContainerBuilder::{new, component, instance, factory, build}`, `Container::get` (it now also accepts `dyn Trait`), `Resolver::get`, and `DiError::{Missing, Cycle, Creation}`.
- Changed: `DiError` is `#[non_exhaustive]`, `Duplicate` gained `sources`, and `Container::get` after `shutdown()` is an error. Code that matches `DiError` exhaustively or destructures `Duplicate { type_name }` must be updated.
- Behavior: creation errors are cached per bean, and resolution depth is capped at 256.

## Configuration

`oxbloom-config` loads typed runtime settings from TOML files, the environment and the command line, and binds them to `serde` types. `oxbloom.toml` is the CLI's build-time manifest and is not read here.

```rust
#[derive(Deserialize)]
struct Server { port: u16, host: String, timeout: Duration, hosts: Vec<String> }

let config = Config::builder()
    .dir("config")
    .profile("dev")
    .args(std::env::args().skip(1))   // --server.port=9000
    .build()?;
let server: Server = config.get("server")?;
```

### Sources and precedence

Lowest to highest: `application.toml` and its imports, the files of the active profiles (`application-{profile}.toml`, plus their imports) in activation order, environment variables, command-line arguments. After that, renamed and removed keys are migrated, then placeholders are resolved. Tables merge deeply; scalars and arrays are replaced whole. A missing `application.toml` gives an empty base. Every file read (`application.toml`, profile files, imports) must be a regular file after following symlinks, otherwise it is `ConfigError::Io` (a FIFO or a device is never opened), and may be at most 1 MiB (`.max_file_size(bytes)`), otherwise it is `ConfigError::FileTooLarge`. `Config::from_str` parses its text only: no imports, profiles, environment, command line or migration, and a `oxbloom.config.import` key in it stays an ordinary key; `build()` follows and removes it.

### Keys

Keys match relaxed: case, `-` and `_` are ignored (`maxConnections`, `max-connections` and `MAX_CONNECTIONS` are one key and bind the field `max_connections`). Two spellings of one key in the same file are `ConfigError::DuplicateKey`; across sources the higher-precedence source wins, so `APP_MAX_CONNECTIONS` overrides `max-connections` from a file. Map keys are canonical too: two spellings of one map key (`my-host` and `my_host` under `[aliases]`) in one file are `DuplicateKey`, and a map key keeps the spelling of the winning source, so an environment override replaces it with the lowercased variable spelling. Limits: an exact field name wins over a relaxed match, but detecting two canonically equal field names in one struct (`jwt_secret` and `jwtsecret`) is not complete, so do not declare them; fields under `#[serde(flatten)]` and internally tagged enums match exactly.

Reserved top-level table: `oxbloom`. Only `oxbloom.config.import`, `oxbloom.profiles.active` and `oxbloom.profiles.group.*` are interpreted by the loader; a user's own `[profiles]` or `[config]` tables (for example `[profiles] active = true` or `[config] import = "auto"`) are ordinary keys and load as plain values. `oxbloom.config.import` is removed by `build()` (and `oxbloom.config`, then `oxbloom`, when that leaves them empty); `oxbloom.profiles.*` stays in the final tree and is visible to `get`, `origin`, `explain` and `keys`, so a root struct with `#[serde(deny_unknown_fields)]` must declare an `oxbloom` field or be bound below the root (for example `config.get("server")`).

### Profiles

- Active profiles come from the first of these that is non-empty: `.profile("a")` calls (repeatable, applied in call order, later wins), `--oxbloom.profiles.active=a,b`, `OXBLOOM_PROFILE=a,b`, `oxbloom.profiles.active` in `application.toml` (a string `"a,b"` or an array). Names allow only letters, digits, `_` and `-`.
- `oxbloom.profiles.active` in a profile file is `ConfigError::Invalid`. `oxbloom.profiles.active` and `oxbloom.profiles.group` are read from `application.toml` and its imports. The environment variable `APP_OXBLOOM__PROFILES__ACTIVE` and the indexed form `--oxbloom.profiles.active[0]=a` do not activate profiles; they only set the key.
- Groups: `oxbloom.profiles.group.prod = ["db", "metrics"]` in `application.toml` (or its imports) activates `db` and `metrics` after `prod`. Group definitions elsewhere (profile files, environment) are ignored. The group's own file (`application-prod.toml`) is required like any profile file: a missing file is `ConfigError::Io`. A group that includes itself is `Invalid`. Groups nest at most 8 levels deep (`Invalid` for `oxbloom.profiles.group.<name>`), and at most 64 profiles are active after expansion, repeats counted once (`Invalid` for `oxbloom.profiles.active`); both limits apply to every way of naming profiles.

### Imports

`oxbloom.config.import = ["extra.toml", "optional:local.toml"]` in any config file (including profile files). Paths are relative to the config directory; an absolute path or `..` is `ImportInvalid`, and so is a file that resolves (symlinks included) outside the directory. A missing file is `ImportMissing` unless prefixed `optional:`; a cycle is `ImportCycle`; more than 8 levels is `ImportTooDeep`; more than 256 entries followed in one load (repeats count) is `ImportLimit`. Imported values override the importing file, later entries win, and `oxbloom.config.import` itself is not part of the final tree. `application.toml` and the profile files themselves are not symlink-checked, only imports are. The `Debug` output of the `Import*` errors contains the path as the developer wrote it; `Display` does not.

### Environment

`APP_SERVER__PORT=9000` sets `server.port`. The prefix is `APP` (change it with `.env_prefix("MYAPP")`); `__` separates segments, a single `_` stays in the key name. A numeric segment after the first is a list index (`APP_HOSTS__0=a`), so a map with numeric keys can only come from files. A variable name with more than 64 segments is `Invalid`, and so is mixing an index and a key under one path in the same source. Keys are lowercased and match relaxed. Values are strings. Two spellings of one key in the environment or on the command line are not reported; they collapse to one key and the last variable name in byte order (command line: the last argument) wins.

### Command line

`.args(iter)` reads `--key=value`. `.` separates segments, `[n]` or `.n` is a list index (`--hosts[0]=a`, `--hosts.1=b`). Ignored: arguments not starting with `--`, without `=`, or with an empty key (`--verbose`, `--key value`, `--=x`); `--key=` sets an empty string. A repeated key keeps the last value, and two spellings of one key collapse to one with the last argument winning (not reported). The same 64-segment cap and mixed index/key rule apply as for the environment. Command-line arguments are visible in process listings, so `.secret_key("security.jwt.secret")` registers keys (and everything below them) that must not come from the command line; such an argument is `Invalid` and points to environment variables or files. The same check applies to values moved there by `renamed`.

### Lists

- Comma-separated (a string bound to a `Vec`, from any source): `"a, b"` gives `["a","b"]`. Items are trimmed. A backslash is special only directly before a comma: a value with one backslash before a comma (`a\,b`) is one item `a,b`; a value with two (`a\\,b`) is one item `a\,b`. Mind the quoting of the source: the TOML basic string `"a\\,b"` has a one-backslash value, so it gives `a,b`, as does the literal string `'a\,b'` and the environment variable `a\,b`; to get the item `a\,b` write `"a\\\\,b"`, `'a\\,b'` or `a\\,b`. A trailing comma gives a trailing empty item (`"a,b,"` is `["a","b",""]`, which fails for `Vec<u16>`). An empty or blank string is an empty list. A string bound to a `String` field is never split. The tree keeps the string, so a placeholder cannot index into it (`${hosts.0}`).
- Indexed: `APP_HOSTS__0=a`, `--hosts[1]=b`. Indices start at 0; a gap is `Invalid`. Lists of structs work (`APP_SERVERS__0__HOST=a`).
- A list from a higher-precedence source replaces the lower one whole; indexed entries do not merge with a list from a file.

### Units

- `std::time::Duration`: `ns`, `us`, `ms`, `s`, `m`, `h`, `d`, fractions (`1.5s`), a bare number is seconds. Negative values, overflow and unknown units are `Invalid`; digits below one nanosecond are truncated. A user struct literally named `Duration` with the fields `secs` and `nanos` is treated as `std::time::Duration`. The table form `{ secs = 3, nanos = 0 }` still works.
- `DataSize` (`oxbloom_config::DataSize`): `KB`, `MB`, `GB`, `TB` are 1000-based, `KiB`, `MiB`, `GiB`, `TiB` are 1024-based (this differs from Spring Boot, where `KB` is 1024), `B` or a bare number is bytes. Integers only (`1.5KB` is `Invalid`). `bytes()` returns the count; `Display` prints `{bytes}B`.

### Placeholders

`${name}`, `${name:default}` (the default may contain placeholders). A name is looked up as a config key first, then as an environment variable of that exact name (the process environment, or the pairs given to `env_from`; `Config::from_str` has no environment). `$${` is a literal `${`. Placeholders are resolved only in values from files (`application.toml`, profile files, imports, `Config::from_str` text). Environment and command-line values are taken literally: `${` and `$${` in them stay as written, and a placeholder that refers to such a value gets its text as is. Resolution runs once over every file string leaf; the result stays a string until it is bound, so `port = "${PORT:8080}"` binds to a `u16`. No nested names (`${${x}}`). An unknown name without default is `UnresolvedPlaceholder`; a cycle, or nesting deeper than 32 (leaf references and defaults share the limit; a chain of 32 placeholder leaves ending in a plain one resolves) is `PlaceholderCycle`; a result over 1 MiB per leaf or 16 MiB per call is `PlaceholderTooLarge`. Errors name the key and the placeholder name, never a resolved value. That name is the one exception to "errors never contain values": it is text from a file value, capped at 32 characters plus a trailing `…` when cut, and for an unterminated `${` it is the text after the `${` (never anything after a `:`). Write `$${` for a literal `${` in files, especially in values that may be secrets. Config files are trusted: a placeholder can read any environment variable. A resolved leaf keeps the origin of the leaf that contains the placeholder, so a secret that reaches a key through a placeholder is not seen as command-line input by origin checks.

### Migration

```rust
let config = Config::builder()
    .renamed("server.hostname", "server.host")
    .removed("legacy.mode", "no longer supported")
    .build()?;
for warning in config.warnings() { eprintln!("{warning}"); }   // never contains values
```

An old key moves to the new one and keeps its origin; if both are set the new one wins. Either way a `ConfigWarning` is recorded; a removed key stays in the tree and warns. `.deny_deprecated(true)` turns the first warning into `ConfigError::Deprecated`. Rules are checked in `build()`: an empty or malformed path, old equal to new (canonically) or one inside the other is `Invalid`. Tables emptied by a move are removed. Rules apply in registration order, so chains work. Migration runs before placeholders, so `${old}` after a rename is unresolved. A renamed command-line value that would land on a registered secret key is rejected.

### Introspection and errors

- `config.origin("server.port")` returns the `Origin` of a leaf (file and line, `line N` for `from_str`, `env APP_X`, `--key`). Tables and arrays as a whole have no origin.
- `config.explain()` lists every leaf as `{ key, origin, kind }`; `config.keys("db")` lists leaf keys under a prefix. Neither ever returns values. Array elements are `list[0]`. Sorting is by bytes, so `a[10]` comes before `a[2]`. A quoted key containing `.` is listed but cannot be reached through `origin` or `keys`.
- Errors name the key, the origin and the reason, never the value; the one exception is the placeholder name of a file value (see Placeholders): ``invalid configuration key `server.port` (from env APP_SERVER__PORT): expected unsigned integer``. A failing list item is named `hosts[1]`.
- `Secret<T>` prints `[REDACTED]` in `Debug` and `Display`; read it with `expose()`. Messages from a custom `Deserialize` impl are replaced by a fixed text. `Config`'s `Debug` prints top-level key names only.
- Environment values are strings. `get` parses them into the integer, float or bool a field expects. Fields typed as strings are never coerced.
- With the `di` feature, `ContainerBuilder::config(config)` registers `Config` as a singleton, and components can take an `Arc<Config>` field.

### Behavior changes from earlier versions

- Keys in files now match case-insensitively and ignore `-`/`_`.
- A plain `Duration` field parses strings such as `30s`, and a comma-separated string binds to a `Vec` field.
- `${` in a string value from a file is special. A literal `${` must be written `$${`, otherwise loading fails with an error naming the key. Environment and command-line values are taken literally.
- Environment variables can now set arrays (indexed or comma-separated).
- `ConfigError` gained variants and `Invalid` gained the `origin` field. `ConfigError`, `Origin` and `WarningKind` are `#[non_exhaustive]`: a `match` on them needs a `_` arm, and `Invalid { key, message }` must be destructured with `..`.
- A numeric environment or command-line path segment is a list index: `APP_HTTP__STATUS__404=x`, which used to set the map key `404`, is now an `Invalid` gap error. Numeric map keys still work from files.
- Map keys are canonical: `my-host` and `my_host` in one table of one file are `DuplicateKey`, and an environment override replaces a map key's spelling with the lowercased variable spelling.
- `oxbloom` is the one reserved top-level table: `oxbloom.config.import`, `oxbloom.profiles.active` and `oxbloom.profiles.group.*` are interpreted (see Keys), and `oxbloom.profiles.*` stays in the final tree.
- Migration: the reserved keys used to be `config.import`, `profiles.active`, `profiles.group.<name>` and `--profiles.active=`. Those old names are no longer interpreted and silently become plain data (no error, no profile is activated, no file is imported); rename them to the `oxbloom.`-prefixed names if you followed older notes.
- Configuration files must be regular files of at most 1 MiB by default.
- `OXBLOOM_PROFILE` accepts a comma-separated list (one name behaves as before).
- Two spellings of one key in a single file are now `DuplicateKey` instead of serde's "duplicate field" error.

## Application lifecycle

`oxbloom-boot` is the single entry point of an Oxbloom application. `OxbloomApplication::builder()` loads the configuration, builds the container, runs the runners, starts the services, waits for a shutdown signal, stops everything in order and returns an exit status. It holds no global state, so two applications can run in one process, for example in parallel tests.

```rust
use std::process::ExitCode;
use std::sync::Arc;
use oxbloom_boot::{OxbloomApplication, Service, WebServer};
use oxbloom_config::Config;
use oxbloom_di::DiError;
use oxbloom_web::App;

fn server(config: Arc<Config>) -> Result<WebServer, DiError> {
    // `?` keeps the `ConfigError` as the source, for the failure analyzers.
    Ok(WebServer::new(App::new(), &config)?)
}

#[tokio::main]
async fn main() -> ExitCode {
    OxbloomApplication::builder()
        .args(std::env::args().skip(1))
        .config(|b| b.dir("config"))
        .configure(|b| b.try_bean(server).provides(|s: Arc<WebServer>| -> Arc<dyn Service> { s }))
        .run_and_exit()
        .await
}
```

### Run order

`run()` performs these steps in a fixed order:

1. It parses the arguments into `ApplicationArguments` and fires `starting`.
2. It builds the `Config` (the arguments are applied to the `ConfigBuilder` after the `config` closure, so `--key=value` overrides and `--oxbloom.profiles.active=dev` always work), fires `environment_prepared`, installs the logging subscriber (only with the feature `logging`, see [Logging](#logging)) and prints the banner.
3. It builds the container. Boot first sets `Config` as the container environment of the conditions, registers `Config`, `ApplicationArguments` and `ShutdownHandle` as instances and passes the active profiles of the configuration to the container, then applies the `configure` closures in call order. It then resolves the order of the auto-configurations and appends each one as a group (an ordering cycle fails here), builds the container and, with `oxbloom.main.conditions-report = "console"`, prints the conditions report. It collects the `RunListener` beans and fires `context_ready`.
4. It runs every `ApplicationRunner` bean, then every `CommandLineRunner` bean, in DI order (`order`, then registration order).
5. It collects the `ExitCodeGenerator` beans. If there is at least one `Service` bean, it installs its signal handlers, then starts the services in ascending `phase`, ties in DI order.
6. It fires `started`, then `ready`.
7. If at least one service started, it waits for Ctrl-C, SIGTERM (Unix) or a `ShutdownHandle` trigger. With no service it does not wait, which makes a command-line tool return right after its runners.
8. It fires `stopping` and stops the started services in reverse start order under one shared deadline, `shutdown_timeout` (default 30 seconds).
9. It calls `Container::shutdown()`, which runs the `pre_destroy` hooks.
10. It computes the exit code and fires `stopped`.

A failure in any step first asks the failure analyzers (see [Failure analysis](#failure-analysis)) and attaches the first analysis to the error, then fires `failed` once, stops the services started so far in reverse order, shuts the container down if it was built, and returns `Err(StartupError)`. `StartupError::phase()` is `Config`, `Container`, `Runner` or `Service`, and `source()` is the underlying `ConfigError`, `DiError`, `RunnerError` or `ServiceError`, so you can downcast it, or use `find_cause` to reach an error further down the chain.

### Services, runners and listeners

- A `Service` is a long-running part of the application, such as a server or a worker. Register a bean that `provides` `dyn Service`. `start` should return once the service runs and spawn its long-running work; `stop` runs in its own task. A `stop` that is still running at the deadline is abandoned (its task is aborted) and its name is written to the output; a service reached after the deadline is not stopped at all. A `stop` that panics is reported, and the remaining services still stop. `phase()` defaults to 0; lower phases start first and stop last.
- An `ApplicationRunner` receives the parsed `ApplicationArguments`, a `CommandLineRunner` the raw argument list. Both run once, before the services start. An error fails the run in the `Runner` phase.
- A `RunListener` observes the run. Builder listeners (`.listener(..)`) receive every event; `RunListener` beans in the container receive `context_ready` onward, after the builder listeners. Every method has an empty default.
- Implement the async traits with `#[oxbloom_boot::async_trait]`, which boot re-exports, so you need no `async-trait` dependency of your own.

### Exit codes and signals

`run()` returns the highest `exit_code()` of the `ExitCodeGenerator` beans, or 0 with no generator. Each code is mapped to a process exit code before the maximum is taken: 0..=255 stays as is, a negative code or one above 255 becomes 255. So a generator returning -1 makes the run exit with 255 even when another returns 0. `run_and_exit()` turns the status into a `std::process::ExitCode`, and into 1 after a startup failure, for which it writes the failure report to the output target first (with `failure_report(false)`, the error's message as one line instead). Use it as the tail of `main`.

Signal handlers are installed from just before the services start (step 5) until the stop sequence ends (step 8), and only when there is a service. A Ctrl-C or SIGTERM while a service is still starting is kept: the services finish starting, the wait ends at once and they stop gracefully. A Ctrl-C or SIGTERM that arrives while the services stop forces the exit: the rest of the stop sequence is abandoned, `Container::shutdown()` is skipped, `stopped` fires and the code is 130 for Ctrl-C or 143 for SIGTERM. This also applies to the first signal after a `ShutdownHandle` trigger. `ShutdownHandle::trigger()` is the portable way to end a run without a signal; a trigger before step 7 is remembered, so a runner can end the application before it waits. Inject it as `Arc<ShutdownHandle>` or get it from `OxbloomApplicationBuilder::shutdown_handle()`.

### Banner and output

Boot writes a banner after `environment_prepared`: by default the Oxbloom flower and wordmark in ASCII art, with `:: Oxbloom ::` and `(v{version})` on the last line. When the output is the default stderr and it is a terminal, the default banner is colored (petals and name in oxide, version in grey); `NO_COLOR` turns the colors off, and a redirected `.output(..)` always gets plain text. Change it with `.banner(Banner::text(..))` or turn it off with `.banner(Banner::off())`. The banner, the failure report and the few diagnostics boot writes (a service that failed to start, abandoned or panicking stops, panicking listeners and analyzers, failed `pre_destroy` hooks) go to stderr by default; `.output(..)` redirects them. No message produced by boot contains configuration values, argument values or secrets, except the port-in-use failure report, which shows the socket address; `Debug` and `Display` of `ApplicationArguments` and `Debug` of the builder show counts only.

### Feature `web`

With the cargo feature `web`, `WebServer` is a `Service` that serves an `oxbloom_web::App`. Its `phase()` is `i32::MAX`, so it starts after every other service and stops before them. `stop` stops accepting connections and waits until the open ones have drained, at most `server.shutdown-timeout` (default 30 s); then the remaining connections are dropped. `WebServer` is a cheap `Clone` handle, and `local_addr()` returns the bound address after `start`, which makes `server.port = 0` usable in tests. A failed bind fails the run in the `Service` phase with a `ServiceError` whose source is a `BindError`; `BindError::address()` and `kind()` return the address it tried and the `std::io::ErrorKind`, while its `Display` and `Debug` show neither the address nor the port.

### Failure analysis

When startup fails, boot asks a list of `FailureAnalyzer`s to explain the failure. The first `FailureAnalysis` it gets is attached to the `StartupError` (`error.analysis()`) and printed as a report with a description and, when the analyzer knows one, an action:

```
***************************
APPLICATION FAILED TO START
***************************

Description:

The web server could not listen on `127.0.0.1:8080` because the port is already in use.

Action:

Stop the process that uses the port, or set `server.port` to a free port (0 picks one).
```

A failure that no analyzer recognises gets the same block with the error's message as the description and no Action section.

Analyzers are asked in this order, and the first `Some` wins: the builder analyzers (`.failure_analyzer(..)`, in call order, for failures in every phase), then the beans that provide `dyn FailureAnalyzer` (in DI order, only when the container was built; a failure to create them is ignored), then the built-in analyzers in the order of the table below. Each call is panic-isolated: a panicking analyzer is skipped and named in the output as `analyzer {name} panicked`. Eager bean creation and wiring failures happen inside `build()`, before any container exists, so register analyzers meant for them on the builder with `failure_analyzer`. If creating the analyzer beans fails, all bean analyzers are dropped silently; if it panics, boot writes the line `failure analyzer beans panicked` and also goes on without them.

| Built-in analyzer | Matches | Description |
|---|---|---|
| Missing configuration key | `ConfigError::Missing { key }` | "The required configuration key `<key>` is not set." The action names `application.toml`, the profile file, the environment variable and `--<key>=<value>`. |
| Bean wiring | `DiError::Cycle`, `Missing`, `Ambiguous`, `Duplicate` | The cycle path; the bean that needs a missing type; the candidates of an ambiguous type; the sources of a duplicate. Each with an action, such as "Mark one of them `primary`, or inject `Vec<Arc<..>>`." |
| Port in use (feature `web`) | a `BindError` of kind `AddrInUse` | "The web server could not listen on `<address>` because the port is already in use." Other bind errors get the generic report. |
| Security (feature `security`) | every `SecurityBuildError` variant | For example "`authorize` was called more than once." with "Put all rules in one `authorize` call." |

The features are those of `oxbloom-boot`: `web` adds `WebServer` and the port analyzer, and `security` enables `oxbloom-security` with its `di` feature and adds the security analyzer. The exact texts are listed in the design, `docs/superpowers/specs/2026-10-25-boot-failure-analysis-design.md`, section "Built-in analyzers".

A custom analyzer recognises its own error with `StartupError::find_cause::<E>()`, which walks the `source()` chain (at most 64 levels) and returns the first error of type `E`:

```rust
struct QuotaAnalyzer;

impl FailureAnalyzer for QuotaAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        failure.find_cause::<QuotaExceeded>()?;
        Some(FailureAnalysis::new("The API quota is used up.", "Raise the quota."))
    }
}

OxbloomApplication::builder().failure_analyzer(QuotaAnalyzer)
```

For `find_cause` to reach an error raised inside a bean, the error must travel as a typed source: return `DiError::creation_from::<T>(error)` from the factory, or use `?` on a `ConfigError` or `SecurityBuildError` (see [Errors](#errors)). A factory that turns the error into a string with `DiError::creation::<T>(e.to_string())` loses it.

Printing: `run()` prints the report only with `.failure_report(true)`. `run_and_exit()` prints it by default, and with `.failure_report(false)` writes the bare error line instead; it never writes both. The report is written once, after the `failed` listeners ran and cleanup finished, as the last step before the error is returned.

Limits:

- Only `try_bean` and `#[bean(fallible)]` rewrite the type name of a `Creation` error to the failing bean's type. A `factory` or `Component::create` that uses `?` on a `ConfigError` is not rewritten: its error is labelled with `ConfigError`'s own type name. Use `try_bean` there, or `map_err(DiError::creation_from::<T>)` inside a `factory`.
- Analyzers only see startup failures. A failure after the application started (a crashed serving task, an error in a handler) is not analyzed.
- The port-in-use report shows the socket address. It is the one report that contains a configuration value, by design; `BindError`'s `Display` and `Debug` do not show it.
- The bad-pattern report cannot show the offending pattern, because `SecurityBuildError::InvalidPattern` does not carry it.
- The description and action are printed as given. An analyzer must not put the block markers (the line of asterisks or `APPLICATION FAILED TO START`) into them, and must not include configuration values, argument values or secrets. This is documented, not enforced.
- `find_cause::<DiError>()` returns the first `DiError` in the chain, which for a bean factory is the `Creation` error; a wiring error nested below a `Creation` error would not be analyzed by the bean-wiring analyzer. Nothing in Oxbloom produces that shape today.
- Boot still writes its diagnostic line `service X failed to start` before the report when a service fails to start.
- The default panic hook still prints the payload of a panicking analyzer to stderr; Oxbloom itself never prints it.

### Configuration keys

| Key | Meaning |
|---|---|
| `oxbloom.main.conditions-report` | `console` prints the conditions evaluation report right after the container is built, `off` (the default) prints nothing. Any other value fails the run in the `Config` phase; the error names the key, not the value. |
| `oxbloom.main.banner-mode` | `console` prints the banner (the default banner when the builder turned it off), `off` prints nothing. Any other value fails the run in the `Config` phase. |
| `application.name` | The application name in the startup events ("Starting demo"). Default `application`. Read only with the feature `logging`. |
| `oxbloom.logging.enabled` | `false` skips installing the logging subscriber. Default `true`. A value that is not a boolean fails the run in the `Config` phase. Read only with the feature `logging`. |
| `server.address` | The IP address `WebServer` binds to. Default `127.0.0.1`. A host name is a `ConfigError`. |
| `server.port` | The port `WebServer` binds to. Default `8080`; `0` picks a free port. |
| `server.shutdown-timeout`, `server.max-body-bytes`, `server.header-read-timeout`, `server.forward-headers.strategy`, `server.forward-headers.trusted-proxies`, `server.error.include-message` | Limits, timeouts, forwarded-header handling and the error switch. Defaults and rules are in [Server properties](#server-properties). Read only with the feature `web`. |

Set them like any other key, for example `--server.port=9000` on the command line or `APP_SERVER__PORT=9000` in the environment.

### Limits

- A panic in a runner or in `Service::start` is not caught. It propagates out of `run()`, and the services started before it are not stopped. Only a panic in `Service::stop` is caught.
- Listener methods are synchronous and must not panic. A panic is caught and reported as `listener {name} panicked`, but the process-wide panic hook still prints the panic to stderr, and the remaining callbacks of that listener for the event are skipped.
- A Ctrl-C or SIGTERM while services stop forces exit code 130 or 143 and skips `Container::shutdown()`, so no `pre_destroy` hook runs.
- Once a service has run, Tokio's process-wide signal handlers stay installed for the rest of the process, so a later Ctrl-C or SIGTERM no longer terminates the process by default.
- Whenever a service is registered, boot needs a Tokio runtime with the time and IO drivers, such as `#[tokio::main]` or a runtime builder with `enable_all()`: boot installs its signal handlers just before the first service starts.
- Arguments are `String`. `std::env::args()` panics on arguments that are not valid UTF-8, so a program that must survive them passes `std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned())` instead.
- `ExitCodeGenerator` beans are collected before the services start, so a lazy generator bean is created at that point, not at exit time.
- A `stop` that exceeds `shutdown_timeout` is abandoned and its task is aborted. A service can therefore not hang `run()` as long as `stop` does not block the thread, but it may not have released its resources.
- The deadline is enforced by the Tokio timer, so it cannot interrupt a `stop` that blocks the thread: a blocking `std::thread::sleep` in `stop` on a current-thread runtime delays `run()` for as long as it blocks.
- The `pre_destroy` hooks of step 9 are not bounded by the deadline, and `Service::start` has no timeout.
- Dropping the `run()` future, or losing it in a `select!`, leaves the started services running and the container is never shut down. End a run with `ShutdownHandle::trigger()` instead.
- The Windows signal branch (Ctrl-C only, there is no SIGTERM) is compiled only on Windows and has not been verified.

## Conditions and auto-configuration

A condition keeps a registration only when it holds. An auto-configuration is a `Configuration` that a crate ships and an application adds as a unit: its beans are registered after the application's own wiring, so a default guarded by `on_missing_bean` steps aside when the application provides its own bean. There is no classpath scanning and no global state: auto-configurations come from explicit lists.

```rust
use std::sync::Arc;
use oxbloom_boot::{OxbloomApplication, auto_configuration};
use oxbloom_di::component;

pub trait Cache: Send + Sync {}
pub struct MemoryCache;
pub struct Metrics;
impl Cache for MemoryCache {}

#[component]
pub struct CacheAutoConfig;

// Only with `cache.enabled` set; the default backs off when the application has a `dyn Cache`.
#[auto_configuration(on_property("cache.enabled"))]
impl CacheAutoConfig {
    #[bean(provides(dyn Cache), on_missing_bean(dyn Cache))]
    fn memory_cache(&self) -> MemoryCache {
        MemoryCache
    }
}

OxbloomApplication::builder()
    .auto_configuration::<CacheAutoConfig>()
    .configure(|b| b.instance(Metrics).on_property_value("metrics.enabled", "true"))
```

### Conditions

A condition is a modifier on the last registration, like `profile` or `lazy`:

| Modifier | Keeps the registration when |
|---|---|
| `.on_bean::<T>()` | a registration kept before it provides `T` (its concrete type, or `dyn Trait` through `provides`) |
| `.on_missing_bean::<T>()` | no registration kept before it provides `T` |
| `.on_property("key")` | the environment has the property, whatever its value, also `false` |
| `.on_property_value("key", "value")` | the property equals `value` exactly (case-sensitive, no trimming, no substring match) |
| `.on_property_value_or_missing("key", "value")` | the property equals `value`, or is absent |
| `.condition("name", \|cx\| ..)` | the closure returns `ConditionOutcome::matched()`; it gets a `ConditionContext` with `has_bean::<T>()`, `property(key)`, `active_profiles()` and `registration()` |

`#[bean]` methods take the same conditions as options: `on_bean(T)`, `on_missing_bean` (the method's own bean type, as in Spring), `on_missing_bean(T)`, `on_property("key")` and `on_property("key", "value")`.

Bare `on_missing_bean` guards the method's concrete return type only, also next to `provides(dyn Trait)`. With `#[bean(provides(dyn Cache), on_missing_bean)]` on a method that returns `MemoryCache`, an application that provides `dyn Cache` through its own `RedisCache` does not make the default back off: both beans stay, and a single `Arc<dyn Cache>` request is `Ambiguous`. To back off for the trait, write `on_missing_bean(dyn Cache)`.

Differences from Spring Boot's `@ConditionalOnProperty`:

- `on_property("key")` matches whenever the key is present, also when its value is `false`. For an on/off switch, write `on_property_value("key", "true")` (in `#[bean]` and `#[auto_configuration]`: `on_property("key", "true")`).
- `on_property_value` compares exactly and case-sensitively, so `TRUE` does not match `"true"`, unlike Spring's `havingValue`.

The rules:

- `build()` evaluates the conditions once, after profile filtering and before any provider runs, in registration order. Each registration is decided against the registrations kept before it: never itself, never a later one. So `on_bean::<Later>()` written before `Later` is registered does not match, and the report says so. Registrations skipped by a profile or by another condition do not count.
- Several conditions on one registration are combined with AND. Profile conditions are combined with OR, so `.profile("dev").profile("test")` means dev or test, while `.on_property("a").on_property("b")` means both. The conditions run in the order they were added, and evaluation stops at the first one that does not match: the later ones are neither run nor recorded.
- A skipped registration is invisible, exactly like one skipped by a profile: it is not a candidate for any key, it never causes `Duplicate`, a dependency on it is `Missing`, and its provider and hooks never run.
- A condition modifier before any registration, and a property condition with an empty key (on the builder or in `Conditions`), make `build()` return `Invalid`, with a text such as "`on_property` needs a non-empty key" that never contains a value.
- The reason of a `ConditionOutcome` appears in the report. Build it from fixed text, type names and key names, never from configuration values. The built-in conditions follow this rule: `on_property_value` records `property "<key>" does not have the expected value` and shows neither value.

### Properties

The property conditions read an `Environment`, a trait with two methods. `property(key) -> Option<String>` returns the text of a scalar value, and `contains(key) -> bool` says whether the key is present at all; its default is `property(key).is_some()`. `on_property` asks `contains`, and the value conditions ask `property`. Set it with `ContainerBuilder::environment(..)`; without one, every property is absent. With the `di` feature of `oxbloom-config`, `Config` implements `Environment`: it finds the key with the relaxed rules of `Config::get` and returns strings as they are, integers in decimal, floats in TOML form, booleans as `true` or `false` and dates in their TOML text form; tables, lists and absent keys are `None`. Its `contains` also counts a table or a list as present, so `on_property("security.jwt")` matches a `[security.jwt]` section, while `on_property_value` still needs a scalar. On a table or a list, `on_property_value_or_missing` still treats the key as not set, so it matches its default; only `on_property` counts a section as present. Property values are compared as text, so the file integer `port = 8080` and the environment or command-line string `8080` both equal `"8080"`. A secret is compared as plain text and never shown. The expected value of `on_property_value(key, value)` sits in the source code and is compared as plain text, so never put a secret there.

Boot sets `Config` as the environment before the `configure` closures run. This is a visible change: a property condition in a `configure` closure now reads the configuration without any extra call, and a closure that calls `environment(..)` replaces `Config` for every condition, those of the auto-configurations included.

### Groups

`group(name, conditions, f)` applies class-level conditions to every registration that `f` makes. `Conditions` holds the same conditions as the builder (`Conditions::new().on_missing_bean::<dyn Cache>()`).

- The group's conditions are decided once, at the group's first registration that a profile keeps, against the registrations kept before it. The group's own registrations do not count, so a group can register the `T` that its `on_missing_bean::<T>()` asks about, and its beans never hide each other.
- When they do not match, every registration of the group is skipped, nested groups included. When they match, each registration's own conditions are evaluated as usual and combined with the group's by AND.
- Nested groups combine their conditions by AND, the outermost decided first. A group with no registrations is a no-op. A group whose registrations are all skipped by profiles is never decided: its conditions do not run and it leaves no record.
- The start of a group is a barrier for modifiers. A modifier written inside `f` before its first registration finds no registration, so it never changes the registration before the group, and `build()` returns `Invalid`. A modifier written after `group(..)` applies to the last registration made inside it.
- `f` should extend the builder it is given. If it returns a different builder, that builder is used as it is and the group has no effect.

### The conditions report

`build_with_report()` returns `(Result<Container, DiError>, ConditionReport)`, also when the build fails, and `Container::condition_report()` returns the report of a successful build. `records()` lists one `ConditionRecord` per evaluated condition, in evaluation order, with `registration()`, `condition()`, `matched()` and `reason()`. A group's records use the group's name as registration. A registration skipped by a profile has no record.

In an application, set `oxbloom.main.conditions-report` to `console`, for example with `--oxbloom.main.conditions-report=console`, and boot prints the report to the output target right after the container is built:

```
CONDITIONS EVALUATION REPORT

Matched:
  hello::auto::GuestAutoConfig: on_property - property "hello.greeting" is set
  hello::auto::GuestAutoConfig::standard_guest_greeter: on_missing_bean - no earlier registration provides `dyn hello::auto::GuestGreeter`

Skipped:
  closure at examples/hello/src/auto.rs:77: on_property_value - property "hello.custom-greeter" is not set
```

A section without records prints `  (none)`. Records are sorted into the sections by their own result: a registration whose first condition matched and whose second did not is skipped, yet its first record is listed under `Matched:`. The setting is `off` by default. Any other value than `off` or `console` (also `Console`, or a list) fails the run in the `Config` phase with an error that names the key but not the value; boot reads the setting before it prints the banner, so a rejected value prints nothing. The report is printed before `context_ready` and the runners, and also when the build fails, before `failed` and the failure report. A build that fails with `Invalid` (builder misuse) evaluates no condition, so its report shows `(none)` in both sections.

### Auto-configuration

An auto-configuration is an `impl` block with `#[auto_configuration(..)]` (import it from `oxbloom_boot`). It implies `#[configuration]`, so its `#[bean]` methods work as usual, and it implements `AutoConfiguration` with three methods: `conditions()` (the class-level conditions), `after()` and `before()` (the order). The struct must also be a `Component`, usually with `#[component]` (a unit struct works; fields are injected): boot registers its instance as the first registration of the configuration's group, so the bean methods get `&self` and the instance is skipped together with the group. The generated code names `::oxbloom_boot` and `::oxbloom_di`, so the crate must depend on both under those names. `AutoConfiguration` can also be implemented by hand on a type that implements `Configuration` and `Component`.

| Attribute option | Meaning |
|---|---|
| `after(A, B)` | `A` and `B` are processed before this configuration |
| `before(C)` | this configuration is processed before `C` |
| `on_bean(T)`, `on_missing_bean(T)`, `on_property("key")`, `on_property("key", "value")` | class-level conditions, decided once for all beans of the configuration; `on_missing_bean` needs its type here |

A crate that ships auto-configurations exposes them as a list, and the application adds lists and single configurations to its builder:

```rust
pub fn auto_configurations() -> AutoConfigurations {
    AutoConfigurations::new().add::<DataSourceAutoConfig>().add::<CacheAutoConfig>()
}

OxbloomApplication::builder()
    .auto_configurations(my_starter::auto_configurations())
    .auto_configuration::<MyOwnAutoConfig>()
    .exclude::<CacheAutoConfig>()
```

- Lists combine in call order. A configuration appears once: adding one again keeps its first position.
- Boot registers the auto-configurations after every `configure` closure, each as one group named by its type name with its class-level conditions. User registrations therefore come first, which gives back-off: a default with `on_missing_bean` skips itself when the application registered its own bean.
- `A::after() = [B]` and `B::before() = [A]` both mean that B is processed before A. The order is a stable topological sort with a fixed tie rule: among the configurations whose predecessors are all placed, the one earliest in the list goes next. For the list `[P, Q, R, S]` with S before Q, the order is P, R, S, Q. This differs from Spring Boot, whose sorter would move S right in front of Q and give P, S, Q, R.
- A reference to a configuration that is not in the list, and a reference of a configuration to itself, are ignored.
- `exclude::<A>()` drops `A` from the combined list, whatever the call order; `after` and `before` entries that name it are ignored, so the order of the rest stays valid. Excluding a configuration that is not in the list does nothing. `A` must implement `AutoConfiguration`. To replace a starter's default with your own wiring, exclude it and register your bean in `configure`.
- Put overrides in `configure`, not in an auto-configuration of your own. Auto-configurations are registered in list order, so `MyAuto` added with `.auto_configuration::<MyAuto>()` after the starter's list comes after the starter's default: when the default's `on_missing_bean` is decided, your bean is not registered yet, so the default is kept, and your bean is kept too. The result is a `Duplicate` for the same concrete type, or two providers of the same trait, so a single `Arc<dyn Trait>` request is `Ambiguous`. A `configure` closure runs before every auto-configuration, so the default backs off. `after(StarterAutoConfig)` on `MyAuto` does not help, because it keeps `MyAuto` later; `before(StarterAutoConfig)` would put it first, but `configure` is the simpler and clearer place.
- An ordering cycle fails the run in the `Container` phase before the container is built, with the fixed text `auto-configurations form an ordering cycle: A -> B -> A`, where each arrow points from a configuration to one it must come after. A built-in failure analyzer, separate from the bean-wiring one, prints the cycle with the action "Remove one `after` or `before` entry so that the order is no longer circular." The text names types only. Because the container is never built, no conditions report is printed.
- There is no `on_feature` condition. A cargo feature is decided at compile time: put `#[cfg(feature = "..")]` on the bean method or on the `.auto_configuration::<..>()` call.

### Auto-configured web and security

`oxbloom-boot` ships three auto-configurations, behind its `security` and `web` features, that wire the pieces the framework already has. `oxbloom_boot::auto_configurations()` returns the list for the enabled features, in dependency order: `JwtDecoderAutoConfiguration` and `SecurityAutoConfiguration` with `security`, then `WebServerAutoConfiguration` with `web`. Add the list to the builder with `.auto_configurations(oxbloom_boot::auto_configurations())`. Nothing is wired until you do, and without either feature the function is not available.

You contribute to the defaults with beans, so no code of yours names the server or the chain:

- **Routes.** Register every controller as a `dyn Controller`: `.provides(|c: Arc<HelloController>| -> Arc<dyn Controller> { c })`. `oxbloom_web::Controller` now has `Send + Sync` as supertraits, which `#[controller]` types already satisfy. The web server is built from all controllers; with no `dyn Controller` bean there is no `WebServer` and no `Service`, so `run()` returns right after the runners. A route declared twice fails startup in the `Container` phase with a `RouteError` as the cause.
- **Rules.** Implement `SecurityCustomizer` (`fn customize(&self, http: HttpSecurity) -> HttpSecurity`) and register it as `dyn SecurityCustomizer`. Customizers run in the order the container gives them (ascending `order`, then registration order), each on the result of the one before. `HttpSecurity::authorize` may be called only once in total, so one customizer should own the rules and others can adjust other parts, such as the entry point.
- **Filters.** Register extra filters as `dyn Filter` beans. Each filter declares its slot with `Filter::order()` (see [Errors and filter order](#errors-and-filter-order)); the server registers the security chain and your filters with `App::filter`, so a filter that keeps the default slot runs after the security chain and never sees a request that the chain rejects.

```rust
use oxbloom_boot::SecurityCustomizer;
use oxbloom_security::HttpSecurity;

struct PublicPaths;
impl SecurityCustomizer for PublicPaths {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| a.get("/api/hello/**").permit_all().any_request().authenticated())
    }
}

OxbloomApplication::builder()
    .auto_configurations(oxbloom_boot::auto_configurations())
    .configure(|b| {
        b.component::<HelloController>()
            .provides(|c: Arc<HelloController>| -> Arc<dyn Controller> { c })
            .instance(PublicPaths)
            .provides(|p: Arc<PublicPaths>| -> Arc<dyn SecurityCustomizer> { p })
    })
    .run_and_exit()
    .await
```

The chain is secure by default. With no customizer, `SecurityAutoConfiguration` builds a chain whose only rule is `any_request().authenticated()`, so every request gets a 401 until you open paths. If a customizer never calls `authorize`, building the chain fails with `NoAuthorizeSection` rather than leaving the server open. When a `dyn JwtDecoder` bean exists, the chain also enables `oauth2_resource_server` with it.

The JWT decoder is switched on by configuration. `JwtDecoderAutoConfiguration` applies only when the `security.jwt` section exists (the group condition is `on_property("security.jwt")`, and for `Config` a table counts as present). It then builds the decoder with `jwt_decoder_from_config`, using an `Arc<dyn JwksFetcher>` bean if you registered one. The keys are those of [Security configuration](#security-configuration). A section that is present but wrong, such as HMAC without a secret, fails startup in the `Container` phase, and the failure report names the key but never a value. A secret given on the command line is refused the same way.

To replace a default, register your own bean in `configure`; the user's registrations are processed before the auto-configurations, so the default backs off:

| You register | Default that backs off |
|---|---|
| a `SecurityFilterChain` | the chain of `SecurityAutoConfiguration`; your chain is the only one |
| a `WebServer` | the server of `WebServerAutoConfiguration` (provide your `WebServer` as a `dyn Service` too, so that boot starts it; the default one is provided that way) |
| a `dyn JwtDecoder` | the decoder of `JwtDecoderAutoConfiguration` |

With `oxbloom.main.conditions-report = "console"` the report lists the skipped default, with its `on_missing_bean` condition naming the type. To drop a whole auto-configuration instead, use `.exclude::<WebServerAutoConfiguration>()`, `.exclude::<SecurityAutoConfiguration>()` or `.exclude::<JwtDecoderAutoConfiguration>()`. Excluding the chain keeps the server, which then has no authentication; excluding the server leaves the chain unused.

Every auto-configuration also describes the keys it reads. `AutoConfiguration::properties()` returns a static `&[PropertyMetadata]` (default: none), and each entry has a `key`, a `type_name`, an optional `default` and a `description`. `OxbloomApplicationBuilder::property_metadata()` returns the keys of the boot itself (`boot_properties()`) plus those of the registered, not excluded auto-configurations, sorted by key. `PropertyMetadata::to_json(&list)` renders a list as a JSON array for documentation or editor tooling:

```rust
let list = OxbloomApplication::builder()
    .auto_configurations(oxbloom_boot::auto_configurations())
    .exclude::<JwtDecoderAutoConfiguration>()
    .property_metadata();
let json = PropertyMetadata::to_json(&list);
// The output is abbreviated here; the real list starts with the boot keys:
// [{"key":"oxbloom.config.import","type":"array of String","default":null,"description":"..."},
//  {"key":"oxbloom.main.banner-mode",...}, ..., {"key":"server.address",...},
//  {"key":"server.port",...}]
```

The metadata contains text about keys only, never values; the description of `security.jwt.secret` says so. The server reads `server.address` (default `127.0.0.1`, an IP address, not a host name), `server.port` (default `8080`) and the other `server.*` keys of [Server properties](#server-properties). Metadata is written by hand next to the code that reads the keys, and tests pin the two together; there is no generation from structs yet.

### Limits

- Order matters. A condition sees only the registrations kept before it, so `on_bean` and `on_missing_bean` depend on where a registration is made. Register the application's beans first; boot does this for auto-configurations.
- Conditions are ANDed, profile conditions are ORed.
- A condition that panics is not caught: the panic propagates out of `build()`, and out of `run()` in an application.
- Auto-configurations come only from explicit lists. There is no link-time discovery and no exclusion through configuration keys.
- Property values are compared as text, after the conversion described in [Properties](#properties).
- Condition options are not available on `#[component(..)]`; use the builder modifiers after `.component::<T>()`.
- The missing-bean failure report does not name registrations that a condition skipped; use the conditions report to find them.
- The conditions report and the failure report follow each other without a blank line between them.
- Boot keeps a second copy of the `Config` as the container environment.

## Logging

The crate `oxbloom-logging` builds a [`tracing`](https://docs.rs/tracing) subscriber from the `logging.*` configuration keys. With the cargo feature `logging` of `oxbloom-boot`, boot installs it for you, so an application needs no setup code. Events from crates that use the `log` facade reach the same subscriber and obey the same levels. The feature is off by default; without it boot is unchanged.

```toml
# application.toml
[logging.level]
root = "info"                 # the default level
"oxbloom_web::router" = "debug"   # a module path; a key with `::` needs quotes
web = "warn"                  # the level of the group `web` (see below)

[logging.group]
web = ["oxbloom_web", "oxbloom_security"]

[logging.console]
format = "compact"            # full (default), compact or pretty
color = "auto"                # auto (default), always or never
```

| Key | Meaning |
|---|---|
| `logging.level.root` | The default level for every target. Default `info`. |
| `logging.level.<target>` | The level of a module path such as `oxbloom_web`, or of a group name. |
| `logging.group.<name>` | A list of targets that share the level `logging.level.<name>`. |
| `logging.console.format` | `full`, `compact`, `pretty` or `json`. Default `full`. |
| `logging.console.color` | `auto` colors only when standard output is a terminal, `always`, `never`. Default `auto`. |

Levels are `off`, `error`, `warn`, `info`, `debug` and `trace`, matched case-insensitively. On the command line a level is `--logging.level.oxbloom_web=debug`, and a target with `::` is written the same way: `--logging.level.oxbloom_web::router=debug`. An invalid value is a `ConfigError::Invalid` that names its key and never contains the value, and with boot it fails the run in the `Config` phase with a report that explains the key. The console output goes to standard output.

The rules for targets and groups:

- A level for a target applies to that module path and everything below it. The longest matching target wins.
- A target name may contain only letters, digits, `_`, `-` and `:`. Anything else is rejected, because the name becomes part of a filter directive and a comma or `=` could add a second one.
- A target with its own level keeps it when its group has a level: the own target beats the group. When a group name equals a target name, the group wins. If two groups list the same target, the group that sorts last by name wins. A group cannot be named `root`.
- Group and `root` names are matched like configuration keys: case, `-` and `_` do not matter.

### Startup events

With the feature on, boot emits `tracing` events at the target `oxbloom::boot`:

- `info`: `Starting {app}` with the active profiles, and `Started {app} in {secs}s`. The name `{app}` is `application.name`, or `application` when the key is absent.
- `info`: each service started and stopped, and the address the web server is listening on.
- `warn`: configuration warnings, such as a deprecated key.
- `debug`: one event per condition decision. They are emitted together after the container is built.
- `error`: the one-line failure summary, in addition to the failure report.

The banner, the conditions report and the failure report are reports, not log lines, and keep going to the output target. A failure that happens before the subscriber exists, such as an invalid `logging.*` value, logs no `error` event; its failure report is the only record.

Boot skips the installation with `.logging(false)` on the builder or `oxbloom.logging.enabled = false` in the configuration. The property is checked for a valid boolean even when the builder turned logging off. A process has one global subscriber. If one is already installed, for example by an earlier application in the same test binary, boot keeps it, registers no `LoggingSystem` bean and emits one `warn` event through the existing subscriber; a bad `logging.*` value is still a failure in that case.

### File output

A file log is written in addition to the console, never instead of it.

```toml
[logging.file]
enabled = true
path = "var/log/myapp"        # a directory, created when missing; default "logs"
name = "myapp"                # file name prefix; default application.name, else "oxbloom"
rotation = "daily"            # hourly, daily (default) or never
max-history = 14              # keep this many files; unset keeps all; 0 is invalid
format = "full"               # full (default), compact or json; files are never colored
```

| Key | Meaning |
|---|---|
| `logging.file.enabled` | `true` writes a log file. Default `false`. |
| `logging.file.path` | The directory for the files. Default `logs`. |
| `logging.file.name` | The file name prefix. Default `application.name`, else `oxbloom`. |
| `logging.file.rotation` | `hourly`, `daily` or `never`. Default `daily`. Size-based rotation is not supported. |
| `logging.file.max-history` | The number of files to keep, at least 1. Unset keeps all of them. |
| `logging.file.format` | `full`, `compact` or `json`. Default `full`. |

A directory that cannot be created or a file that cannot be opened fails startup with `LoggingError::Io`, which carries the path; with boot the failure report suggests checking the directory and its permissions.

- Pruning is by file-name prefix. With `max-history`, every file in the log directory whose name starts with the configured prefix is a candidate for deletion, including unrelated ones (the prefix `app` also matches `application.log`). With `rotation = "never"` and `max-history = 1` the existing file is deleted at start. Use a dedicated directory and a distinctive prefix.
- The file writer is non-blocking and lossy: when its buffer fills, events are dropped and the caller is not blocked. Lines that are buffered are flushed when the last `LoggingSystem` clone drops, which boot does when `run()` returns, after the `failed` or `stopped` listeners. Events logged after `run()` returns are not written to the file.
- `LoggingSystem` creates the log directory and file and starts the writer before it installs the subscriber, so an init that fails with `AlreadyInstalled` leaves an empty log file. `logging.file.enabled = true` is ignored when logging is disabled (`oxbloom.logging.enabled = false` or `.logging(false)`).
- Two `LoggingSystem` builds in one process that point at the same directory run two writers on one file, and both prune it. This only happens in tests.

### JSON output

`logging.console.format = "json"` and `logging.file.format = "json"` write one JSON object per line. Console and file share the field options.

```toml
[logging.console]
format = "json"

[logging.json.fields]
thread = false                # leave a field out (all six default to true)

[logging.json.names]
timestamp = "ts"              # rename a field in the output
message = "msg"
```

`<field>` in `logging.json.fields.<field>` and `logging.json.names.<field>` is one of `timestamp`, `level`, `target`, `message`, `span` and `thread`. An unknown field, an empty name, or two included fields with the same output name is a `ConfigError::Invalid` that names the key.

```json
{"id":3,"level":"info","msg":"created todo","span":{"id":7,"name":"request"},"target":"todos","ts":"2026-11-01T09:30:12.481Z"}
```

- The keys are the configured names of the enabled fields. The order of the keys in a line is alphabetical and not guaranteed, so parse the line instead of relying on a position. Event fields other than `message` are extra keys, for example `id` above. A built-in key wins over an event field with the same name.
- `span` is the innermost span as a flat object: its fields plus `name`. A span field called `name` is replaced by the span name.
- `timestamp` is RFC 3339 in UTC with millisecond precision; `level` is lowercase.
- Message text is escaped, so each event is one line.

### Request logging

With the feature `web` of `oxbloom-boot`, `RequestLogAutoConfiguration` registers a `RequestLogFilter` as a `dyn Filter` bean when `logging.request.enabled` is `true`. It logs one event per request, after the response exists, with the target `oxbloom::web::request` and the fields `method`, `path`, `status`, `latency_ms` and `headers`.

```toml
[logging.request]
enabled = true
level = "info"                # error, warn, info (default), debug or trace
headers = ["user-agent", "x-request-id"]   # the allowlist; default empty

[logging.level]
"oxbloom::web::request" = "debug"          # needed only when level is below the root level
```

| Key | Meaning |
|---|---|
| `logging.request.enabled` | `true` registers the filter. Default `false`. |
| `logging.request.headers` | The request header names to print. Default empty. |
| `logging.request.level` | The level of the request event. Default `info`. |

- The query string is never logged, only the path. A path that still carries a query (a hand-built `Request`) is cut at the first `?`.
- The header allowlist is the operator's responsibility. Redacted regardless of the allowlist: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key` and `x-auth-token`, in any case and with `-` or `_` as the separator (not with `.`), and any value flagged sensitive. A header value prints as `name="value"`; a redacted one prints as `name=<redacted>`. A name that only contains a credential word, such as `x-csrf-token`, is not redacted.
- A `logging.request.level` below the root level, or below `logging.level.oxbloom::web::request`, logs nothing unless that level is lowered to match.
- The filter takes the outermost slot, `order::REQUEST_LOG`, so it records the answers of the error handling, the body limit and the security chain too: 401, 403, 404, 405, 413 and 500 are in the request log.
- An application that registers its own `RequestLogFilter` bean replaces the auto-configured one.

### Changing levels at runtime

Boot registers the `LoggingSystem` as a bean, so any component can inject it:

```rust
use std::sync::Arc;
use oxbloom_logging::{Level, LoggingSystem};

#[component]
struct Diagnostics {
    logging: Arc<LoggingSystem>,
}

impl Diagnostics {
    fn verbose(&self) -> Result<(), oxbloom_logging::LoggingError> {
        self.logging.set_level("my_app::db", Level::Trace)
    }
}
```

- `set_level(target, level)` changes one target; `set_level("root", ..)` changes the default. For a group name it sets the level of the group; members that have their own level keep it.
- `reset(target)` returns the target to the level of the last `init` or `reload`. For a group it restores the group level and each member's own entry.
- `level(target)` returns the level in force.
- `reload(&config)` re-applies all `logging.level.*` and `logging.group.*` keys from a `Config` you pass, removes keys that are gone and drops earlier `set_level` changes. It validates `logging.console.*`, `logging.file.*` and `logging.json.*` but does not apply them, because the output layers are fixed at install time. There is no file watcher and no SIGHUP handler in this slice; calling `reload` is up to the application.
- Replacing the filter is not atomic with respect to events on other threads: an event may see the old level or the new one.
- Calling `set_level` from inside a logging callback does not deadlock.

A library crate must never call `LoggingSystem::init`. Only boot or the application does. A library emits events with `tracing` and leaves the subscriber to the application.

Without boot, call `LoggingSystem::init(&config)` once at the start of `main`; a second call, or any other installed subscriber, gives `LoggingError::AlreadyInstalled`.

### Invalid configuration

A built-in analyzer for invalid `logging` configuration (feature `logging`) turns a `ConfigError::Invalid` under the key `logging` into a failure report that names the key and lists the accepted values. It is not a public item.

### Limits

- Output layers (console, file and the JSON options) are fixed when the subscriber is installed. `set_level`, `reset` and `reload` change levels only; they apply to the console and the file alike.
- File rotation is time-based only; see [File output](#file-output) for the pruning and buffering limits.
- `RequestLogAutoConfiguration` needs the feature `web`. A panic in a filter ordered before the request log loses that request's log event; see [Errors and filter order](#errors-and-filter-order). The events only appear when a subscriber is installed, for example with the feature `logging`.
- No HTTP endpoint for levels (C8), no MDC-style context fields and no per-appender level filters.
- One global subscriber per process; `LoggingSystem::init` fails with `AlreadyInstalled` the second time.
- A user-registered `LoggingSystem` bean is a duplicate-bean error, because boot registers its own.

## Security

`oxbloom-security` is a Spring-Security-style layer for `oxbloom-web`: a `SecurityFilterChain` filter reads `Authorization: Bearer <token>`, authenticates it, then authorizes the request against ordered rules. Only bearer JWTs (resource server) are implemented.

```rust
let chain = HttpSecurity::new()
    .authorize(|a| a
        .get("/public/**").permit_all()
        .path("/admin/**").has_role("ADMIN")
        .any_request().authenticated())
    .oauth2_resource_server(|r| r
        .jwt(Arc::new(decoder))
        .converter(JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_")))
    .build()?;                       // no or a second `authorize` section, an invalid pattern, or a rule after `any_request()`, is an error
let app = App::new().filter(chain);  // handlers read `req.authentication()` (trait `RequestSecurityExt`)
```

- Deny by default: a request that matches no rule is denied. One chain per application; `HttpSecurity::build()` fails without an `authorize` section and when `authorize` is called more than once (`DuplicateAuthorizeSection`; sections are never merged, so put every rule in one call). The first matching rule wins. `has_role("X")` checks the authority `ROLE_X`.
- Filters run before routing, so an unknown path is denied (401/403) unless a rule permits it, so anonymous clients cannot probe which routes exist (a `permit_all` rule, or a caller the rules allow, still reaches the router's 404). The exception: a request body over the limit gets 413 before the security chain runs (the body limit has the slot `order::BODY_LIMIT`, ahead of `order::SECURITY`).
- Matching uses the raw, undecoded path and Ant patterns: `*` is exactly one segment, `**` is zero or more. Literals are case-sensitive; a trailing slash on the request is ignored. Any other `*` and any `{`, `}` or `?` in a segment (`/users/{id}`, `/files/*.pdf`, `a*`, `***`) is rejected by `build()` with `InvalidPattern`; write `/users/*` instead.
- A `HEAD` request is authorized as `GET`: a `get(..)` rule also governs `HEAD`, and rules are tried in order with both views, so an earlier `get(..)` rule shadows a later `head(..)` rule and vice versa.
- Responses: anonymous or bad token is 401 with `WWW-Authenticate: Bearer` (plus `error="invalid_token"` when a token failed), authenticated but not allowed is 403, and a key-source outage (`ProviderUnavailable`) is 503 with no challenge. The answers are `application/problem+json` bodies with the headers kept (see [Errors and filter order](#errors-and-filter-order)). A malformed `Authorization` header (several headers, a value that is not valid text, `Bearer` without a token, whitespace inside the token) is answered 401 `error="invalid_request"` by the chain itself and bypasses a custom `entry_point`. Another scheme such as `Basic` is treated as anonymous, and so is every `Authorization` header when no bearer provider is registered. Errors, responses and the `Debug` output of the security types never contain tokens, keys or claim values; `JwksSource` prints its URI without userinfo or query string. The server marks `Authorization`, `Proxy-Authorization` and `Cookie` header values sensitive (and the chain marks `Authorization` sensitive again, also for hand-built requests), so `Debug` of a `Request` or its headers shows `Sensitive` for them. Other headers, including custom credential headers, and the query and body are printed as sent.
- If `oauth2_resource_server` is called while another bearer provider is registered, the first bearer provider wins.
- Not implemented (deliberate cuts): sessions, password login and HTTP Basic, CSRF, method security, security headers, OAuth2 login, opaque-token introspection, several chains per app.

### JWT

- The algorithm allow-list is fixed when the decoder is built; the token's `alg` header never selects the key type, and `alg: none` is rejected. Build with `JwtDecoderBuilder::with_secret` (HS256/384/512, as `Secret<Vec<u8>>`; the secret must be at least as long as the hash output: 32, 48 or 64 bytes), `with_public_key_pem` (RS256/384/512, ES256/384) or `with_jwks`.
- `exp` is required (opt out with `require_exp(false)`), `nbf` is checked, clock skew is 60 s (at most 1 h), `sub` is required. `iss` is checked when an issuer is configured. **`aud` is not checked unless you configure an audience; set it.**
- Authorities: `SCOPE_<x>` from `scope` (space-delimited string) or `scp` (array). `authorities_claim("roles", "ROLE_")` adds a claim mapping; it does not replace the default one.
- `on_failure(|reason| ..)` receives a `FailureReason` for logging; it carries no token text. It is not called for JWKS outages.

### JWKS (feature `jwks`)

```rust
let source = JwksSource::builder(Arc::new(ReqwestJwksFetcher::new()?), "https://issuer.example/.well-known/jwks.json").build()?;
let decoder = JwtDecoderBuilder::with_jwks(source, vec![Algorithm::RS256]).issuer("https://issuer.example").audience(["my-api"]).build()?;
```

- The URI must be `https` unless `allow_insecure_http()` is called (loopback tests only). The body is capped at 1 MiB, the request times out after 5 s, redirects are not followed.
- Keys are cached for 5 min and looked up by `kid`. An unknown `kid` triggers one refresh, and refreshes happen at most once per 30 s however many tokens arrive. Concurrent lookups share one fetch. After a failed refresh the stale set is served until it is older than 2x the TTL, then requests fail with 503. A token with a brand-new `kid` can therefore be rejected for up to 30 s after a key rotation. A key the issuer has removed stops being accepted at the first successful refresh after the TTL (up to 5 min plus 30 s with a healthy issuer); while refreshes keep failing it can be accepted for up to 2x the TTL (10 min).
- Only keys whose `kty`/`alg` fit the decoder's allow-list are used; `use`, if present, must be `sig`.
- The feature uses `reqwest` with rustls (aws-lc-rs), so building needs a C compiler. `HTTP_PROXY`/`HTTPS_PROXY` environment variables are honored. Without the feature there is no HTTP client in the dependency tree.
- Supply-chain note: the `jsonwebtoken` backend `rust_crypto` pulls in `rsa` 0.9, which carries advisory RUSTSEC-2023-0071 (a timing leak in private-key operations). This crate only verifies signatures and never uses private keys, but `cargo audit` will report it.

### Security configuration

`jwt_decoder_from_config(&config, fetcher)` builds a decoder from `security.jwt`. Register the secret key when you build the `Config`; the decoder also refuses a command-line secret on its own:

```rust
use oxbloom_security::{SECRET_KEY, jwt_decoder_from_config};

let config = Config::builder().dir("config").secret_key(SECRET_KEY).build()?;
let decoder = jwt_decoder_from_config(&config, None)?;
```

The section:

```toml
[security.jwt]
issuer = "https://issuer.example"
jwk-set-uri = "https://issuer.example/.well-known/jwks.json"   # needs a fetcher argument
algorithms = ["RS256"]
audience = ["my-api"]
clock-skew-secs = 60
# secret = "..."         # HMAC only; set it from APP_SECURITY__JWT__SECRET
# public-key-pem = "..." # alternative to jwk-set-uri for RS*/ES*
```

- Empty or whitespace-only items in `audience` and `algorithms` are an error.
- Exactly one key source: `secret`, `public-key-pem` or `jwk-set-uri`. A missing or contradictory combination is an error naming the key.
- Unknown keys under `security.jwt` are rejected.
- `jwk-set-uri` must be an `https` URI; the configuration has no way to allow plain `http` (only code can, with `allow_insecure_http()`).
- `algorithms` and `audience` are lists; from environment variables set them comma-separated (`APP_SECURITY__JWT__AUDIENCE=a,b`) or indexed (`APP_SECURITY__JWT__AUDIENCE__0=a`).
- Keys match case-insensitively ignoring `-` and `_`, so `jwk-set-uri`, `jwk_set_uri`, `jwkSetUri` and `JWK_SET_URI` (environment) are one key. Two spellings of one key in the same file are a `DuplicateKey` error.
- Register `oxbloom_security::SECRET_KEY` (`"security.jwt.secret"`) with `Config::builder().secret_key(SECRET_KEY)` so the HMAC secret cannot be passed as a command-line argument.

Features of `oxbloom-security`: `jwks` (`ReqwestJwksFetcher`) and `di` (`ContainerBuilder::security_chain(chain)`, resolve `Arc<SecurityFilterChain>`).

## Errors and filter order

`oxbloom-web` has one error model and one ordered filter chain. Both are on by default; no feature or key switches them on.

### Filter order

Every `Filter` has a slot, a plain `i32`. A lower number runs first (it is the outer layer), and filters with the same number keep their registration order. The slots are constants in `oxbloom_web::order`:

| Constant | Value | Filter |
|---|---|---|
| `REQUEST_LOG` | -400 | `RequestLogFilter` |
| `CORS` | -350 | `CorsFilter`; outside error handling and security |
| `ERROR_HANDLING` | -300 | built in; renders every error answer |
| `NEGOTIATION` | -250 | built in with the feature `json`; answers 406 |
| `BODY_LIMIT` | -200 | built in; answers 413 |
| `COMPRESSION` | -150 | `CompressionFilter`; outside security |
| `SECURITY` | -100 | `SecurityFilterChain` |
| `DEFAULT` | 0 | every filter that declares no slot |

A filter declares its slot by overriding the provided method `Filter::order()`, which returns `order::DEFAULT` unless you override it. `App::filter(f)` places `f` at `f.order()`. To choose a slot from code without changing the filter, use `App::filter_ordered(order, f)`, which wraps the filter in `Ordered`. Boot registers the security chain and every `dyn Filter` bean with plain `App::filter`, so each one lands at its own slot; the order of beans in the container only breaks ties.

```rust
use oxbloom_web::{App, Filter, Next, Request, Response, order};

struct Audit;

#[async_trait::async_trait]
impl Filter for Audit {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        next.run(req).await
    }
    // After security: sees only the requests the chain let through, authenticated.
    fn order(&self) -> i32 {
        order::SECURITY + 50
    }
}

let app = App::new().filter(Audit);
// Same effect for a filter that does not know its slot:
// App::new().filter_ordered(order::SECURITY + 50, Audit)
```

Because the request log is outermost, it records what the security chain and the body limit answer. The request log event now exists for 401, 403, 404, 405, 413 and 500. An oversized request body gets 413 before security runs, so a client without credentials sees 413, not 401. A plain filter at `DEFAULT` runs after the security chain and never sees a request that the chain rejects.

Two zones need care. A filter ordered below `order::SECURITY` runs before authentication: it sees unauthenticated requests (no `Authentication` in the extensions), and a response it returns itself is sent without any authorization check. A filter ordered below `order::BODY_LIMIT` sees an empty body for a request whose body was over `server.max-body-bytes`, and it can tell why only by calling `Request::body_too_large()`, which is `true` exactly when the server cut the body.

### Errors

A handler reports an error by returning an `HttpError`, alone or inside a `Result`:

```rust
use http::StatusCode;
use oxbloom_web::{HttpError, Request};

async fn get_todo(req: Request) -> Result<String, HttpError> {
    let id = req.param("id").unwrap_or_default();
    Err(HttpError::new(StatusCode::NOT_FOUND)
        .detail(format!("no todo {id}"))
        .with("todo", id))
}
```

`IntoResponse` is implemented for `Result<T, E>` when both sides implement it. An application error enum maps to a response by implementing `IntoResponse` itself, usually by turning its variants into an `HttpError`. The builder methods are `title`, `detail`, `type_uri` and `with(key, value)`; `with` adds an extension member whose value is a string, an integer or a boolean.

The answer is `application/problem+json` (RFC 9457):

```json
{"type":"about:blank","title":"Not Found","status":404,"detail":"no todo 7","instance":"/api/todos/7","todo":"7"}
```

- Members, in order: `type` (default `about:blank`), `title` (the author's, else the status reason phrase), `status`, `detail` (only when set), `instance` (the request path, never the query) and then the extension members. An extension whose key equals one of the five built-in members is dropped.
- The framework answers 404, 405 (with `Allow`), 413 and a panic in a handler or filter (500) the same way, and so does the security chain for 401, 403 and 503. Headers already on the response, such as `WWW-Authenticate` and `Allow`, are kept.
- A `detail` set by the author is always shown, because the author wrote it for the client. `server.error.include-message` controls only the internal messages that the framework supplies, which today means the panic message: with `never` (the default) it stays out of the response, with `always` it becomes the `detail` of the 500 answer. The panic message is always in the `error` log event `request panicked: ...` (target `oxbloom::web`), and Rust's default panic hook also prints it to standard error.
- A response that already has a body is never rewritten, not even when it carries error attributes. A handler can still return its own error body.
- A `HEAD` request that ends in an error gets the status and the headers, with no body.

To change the shape, implement `ErrorRenderer` (`fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response`) and register it as a `dyn ErrorRenderer` bean (the default `ProblemRenderer` has `on_missing_bean`, so yours wins), or call `App::error_renderer` without boot. An `Arc` of a renderer, also an `Arc<dyn ErrorRenderer>`, is a renderer too, so a shared one can be passed to `App::error_renderer` as it is. `RequestInfo` holds the method and the path.

```rust
use std::sync::Arc;
use oxbloom_web::{ErrorAttributes, ErrorRenderer, IntoResponse, RequestInfo, Response};

struct PlainRenderer;

impl ErrorRenderer for PlainRenderer {
    fn render(&self, attrs: &ErrorAttributes, info: &RequestInfo) -> Response {
        let mut res = format!("{} at {}", attrs.status.as_u16(), info.path).into_response();
        *res.status_mut() = attrs.status;
        res
    }
}

// With boot: .instance(PlainRenderer)
//            .provides(|r: Arc<PlainRenderer>| -> Arc<dyn ErrorRenderer> { r })
```

### Server properties

```toml
# application.toml
[server]
address = "127.0.0.1"
port = 8080
shutdown-timeout = "30s"
max-body-bytes = "1MiB"
header-read-timeout = "10s"

[server.error]
include-message = "never"

[server.forward-headers]
strategy = "none"
trusted-proxies = ["127.0.0.1", "::1"]
```

| Key | Default | Meaning |
|---|---|---|
| `server.address`, `server.port` | `127.0.0.1`, `8080` | Where the server listens (`0` picks a free port). |
| `server.shutdown-timeout` | `30s` | How long graceful shutdown waits for open connections. After that they are dropped and a `warn` event is logged on target `oxbloom::web`. `0` drops open connections at once. Dropping the future of `serve` aborts in-flight connections. |
| `server.max-body-bytes` | `1MiB` | Largest request body; a larger one gets 413. `0` rejects every non-empty body. |
| `server.header-read-timeout` | `10s` | Time a client has to send the request headers. Must be above 0 and at most one day; `App::header_read_timeout` clamps to one day. |
| `server.forward-headers.strategy` | `none` | `none` or `framework`; see the next section. |
| `server.forward-headers.trusted-proxies` | loopback only | A list of IP addresses and CIDR networks (IPv4 and IPv6). Used when the strategy is `framework`. |
| `server.error.include-message` | `never` | `never` or `always`; see Errors. |
| `server.cors.allowed-origins` | unset (CORS off) | A list: `*` or entries such as `https://app.example`. Setting it switches CORS on; see [CORS](#cors). An explicitly empty list fails startup. |
| `server.cors.allowed-methods` | `GET`, `HEAD`, `POST` | Methods a preflight allows, upper case; `*` allows every method. |
| `server.cors.allowed-headers` | none | Request headers a preflight allows; `*` allows every header. |
| `server.cors.exposed-headers` | none | Response headers that scripts may read. |
| `server.cors.allow-credentials` | `false` | Cannot be combined with `*` in the three lists above. |
| `server.cors.max-age` | `30m` | How long a browser may cache a preflight answer. |
| `server.compression.enabled` | `false` | Switches compression on; fails startup without a codec feature. See [Compression](#compression). |
| `server.compression.min-size` | `1KiB` | Smaller bodies are not compressed. |
| `server.compression.mime-types` | `text/*`, `application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml`, `application/xml` | Types that are compressed; `family/*` matches a family. |
| `server.compression.level` | `default` | `fast`, `default` or `best`. |
| `server.compression.max-body-bytes` | `8MiB` | Larger bodies are not compressed. |
| `server.static.enabled` | `false` | Serves `server.static.directory` at `server.static.path`; see [Static resources](#static-resources). |
| `server.static.directory` | `static` | Must exist when static resources are enabled. |
| `server.static.path` | `/` | The URL prefix. |
| `server.static.index` | `index.html` | The file served for a directory: 1 to 255 bytes, not starting with `.`, without `/`, `\`, `%` or NUL; otherwise startup fails. |
| `server.static.cache-max-age` | `0s` | `max-age` of `Cache-Control`; `0s` sends `no-cache`. |
| `server.static.max-file-bytes` | `10MiB` | Larger files answer 404. |

Durations and sizes use the usual units (`500ms`, `30s`, `1MiB`, or plain bytes). Invalid values fail startup naming the key and never the value, in the `Container` phase; the failure report is the generic container report, because the built-in analyzer explains missing keys only. The same keys exist in code as `App::shutdown_timeout`, `App::max_body_bytes`, `App::header_read_timeout`, `App::forwarded`, `App::cors` and `App::compression` (the static keys build a `StaticResources` controller that you register with `App::routes`). For the CORS, compression and static keys, the `server.*` section is read once by `WebServer::new`; a key that is absent leaves what the code set, and `enabled = true` or a present `server.cors.allowed-origins` adds the filter or the routes. A `server.static.path` that conflicts with a route of the application fails startup naming that key.

The sub-keys (`server.compression.min-size`, `mime-types`, `level` and `max-body-bytes`; the `server.cors.*` keys other than `allowed-origins`; the `server.static.*` keys other than `enabled`) are always validated, but they have no effect without their switch key (`server.compression.enabled = true`, `server.cors.allowed-origins`, `server.static.enabled = true`). They never tune a filter or route set that the code registered with `App::compression`, `App::cors` or `App::routes`.

`WebServer::new(app, &config)` applies only the keys that are present. A setting made in code wins when its key is absent, so `WebServer::new(App::new().max_body_bytes(16), &config)` keeps the 16 bytes unless the configuration has `server.max-body-bytes`; the defaults in the table are the `App`'s own. `strategy` and `trusted-proxies` each replace only their own part of `App::forwarded`, so `trusted-proxies` alone keeps the strategy set in code. `server.error.include-message` only configures the `ProblemRenderer` that the auto-configuration registers: `WebServer::new` validates it but sets no renderer, so without the auto-configuration choose one with `App::error_renderer`.

### Forwarded headers

Behind a reverse proxy the TCP peer is the proxy, not the user. With `server.forward-headers.strategy = "framework"` (or `App::forwarded(ForwardedConfig { strategy: ForwardStrategy::Framework, .. })`) the server resolves the real client and stores a `ClientInfo { addr, scheme, host }` in the request extensions. `Request::client_addr()`, `scheme()` (`Scheme::Http` or `Scheme::Https`) and `host()` read it. The default strategy is `none`: the headers are ignored, the client is the peer, the scheme is `Http` and `host()` is `None`.

**Deployment requirement: the trusted proxy must set or strip `Forwarded`, must overwrite or strip `X-Forwarded-Proto` and `X-Forwarded-Host`, and must append to `X-Forwarded-For`. Otherwise clients control their own address, scheme and host, and the framework cannot detect this.** Enable the strategy only behind a proxy you have checked for this, and list only that proxy in `trusted-proxies`.

The algorithm, when the strategy is `framework`:

1. The headers count only when the TCP peer is in the trusted list. Otherwise they are ignored.
2. The `Forwarded` header (RFC 7239) is read first. Only when it is absent are `X-Forwarded-For`, `X-Forwarded-Proto` and `X-Forwarded-Host` read. A `Forwarded` header that is present but unusable does not fall back to the `X-Forwarded-*` headers.
3. The hops (`X-Forwarded-For` or the `for=` parameters) are walked from right to left. The first hop that is not trusted is the client. If every hop is trusted, the leftmost one is the client. The leftmost entry is client-controlled, so it is never used just because it is first. With `Forwarded`, `proto` and `host` come from the client's element. A list in `X-Forwarded-Proto` or `X-Forwarded-Host` is read at the client's hop when it has as many entries as `X-Forwarded-For`; otherwise (also without `X-Forwarded-For`) its rightmost entry, written by the proxy nearest the server, is used.
4. Any element that is not a valid IP address (with optional port, and the bracketed IPv6 form) makes the whole header unusable. Then any `proto` and `host` are dropped too: the peer stays the client, the scheme is `Http` and the host is `None`.
5. `proto` gives the scheme `Https` when it is `https` (in any case); any other value leaves the scheme `Http`. `host` must be a valid authority (a host name or IP address with an optional port), otherwise the host is `None`.

The default `server.forward-headers.trusted-proxies` list is loopback only (`127.0.0.0/8` and `::1`), so a proxy on another host must be listed. An empty list trusts nobody. A forwarded address without a port has port `0`. `Request::client_addr()` is `None` for a request built by hand and run with `App::handle`, because there is no socket.

Nothing in `oxbloom-security` uses the client address yet; rules by address are a follow-up.

### Limits

- A panic in a handler or in a filter ordered after `ERROR_HANDLING` is answered 500 by the built-in error filter, and the request log records that 500. A panic in a filter ordered before `ERROR_HANDLING` is caught by an outer catch and also answered 500 (with the `request panicked` event), but it unwinds through every filter outside it. So the request log event of that request is lost when the request log itself panics or when any filter ordered between `REQUEST_LOG` and `ERROR_HANDLING` (for example at -350) panics. A filter ordered before `REQUEST_LOG` loses the event only when it panics before it calls `next.run`; a panic after that comes when the event is already written.
- The panic message in the `request panicked` event has its control characters escaped (`\n`, `\r`, `\u{1b}` and so on), so it stays on one line; `ErrorAttributes.internal` keeps the raw message.
- `HttpError` extension values are strings, integers and booleans. Two `with` calls with the same key can produce two members with that name.
- A status without a reason phrase (for example 499 or 599) renders an empty `title`.
- A rendered response drops the extensions of the original response.
- A renderer that panics escapes the outer catch.
- `ErrorAttributes` on a 2xx or 3xx response is rendered as problem+json.
- There are no per-route error renderers and no handlers keyed by error type.

## JSON and content negotiation

The Cargo feature `json` of `oxbloom-web` is on by default. It adds `Json<T>` for responses, `Request::json::<T>()` for request bodies and a built-in `Accept` check. Without the feature (`default-features = false`) none of them is compiled and `serde_json` is not a dependency; the slot `order::NEGOTIATION` exists in both builds. `oxbloom-boot` (feature `web`) and `oxbloom-security` depend on `oxbloom-web` with its default features, so the feature `json` is on in any application that uses them; `default-features = false` only removes it for applications that use `oxbloom-web` alone.

```rust
use http::StatusCode;
use oxbloom_web::{HttpError, Json, Request};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct NewTodo {
    title: String,
}

#[derive(Serialize)]
struct Todo {
    id: u64,
    title: String,
}

async fn create(req: Request) -> Result<(StatusCode, Json<Todo>), HttpError> {
    let body: NewTodo = req.json()?;
    Ok((StatusCode::CREATED, Json(Todo { id: 1, title: body.title })))
}
```

`Json(value)` answers 200 with `content-type: application/json` and the serialized value; `(StatusCode, Json<T>)` picks another status. If serialization fails, the answer is a 500 problem without a `detail`; the error text is logged at level `error` on target `oxbloom::web` and never sent.

`Request::json::<T>()` returns `Result<T, HttpError>`, so `?` works in a handler that returns `Result<_, HttpError>`:

- The `Content-Type` must be `application/json` or a `+json` type with a non-empty prefix, such as `application/vnd.api+json`. Parameters (`;charset=utf-8`) are allowed and case is ignored. `application/jsonx`, `application/json-seq`, a bare `+json`, any other type and a missing header are 415. A client must send `Content-Type: application/json`; Oxbloom does not guess a type for a client that omits it, not even for an empty body.
- A body that is empty or not valid for `T` is 400. The `detail` names only the error class and the position, never the body or a value from it.

```json
{"type":"about:blank","title":"Unsupported Media Type","status":415,"instance":"/api/todos","accepted":"application/json"}
{"type":"about:blank","title":"Bad Request","status":400,"detail":"invalid JSON body: syntax error at line 1 column 8","instance":"/api/todos"}
{"type":"about:blank","title":"Not Acceptable","status":406,"instance":"/api/todos","supported":"application/json"}
```

The three answers are, in order, a 415 (member `accepted`), a 400 (error class `syntax`, `data` or `eof`, then line and column) and a 406 (member `supported`). All are `application/problem+json`, as described in [Errors](#errors).

The 406 comes from a built-in filter at `order::NEGOTIATION` (-250), inside `ERROR_HANDLING` and outside `BODY_LIMIT`. It runs after the handler and acts only on a response built by `Json<T>` (it carries a `JsonBody` marker in its extensions); error answers and other bodies are never touched. The marker survives the router's body strip, so a `HEAD` request that does not accept JSON gets 406 as well. A `Json` answer is checked against `Accept` when it comes from a handler or from a filter ordered after `order::NEGOTIATION`; a `Json` answer from a filter ordered before it is never negotiated, and the built-in security chain answers with `HttpError`, which is never negotiated. The rules for `Accept`:

- A range is `*/*`, `type/*` or `type/subtype`, with a `q` value as its only parameter that matters. Anything else (`application/*+json`, `*/json`, `/`) is an invalid range and is skipped.
- `q` follows RFC 9110: `0` or `1`, optionally followed by a point and up to three digits, and `1.xxx` only with zeros. An invalid `q` (`1.5`, `abc`) skips the range.
- Media types and parameter names are case-insensitive. A comma or semicolon inside a quoted parameter value does not split a range. Several `Accept` headers count as one list.
- The most specific range that matches `application/json` decides: `application/json`, then `application/*`, then `*/*`. The answer is acceptable when that range has `q > 0`; `q=0` excludes JSON.
- With no `Accept` header, no valid range at all, or only header lines that are skipped, the answer is acceptable. A header line is skipped when it has a byte that is not visible ASCII (so `é` counts too) or an unterminated double quote.

A request body over `server.max-body-bytes` gets 413 from the body-limit filter before `Request::json` runs, and a request the security chain rejects keeps its 401 or 403; neither becomes a 406. Not covered: parameters of `Accept` other than `q`, typed extractors, pluggable converters, and request body decompression; see [`docs/ROADMAP-core.md`](docs/ROADMAP-core.md).

## CORS

`CorsFilter` answers browser cross-origin requests. It sits at `order::CORS` (-350), outside error handling and security, so error answers (401, 403, 404, 413, 500) carry the CORS headers and an allowed preflight never reaches the security chain. It needs no Cargo feature. Register it with `App::cors(CorsConfig)`, which returns a `CorsError` for an unusable setting, or switch it on with `server.cors.allowed-origins`.

```rust
use std::time::Duration;
use oxbloom_web::{App, CorsConfig};

let app = App::new().cors(
    CorsConfig::new()
        .allowed_origins(["https://app.example"])
        .allowed_methods(["GET", "POST", "DELETE"])
        .allowed_headers(["Content-Type", "Authorization"])
        .exposed_headers(["X-Total"])
        .allow_credentials(true)
        .max_age(Duration::from_secs(600)),
)?;
# Ok::<(), oxbloom_web::CorsError>(())
```

Defaults: methods `GET`, `HEAD` and `POST`, no allowed and no exposed headers, credentials off, max age 30 minutes. CORS is off until an origin list is given.

- A request without an `Origin` header, or from an origin that is not allowed, passes and gets no CORS headers. The filter does not block it; the browser enforces the policy. When the allowed origins are a list (not `*`), every answer that is not a preflight answer, including these, gets `Vary: Origin`, so a shared cache never serves a copy without CORS headers to an allowed origin.
- A preflight is an `OPTIONS` request with `Origin` and `Access-Control-Request-Method`. From an allowed origin that asks for an allowed method and allowed headers, it is answered 204 by the filter with `Access-Control-Allow-Origin`, `-Allow-Methods`, `-Allow-Headers` (when there are any), `-Max-Age`, `-Allow-Credentials` (when on) and a `Vary` header. Any other preflight passes through to the rest of the chain with no CORS headers, and an `OPTIONS` request without `Access-Control-Request-Method` is not a preflight.
- Every other request from an allowed origin gets `Access-Control-Allow-Origin`, `Vary: Origin` and, when configured, `Access-Control-Expose-Headers` and `Access-Control-Allow-Credentials`. An existing `Vary` that lists `Origin` or `*` is not repeated.
- `Access-Control-Allow-Origin` echoes the request origin, or is `*` when the allowed origins are `*`.

Rules for the settings:

- The origin list must not be empty. An entry is `scheme://host[:port]` with no path and no trailing slash, or `null`, or the single entry `*`. The scheme and host are compared without regard to case. Default ports are not stripped: list `https://a.example`, not `https://a.example:443`, because a browser does not send the default port.
- `*` must be the only entry of its list. This holds for origins, methods and headers. `*` in any of the three cannot be combined with `allow-credentials`; the error names `allow-credentials`.
- Methods are compared exactly and case-sensitively, so write them in upper case. Header names are compared without regard to case. `*` for headers allows and echoes every requested header; `*` for methods allows every method.
- A request with two `Origin` headers counts as not allowed.
- Listing `null` together with `allow-credentials` is a foot-gun: sandboxed frames and local files send `Origin: null`, so any such page could then make credentialed calls. The filter accepts it; do not do it.
- There are no per-path rules; one filter applies to the whole application.

From the properties: `server.cors.allowed-origins` (a list) switches CORS on, and `allowed-methods`, `allowed-headers`, `exposed-headers`, `allow-credentials` and `max-age` refine it; the keys are in [Web server properties](#server-properties). A bad list names the matching key. A present `server.cors.allowed-origins` adds a CORS filter next to one set in code with `App::cors`, so both run; use one or the other.

## Compression

`CompressionFilter` compresses response bodies with gzip, brotli or zstd, as the `Accept-Encoding` of the request allows. It sits at `order::COMPRESSION` (-150), inside the body limit and outside security. Register it with `App::compression(CompressionConfig)` or set `server.compression.enabled = true`.

The Cargo features of `oxbloom-web` are `gzip` (`flate2`), `brotli` (`brotli`) and `zstd` (`zstd`); `compression` enables all three and is in `default` next to `json`. A codec whose feature is off is never chosen, and `oxbloom_web::codecs()` lists those compiled in. `oxbloom-boot` keeps the default features of `oxbloom-web`, so all codecs are on there. `server.compression.enabled = true` fails startup when no codec is compiled in.

```rust
use oxbloom_web::{App, CompressionConfig, CompressionLevel};

let app = App::new().compression(
    CompressionConfig::new()
        .min_size(512)
        .mime_types(["text/*", "application/json"])
        .level(CompressionLevel::Default)
        .max_body_bytes(4 << 20),
);
```

Defaults: bodies of at least 1024 bytes and at most 8 MiB; the types `text/*`, `application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml` and `application/xml`; level `default` (gzip 6, brotli 5, zstd 3; `fast` is 1, 2, 1 and `best` is 9, 11, 19). An entry ending in `/*` matches a whole family; an entry `*/*` matches nothing.

A response is compressed only when all of these hold: a compiled codec is acceptable to the client, the body size is within the limits, the `Content-Type` is in the list, the response has no `Content-Encoding` and no `Content-Range`, the body is not empty, the status is not 1xx, 204 or 304, and `Cache-Control` has no `no-transform`. Otherwise it passes unchanged. A compressed response gets `Content-Encoding`, loses `Content-Length` (the server sets it), gets `Vary: Accept-Encoding` unless it already lists it, and a strong `ETag` becomes weak.

- `HEAD` gets the same compression headers as `GET`, with an empty body. The router records the length of the stripped body, so the filter decides as it would for `GET`.
- `Accept-Encoding` names are case-insensitive. A malformed entry is skipped. When a name is listed twice, the first entry wins. A codec that is not listed takes the `q` of `*`, and is not acceptable when `*` is absent. The best `q` above zero wins; ties go to zstd, then brotli, then gzip. `x-gzip` is not recognised.
- A client that sends `identity;q=0` and cannot use any compiled codec gets 406, but only for a response that would otherwise have been compressed. `*;q=0` alone rules out the codecs and does not refuse the identity.
- Encoding runs synchronously inside the request. Level `best` on a large body can take seconds; keep `max-body-bytes` and the level in proportion.
- Level `best` costs memory too: brotli quality 11 with a 4 MiB window and zstd level 19 can use tens of MB per encode, and there is no limit on how many encodes run at once.
- Error answers rendered by the error filter are not compressed: the error filter (`order::ERROR_HANDLING`) is outside the compression filter, so the compression filter never sees the rendered body. A body that a handler builds itself is compressed like any other.

**BREACH.** Compressing a response that mixes a secret (a CSRF token, a session value) with input the attacker controls, served over TLS, can leak the secret through the compressed length. Do not compress such responses: leave their type out of `mime-types`, or send `Cache-Control: no-transform`. Compression is off until you enable it.

## Static resources

`StaticResources` is a `Controller` that serves `GET` and `HEAD` below a URL prefix from a `ResourceSource`. There are two sources: `DiskResources` (a directory, read with `tokio::fs`) and `EmbeddedResources` (files compiled into the binary). It needs no Cargo feature. Answers carry `Content-Type` (from a built-in table by file extension), a strong `ETag` (a fixed FNV-1a 64 hash for embedded files, size and modification time for disk files), `Last-Modified` for disk files and `Cache-Control` (`no-cache` by default, `public, max-age=N` with `cache_max_age`). `If-None-Match` (weak comparison, lists, `*`) and, only without it, `If-Modified-Since` give 304 with the same validators and no body.

```rust
use std::sync::Arc;
use std::time::Duration;
use oxbloom_web::{App, Controller, DiskResources, EmbeddedResources, StaticResources};

// From a directory, below /assets.
let assets = StaticResources::new("/assets", DiskResources::new("static").max_file_bytes(1 << 20))
    .index("index.html")
    .cache_max_age(Duration::from_secs(3600));

// From files compiled into the binary, below /docs.
static FILES: &[(&str, &[u8])] = &[("index.html", b"<h1>hello</h1>")];
let site = StaticResources::new("/docs", EmbeddedResources::new(FILES));

let app = App::new()
    .routes(Controller::routes(Arc::new(assets)))?
    .routes(Controller::routes(Arc::new(site)))?;
# Ok::<(), oxbloom_web::RouteError>(())
```

A directory serves its index file (default `index.html`) or 404 without one. A prefix is normalised to a leading `/` and no trailing one, and braces in it are literal. A method other than `GET` and `HEAD` is 405 with `Allow: GET, HEAD, OPTIONS`. Mounted at `/`, the catch-all route turns a non-`GET` request to an unknown path into 405 instead of 404.

The path rules are strict, because `oxbloom-security` matches raw request paths and every file must have exactly one raw spelling, so a path rule cannot be bypassed by encoding. All of these answer 404:

- `..` and `.` segments, an empty segment (`//`) and any segment that starts with `.` (dotfiles). One trailing `/` is allowed.
- A raw byte that browsers escape: a non-ASCII byte (`/café`), space, `"`, `#`, `<`, `>`, `?`, a backtick, `{` or `}`. These must come as escapes (`/caf%C3%A9`, `%20`, `%7B`).
- An escape of any other byte, such as `%61`, `%2B`, `:` as `%3A`, and `[`, `]`, `^`, `|` as `%5B`, `%5D`, `%5E`, `%7C` (write those four raw); an escape with lower-case hex digits (`%c3%a9`); an encoded `/` (`%2F`), NUL (`%00`) or backslash. The allowed escapes (non-ASCII bytes, the characters of the previous point and `%25`) are decoded exactly once, in upper-case hex. A double-encoded `%252e%252e` stays literal and misses.
- A segment over 255 bytes, a path over 1024 bytes, and (for `DiskResources`) a file over `max_file_bytes` (default 10 MiB, `server.static.max-file-bytes`), which also logs a warning.
- A symlink that leaves the root: the root is canonicalised once and every file must stay under it. On Unix the file is opened with `O_NOFOLLOW | O_NONBLOCK` on Linux (including Android), Apple systems, FreeBSD, OpenBSD and NetBSD, so a symlink or FIFO swapped in after the checks is not followed; other Unix targets rely on the earlier checks. The served tree must be writable only by people you trust.

**Index files are the one exception** to the single spelling: `/docs`, `/docs/` and `/docs/index.html` all serve `docs/index.html`. Protect a directory with a `/docs/**` rule, which also matches `/docs`, not with a rule on the index file name or with `/docs/*`.

A read error other than "not found" answers 500 and is logged, never shown. The index name set with `index` must be a plain file name; an unclean one is ignored by the web API (every directory is then 404) and rejected at startup by `server.static.index`.

**Case-insensitive file systems** (the macOS default, Windows): `/ADMIN/x` can serve `admin/x`, which a case-sensitive raw rule on `/admin/**` does not match. Use case-insensitive rules, or keep the root on a case-sensitive file system.

Not supported: streaming bodies (a file is read whole), range requests, `If-Match`, directory listings, SPA fallback to the index, and pre-compressed sibling files (`app.js.gz`). With compression on, a static file can be compressed per request; its `ETag` then becomes weak.

From the properties: `server.static.enabled = true` serves `server.static.directory` (default `static`, which must exist) at `server.static.path` (default `/`); see the next section.

## Example

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
- `--oxbloom.main.conditions-report=console` prints the conditions evaluation report shown in [The conditions report](#the-conditions-report); with the override flag, the `on_property_value` record moves to `Matched:` and the default's `on_missing_bean` record to `Skipped:`.

### The todos example

`examples/todos` is a CRUD REST API on the web, security and logging stack. It shows `#[controller("/api/todos")]` with `#[post]`, `#[get]`, `#[put]`, `#[patch]` and `#[delete]`, path parameters, a query filter (`?done=true`), status codes (201 with a `Location` header, 204, 400, 404, 415, 422), a thread-safe in-memory store, a `SecurityCustomizer`, and `tracing` events at the target `todos` that boot's logging subscriber prints. Bodies are read with `Request::json` and answers are built with `Json<T>`; a body without `Content-Type: application/json` is 415. Run it with `make run-todos ARGS="--server.port=0"` (or `cargo run -p todos -- --server.port=0`); it prints the address. The handlers log at `info`, and `--logging.level.todos=warn` hides their events.

```
curl -X POST -H 'content-type: application/json' -d '{"title":"try it"}' http://127.0.0.1:<port>/api/todos
curl "http://127.0.0.1:<port>/api/todos?done=false"
```

`PUT` and `PATCH` on an unknown id are 404 (they never create), and concurrent `POST`s never share an id. The example's security rules open `/api/todos/**` so that it runs without a token; the doc comment of `PublicPaths` shows how to protect the writes. This opening is intentional for the example, and the example must not be deployed as it is.

## HTTP behavior

- Unknown path: 404. Known path, wrong method: 405 with `Allow`.
- `HEAD` falls back to the `GET` handler, body stripped. `OPTIONS` answers 204 with `Allow`.
- `TRACE` and `CONNECT` return 405 unless enabled in `RouterConfig`.
- Request bodies are capped (default 1 MiB, 413 above); header read timeout 10 s. These limits, the shutdown timeout and forwarded headers are `server.*` keys; see [Server properties](#server-properties).
- Error answers are `application/problem+json`; see [Errors and filter order](#errors-and-filter-order).
- `Json<T>` answers, `Request::json` and the `Accept` check (415, 400, 406) are described in [JSON and content negotiation](#json-and-content-negotiation).
- CORS headers, compression and static files are described in [CORS](#cors), [Compression](#compression) and [Static resources](#static-resources).

## Roadmap

Security slices:

| Slice | Content |
|---|---|
| 5 (done) | Filter layer, security core, JWT resource server (static keys, JWKS) |
| 6 | `PasswordEncoder`, `UserDetailsService`, HTTP Basic, form login with sessions |
| 7 | CSRF, session management, security headers |
| 8 | Method security (`@PreAuthorize` equivalent) |
| 9 | OAuth2 login and client |

Core features modelled on Spring Boot (application lifecycle, configuration v2, DI v2, logging, conditions and auto-configuration, SSL bundles, health, tasks, test support) are planned in [`docs/ROADMAP-core.md`](docs/ROADMAP-core.md); slices C1 (DI v2), C3 (application lifecycle in `oxbloom-boot`: C3a, the lifecycle, and C3b, failure analyzers and the startup failure report), C4a (conditions and the auto-configuration mechanism) and C4b (the web server and security auto-configurations, property metadata) and C5a (logging: levels, groups, runtime change and startup events), C5b (file and JSON output, request logging) and C6a (filter order, uniform errors, `server.*` keys) and C6b-1 (`Json<T>`, `Request::json`, content negotiation) and C6b-2 (CORS, compression, static resources) and C10a (test support: `ContextRunner`, clocks) and C10b (`TestClient`, `TestApp`, `App::handle_from`) are done; the web layer has no open slice. The overall project plan is in `IDEA.md`.
