# oxbloom-config design (Slice 4)

Date: 2026-10-24

## Purpose

`oxbloom-config` loads typed runtime settings for Oxbloom applications. Security, AI and the web server will depend on it later. This slice delivers the loader, typed binding through serde, secret redaction, and an optional DI integration.

## Scope

In scope:

- Loading `application.toml` and an optional `application-{profile}.toml` overlay.
- Environment variable overrides.
- Binding a config subtree to a user struct through `serde::Deserialize`.
- A `Secret<T>` wrapper that never prints its value.
- An optional `di` feature that registers `Config` as a singleton in `oxbloom-di`.

Out of scope (later slices):

- A `#[config_properties]` macro in `oxbloom-macros` that binds and auto-registers typed structs. This is planned on top of the serde binding.
- Config sources other than TOML files and environment variables.
- Hot reload.
- Secret managers.
- CLI changes. `oxbloom.toml` stays the build-time component manifest and is not read or written by this crate.

## Decisions

- Runtime config lives in `application.toml`, separate from `oxbloom.toml`.
- Binding uses serde. No custom macro in this slice.
- The loader is our own thin layer on `toml::Value` and serde. The `config` and `figment` crates were rejected as too heavy, and for giving us less control over error messages and secret handling. The public API does not expose the internals, so they can be swapped later.
- `oxbloom-config` does not depend on `oxbloom-web` or `oxbloom-di` unless the `di` feature is enabled.

## Public API

```rust
let config = Config::builder()
    .dir("config")              // directory with application*.toml, default "."
    .profile("dev")             // default: OXBLOOM_PROFILE env var, else none
    .env_prefix("APP")          // default "APP"
    .build()?;

let server: ServerConfig = config.get("server")?;
```

- `Config::builder() -> ConfigBuilder`.
- `ConfigBuilder` methods: `dir(path)`, `profile(name)`, `env_prefix(prefix)`, `env_from(iter)` (injects an environment map instead of reading the process environment, for tests), `build() -> Result<Config, ConfigError>`.
- `Config::from_str(toml: &str) -> Result<Config, ConfigError>` builds a config from a string, with no filesystem or environment access.
- `Config::get<T: DeserializeOwned>(&self, prefix: &str) -> Result<T, ConfigError>` binds a subtree. A dotted prefix such as `"db.pool"` is allowed. An empty prefix binds the whole config.
- `Secret<T>` wraps a value. `Debug` and `Display` print `[REDACTED]`. `expose(&self) -> &T` returns the value. It implements `Deserialize`.

## Loading and precedence

Precedence, highest first:

1. Environment variables.
2. `application-{profile}.toml`.
3. `application.toml`.

Rules:

- Tables merge deeply. Scalars and arrays are replaced whole.
- A missing `application.toml` is not an error and gives an empty base.
- A profile that was requested (through `.profile()` or `OXBLOOM_PROFILE`) but has no file is `ConfigError::Io`.
- The profile name must match `[A-Za-z0-9_-]+`. Anything else is rejected, so a profile cannot be used for path traversal.

## Environment overrides

- A variable named `{PREFIX}_{KEY}` maps to a config key. Strip the prefix, lowercase the rest, and split on `__` to get the path. `APP_SERVER__PORT` becomes `server.port`. A single `_` stays inside a key name, so `APP_DB__MAX_CONNECTIONS` becomes `db.max_connections`.
- Variables without the prefix are ignored.
- Variables with the prefix whose path matches no struct field are ignored, not errors.
- Env values are strings. They replace the TOML node as a string. Coercion happens at `get::<T>` time through a lenient deserializer: when the target field is an integer, float or bool and the node is a string, the string is parsed to that type. A string that does not parse gives `ConfigError::Invalid`. Fields typed as strings are never coerced.

## Errors

`ConfigError` has these variants:

- `Io { path, source }`
- `Parse { path, line, message }`
- `Missing { key }`
- `Invalid { key, message }`

Rules:

- Messages name the key and the file path.
- Messages never contain configuration values. This includes `Parse` errors: the TOML parser's snippet of the offending line is not included in `message`.
- `ConfigError` implements `std::error::Error` and is `Send + Sync + 'static`.

## DI integration (`di` feature)

- Enabling the `di` feature adds a dependency on `oxbloom-di`.
- It provides an extension on `ContainerBuilder`, `config(Config)`, which registers `Config` as a singleton.
- Components take `Arc<Config>` as a field. A provider reads typed settings with `config.get::<T>(prefix)`.
- Without the feature, `oxbloom-config` has no dependency on `oxbloom-di`.

## Dependencies

- `serde` (with `derive`) and `toml`, both used elsewhere in the workspace. Versions come from the workspace and stay consistent with `oxbloom-cli`.
- `tempfile` as a dev-dependency for the file-based integration tests.

## Testing

All tests run without the real process environment (`env_from`) and without network.

Unit tests:

- Deep merge of tables, and whole replacement of scalars and arrays.
- Env key mapping, including `__` versus `_`, prefix filtering, and ignored unknown variables.
- Coercion of integers, floats and bools from strings, a failing parse, and no coercion for string fields.
- Profile overlay precedence, a missing base file, a missing requested profile file, and a profile name with path characters rejected.
- `Missing` and `Invalid` errors name the key.
- `Secret` redaction in `Debug` and `Display`.
- A configured secret value does not appear in the `Display` or `Debug` output of any `ConfigError`.

Integration tests:

- A real temporary directory with `application.toml` and `application-dev.toml`, loaded through the builder.
- With the `di` feature: a component receives `Arc<Config>` from the container and reads a typed setting.

Done means:

- `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` (with and without the `di` feature), and `cargo test --workspace` (with and without `di`) all pass.
- Public items are documented.
- The README has a section for `oxbloom-config`.
- The code graph is updated with `graphify update .`.

## Risks

- The lenient coercion deserializer is the one non-trivial part. It needs thorough tests, including nested structs, `Option<T>` fields and `Vec<T>` fields from TOML, so that coercion only applies to scalar leaves.
- Env override of array values is not supported in this slice. An env var sets a scalar string. This is documented.
