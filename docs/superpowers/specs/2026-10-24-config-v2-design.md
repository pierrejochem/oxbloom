# oxbloom-config v2 design (slice C2: binding, sources, diagnostics)

Date: 2026-10-24

## Purpose

Extend `oxbloom-config` with the binding and diagnostics features of Spring Boot's configuration system: relaxed key matching, list and unit binding, placeholders, more property sources (command line, imports, profile groups), value origins in errors, and renamed-key migration. This is sub-slices C2a (binding and sources) and C2c (diagnostics) of `docs/ROADMAP-core.md`, delivered as one slice because both change how the loader stores values. The `#[config_properties]` macro, validation and metadata generation (C2b) are out of scope and depend on DI v2 (C1).

## Scope

In scope:

- Relaxed key matching across all sources.
- List values from environment and command line (comma-separated and indexed).
- Typed units: `std::time::Duration` and a new `DataSize`.
- `${placeholder}` resolution.
- Command-line property source, `oxbloom.config.import`, profile groups, repeatable profiles.
- Per-value origins, origin-aware errors, `Config::origin`, `Config::explain`, `Config::keys`.
- Renamed and removed key migration with collected warnings.
- Follow-up in `oxbloom-security`: adapt to the changed `ConfigError`, register its secret key, drop the "lists cannot come from environment variables" limitation.

Out of scope:

- `#[config_properties]`, validation, metadata generation (C2b).
- Other file formats (YAML, JSON, properties). TOML only.
- Hot reload and remote config sources.
- Writing logs. Warnings are collected and returned; printing them belongs to the application slice (C3).

## Decisions

- Storage: an own value tree (`Node`) with an origin per node replaces `toml::Value` inside `Config`. Alternatives rejected: a side table of origins (two structures to keep in sync), and the `figment` or `config` crates (cannot give relaxed keys, our placeholder rules or secret-safe errors).
- Key matching: all keys from all sources are normalized.
- Lists: both comma-separated and indexed forms.
- Placeholders resolve against config keys, then environment variables.
- Sources: command line, `oxbloom.config.import` and profile groups are included.
- `std::time::Duration` binds directly from strings.
- Migration: a builder registry that maps old keys to new ones and collects warnings.
- The public API of `Config` (`get`, `from_str`, `builder`) and of `ConfigBuilder` methods that exist today does not change signature. `ConfigError` gains fields and variants.

## Data model

### Node and origin

- `Node` is a table, an array or a scalar (string, integer, float, boolean, datetime). Every node carries an `Origin`.
- `Origin` has these forms:
  - `File { path, line }`: a value from a configuration file, with the path shown relative to the config directory where possible.
  - `Inline { line }`: a value from `Config::from_str`.
  - `Env { name }`: an environment variable, by name.
  - `Cli { arg }`: a command-line argument, by its key part.
- `Origin` implements `Display`: `application-dev.toml:4`, `line 3`, `env APP_SERVER__PORT`, `--server.port`. It never contains a value.
- Table entries are keyed by canonical key. Each entry also stores the spelling used by the winning source, for display and for map-typed fields.

### Canonical key

Lowercase, with `-` and `_` removed. `maxConnections`, `max_connections`, `max-connections` and `MAX_CONNECTIONS` are one key.

- Two spellings of one canonical key in the same file (or `Config::from_str` text) are `ConfigError::DuplicateKey { key, first, second }`, where `first` and `second` are origins. Two spellings of one key in the environment or on the command line are not reported; they collapse to one key and the last variable name in byte order (command line: the last argument) wins.
- Across sources the higher-precedence one wins.
- Known trade-off: `ab` and `a_b` are the same key.

## Load pipeline

Sources, from lowest to highest precedence:

1. `application.toml`, with its `oxbloom.config.import` files merged over it.
2. For each active profile, expanded through profile groups in order: `application-{profile}.toml` and its imports. Later profiles win over earlier ones.
3. Environment variables.
4. Command-line arguments.

Then, in order:

5. Migration of renamed and removed keys.
6. Placeholder resolution over the string leaves from files (amended in the final review: environment and command-line leaves are not resolved).
7. The tree is frozen inside `Config`.

### Profiles

