# ADR 0087: Sessions come in two backends behind one filter, and a loaded session is only written back conditionally

- **Status:** Accepted
- **Date:** 2026-11-15
- **Source:** the security Slice 6b design spec `2026-11-15-sessions-design.md` (a local file, not linked) and its plan

## Context

Form login needs state between requests. Which kinds of session storage does Oxbloom offer, and how does a handler reach the session? What does a session cookie look like, and how is it protected? What happens when two requests of one browser run at the same time, in particular when one of them is a logout? What does a client see when the store fails? How much of Spring's Slice 6 (sessions, form login, logout, CSRF) fits one slice?

The security chain comes from Slice 5, and the boot wiring follows [ADR 0048](0048-contributor-beans.md). Password login is [ADR 0086](0086-password-login.md), which left sessions to this slice.

## Decision

**Two backends, one filter.** `SessionBackend` has two variants. `SessionBackend::Server` keeps the data in a `SessionStore` and puts only a random id in the cookie. `SessionBackend::Cookie` keeps the data in the cookie itself, through `CookieSessionBackend`. `SessionFilter` is the same for both: it loads the session before the rest of the chain, hands it to the handler, and commits it once after the chain. The default is the server side with the `InMemorySessionStore`. The stateless backend is chosen with `security.session.store = "cookie"`.

**The filter runs at `SESSION_ORDER`, one step before the security chain.** `SESSION_ORDER` is `oxbloom_web::order::SECURITY - 1` (-101). Authentication can then read the session. A handler reads the session with `RequestSessionExt::session`, which returns `None` when no session filter ran.

**The session handle.** `Session` is a cheap clone around shared state. `get` and `set` use `serde` and store JSON values; `remove`, `invalidate` and `rotate_id` change the state; `is_new` and `id` read it. Nothing is written until the filter commits, so a request that does not touch the session costs no store write. The commit happens whatever the response status, so a handler that wrote to the session and then answered 404 still gets its cookie. A `Session` clone that is used after the filter has committed (for example one moved into a spawned task) holds an empty, detached placeholder and loses its writes silently.

**The id.** A session id is 32 random bytes from `aws-lc-rs`, written in unpadded base64url (43 characters). With the server backend a cookie value that does not have this shape is not even looked up. With the stateless backend `Session::id()` is a random value on every request, because the cookie carries no id.

**The store trait.** `SessionStore` has six methods:

- `load(id)` returns the data of a live session.
- `save(id, data, ttl)` inserts or replaces. The filter uses it only for a session minted in this request.
- `update(id, data, ttl)` writes only when the id still exists and returns `false` when it does not.
- `rotate(old, new)` moves a session to a new id and returns `false` when the old id is missing.
- `delete(id)` removes a session.
- `maintain()` is housekeeping, a no-op by default; the filter calls it at most once a minute.

`update` and `rotate` are the reason a slow request cannot bring a logged-out session back. Without them, a request that loaded session S, then ran while another request logged S out, would write S again with its old data and set the cookie. With them, the write finds no S, nothing is stored, and the filter leaves the cookie alone. It does not clear the cookie: the server never adopts an unknown id, so a clear protects nothing, and it could delete a newer cookie that the browser received from a later response, such as a double-submitted login or a new login after a logout. (The first version of this decision cleared the cookie; the final review of the slice reversed that.) The trait contract is therefore conditional write for a loaded session, and a store for Redis or SQL must honour it:

- Redis: `SET key value XX` (or a script) for `update`; `RENAMENX` or a script for `rotate`.
- SQL: `UPDATE .. WHERE id = ?` with a check of the row count for `update`; for `rotate`, `UPDATE .. SET id = new WHERE id = old AND expires > now` with a row-count check.

**The absolute timeout is enforced by the filter.** A store bean that an application builds itself never sees `security.session.absolute-timeout`, so the filter does not rely on the store for it. It passes the remaining lifetime of the session, the idle timeout cut short by the absolute deadline, as the `ttl` of `save` and `update`, and it treats loaded data whose creation time plus the absolute timeout has passed as unknown, for both backends. A store that honours only the TTL therefore never serves a session past its absolute deadline.

A consequence of the contract: if request A rotates the id and commits before request B logs out with the old id, the session lives on under the new id. A logout of the old id then does not end it. Slice 6c builds logout, and it should invalidate by the current session id and call `rotate_id` only at login.

