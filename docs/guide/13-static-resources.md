# Static resources

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