- Active profiles come from, in this order of preference: `ConfigBuilder::profile` calls (repeatable; if any, they are used and nothing else is consulted), then `--oxbloom.profiles.active=a,b` on the command line, then the `OXBLOOM_PROFILE` environment variable (comma-separated), then `oxbloom.profiles.active` in `application.toml`.
- `oxbloom.profiles.active` in a profile file is `ConfigError::Invalid`.
- Profile groups: `oxbloom.profiles.group.<name> = ["db", "metrics"]` in `application.toml`. Activating `<name>` activates its members after it, in order. A group that expands to itself is a cycle and is `Invalid`.
- Limits (final review): groups nest at most 8 levels deep (`Invalid` for `oxbloom.profiles.group.<name>`), and at most 64 profiles are active after expansion, each name counted once, first occurrence kept (`Invalid` for `oxbloom.profiles.active`). Both limits apply whatever named the profiles. Both messages are fixed text.
- `oxbloom.profiles.active` and `oxbloom.profiles.group` are read from `application.toml` and its imports. `APP_OXBLOOM__PROFILES__ACTIVE` and `--oxbloom.profiles.active[0]=a` do not activate profiles; only the `--oxbloom.profiles.active=a,b` form, `OXBLOOM_PROFILE`, `profile` calls and the base-file key do.
- Reserved keys: `oxbloom.config.import`, `oxbloom.profiles.active` and `oxbloom.profiles.group` live under the one reserved top-level table `oxbloom`. A user's own `[config] import = "auto"` or `[profiles] active = true` are ordinary keys; the old unprefixed names (`config.import`, `profiles.active`, `profiles.group`, `--profiles.active=`) are no longer interpreted and silently become plain data. `oxbloom.config.import` is removed from the final tree (an `oxbloom.config` or `oxbloom` table that becomes empty is removed too); `oxbloom.profiles.*` stays in it, so a root struct with `#[serde(deny_unknown_fields)]` must declare an `oxbloom` field or bind a sub-prefix. `Config::from_str` skips imports, profiles, environment, command line and migration, and keeps `oxbloom.config.import` as an ordinary key.
- Profile names must match `[A-Za-z0-9_-]+`, as today. A requested profile without a file is `ConfigError::Io`, as today.

### Imports

- `oxbloom.config.import = ["extra.toml", "optional:local.toml"]` in any configuration file.
- Paths are relative to the config directory. An absolute path, a path containing `..` or one that leaves the config directory is `ConfigError::ImportInvalid { path }`.
- Imported values override the importing file. Imports are processed in list order, later entries win.
- A missing file is `ConfigError::ImportMissing { path }` unless the entry has the `optional:` prefix.
- A file that imports itself, directly or indirectly, is `ConfigError::ImportCycle { files }`. Depth above 8 is `ConfigError::ImportTooDeep`.
- Containment is checked on the canonicalized path, so a symlink that leaves the config directory is `ImportInvalid`. `application.toml` and the profile files themselves are not symlink-checked, only imports are.
- One load follows at most 256 import entries in total (every file, nested imports and repeats included); the 257th is `ConfigError::ImportLimit` (fixed text, no path). This bounds import fan-out, where a file reached many ways is read each time.
- Every configuration file read (base, profile, import) must be a regular file, checked with `std::fs::metadata` after following symlinks and before opening; otherwise `ConfigError::Io` with `ErrorKind::InvalidInput` ("not a regular file"). Files are read through `take(cap + 1)`; over the cap (default 1 MiB, `ConfigBuilder::max_file_size`) is `ConfigError::FileTooLarge { path }` with fixed text. (Final review.)
- `oxbloom.config.import` is removed from the final tree. The `path` fields of `ImportMissing`, `ImportInvalid` and `ImportCycle` hold the file names as written in the config and may appear in `Debug`; `Display` is fixed text.

### Environment variables

As today: prefix (default `APP`), `__` separates path segments, single `_` stays inside a segment, keys normalize to canonical form. A numeric segment after the first creates an array element (see Lists), so numeric map keys (`APP_HTTP__STATUS__404`) cannot come from the environment or the command line. The injectable `env_from` supplies the variables for tests.

Environment and command-line key paths have at most 64 segments (index segments included); more is `ConfigError::Invalid` with a fixed message and the key clipped to 128 characters. Using an index and a table key directly under one path in the same source (`x.0` with `x.foo`) is `ConfigError::Invalid` naming the path. Nesting in TOML files is bounded by the parser.

### Command line

- `ConfigBuilder::args(iter)` takes arguments of the form `--key=value`, with `.` separating path segments and `[n]` or `.n` for list indices. `--key value` (space-separated) is not a property: the argument without `=` is ignored, and an argument not starting with `--` is ignored.
- Arguments follow the same normalization as environment variables, and the same 64-segment cap and index/key conflict rule.
- Command-line arguments are visible in process listings. `ConfigBuilder::secret_key("path")` registers a key that must not come from the command line; a command-line argument for it, or for any key below it, is `ConfigError::Invalid` with a message pointing to environment variables or files. The same check applies to command-line values that `renamed` moves: each moved leaf whose final path equals or lies below a registered secret key is rejected.

