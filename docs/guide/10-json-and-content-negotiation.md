# JSON and content negotiation

The Cargo feature `json` of `oxbloom-web` is on by default. It adds `Json<T>` for responses, `Request::json::<T>()` for request bodies and a built-in `Accept` check. Without the feature (`default-features = false`) none of them is compiled and `serde_json` is not a dependency; the slot `order::NEGOTIATION` exists in both builds. `oxbloom-boot` (feature `web`) and `oxbloom-security` depend on `oxbloom-web` with its default features, so the feature `json` is on in any application that uses them; `default-features = false` only removes it for applications that use `oxbloom-web` alone.

```rust
use http::StatusCode;
use oxbloom_web::{HttpError, Json, Request};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct NewTodo {
    title: String,
}

#[derive(Serialize)]
struct Todo {
    id: u64,
    title: String,
}

async fn create(req: Request) -> Result<(StatusCode, Json<Todo>), HttpError> {
    let body: NewTodo = req.json()?;
    Ok((StatusCode::CREATED, Json(Todo { id: 1, title: body.title })))
}
```

`Json(value)` answers 200 with `content-type: application/json` and the serialized value; `(StatusCode, Json<T>)` picks another status. If serialization fails, the answer is a 500 problem without a `detail`; the error text is logged at level `error` on target `oxbloom::web` and never sent.

`Request::json::<T>()` returns `Result<T, HttpError>`, so `?` works in a handler that returns `Result<_, HttpError>`:

- The `Content-Type` must be `application/json` or a `+json` type with a non-empty prefix, such as `application/vnd.api+json`. Parameters (`;charset=utf-8`) are allowed and case is ignored. `application/jsonx`, `application/json-seq`, a bare `+json`, any other type and a missing header are 415. A client must send `Content-Type: application/json`; Oxbloom does not guess a type for a client that omits it, not even for an empty body.
- A body that is empty or not valid for `T` is 400. The `detail` names only the error class and the position, never the body or a value from it.

```json
{"type":"about:blank","title":"Unsupported Media Type","status":415,"instance":"/api/todos","accepted":"application/json"}
{"type":"about:blank","title":"Bad Request","status":400,"detail":"invalid JSON body: syntax error at line 1 column 8","instance":"/api/todos"}
{"type":"about:blank","title":"Not Acceptable","status":406,"instance":"/api/todos","supported":"application/json"}
```

The three answers are, in order, a 415 (member `accepted`), a 400 (error class `syntax`, `data` or `eof`, then line and column) and a 406 (member `supported`). All are `application/problem+json`, as described in [Errors](03-dependency-injection.md#errors).

The 406 comes from a built-in filter at `order::NEGOTIATION` (-250), inside `ERROR_HANDLING` and outside `BODY_LIMIT`. It runs after the handler and acts only on a response built by `Json<T>` (it carries a `JsonBody` marker in its extensions); error answers and other bodies are never touched. The marker survives the router's body strip, so a `HEAD` request that does not accept JSON gets 406 as well. A `Json` answer is checked against `Accept` when it comes from a handler or from a filter ordered after `order::NEGOTIATION`; a `Json` answer from a filter ordered before it is never negotiated, and the built-in security chain answers with `HttpError`, which is never negotiated. The rules for `Accept`:

- A range is `*/*`, `type/*` or `type/subtype`, with a `q` value as its only parameter that matters. Anything else (`application/*+json`, `*/json`, `/`) is an invalid range and is skipped.
- `q` follows RFC 9110: `0` or `1`, optionally followed by a point and up to three digits, and `1.xxx` only with zeros. An invalid `q` (`1.5`, `abc`) skips the range.
- Media types and parameter names are case-insensitive. A comma or semicolon inside a quoted parameter value does not split a range. Several `Accept` headers count as one list.
- The most specific range that matches `application/json` decides: `application/json`, then `application/*`, then `*/*`. The answer is acceptable when that range has `q > 0`; `q=0` excludes JSON.
- With no `Accept` header, no valid range at all, or only header lines that are skipped, the answer is acceptable. A header line is skipped when it has a byte that is not visible ASCII (so `é` counts too) or an unterminated double quote.

A request body over `server.max-body-bytes` gets 413 from the body-limit filter before `Request::json` runs, and a request the security chain rejects keeps its 401 or 403; neither becomes a 406. Not covered: parameters of `Accept` other than `q`, typed extractors, pluggable converters, and request body decompression; see [`docs/ROADMAP-core.md`](../ROADMAP-core.md).
