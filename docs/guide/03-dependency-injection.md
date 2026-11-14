# Dependency injection

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

## Registrations

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

## Injectable shapes

The same shapes work as `#[component]` fields, as `bean` parameters and through `Resolver::get`, `get_opt`, `get_all` and the matching `Container` methods. The set of shapes is closed: the `Inject` trait is sealed.

| Shape | Meaning |
|---|---|
| `Arc<T>`, `Arc<dyn Trait>` | exactly one provider, or the single `primary` provider; otherwise `Missing` or `Ambiguous` |
| `Option<Arc<..>>` | `None` when nothing provides the key; a provider that fails to create is still an error |
| `Vec<Arc<..>>` | every provider, sorted by `order` and then by registration order; empty when there is none |

## Modifiers

A modifier applies to the registration just before it. A modifier before any registration, or one whose type does not match the last registration, makes `build()` return `Invalid`; the builder methods themselves never fail.

- `.provides(|a: Arc<Impl>| -> Arc<dyn Trait> { a })` also registers the bean as a trait object. The trait must be `Send + Sync`. `#[component(provides(dyn Trait))]` does the same.
- `.primary()` picks this provider when a single `Arc<dyn Trait>` has several providers. Without exactly one `primary` provider, a single request is `Ambiguous`.
- `.order(n)` sets the position in `Vec` requests (ascending, default `0`).
- `.lazy()` creates the bean on its first request instead of during `build()`. An eager bean that depends on a lazy one creates it during `build()`. A lazy bean's errors appear at first use.
- `.post_construct(|t: &T| ...)`, `.pre_destroy(|t: &T| ...)` and `.lifecycle::<T>()` (for `impl Lifecycle for T`) add lifecycle hooks.
- `.profile("dev")`, `.profile("!prod")` and `.profile_not("prod")` add profile conditions.
- `.on_bean::<T>()`, `.on_missing_bean::<T>()`, `.on_property(..)`, `.on_property_value(..)`, `.on_property_value_or_missing(..)` and `.condition(..)` add conditions; see [Conditions and auto-configuration](06-conditions-and-auto-configuration.md#conditions-and-auto-configuration).

`#[component(..)]` takes the options `provides(dyn A, ..)`, `primary`, `order = n`, `lazy`, `lifecycle` and `profile = "dev" | "!prod" | ["dev", "test"]`. They are applied through `Component::configure`, which a hand-written `Component` can implement too.

## Profiles

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

## Lifecycle and shutdown

`post_construct` runs once, right after the bean is created and after the hooks of its dependencies; no dependent receives the bean before its hook succeeded. A failing hook is a `Creation` error. If `build()` fails, the beans that were already created are destroyed in reverse creation order before the error is returned.

`container.shutdown()` runs the `pre_destroy` hooks of every created bean in reverse creation order and returns a `ShutdownReport` with the fields `destroyed` and `failures` (both report structs are `#[non_exhaustive]`). A panicking `pre_destroy` is caught and reported as a `DestroyFailure` with fixed text. A second call returns an empty report. After shutdown every `get*` returns `ShutDown`; `Arc` handles handed out earlier stay valid. Dropping a container does not shut it down, and hooks are not async.

Lazy creation and shutdown share one container-wide lock that is reentrant for the creating thread. Concurrent first requests create a lazy bean once, and two threads never deadlock on each other's lazy chains. A lazy provider must not wait for another thread that itself creates a lazy bean of the same container. `shutdown` waits for a lazy creation in flight and destroys that bean too. A `shutdown` called from inside a provider or `post_construct` hook on the creating thread closes the container at once: the rest of that chain creates nothing and gets `ShutDown`, and nothing created is left undestroyed.

## Errors

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

## Limits

- Dependency depth is capped at 256, and `bean` / `try_bean` take at most 12 parameters.
- Creation is recursive, so a deep graph needs stack. A 256-deep chain on x86_64 Linux needs about 206 KB of stack in a debug build for simple closures, 278 KB for `#[component]` structs with four fields, 298 KB for `.bean` functions with four parameters and 322 KB for `#[bean]` methods with four parameters. `DiError` stayed at 56 bytes when `Creation` gained its `source` field, so these figures did not change. Release builds need 79 to 119 KB. The main thread has 8 MB; run very deep graphs on a thread with enough stack.
- Only singletons: transient and request scopes, injection of single configuration properties, and qualifiers by name are not implemented.

## Compatibility with the first container

- Unchanged: `Component`, `ContainerBuilder::{new, component, instance, factory, build}`, `Container::get` (it now also accepts `dyn Trait`), `Resolver::get`, and `DiError::{Missing, Cycle, Creation}`.
- Changed: `DiError` is `#[non_exhaustive]`, `Duplicate` gained `sources`, and `Container::get` after `shutdown()` is an error. Code that matches `DiError` exhaustively or destructures `Duplicate { type_name }` must be updated.
- Behavior: creation errors are cached per bean, and resolution depth is capped at 256.