## Binding

`Config::get::<T>(prefix)` binds a subtree through the lenient deserializer, now implemented over `Node`.

### Struct, map and enum matching

- For a struct, serde provides the field names (including `rename` and `alias`). A table entry matches the field whose canonical name equals the entry's canonical key, and the declared field name is emitted. A key without a matching field is emitted as written, so `deny_unknown_fields` reports it.
- Two declared fields with the same canonical form are `Invalid` at bind time **when serde's field list shows them as different fields**: serde lists a field's name, `rename` and `alias` names as one ascending run, so a field with a rename and an alias is indistinguishable from two fields whose names happen to be declared in ascending order. The binder reports a collision only when equal canonical names cannot be one field's ascending run (declaration order matters), and the limit is documented on `Config::get`. An exact-name match always wins over a relaxed match. Fields with canonically equal names should be avoided.
- Map fields (`HashMap<String, V>`) get their keys in the spelling of the winning source.
- Unit enum variants match by canonical form.

### Scalars

As today: a string leaf parses to an integer, float or boolean when the target field has that type; string fields are never coerced; error messages use type names and never values.

### Lists

- Indexed form: `APP_X__ALGORITHMS__0=RS256`, `__1=ES256`, or `--x.algorithms[0]=RS256`. A numeric path segment in environment or command-line keys creates an array element. Indices must start at 0 and have no gap, otherwise `ConfigError::Invalid` naming the key. Numeric-keyed maps cannot be set from environment or command line; from files they work.
- Comma-separated form: a string leaf bound to a sequence is split on commas and each item is trimmed. `\,` is a literal comma. A backslash is special only directly before a comma: a value with one backslash before a comma (`a\,b`) is one item `a,b`, a value with two (`a\\,b`) is one item `a\,b`, and a literal backslash cannot end an item. (A TOML basic string `"a\\,b"` and a TOML literal string `'a\,b'` both have a one-backslash value, as does the environment value `a\,b`: all give `a,b`.) A trailing comma yields a trailing empty item (`a,b,` is `["a","b",""]`). An empty or blank string is an empty list. This applies only when the target is a sequence; the tree keeps the string, so placeholders cannot index into it.
- A higher-precedence list replaces a lower-precedence one as a whole. Indexed entries do not merge element by element with a list from a file.

### Duration

- `std::time::Duration` fields bind directly from strings: `500ms`, `30s`, `5m`, `2h`, `1d`, with units `ns`, `us`, `ms`, `s`, `m`, `h`, `d`, and fractions such as `1.5s`. A bare number means seconds.
- Negative values, overflow and unknown units are `Invalid` naming the key. Digits below one nanosecond are truncated. A TOML integer is a number of seconds.
- Implementation: the deserializer recognizes serde's request for `Duration` (a struct named `Duration` with fields `secs` and `nanos`). A test pins this serde behavior. The native table form `{ secs = 3, nanos = 0 }` still works.

### DataSize

- New public type `DataSize` with `bytes() -> u64`, `Display`, `Deserialize`. It parses `512` (bytes), `10MB`, `4KiB`, integers only. `Display` prints `{bytes}B` (`512B`).
- `KB`, `MB`, `GB`, `TB` are 1000-based; `KiB`, `MiB`, `GiB`, `TiB` are 1024-based. This differs from Spring Boot, where `KB` is 1024, on purpose.

### Placeholders