**The in-memory store.** `InMemorySessionStore` holds the sessions in sharded maps behind mutexes. It is per process: sessions are lost at a restart and not shared between instances, so behind a load balancer it needs sticky routing or the stateless backend. `security.session.max-sessions` (default 100 000) caps the number of sessions. In this slice, at the cap a new session evicted the session with the oldest last touch, and the eviction scanned every shard, so it cost O(n) per new save at the cap. Slice 6c replaced this scan with sampled eviction at constant cost per write (see [ADR 0088](0088-form-login.md)). Anonymous endpoints that write the session already existed, such as `GET /api/visits` in the todos example, and Slice 6c added the saved request for the redirect after login, so a cookieless flood could fill the cap and evict the sessions of real users, which logs them out. That limit stays, now at constant cost per write. Racing saves can exceed the cap by their number. The memory cost is the cap times the size of one session's values; a session is not limited in size on the server side, so keep values small.

**The stateless backend.** `CookieSessionBackend` seals the session with AES-256-GCM through `aws-lc-rs`, the crypto provider the crate already has. The key is derived from the whole secret with HKDF-SHA256, an empty salt and the info `oxbloom session cookie v1`. The cookie name is the associated data, so a value cannot be moved to another cookie name. Every write uses a random 96-bit nonce. A secret must be at least 32 bytes. `security.session.old-secrets` lists earlier secrets that may still open a cookie, so a key can rotate without logging everybody out; new cookies always use the current secret.

Random 96-bit nonces limit how many cookies one key may seal: after about 2^32 writes under one key the risk of a repeated nonce stops being negligible. An application with a high write rate should therefore rotate the secret periodically (move the current secret into `old-secrets` and set a new one). The sealed payload is JSON and carries the format version as the field `"v"`, the creation and touch times and the values. The idle and absolute timeouts are decided from these times, with the clock of the backend.

The price of statelessness is that there is no server-side revocation. A copy of a cookie stays valid until its expiry, even after the user logged out in another browser, and `invalidate` only asks the browser to drop the cookie. The size limit `security.session.max-cookie-bytes` (default 3072) bounds the cookie; a session that grows beyond it fails the commit with `TooLarge`, which is a 503 (see below). The limit counts the cookie value only, not the name and the attributes.

**The cookie.** The name is `OXBLOOM_SESSION` by default. The cookie is always `HttpOnly`, has `Path=/` and `Max-Age`, and `SameSite=Lax` unless configured. `Secure` follows `security.session.secure`: `auto` (the default) sets it when the request scheme is HTTPS, including a forwarded `https` from a trusted proxy; `always` and `never` override that. `SameSite=None` needs `secure = "always"`, and the reader refuses any other combination. The filter adds `Cache-Control: no-store` with the cookie unless the handler set its own `Cache-Control`, and keeps every `Set-Cookie` the handler produced. The cookie and the request `Cookie` header are marked sensitive, so header logging never prints them.

**Reading the cookie.** The filter reads every `Cookie` header and tries at most 8 values of the session cookie name per request (for the server backend only values of the 43-character id shape), which bounds the store lookups and the decryptions an attacker can cause with one request. A `Cookie` header longer than 8 KiB counts as no header. A missing, unknown, expired or undecryptable cookie gives a fresh empty session, and so does stored data past its absolute timeout.

**When a cookie is set.** An empty session that was never touched is not stored and sets no cookie, so a crawler does not fill the store. A session with changes is saved and its cookie set. An untouched, loaded session is written back to extend its idle timeout only when its last touch is more than 60 seconds old, which avoids a store write and a `Set-Cookie` on every request. An invalidated session is deleted and its cookie expired with `Max-Age=0`. A session past its absolute timeout is cleared without a write.

**A store error is a 503.** A `SessionError` while loading or committing (a failed store, a failed seal, a session over the cookie limit) makes the filter answer 503 with an empty body and no session cookie. The application's error filter renders that as its problem document, so the client sees the server's own 503 body. The filter logs a fixed line at `error` without an id, a value or the error text. A handler that already ran keeps its side effects, but its response, including its `Set-Cookie` headers, is replaced by the 503. The alternative, starting an empty session in silence, would log a user out without telling anyone and would hide a broken store.

