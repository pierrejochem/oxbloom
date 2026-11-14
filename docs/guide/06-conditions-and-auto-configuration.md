# Conditions and auto-configuration

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

## Conditions

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

## Properties

The property conditions read an `Environment`, a trait with two methods. `property(key) -> Option<String>` returns the text of a scalar value, and `contains(key) -> bool` says whether the key is present at all; its default is `property(key).is_some()`. `on_property` asks `contains`, and the value conditions ask `property`. Set it with `ContainerBuilder::environment(..)`; without one, every property is absent. With the `di` feature of `oxbloom-config`, `Config` implements `Environment`: it finds the key with the relaxed rules of `Config::get` and returns strings as they are, integers in decimal, floats in TOML form, booleans as `true` or `false` and dates in their TOML text form; tables, lists and absent keys are `None`. Its `contains` also counts a table or a list as present, so `on_property("security.jwt")` matches a `[security.jwt]` section, while `on_property_value` still needs a scalar. On a table or a list, `on_property_value_or_missing` still treats the key as not set, so it matches its default; only `on_property` counts a section as present. Property values are compared as text, so the file integer `port = 8080` and the environment or command-line string `8080` both equal `"8080"`. A secret is compared as plain text and never shown. The expected value of `on_property_value(key, value)` sits in the source code and is compared as plain text, so never put a secret there.

Boot sets `Config` as the environment before the `configure` closures run. This is a visible change: a property condition in a `configure` closure now reads the configuration without any extra call, and a closure that calls `environment(..)` replaces `Config` for every condition, those of the auto-configurations included.

## Groups

`group(name, conditions, f)` applies class-level conditions to every registration that `f` makes. `Conditions` holds the same conditions as the builder (`Conditions::new().on_missing_bean::<dyn Cache>()`).

- The group's conditions are decided once, at the group's first registration that a profile keeps, against the registrations kept before it. The group's own registrations do not count, so a group can register the `T` that its `on_missing_bean::<T>()` asks about, and its beans never hide each other.
- When they do not match, every registration of the group is skipped, nested groups included. When they match, each registration's own conditions are evaluated as usual and combined with the group's by AND.
- Nested groups combine their conditions by AND, the outermost decided first. A group with no registrations is a no-op. A group whose registrations are all skipped by profiles is never decided: its conditions do not run and it leaves no record.
- The start of a group is a barrier for modifiers. A modifier written inside `f` before its first registration finds no registration, so it never changes the registration before the group, and `build()` returns `Invalid`. A modifier written after `group(..)` applies to the last registration made inside it.
- `f` should extend the builder it is given. If it returns a different builder, that builder is used as it is and the group has no effect.

## The conditions report

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

## Auto-configuration

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

## Auto-configured web and security

`oxbloom-boot` ships three auto-configurations, behind its `security` and `web` features, that wire the pieces the framework already has. `oxbloom_boot::auto_configurations()` returns the list for the enabled features, in dependency order: `JwtDecoderAutoConfiguration` and `SecurityAutoConfiguration` with `security`, then `WebServerAutoConfiguration` with `web`. Add the list to the builder with `.auto_configurations(oxbloom_boot::auto_configurations())`. Nothing is wired until you do, and without either feature the function is not available.

You contribute to the defaults with beans, so no code of yours names the server or the chain:

- **Routes.** Register every controller as a `dyn Controller`: `.provides(|c: Arc<HelloController>| -> Arc<dyn Controller> { c })`. `oxbloom_web::Controller` now has `Send + Sync` as supertraits, which `#[controller]` types already satisfy. The web server is built from all controllers; with no `dyn Controller` bean there is no `WebServer` and no `Service`, so `run()` returns right after the runners. A route declared twice fails startup in the `Container` phase with a `RouteError` as the cause.
- **Rules.** Implement `SecurityCustomizer` (`fn customize(&self, http: HttpSecurity) -> HttpSecurity`) and register it as `dyn SecurityCustomizer`. Customizers run in the order the container gives them (ascending `order`, then registration order), each on the result of the one before. `HttpSecurity::authorize` may be called only once in total, so one customizer should own the rules and others can adjust other parts, such as the entry point.
- **Filters.** Register extra filters as `dyn Filter` beans. Each filter declares its slot with `Filter::order()` (see [Errors and filter order](09-errors-and-filter-order.md#errors-and-filter-order)); the server registers the security chain and your filters with `App::filter`, so a filter that keeps the default slot runs after the security chain and never sees a request that the chain rejects.

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

The JWT decoder is switched on by configuration. `JwtDecoderAutoConfiguration` applies only when the `security.jwt` section exists (the group condition is `on_property("security.jwt")`, and for `Config` a table counts as present). It then builds the decoder with `jwt_decoder_from_config`, using an `Arc<dyn JwksFetcher>` bean if you registered one. The keys are those of [Security configuration](08-security.md#security-configuration). A section that is present but wrong, such as HMAC without a secret, fails startup in the `Container` phase, and the failure report names the key but never a value. A secret given on the command line is refused the same way.

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

The metadata contains text about keys only, never values; the description of `security.jwt.secret` says so. The server reads `server.address` (default `127.0.0.1`, an IP address, not a host name), `server.port` (default `8080`) and the other `server.*` keys of [Server properties](09-errors-and-filter-order.md#server-properties). Metadata is written by hand next to the code that reads the keys, and tests pin the two together; there is no generation from structs yet.

## Limits

- Order matters. A condition sees only the registrations kept before it, so `on_bean` and `on_missing_bean` depend on where a registration is made. Register the application's beans first; boot does this for auto-configurations.
- Conditions are ANDed, profile conditions are ORed.
- A condition that panics is not caught: the panic propagates out of `build()`, and out of `run()` in an application.
- Auto-configurations come only from explicit lists. There is no link-time discovery and no exclusion through configuration keys.
- Property values are compared as text, after the conversion described in [Properties](06-conditions-and-auto-configuration.md#properties).
- Condition options are not available on `#[component(..)]`; use the builder modifiers after `.component::<T>()`.
- The missing-bean failure report does not name registrations that a condition skipped; use the conditions report to find them.
- The conditions report and the failure report follow each other without a blank line between them.
- Boot keeps a second copy of the `Config` as the container environment.