- Syntax: `${name}`, `${name:default}`, and `$${` for a literal `${`.
- A name resolves first to the merged configuration (canonical key match) and then to the environment variable with that exact name (through the injectable environment). A default may itself contain placeholders. Nested names (`${${x}}`) are not supported.
- Resolution runs once, after migration, over every string leaf from a file (`application.toml`, profile files, imports, `Config::from_str` text). Amended in the final review: values from the environment and the command line are taken literally (no `${...}` expansion, `$${` is not unescaped), and a placeholder that refers to such a leaf gets its text as is. The result stays a string; scalar coercion happens at bind time.
- An unresolved name without a default is `ConfigError::UnresolvedPlaceholder { key, name }`; a cycle is `ConfigError::PlaceholderCycle { keys }`. Neither contains a resolved value; `name` is text from a file value, capped at 32 characters plus `…`.
- Depth is one shared limit of 32: leaf-to-leaf references and defaults nested in defaults count together, and exceeding it is `PlaceholderCycle`. A chain of 32 placeholder leaves ending in a plain leaf (33 leaves) still resolves.
- Expansion is budgeted: one resolved string may be at most 1 MiB and one load may resolve at most 16 MiB in total; more is `ConfigError::PlaceholderTooLarge { key }` (fixed text, the leaf key only, no value). The check runs while the string is built, before an oversized allocation.
- For an unterminated `${` the error shows at most 32 characters after the `${` (plus a trailing `…` when it is cut) (never anything after a `:`); write `$${` for a literal `${`, especially in values that may be secrets.
- A resolved leaf keeps the origin of the leaf that contains the placeholder, so a secret that reaches a key through a placeholder from a command-line leaf is not seen as command-line input by origin checks.
- Configuration files are trusted input: a placeholder can read any environment variable. This is documented.
- A resolved leaf keeps the origin of the leaf that contained the placeholder.

## Errors

- `Invalid` and `Missing` keep their fields. `Invalid` gains `origin: Option<Origin>`, filled when the failing value has one.
- `Display` names key, origin and reason, never the value (the one exception is the placeholder name from a file value, see Placeholders): ``invalid configuration key `server.port` (from env APP_SERVER__PORT): expected unsigned integer``. A failing list element is named by index, for example `algorithms[1]`.
- New variants, all with fixed text: `DuplicateKey`, `UnresolvedPlaceholder`, `PlaceholderCycle`, `PlaceholderTooLarge`, `ImportCycle`, `ImportMissing`, `ImportInvalid`, `ImportTooDeep`, `ImportLimit`, `Deprecated`, `FileTooLarge`.
- `Parse` also carries the column when the parser reports it. `Io` is unchanged.
- `ConfigError` is `Send + Sync + 'static` as today.

## Migration and warnings

- Builder registry: `renamed("old.key", "new.key")` and `removed("old.key", "reason")`, matched by canonical key.
- If the old key is present and the new key is unset, the node moves to the new key and keeps its origin. If both are present, the new key wins, the old node is dropped and a warning is produced. A removed key stays in the tree and produces a warning.
- Rules are validated in `build()`, before anything is migrated: an empty or malformed path (empty segment), `old` canonically equal to `new`, or one path being a prefix of the other is `ConfigError::Invalid` with a fixed message (a programmer error).
- After a move, parent tables that the move emptied are removed (never the root), so an emptied table does not flip an optional struct to `Some`. `removed` rules do not prune.
- Rules apply in registration order, so rename chains (`a` to `b`, `b` to `c`) work. Migration runs before placeholders: `${old}` after a rename is unresolved. Renaming out of a secret key does not carry the secret marker to the new key.
- `ConfigWarning { key, origin, kind }` with `kind` being `Renamed { to }` or `Removed { reason }`. A warning never contains a value and implements `Display`.
- `Config::warnings() -> &[ConfigWarning]` returns warnings in discovery order.
- `ConfigBuilder::deny_deprecated(true)` makes `build()` fail with `ConfigError::Deprecated { key, origin }` for the first warning.

## Introspection

- `Config::origin(key) -> Option<&Origin>`, matched by canonical key per segment. Only leaves (scalars and array elements, addressed as `list[0]` or `list.0`) have an origin; a table, an array as a whole and the empty key give `None`, because a table's origin would be the last source that touched it.
- `Config::explain() -> Vec<Entry>` with `{ key, origin, kind }`, one entry per leaf, where `kind` is the value type (`string`, `integer`, `float`, `boolean`, `datetime`). Keys use the spelling of the winning source, array elements are `list[0]`. Values are never revealed and there is no switch to reveal them.
- `Config::keys(prefix) -> Vec<String>` lists leaf keys under a prefix in the same notation.
- Both are sorted by bytes, so `a[10]` sorts before `a[2]`. A quoted key containing `.` is listed but cannot be reached through `origin` or `keys(prefix)`.
- `Config`'s `Debug` stays "top-level key names only".

## Compatibility

