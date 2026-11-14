# Configuration

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

## Sources and precedence

Lowest to highest: `application.toml` and its imports, the files of the active profiles (`application-{profile}.toml`, plus their imports) in activation order, environment variables, command-line arguments. After that, renamed and removed keys are migrated, then placeholders are resolved. Tables merge deeply; scalars and arrays are replaced whole. A missing `application.toml` gives an empty base. Every file read (`application.toml`, profile files, imports) must be a regular file after following symlinks, otherwise it is `ConfigError::Io` (a FIFO or a device is never opened), and may be at most 1 MiB (`.max_file_size(bytes)`), otherwise it is `ConfigError::FileTooLarge`. `Config::from_str` parses its text only: no imports, profiles, environment, command line or migration, and a `oxbloom.config.import` key in it stays an ordinary key; `build()` follows and removes it.

## Keys

Keys match relaxed: case, `-` and `_` are ignored (`maxConnections`, `max-connections` and `MAX_CONNECTIONS` are one key and bind the field `max_connections`). Two spellings of one key in the same file are `ConfigError::DuplicateKey`; across sources the higher-precedence source wins, so `APP_MAX_CONNECTIONS` overrides `max-connections` from a file. Map keys are canonical too: two spellings of one map key (`my-host` and `my_host` under `[aliases]`) in one file are `DuplicateKey`, and a map key keeps the spelling of the winning source, so an environment override replaces it with the lowercased variable spelling. Limits: an exact field name wins over a relaxed match, but detecting two canonically equal field names in one struct (`jwt_secret` and `jwtsecret`) is not complete, so do not declare them; fields under `#[serde(flatten)]` and internally tagged enums match exactly.

Reserved top-level table: `oxbloom`. Only `oxbloom.config.import`, `oxbloom.profiles.active` and `oxbloom.profiles.group.*` are interpreted by the loader; a user's own `[profiles]` or `[config]` tables (for example `[profiles] active = true` or `[config] import = "auto"`) are ordinary keys and load as plain values. `oxbloom.config.import` is removed by `build()` (and `oxbloom.config`, then `oxbloom`, when that leaves them empty); `oxbloom.profiles.*` stays in the final tree and is visible to `get`, `origin`, `explain` and `keys`, so a root struct with `#[serde(deny_unknown_fields)]` must declare an `oxbloom` field or be bound below the root (for example `config.get("server")`).

## Profiles

- Active profiles come from the first of these that is non-empty: `.profile("a")` calls (repeatable, applied in call order, later wins), `--oxbloom.profiles.active=a,b`, `OXBLOOM_PROFILE=a,b`, `oxbloom.profiles.active` in `application.toml` (a string `"a,b"` or an array). Names allow only letters, digits, `_` and `-`.
- `oxbloom.profiles.active` in a profile file is `ConfigError::Invalid`. `oxbloom.profiles.active` and `oxbloom.profiles.group` are read from `application.toml` and its imports. The environment variable `APP_OXBLOOM__PROFILES__ACTIVE` and the indexed form `--oxbloom.profiles.active[0]=a` do not activate profiles; they only set the key.
- Groups: `oxbloom.profiles.group.prod = ["db", "metrics"]` in `application.toml` (or its imports) activates `db` and `metrics` after `prod`. Group definitions elsewhere (profile files, environment) are ignored. The group's own file (`application-prod.toml`) is required like any profile file: a missing file is `ConfigError::Io`. A group that includes itself is `Invalid`. Groups nest at most 8 levels deep (`Invalid` for `oxbloom.profiles.group.<name>`), and at most 64 profiles are active after expansion, repeats counted once (`Invalid` for `oxbloom.profiles.active`); both limits apply to every way of naming profiles.

## Imports

`oxbloom.config.import = ["extra.toml", "optional:local.toml"]` in any config file (including profile files). Paths are relative to the config directory; an absolute path or `..` is `ImportInvalid`, and so is a file that resolves (symlinks included) outside the directory. A missing file is `ImportMissing` unless prefixed `optional:`; a cycle is `ImportCycle`; more than 8 levels is `ImportTooDeep`; more than 256 entries followed in one load (repeats count) is `ImportLimit`. Imported values override the importing file, later entries win, and `oxbloom.config.import` itself is not part of the final tree. `application.toml` and the profile files themselves are not symlink-checked, only imports are. The `Debug` output of the `Import*` errors contains the path as the developer wrote it; `Display` does not.

## Environment

`APP_SERVER__PORT=9000` sets `server.port`. The prefix is `APP` (change it with `.env_prefix("MYAPP")`); `__` separates segments, a single `_` stays in the key name. A numeric segment after the first is a list index (`APP_HOSTS__0=a`), so a map with numeric keys can only come from files. A variable name with more than 64 segments is `Invalid`, and so is mixing an index and a key under one path in the same source. Keys are lowercased and match relaxed. Values are strings. Two spellings of one key in the environment or on the command line are not reported; they collapse to one key and the last variable name in byte order (command line: the last argument) wins.

## Command line

`.args(iter)` reads `--key=value`. `.` separates segments, `[n]` or `.n` is a list index (`--hosts[0]=a`, `--hosts.1=b`). Ignored: arguments not starting with `--`, without `=`, or with an empty key (`--verbose`, `--key value`, `--=x`); `--key=` sets an empty string. A repeated key keeps the last value, and two spellings of one key collapse to one with the last argument winning (not reported). The same 64-segment cap and mixed index/key rule apply as for the environment. Command-line arguments are visible in process listings, so `.secret_key("security.jwt.secret")` registers keys (and everything below them) that must not come from the command line; such an argument is `Invalid` and points to environment variables or files. The same check applies to values moved there by `renamed`.

