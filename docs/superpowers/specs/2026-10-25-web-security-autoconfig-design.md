# C4b: Auto-configure the web server, security chain and JWT decoder

Date: 2026-10-25. Slice C4b of `docs/ROADMAP-core.md`. Depends on C4a (conditions and the auto-configuration mechanism), C3 (boot, services, failure analyzers) and Slice 5 (security).

## Goal

An application that enables the `web` and `security` features of `oxbloom-boot` gets a working `WebServer`, a `SecurityFilterChain` and, when `security.jwt` is configured, a `JwtDecoder`, without writing the factory functions itself. User code contributes routes, security rules and filters through beans. A user bean of the same type replaces the matching default. Each auto-configuration also describes its configuration keys as `PropertyMetadata`, which the application can list and dump as JSON.

## Decisions already taken (brainstorm, 2026-10-25)

- Contributor beans are the extension model. Replace-only and automatic controller collection were rejected.
- Property metadata means runtime descriptors. A build-time processor and a derive macro were rejected.
- The auto-configurations live in `oxbloom-boot`, behind its `web` and `security` features. `oxbloom-web` and `oxbloom-security` cannot host them, because `oxbloom-boot` depends on both and `AutoConfiguration` is a boot trait.
- Opt-in stays explicit, as decided in C4a: the application passes the list to the builder. There is no link-time registry.

## Non-goals

The full `server.*` property set (C6), the `registry` feature, exclusion by configuration key, startup logging (C5), macro-generated metadata, and checking metadata against the real config structs at compile time.

## Contributor beans

Routes: the existing `oxbloom_web::Controller` trait is the route contributor. A controller registers as `dyn Controller` with `provides(|c: Arc<MyController>| -> Arc<dyn Controller> { c })`. No new trait is added. If `Arc<dyn Controller>` does not satisfy the DI bounds (`Send + Sync`), `Controller` gets a `Send + Sync` supertrait; this is allowed because every `#[controller]` type is a component and already `Send + Sync`.

Security rules: `SecurityCustomizer`, a new trait in `oxbloom-boot` (feature `security`):

```rust
pub trait SecurityCustomizer: Send + Sync {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity;
}
```

Customizers are injected as `Vec<Arc<dyn SecurityCustomizer>>`, which the container sorts by `order` and then by registration order. They are applied in that order, each on the result of the one before.

Filters: beans registered as `dyn Filter` are injected as `Vec<Arc<dyn Filter>>`, ordered the same way, and added to the `App` after the security chain.

## Auto-configurations

All three implement `AutoConfiguration`, are `#[component]`s, and are returned by `oxbloom_boot::auto_configurations()` in this order.

### `JwtDecoderAutoConfiguration` (feature `security`)

- Conditions: `on_property("security.jwt")` (group level) and `on_missing_bean(dyn JwtDecoder)` on the bean.
- Bean: `Arc<dyn JwtDecoder>`, built with `jwt_decoder_from_config(&config, fetcher)`, where `fetcher` is an optional `dyn JwksFetcher` bean.
- A broken `security.jwt` section fails the build as `DiError::Creation` with the `ConfigError` as typed source. The existing config-key and security analyzers report it.

### `SecurityAutoConfiguration` (feature `security`, after `JwtDecoderAutoConfiguration`)

- Condition: `on_missing_bean(SecurityFilterChain)`.
- Bean: `SecurityFilterChain`, built from `HttpSecurity::new()`:
  1. The customizers are applied in order.
  2. If no customizer was registered, the default rule is `authorize(|a| a.any_request().authenticated())`. The application is closed unless it opens routes.
  3. If an `Option<Arc<dyn JwtDecoder>>` bean exists, `oauth2_resource_server(|r| r.jwt(decoder))` is added.
  4. `build()`; a `SecurityBuildError` becomes `DiError::Creation` with a typed source.

### `WebServerAutoConfiguration` (features `web` and `security`, after `SecurityAutoConfiguration`; with `web` only, the chain is absent)

- Conditions: `on_bean(dyn Controller)` and `on_missing_bean(WebServer)`. A server without routes would only answer 404, so no controller means no server.
- Bean: `WebServer` plus `provides(dyn Service)`. It builds the `App` from the optional `Arc<SecurityFilterChain>`, the ordered filters, and the routes of every `dyn Controller`, then calls `WebServer::new(app, &config)`.
- A duplicate route is a `RouteError`; the bean fails with `DiError::Creation` and the `RouteError` as source. A bad `server.address` or `server.port` is a `ConfigError` source, as today.

### Entry point

`oxbloom_boot::auto_configurations() -> AutoConfigurations` returns the list for the enabled features (`security` adds the first two, `web` adds the third). `exclude::<A>()` removes any of them, as for every auto-configuration.