- Kept without signature change: `Config::get`, `from_str`, `builder`; `ConfigBuilder::dir`, `profile`, `env_prefix`, `env_from`, `build`; `Secret`; the `di` feature and `ContainerBuilderExt`.
- `ConfigBuilder::profile` becomes repeatable; one call behaves as before. `OXBLOOM_PROFILE` accepts a comma-separated list; one name behaves as before.
- `ConfigError` gained variants and `Invalid` gained `origin`. Amended in the final review: `ConfigError`, `Origin` and `WarningKind` are `#[non_exhaustive]` before the first release, so matches outside the crate need a `_` arm and `Invalid { key, message }` patterns need `..`. Two spellings of one key in a single file are now `DuplicateKey` instead of serde's "duplicate field" error.
- Behavior changes, all documented in the README: keys in files match case-insensitively and ignore `-`/`_`; plain `Duration` fields parse strings; a comma-separated string binds to a `Vec` field; `${` in a string from a file is special and a literal needs `$${`; a numeric environment or command-line segment is a list index (`APP_HTTP__STATUS__404=x`, which used to set the map key `404`, is now an `Invalid` gap error; numeric map keys work from files only); map keys are canonical (two spellings of one map key in one file are `DuplicateKey`, an environment override replaces a map key's spelling with the lowercased variable spelling); `oxbloom.config.import`, `oxbloom.profiles.active` and `oxbloom.profiles.group.*` are under the reserved `oxbloom` table (the old unprefixed names became plain data); configuration files must be regular files of at most 1 MiB by default.
- `oxbloom-security` is updated in the same slice: it adapts to the changed `ConfigError` construction, registers `security.jwt.secret` with `secret_key`, and its documentation no longer says `algorithms` and `audience` cannot be set from environment variables. Its existing alias keys stay and become redundant.
- Security follow-up decided during implementation: `jwt_decoder_from_config` also rejects a secret whose origin is `Origin::Cli`, with the same `Invalid` as the command-line source, whether or not the caller registered `SECRET_KEY` (defense in depth, fails closed); empty or whitespace-only items in `audience` and `algorithms` are `ConfigError::Invalid` naming the key, so `a,,b` cannot admit a token with `aud: ""`.

## Dependencies

No new runtime dependency is planned. `toml` 1.1.8 already provides spans; the first plan task verifies which API gives per-value spans in this version and falls back to a line-offset index over the source text if needed. `tempfile` stays a dev-dependency.

## Testing

All tests are offline and use `env_from` and `args` for every input.

- Key normalization: four spellings bind to one field; same-source collision names both origins; cross-source precedence; enum variants; map key spelling; two fields with one canonical form.
- Lists: comma form, `\,` escape, empty string, indexed form from environment and command line, gap in indices, replace-whole across sources, list of structs from indexed environment.
- Units: every `Duration` unit, fractions, bare number, negative, overflow, unknown unit; `DataSize` 1000 versus 1024; the pinning test for serde's `Duration` request; native table still works.
- Placeholders: config key, environment fallback, default, nested default, escape, cycle, unresolved, placeholder in an imported file, coercion of the result, no value in any error.
- Sources: command-line precedence; ignored and rejected argument forms; the `secret_key` rule; import order, `optional:`, cycle, depth, absolute and `..` rejection; profile groups and their cycle; `oxbloom.profiles.active` in base file works and in a profile file is rejected; profile preference order.
- Migration: renamed with new key unset, both set, removed, canonical matching, `deny_deprecated`.
- Origins: each source type reports the right origin; line numbers for nested tables and arrays; `explain` masks every value; `keys` lists leaves.
- Hygiene: a unique marker value passes through file, environment, command line, import and placeholder default, and appears in no `Display` or `Debug` of any error, warning, origin or `explain` entry.
- Compatibility: all existing `oxbloom-config` and `oxbloom-security` tests pass with only the mechanical `ConfigError` change in security.
- Property-style tests without new dependencies: normalization is idempotent; table merge is associative.
- Doc examples for every new public item per `AGENT.md`.

Done means: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` and `cargo test --workspace` pass, each also with `-p oxbloom-config --features di` and with `-p oxbloom-security --all-features`; the README configuration section, `docs/API.md` and crate docs are updated; `graphify update .` has run.

## Risks

- Rewriting merge and the deserializer is the riskiest part. The existing tests and the extended hygiene test are the guard.
- Canonical keys can make a typo match something unintended. This is the accepted cost of relaxed binding; `deny_unknown_fields` still catches non-matching names.
- Placeholders let configuration files read environment variables. Documented, files are trusted.
- The `Duration` approach relies on serde's std implementation details. The pinning test turns a serde change into a clear failure.
- The slice is large. The plan splits it into tasks: node tree and origins; key normalization; source pipeline (imports, profiles, command line); lists; units; placeholders; migration; errors and introspection; security follow-up; documentation.