## Lists

- Comma-separated (a string bound to a `Vec`, from any source): `"a, b"` gives `["a","b"]`. Items are trimmed. A backslash is special only directly before a comma: a value with one backslash before a comma (`a\,b`) is one item `a,b`; a value with two (`a\\,b`) is one item `a\,b`. Mind the quoting of the source: the TOML basic string `"a\\,b"` has a one-backslash value, so it gives `a,b`, as does the literal string `'a\,b'` and the environment variable `a\,b`; to get the item `a\,b` write `"a\\\\,b"`, `'a\\,b'` or `a\\,b`. A trailing comma gives a trailing empty item (`"a,b,"` is `["a","b",""]`, which fails for `Vec<u16>`). An empty or blank string is an empty list. A string bound to a `String` field is never split. The tree keeps the string, so a placeholder cannot index into it (`${hosts.0}`).
- Indexed: `APP_HOSTS__0=a`, `--hosts[1]=b`. Indices start at 0; a gap is `Invalid`. Lists of structs work (`APP_SERVERS__0__HOST=a`).
- A list from a higher-precedence source replaces the lower one whole; indexed entries do not merge with a list from a file.

## Units

- `std::time::Duration`: `ns`, `us`, `ms`, `s`, `m`, `h`, `d`, fractions (`1.5s`), a bare number is seconds. Negative values, overflow and unknown units are `Invalid`; digits below one nanosecond are truncated. A user struct literally named `Duration` with the fields `secs` and `nanos` is treated as `std::time::Duration`. The table form `{ secs = 3, nanos = 0 }` still works.
- `DataSize` (`oxbloom_config::DataSize`): `KB`, `MB`, `GB`, `TB` are 1000-based, `KiB`, `MiB`, `GiB`, `TiB` are 1024-based (this differs from Spring Boot, where `KB` is 1024), `B` or a bare number is bytes. Integers only (`1.5KB` is `Invalid`). `bytes()` returns the count; `Display` prints `{bytes}B`.

## Placeholders

`${name}`, `${name:default}` (the default may contain placeholders). A name is looked up as a config key first, then as an environment variable of that exact name (the process environment, or the pairs given to `env_from`; `Config::from_str` has no environment). `$${` is a literal `${`. Placeholders are resolved only in values from files (`application.toml`, profile files, imports, `Config::from_str` text). Environment and command-line values are taken literally: `${` and `$${` in them stay as written, and a placeholder that refers to such a value gets its text as is. Resolution runs once over every file string leaf; the result stays a string until it is bound, so `port = "${PORT:8080}"` binds to a `u16`. No nested names (`${${x}}`). An unknown name without default is `UnresolvedPlaceholder`; a cycle, or nesting deeper than 32 (leaf references and defaults share the limit; a chain of 32 placeholder leaves ending in a plain one resolves) is `PlaceholderCycle`; a result over 1 MiB per leaf or 16 MiB per call is `PlaceholderTooLarge`. Errors name the key and the placeholder name, never a resolved value. That name is the one exception to "errors never contain values": it is text from a file value, capped at 32 characters plus a trailing `…` when cut, and for an unterminated `${` it is the text after the `${` (never anything after a `:`). Write `$${` for a literal `${` in files, especially in values that may be secrets. Config files are trusted: a placeholder can read any environment variable. A resolved leaf keeps the origin of the leaf that contains the placeholder, so a secret that reaches a key through a placeholder is not seen as command-line input by origin checks.

## Migration

```rust
let config = Config::builder()
    .renamed("server.hostname", "server.host")
    .removed("legacy.mode", "no longer supported")
    .build()?;
for warning in config.warnings() { eprintln!("{warning}"); }   // never contains values
```

An old key moves to the new one and keeps its origin; if both are set the new one wins. Either way a `ConfigWarning` is recorded; a removed key stays in the tree and warns. `.deny_deprecated(true)` turns the first warning into `ConfigError::Deprecated`. Rules are checked in `build()`: an empty or malformed path, old equal to new (canonically) or one inside the other is `Invalid`. Tables emptied by a move are removed. Rules apply in registration order, so chains work. Migration runs before placeholders, so `${old}` after a rename is unresolved. A renamed command-line value that would land on a registered secret key is rejected.

## Introspection and errors

- `config.origin("server.port")` returns the `Origin` of a leaf (file and line, `line N` for `from_str`, `env APP_X`, `--key`). Tables and arrays as a whole have no origin.
- `config.explain()` lists every leaf as `{ key, origin, kind }`; `config.keys("db")` lists leaf keys under a prefix. Neither ever returns values. Array elements are `list[0]`. Sorting is by bytes, so `a[10]` comes before `a[2]`. A quoted key containing `.` is listed but cannot be reached through `origin` or `keys`.
- Errors name the key, the origin and the reason, never the value; the one exception is the placeholder name of a file value (see Placeholders): ``invalid configuration key `server.port` (from env APP_SERVER__PORT): expected unsigned integer``. A failing list item is named `hosts[1]`.
- `Secret<T>` prints `[REDACTED]` in `Debug` and `Display`; read it with `expose()`. Messages from a custom `Deserialize` impl are replaced by a fixed text. `Config`'s `Debug` prints top-level key names only.
- Environment values are strings. `get` parses them into the integer, float or bool a field expects. Fields typed as strings are never coerced.
- With the `di` feature, `ContainerBuilder::config(config)` registers `Config` as a singleton, and components can take an `Arc<Config>` field.

## Behavior changes from earlier versions

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