## Property metadata

```rust
pub struct PropertyMetadata {
    pub key: &'static str,
    pub type_name: &'static str,
    pub default: Option<&'static str>,
    pub description: &'static str,
}
```

- `AutoConfiguration` gains `fn properties() -> &'static [PropertyMetadata] { &[] }`.
- The three auto-configurations declare their keys: `server.address`, `server.port`, `security.jwt.*` as exactly the keys that `jwt_decoder_from_config` reads. The secret key is described but never carries a value.
- `oxbloom_boot::boot_properties()` lists the boot keys: `oxbloom.config.import`, `oxbloom.profiles.active`, `oxbloom.profiles.group`, `oxbloom.main.banner-mode` and `oxbloom.main.conditions-report` (see the Amendments: the first version of this text listed four keys). 
- `OxbloomApplicationBuilder::property_metadata() -> Vec<PropertyMetadata>` returns the boot keys plus the keys of the registered auto-configurations, without the excluded ones, sorted by key, without duplicates.
- `PropertyMetadata::to_json(list: &[PropertyMetadata]) -> String` writes a stable JSON array of objects with the fields in the order above. Strings are escaped. A missing default is `null`.
- A test per declared key reads the code path that consumes it and fails when a key is renamed in code without updating the metadata.

## Failure behavior

- A user bean for `SecurityFilterChain`, `WebServer` or `dyn JwtDecoder` makes the matching bean skip; the conditions report shows why.
- User customizers and filters are never skipped.
- Every failure above is a `Container`-phase `StartupError` and goes through the existing analyzers; the new tests check that the report names the key.

## Migration of `examples/hello`

- Remove `security_chain` and `web_server` from `examples/hello/src/web.rs`.
- `HelloController` is registered `provides(dyn Controller)`.
- A `PublicPaths` bean implements `SecurityCustomizer` with the current rules (`/api/hello/**` and `/api/greetings/**` public, everything else authenticated).
- The builder gets `.auto_configurations(oxbloom_boot::auto_configurations())`.
- The existing example tests pass without change; this is the compatibility proof.

## Testing

- Per auto-configuration: matches and skips (property absent, user bean present, no controller), checked through the condition report.
- Integration over real HTTP: 401 by default, a valid JWT accepted, a user-supplied chain overrides the default, no controller means no server, a duplicate route fails startup with a report.
- Metadata: JSON golden text, exclusion removes keys, each declared key is consumed by code.
- A doc example for every public item.
- Stack probes re-run after the change; the boot probe was 241 KB and `build`'s frame 0x178.

## Documentation

`README.md` (a section on auto-configured web and security), `docs/API.md`, `AGENT.md` and `docs/ROADMAP-core.md` (C4b done, follow-ups listed).

## Open risks

- `dyn Controller` may need the `Send + Sync` supertrait (see above).
- `on_bean(dyn Controller)` at group level depends on C4a evaluating group conditions against registrations kept before the group. User `configure` closures run first, so user controllers count.

## Amendments

- The existing analyzers did not cover a `ConfigError::Invalid`: only `Missing` had one, and an invalid key in general deliberately gets no analysis. Boot therefore adds `JwtConfigAnalyzer` (feature `security`), which explains an `Invalid` error only when its key is `security.jwt` or lies under it. Its description names the key and the message, which never contains the value.
- `on_property("security.jwt")` could not match a section, because `Config` returns no property text for a table. `on_property` now asks `Environment::contains`, which `Config` answers with true for a table or a list (see the C4a spec, amendment 17).
- `oxbloom_web::Controller` gained `Send + Sync` supertraits. `Arc<dyn Controller>` did not satisfy the bounds of `provides` without them. `#[controller]` types and the other implementors in the workspace needed no change.
- The `autoconfig_no_impls` trybuild cases (`exclude_not_auto_configuration`, `not_a_component`) were moved out of `tests/ui/autoconfig` and run only when neither `security` nor `web` is on, because rustc lists the built-in `AutoConfiguration` types in their output when they exist.
- `auto_configurations()` and the `autoconfigure` module are available with `security` or `web`; the list contains only the entries of the enabled features.
- `boot_properties()` lists five keys, not four: `oxbloom.config.import` (read by the config loader, an array of strings, no default) was missing. The sentence "these are the only `oxbloom.*` keys the code reads today" was wrong. A grep of the workspace found no other `oxbloom.*` key that the code reads. Tests in `tests/metadata.rs` set each of the five keys through `Config::from_str` or the config builder and check its effect.
- `?` through `From<ConfigError>` and `From<SecurityBuildError>` made a failure report say `failed to create ...ConfigError`. The three auto-configurations now use `DiError::creation_from::<Bean>`, so the message names the bean and the typed source is kept.