**Concurrency.** Concurrent requests on one session are last-writer-wins: each loads the data, changes its copy and writes the whole session back. The conditional write only protects against resurrection, not against lost updates between two live requests. Applications that need atomic counters or lists keep them outside the session.

**Configuration and secrets.** The keys are `security.session.{store,cookie-name,secure,same-site,idle-timeout,absolute-timeout,max-sessions,max-cookie-bytes,secret,old-secrets}`. The reader checks every value at startup and names the key, never the value. `secret` and `old-secrets` are refused on the command line, because arguments show up in process listings. The check reads the origin of the value, like the one for `security.jwt.secret`, so a placeholder that is resolved from another source can evade it. An environment variable for `old-secrets` is split on commas; a secret that contains a comma must use the indexed form (`APP_SECURITY__SESSION__OLD_SECRETS__0`).

**Boot.** `SessionAutoConfiguration` runs after the password encoder and before the security chain, and only when the table `security.session` exists, so an application that never configures sessions gets no filter. It registers a `SessionFilter` as a `dyn Filter`. A user `dyn SessionStore` bean replaces the in-memory store with `store = "memory"` and is ignored with `store = "cookie"`. A user `SessionFilter` bean steps the configured one aside, but it must also be provided as `dyn Filter` to run. One optional `Arc<dyn Clock>` bean replaces the system clock in the filter, the store and the cookie backend, which each decide expiry themselves and must agree. The `security` feature of `oxbloom-boot` now also enables `dep:oxbloom-web`, which `oxbloom-security` already depends on, so nothing new is built.

**The split.** Slice 6b is the session layer: the handle, the two backends, the filter and the configuration. Login and logout endpoints are Slice 6c, and CSRF protection is Slice 6d. Splitting keeps each slice reviewable, and the concurrency rules of this slice are what logout needs.

## Consequences

Good:

- Handlers use one small `Session` type, whatever the backend.
- A logout cannot be undone by a request that was already running.
- The stateless backend needs no store and no sticky routing.
- A wrong key, value or secret fails at startup with the key named, and no secret reaches a log or an error text.
- A session that nobody writes costs no store write.

Bad:

- There is no CSRF protection in this slice. The only mitigation is `SameSite=Lax`, which does not stop every cross-site request; CSRF needs tokens or `Origin` checks, which come with Slice 6d (done, see [ADR 0089](0089-csrf-tokens.md)). `HttpOnly` only keeps scripts from reading the cookie and does nothing against cross-site requests. Do not build state-changing session endpoints for browsers before Slice 6d.
- There is no login or logout endpoint in this slice, so an application builds its own on `Session` (Slice 6c adds them, see [ADR 0088](0088-form-login.md)).
- A stateless cookie cannot be revoked on the server. A stolen copy works until it expires.
- The in-memory store is per process, loses its sessions at a restart, and its eviction at the cap was O(n) per new save in this slice (Slice 6c made it constant, see [ADR 0088](0088-form-login.md)); an anonymous flood that writes sessions can evict the sessions of real users. The global cap can be exceeded by the number of racing saves.
- Last-writer-wins between concurrent requests of one session.
- A store failure answers 503 for the whole request, and the side effects of the handler stand.
- Random nonces cap one key at about 2^32 writes, so the secret must be rotated periodically.
- The trait gained `update` and `maintain`, and `rotate` returns `Result<bool>`. An application with its own store written against an earlier draft must implement the conditional writes.
- A `Session` clone used after the filter committed loses its writes without an error.

## Alternatives considered

- Only the in-memory store: rejected, because a deployment with several instances needs shared state or none.
- Only the stateless cookie: rejected, because it cannot be revoked and its size is bounded.
- Unconditional `save` for every commit: rejected, because a slow request would resurrect a logged-out session and logout is a security operation. Documenting last-writer-wins for that case would have left the hole open.
- Starting an empty session when the store fails: rejected, see the 503 above.
- A signed but unencrypted cookie: rejected, because session values often hold data that should not be readable in the browser.
- Reusing the JWT key for the cookie: rejected, a separate secret with its own rotation list keeps the two apart.
- Writing the session back on every request: rejected, one write a minute per active session is enough for the idle timeout.
- Locking a session for the duration of a request: rejected, because it would serialize a browser's requests and a store would need a lock protocol.
