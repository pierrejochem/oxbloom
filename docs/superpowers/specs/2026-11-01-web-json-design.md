# C6b-1: JSON bodies and content negotiation

Date: 2026-11-01. Builds on C6a (`docs/superpowers/specs/2026-11-01-web-errors-ordering-design.md`). This is the first part of roadmap slice C6b. Static resources, compression and CORS stay open as C6b-2 and later.

## Goal

An application can read a JSON request body and write a JSON response without hand-written helpers. Wrong media types and unacceptable `Accept` headers get the same problem+json answers as every other framework error. Handler signatures do not change: a handler still takes a `Request` and returns something that implements `IntoResponse`.

## Decisions

- JSON is exposed as `Json<T>` (a response type) and `Request::json::<T>()` (a parsing method). There are no typed extractors in this slice.
- Content negotiation means an `Accept` check on JSON responses (406) and a `Content-Type` check on JSON requests (415). There is no pluggable format registry, because JSON is the only format.
- Serde support sits behind a Cargo feature `json` in `oxbloom-web`, enabled by default. Applications that do not want it use `default-features = false`.
- The slice adds no stack-probe growth over the C6a numbers (boot 238 KB default, 239 KB with `logging`, `build()` frame `sub $0x178,%rsp`, DI 206/277/230/194 KB) and changes no boot code.

## Out of scope

Typed extractors (`Path`, `Query`, `Json<T>` as a handler argument), pluggable message converters, formats other than JSON, `Accept-Charset` and `Accept-Language`, request body decompression, static resources, compression, CORS, and a configurable JSON mapper.

## API (`oxbloom-web`, feature `json`)

- `Json<T>(pub T)` implements `IntoResponse` where `T: serde::Serialize`. The response has status 200, the header `content-type: application/json` and the serialized body. `(StatusCode, Json<T>)` already works through the existing tuple implementation, so a handler can answer 201 with a JSON body.
- `Json<T>` marks its response with a `JsonBody` value in the response extensions. The marker is what the negotiation filter looks for.
- If serialization fails, `Json<T>::into_response` returns an `HttpError` response with status 500 and no `detail`. The serializer error text is logged at level `error` on target `oxbloom::web` and is never sent to the client. It does not panic and does not fall back to an empty object.
- `Request::json::<T>() -> Result<T, HttpError>` where `T: serde::de::DeserializeOwned` parses `self.body`.
  - The `Content-Type` header must be `application/json` or a media type with the `+json` suffix. Parameters such as `charset=utf-8` are allowed, and the comparison ignores ASCII case. Otherwise the result is a 415 `HttpError`. A request without a `Content-Type` header gets 415 too, because the body type is unknown.
  - A body that is empty or is not valid JSON for `T` gives a 400 `HttpError`. Its `detail` names the error class (`syntax`, `data`, `eof`) and the line and column serde_json reports. It never contains the body text or the offending value, because the body may hold secrets.
  - A request whose body exceeded `server.max-body-bytes` never reaches the parser: the body-limit filter has already answered 413.
- `HttpError` implements `IntoResponse`, so `let todo: NewTodo = req.json()?;` works in a handler that returns `Result<T, HttpError>`.

## Negotiation

- A new built-in `JsonNegotiationFilter` is added to every `App` when the feature `json` is on, at the new constant `order::NEGOTIATION = -250`. That slot is inside `ERROR_HANDLING` (-300), so the 406 it produces is rendered by the `ErrorRenderer`, and outside `BODY_LIMIT` and `SECURITY`.
- The filter reads the `Accept` header of the request, runs the rest of the chain, and then looks at the response. If the response carries the `JsonBody` marker and the `Accept` header excludes `application/json`, the filter drops the response and answers 406 as an `HttpError`. Every other response passes unchanged. The handler has already run at that point, which matches Spring: negotiation happens when the body is written.
- `Accept` handling follows RFC 9110. No `Accept` header means any type is acceptable. Media ranges are `*/*`, `type/*` and `type/subtype`, with `q` values. `application/json` is acceptable when the most specific matching range has `q > 0`. A range `application/*+json` is not a valid range and does not match. A header that cannot be parsed counts as absent. Several `Accept` header lines are joined with commas.
- `application/problem+json` and the other framework error answers are not `JsonBody` responses, so a client that sends `Accept: application/json` still receives problem+json errors without a 406 loop.
- `order::NEGOTIATION` is public and documented next to the other order constants.

## Errors

406, 415 and 400 are built with `HttpError`, so they are rendered as `application/problem+json` and appear in the request log like the other error answers. Their `title` is the standard reason phrase. The 415 answer lists the accepted media type in the extension member `accepted` (`application/json`). The 406 answer lists it in `supported`.

## Dependencies and features

`oxbloom-web` gains `serde` and `serde_json` as optional dependencies behind the feature `json`, which is in `default`. `oxbloom-boot` and `examples/todos` use the default features. Without the feature, `Json`, `Request::json`, `JsonBody`, `JsonNegotiationFilter` and the `order::NEGOTIATION` filter are not compiled; the constant stays public so the order table is the same in both builds.

## `examples/todos`

`json.rs` is deleted. The handlers use `Json<T>` for responses and `req.json()` for bodies. The 400 answer that used `{"error": ...}` now comes from `Request::json`, so every error of the example is problem+json. This closes the deferred C6a item about mixed error shapes.

## Testing

- `Json<T>`: body and headers, status through `(StatusCode, Json<T>)`, a serialization failure answers 500 without leaking the error text, and the marker is set.
- `Request::json`: valid body; `content-type` variants (`application/json`, with `charset`, upper case, `application/vnd.api+json`, `text/plain`, missing) ; empty body; invalid syntax; a type mismatch; the `detail` never contains the body or a value from it.
- Negotiation matrix: no `Accept`, `*/*`, `application/*`, `application/json`, `application/json;q=0`, `text/html`, `text/html, application/json;q=0.5`, `application/json;q=0, */*;q=0.1`, a malformed header, and two `Accept` lines. Error answers are not rewritten. A non-JSON response is never turned into 406.
- Ordering: the 406 is rendered as problem+json and logged by the request log; a security rejection and a 413 still come first when they apply; `filter_ordered` can place a user filter on either side of `NEGOTIATION`.
- Feature off: the crate builds with `--no-default-features`, and `order::NEGOTIATION` exists.
- Gate: `cargo test --workspace --all-features`, `cargo build -p oxbloom-web --no-default-features`, clippy `-D warnings`, `cargo fmt --check`, the feature combinations of `oxbloom-boot`, `make check-examples`, and the stack probes (zero growth).

## Documentation

README (new section "JSON and content negotiation", and `order::NEGOTIATION` in the order table), `docs/API.md` for each new public item, the crate docs of `oxbloom-web` (including the `json` feature), the roadmap (C6b-1 done, C6b-2 open, follow-ups), `AGENT.md` (new module and feature), and the `examples/todos` doc comments and README mentions of `json.rs`.
