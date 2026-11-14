# oxbloom-cli: `new` and `list` — Design

Date: 2026-10-23
Status: awaiting review

## Purpose

Let a user run `oxbloom new <name>` and get a project that compiles, runs, and passes its own test, with no manual editing. The CLI must be fully scriptable (flags only) and must produce identical output for identical input and CLI version.

## Scope

In scope:
- New crate `crates/oxbloom-cli`, binary name `oxbloom`.
- Commands `new` and `list`.
- The `oxbloom.toml` manifest, as the single source of truth for what a generated project contains.
- One generated component: `web = "rest"`, built on the existing `oxbloom-web` crate.

Out of scope (later slices): `oxbloom add`, interactive menus, `--web mvc`, security, database, jobs, tests-scaffolding flag, AI, React frontend, publishing to crates.io.

## Principle: no flag that does nothing

Only components that are implemented and tested may be selectable. The flags `--security`, `--database`, `--jobs`, `--ai`, `--tests` and `--frontend` are accepted by the parser so the failure is explicit, and each exits with a non-zero status and the message `<feature> is not implemented yet`. They never write anything into the manifest.

## Commands

### `oxbloom new <name>`

| Option | Default | Meaning |
|---|---|---|
| `--web <rest>` | `rest` | Web style. Only `rest` accepted. |
| `--oxbloom-path <dir>` | workspace the CLI was built from | Directory containing the oxbloom workspace; used for path dependencies. Must contain `crates/oxbloom-web/Cargo.toml`. |
| `--dir <parent>` | current directory | Parent directory; project is created at `<parent>/<name>`. |

Rules:
- `<name>` must match `[a-z][a-z0-9_-]*`. Anything else is rejected before touching the filesystem.
- The target directory must not exist or must be empty; otherwise exit non-zero without writing.
- On any failure after the target directory was created by this run, the partially written directory is removed. A directory that existed beforehand (empty) is left in place but emptied of files this run created.

### `oxbloom list`

Prints one line per component: `web   available (rest)` and `security, database, jobs, ai, tests, frontend   planned`. Output is static text derived from a single component table in the code, which also drives the "not implemented yet" check, so the two cannot disagree.

## Manifest: `oxbloom.toml`

```toml
[project]
name = "demo"
cli_version = "0.1.0"

[components]
web = "rest"
```

- Parsed with serde and `toml`; `#[serde(deny_unknown_fields)]` on every table.
- `cli_version` is the version of the CLI that generated the project; it is informational in this slice.
- Generation flow: build the manifest from CLI arguments, serialize it, then render every other file from the in-memory manifest value. Rendering never reads CLI arguments directly, so a later `oxbloom add` can regenerate from a manifest file alone.

## Generated project

```
<name>/
  Cargo.toml
  oxbloom.toml
  .gitignore
  src/lib.rs
  src/main.rs
  tests/hello.rs
```

- `Cargo.toml`: package `<name>`, edition 2024, dependencies `oxbloom-web` (path dependency to `<oxbloom-path>/crates/oxbloom-web`), `tokio` with features `rt-multi-thread`, `macros`; dev-dependency `tokio` with `rt`, `macros`. `tokio` and `http`/`bytes` versions are the same as in the oxbloom workspace `Cargo.lock` at CLI build time, injected as constants, so a generated project resolves the same versions.
- `src/main.rs`: a `HelloController` mounted at `/api/hello` with `GET ""` returning a greeting, and `main` that reads `PORT` (default 8080) and calls `App::run`. The router construction lives in a `pub fn app() -> App` in `src/lib.rs` so the test can use it; `main.rs` only binds and runs. (`src/lib.rs` is therefore generated too.)
- `tests/hello.rs`: builds the `App`'s router through the library and asserts `GET /api/hello` returns 200 with the greeting.
- `.gitignore`: `/target`.

## Reproducibility

- Files are written in a fixed, sorted order.
- No timestamps, usernames, or random values appear in any generated file.
- The only machine-specific value is the absolute oxbloom path; it is an explicit input (`--oxbloom-path`), so same flags give same output.

## Module layout inside the crate

| Module | Responsibility | Depends on |
|---|---|---|
| `components` | Table of components and their status; used by `list` and flag validation | none |
| `manifest` | `Manifest` types, serialize/parse | serde, toml |
| `naming` | Project-name validation | none |
| `templates` | Pure functions `Manifest + paths -> Vec<(path, contents)>` | `manifest` |
| `generate` | Writes the file list to disk, handles cleanup on failure | `templates` |
| `cli` / `main` | clap definitions, wiring, exit codes | all above |

`templates` and `manifest` do no I/O, so they are unit-testable without a filesystem.

## Dependencies

`clap` (derive), `anyhow`, `serde` (derive), `toml`. Dev: `tempfile`. Versions come from `cargo add` and live in `[workspace.dependencies]` only if shared with another crate.

## Error handling

All user errors (bad name, existing non-empty directory, unimplemented feature, missing oxbloom path) produce a single-line message on stderr and exit code 2 for usage errors, 1 for I/O errors. No panics on user input.

## Testing

Unit tests:
- Name validation: accepts `demo`, `my-app`, `a1`; rejects empty, `1app`, `Demo`, `a b`, `../x`.
- Manifest round-trip and rejection of unknown fields.
- Each unimplemented flag yields the expected error and writes nothing.
- Templates: generated `Cargo.toml` contains the expected path dependency; file list is exactly the six files above in sorted order.
- Determinism: generate twice into two temp dirs with the same flags, compare all file bytes.
- Non-empty target directory is refused and left untouched.

Integration test (`#[ignore]`, run explicitly with `cargo test -p oxbloom-cli -- --ignored`): run the built binary to create a project in a temp dir, then run `cargo check` and `cargo test` inside it and require both to succeed. This is the evidence that "generated project actually compiles"; it is reported separately from the default test run.

## Verification for this slice

`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, then the ignored integration test, then a manual `oxbloom new demo --dir /tmp/...` followed by `cargo run` and a `curl` of `/api/hello`. Results are reported exactly as run.

## Open items

- The oxbloom path default is only valid on the machine that built the CLI. Acceptable until crates.io publishing; the template then switches to a version dependency.
- The directory is not a git repository, so this spec is not committed.
