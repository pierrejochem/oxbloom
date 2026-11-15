# Sessions

Oxbloom keeps per-browser state in a session. The crate `oxbloom-security` holds the `Session` handle, the `SessionFilter`, two backends and the settings, and the feature `security` of `oxbloom-boot` wires them from the `security.session` keys. This is security Slice 6b. There is no login or logout endpoint yet (Slice 6c) and no CSRF protection (Slice 6d), so read [Limits](#limits) before you build on sessions.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["security"] }
```

The design is in the Slice 6b spec and plan (both are local files under `docs/superpowers`, so this page names them without a link), the decisions are in [ADR 0087](../adr/0087-sessions.md), and every public item has an example in the [API reference](../API.md#47-oxbloom-security). The example `examples/todos` counts visits in a session on `GET /api/visits` (see [Example](14-example.md#example)).

## Turning sessions on

The filter is registered only when the table `security.session` exists, for example `[security.session]` in a file, any key under it, or an environment variable such as `APP_SECURITY__SESSION__STORE=memory`. An application without that table has no session filter and a request has no session.

```toml
[security.session]
store = "memory"
```

## Using the session

A handler reads the session from the request with `RequestSessionExt::session`. It returns `None` when no session filter ran.

```rust
use oxbloom_security::RequestSessionExt;
use oxbloom_web::Request;

async fn visit(req: Request) -> String {
    let session = req.session().expect("the session filter ran");
    let visits = session.get::<u32>("visits").ok().flatten().unwrap_or(0) + 1;
    session.set("visits", &visits).ok();
    visits.to_string()
}
```

`Session` has these methods:

| Method | Does |
|---|---|
| `get::<T>(key)` | Returns `Result<Option<T>, SessionError>`: `None` for a missing key, `SessionError::Serialization` when the stored value does not fit `T`. |
| `set(key, &value)` | Stores a value that `serde` turns into JSON and marks the session for writing. Fails with `Serialization` and leaves the session unchanged. |
| `remove(key)` | Removes a value. |
| `invalidate()` | Drops the values now, deletes the stored session and expires the cookie at the end of the request. A later `set` in the same request starts a new session with a new id. |
| `rotate_id()` | Moves the session to a new id and keeps its values and creation time, so an id known before a login stops working after it (session fixation). The old id is deleted at the end of the request. |
| `is_new()` | Whether the session started in this request. |
| `id()` | The current id. Treat it as a secret and never log it. With the stateless backend it is random on every request. |

`Session` is a handle around shared state: clones see the same data. Nothing is written until the filter commits, once, after the rest of the chain. A clone that is used after the filter committed (for example one moved into a spawned task) holds an empty placeholder: it reads nothing, and what it writes is lost silently. Do not keep a `Session` past the end of the request.

Values are JSON. Keep them small: the stateless backend is bounded by the cookie size, and the in-memory store holds them in the memory of the process.

## Two backends

### Server side

With `store = "memory"` (the default), the data lives in a `SessionStore` and the cookie holds only a random id (32 random bytes in unpadded base64url, 43 characters). `InMemorySessionStore` is the store that ships. It keeps the sessions in sharded maps in the process. The consequences:

- Sessions are lost at a restart and are not shared between instances. Behind a load balancer, use sticky routing or the stateless backend.
- `security.session.max-sessions` (default 100 000) caps the number of sessions. At the cap, a new session evicts the one with the oldest last touch. The eviction scans every shard, so it costs O(n) per new save at the cap. This matters once an anonymous endpoint writes the session: a flood of such requests makes each one scan the store. Sampled eviction is planned before Slice 6d. Racing saves can exceed the cap by their number.
- The memory cost is the cap times the size of the values of one session. The server side has no size limit per session.

To use another store, register your own bean. It replaces the in-memory store with `store = "memory"` and is ignored with `store = "cookie"`:

```rust
.configure(|b| {
    b.instance(RedisSessions::new())
        .provides(|s: Arc<RedisSessions>| -> Arc<dyn SessionStore> { s })
})
```

#### The store contract

`SessionStore` is an async trait with six methods. `SessionData` is the stored value, with `new`, `values`, `values_mut`, `created`, `touched`, `touch`, `is_empty` and `from_parts`.

| Method | Contract |
|---|---|
| `load(id)` | The data of a live session, or `None`. |
| `save(id, data, ttl)` | Inserts or replaces. The filter uses it only for a session minted in this request. |
| `update(id, data, ttl)` | Writes only when the id still exists. `false` means the session was deleted meanwhile, and the filter clears the cookie. |
| `rotate(old, new)` | Moves the session to a new id. `false` when the old id is missing. |
| `delete(id)` | Removes a session. |
| `maintain()` | Housekeeping. A no-op by default; the filter calls it at most once a minute. |

`update` and `rotate` must be conditional writes, because the filter relies on them so that a slow request cannot bring a logged-out session back. A Redis store uses `SET key value XX` (or a script) for `update` and `RENAMENX` (or a script) for `rotate`. An SQL store uses `UPDATE .. WHERE id = ?` and checks the row count for `update`, and `UPDATE .. SET id = new WHERE id = old AND expires > now` with a row-count check for `rotate`.

### Stateless cookie

With `store = "cookie"`, the whole session is in the cookie, sealed by `CookieSessionBackend`. There is nothing to store and no sticky routing is needed. The cookie is encrypted and authenticated with AES-256-GCM through `aws-lc-rs`:

- The key comes from HKDF-SHA256 over the whole secret with an empty salt and the info `oxbloom session cookie v1`.
- The cookie name is the associated data, so a value cannot be moved to another cookie name.
- Every write uses a random 96-bit nonce.
- A secret must be at least 32 bytes.
- `security.session.old-secrets` lists earlier secrets that still open cookies, tried in order after the current one. New cookies always use the current secret.

```toml
[security.session]
store = "cookie"
secret = "a-secret-of-at-least-32-bytes-from-the-environment"
old-secrets = ["the-previous-secret-of-at-least-32-bytes"]
```

Rotate the secret periodically: random 96-bit nonces cap one key at about 2^32 writes. To rotate, move the current secret into `old-secrets` and set a new one; users keep their sessions until you remove the old secret.

The stateless backend has these limits:

- There is no server-side revocation. A copy of a cookie stays valid until its expiry, also after a logout elsewhere. `invalidate` only asks the browser to drop the cookie.
- The cookie size is bounded by `security.session.max-cookie-bytes` (default 3072, at least 256). A session that grows beyond it fails the commit with `SessionError::TooLarge`, which answers 503. The limit counts the cookie value, not the name and the attributes.
- `Session::id()` is random on every request, so do not use it as a key in other places.
- The backend decides expiry with its own clock, see [Clock](#clock).

## What the filter does

`SessionFilter` runs at `SESSION_ORDER`, which is `oxbloom_web::order::SECURITY - 1` (-101): one step before the security chain, so authentication can read the session (see [Errors and filter order](09-errors-and-filter-order.md#errors-and-filter-order)). For each request it:

1. Reads the session cookie from the `Cookie` headers. It tries at most 8 values of the session cookie name per request (with the server backend only values of the 43-character id shape), and a `Cookie` header over 8 KiB counts as none. A missing, unknown, expired or undecryptable cookie gives a fresh empty session.
2. Puts the `Session` in the request and runs the rest of the chain.
3. Commits the session, whatever the response status, so a handler that wrote to the session and then answered 404 still gets its cookie.

What the commit does:

- An empty session that was never touched is never stored and sets no cookie.
- A session with changes is saved and its cookie is set.
- An untouched session is saved again to extend its idle timeout only when its last touch is more than 60 seconds old.
- An invalidated session is deleted and its cookie expires with `Max-Age=0`.
- A session past its absolute timeout is cleared without a write.
- `Cache-Control: no-store` is added with the cookie unless the handler set its own `Cache-Control`. Existing `Set-Cookie` headers of the handler are kept, and the session cookie is appended.

The cookie is `HttpOnly`, has `Path=/` and a `Max-Age` (the idle timeout, cut short by the absolute timeout), and `SameSite=Lax` by default. `Secure` follows `security.session.secure`: `auto` (the default) uses the request scheme, including a forwarded `https` from a trusted proxy (see [HTTP behavior](15-http-behavior.md#http-behavior)); `always` and `never` override it. Cookie values are marked sensitive, so request logging and `Debug` output never print them.

### A store error is a 503

A store error, a failed seal or a `TooLarge` session at load or commit answers 503, and the client gets no session cookie. The filter's own body is empty, and the error filter of the web crate renders it, so the client sees the server's problem document. The filter logs a fixed line at `error` without an id, a value or the error text. A handler that already ran keeps its side effects, but its response, with its `Set-Cookie` headers, is replaced by the 503. The filter never starts an empty session in silence, because that would log a user out without telling anyone.

### Concurrency

- A logout that races another request on the same session cannot bring the session back: a loaded session is written back only with `update` and moved only with `rotate`, and a missing id clears the cookie instead.
- If a `rotate` commits before a concurrent logout of the old id, the session lives on under the new id, so a logout of the old id does not end it. A logout should invalidate by the current id, and `rotate_id` should be called only at login.
- Concurrent requests on one session are last-writer-wins. Each loads a copy and writes the whole session back, so one request's change can overwrite another's. Keep counters and lists that need exactness outside the session.
- A `Session` clone used after the filter ran loses its writes silently.

## Configuration

`SessionAutoConfiguration` reads these keys. Durations take the configuration's duration form (`90s`, `30m`, `8h`).

| Key | Default | Meaning |
|---|---|---|
| `security.session.store` | `memory` | `memory` (server side) or `cookie` (stateless). |
| `security.session.cookie-name` | `OXBLOOM_SESSION` | The cookie name; a non-empty HTTP token. |
| `security.session.secure` | `auto` | `auto`, `always` or `never`. |
| `security.session.same-site` | `lax` | `lax`, `strict` or `none`. `none` needs `secure = "always"`. |
| `security.session.idle-timeout` | `30m` | Unused time before expiry; at least one second and at most `absolute-timeout`. |
| `security.session.absolute-timeout` | `8h` | Total lifetime however active; positive. |
| `security.session.max-sessions` | `100000` | The cap of the in-memory store; at least 1. |
| `security.session.max-cookie-bytes` | `3072` | The largest session cookie, in bytes; at least 256. |
| `security.session.secret` | none | The key of the stateless backend; at least 32 bytes; required with `store = "cookie"`. |
| `security.session.old-secrets` | none | Earlier keys that still open cookies; each at least 32 bytes. |

Unknown keys under `security.session` and values of the wrong type or out of range fail the startup with the key named and never the value. A secret from the command line is refused with the key named, because arguments show up in process listings; set `secret` and `old-secrets` in a file or an environment variable (`APP_SECURITY__SESSION__SECRET`). A secret that arrives through a placeholder can evade the check, as for `security.jwt.secret`. An environment variable for `old-secrets` is split on commas, so use the indexed form (`APP_SECURITY__SESSION__OLD_SECRETS__0`) for a secret that contains a comma.

### What the auto-configuration registers

`SessionAutoConfiguration` runs after the password encoder and before the security chain, and only when the table `security.session` exists. It registers a `SessionFilter` as a `dyn Filter`. The `security` feature of `oxbloom-boot` now also enables `dep:oxbloom-web`, which `oxbloom-security` already depended on.

- A user `dyn SessionStore` bean replaces the in-memory store with `store = "memory"` and is ignored with `store = "cookie"`.
- A user `SessionFilter` bean steps the configured one aside. It must also be provided as `dyn Filter`, or it does not run.
- One optional `Arc<dyn Clock>` bean feeds the filter, the in-memory store and the cookie backend.

### Clock

The filter, the in-memory store and the cookie backend each decide time with a clock. When you build them by hand, give all of them the same clock (`SessionFilter::with_clock`); the boot wiring does it for you with the `Arc<dyn Clock>` bean, which is how tests control time.

### Without boot

```rust
use std::sync::Arc;
use oxbloom_core::SystemClock;
use oxbloom_security::{InMemorySessionStore, SessionBackend, SessionFilter, SessionSettings};

let settings = SessionSettings::default();
let store = InMemorySessionStore::new(&settings, Arc::new(SystemClock));
let filter = SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings)?;
let app = oxbloom_web::App::new().filter(filter);
```

`SessionFilter::new` validates the settings and fails with `SessionError::InvalidSettings`. `SessionSettings` has builder setters for every key except the secrets. `SessionConfig::from_config` reads the keys from a `Config` for code that wires the filter itself.

## Limits

- There is no CSRF protection yet. The cookie is `HttpOnly` and `SameSite=Lax`, which does not stop every cross-site request. CSRF tokens come with Slice 6d. Do not build state-changing endpoints on sessions for browsers before that.
- There is no login or logout endpoint (Slice 6c). An application that builds its own should call `rotate_id` at login and `invalidate` at logout.
- A stateless cookie cannot be revoked on the server; a copy stays valid until its expiry.
- The in-memory store is per process: sessions are lost at a restart and not shared between instances.
- At the cap, the in-memory store evicts the oldest-touched session and scans every shard for it, which is O(n) per new save at the cap. A flood needs an anonymous endpoint that writes the session; sampled eviction is planned before Slice 6d. The cap can be exceeded by the number of racing saves.
- Concurrent requests on one session are last-writer-wins.
- A store error answers 503 and the side effects of the handler stand.
- Random nonces cap one key of the stateless backend at about 2^32 writes: rotate the secret periodically.
