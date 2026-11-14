# Compression

`CompressionFilter` compresses response bodies with gzip, brotli or zstd, as the `Accept-Encoding` of the request allows. It sits at `order::COMPRESSION` (-150), inside the body limit and outside security. Register it with `App::compression(CompressionConfig)` or set `server.compression.enabled = true`.

The Cargo features of `oxbloom-web` are `gzip` (`flate2`), `brotli` (`brotli`) and `zstd` (`zstd`); `compression` enables all three and is in `default` next to `json`. A codec whose feature is off is never chosen, and `oxbloom_web::codecs()` lists those compiled in. `oxbloom-boot` keeps the default features of `oxbloom-web`, so all codecs are on there. `server.compression.enabled = true` fails startup when no codec is compiled in.

```rust
use oxbloom_web::{App, CompressionConfig, CompressionLevel};

let app = App::new().compression(
    CompressionConfig::new()
        .min_size(512)
        .mime_types(["text/*", "application/json"])
        .level(CompressionLevel::Default)
        .max_body_bytes(4 << 20),
);
```

Defaults: bodies of at least 1024 bytes and at most 8 MiB; the types `text/*`, `application/json`, `application/problem+json`, `application/javascript`, `image/svg+xml` and `application/xml`; level `default` (gzip 6, brotli 5, zstd 3; `fast` is 1, 2, 1 and `best` is 9, 11, 19). An entry ending in `/*` matches a whole family; an entry `*/*` matches nothing.

A response is compressed only when all of these hold: a compiled codec is acceptable to the client, the body size is within the limits, the `Content-Type` is in the list, the response has no `Content-Encoding` and no `Content-Range`, the body is not empty, the status is not 1xx, 204 or 304, and `Cache-Control` has no `no-transform`. Otherwise it passes unchanged. A compressed response gets `Content-Encoding`, loses `Content-Length` (the server sets it), gets `Vary: Accept-Encoding` unless it already lists it, and a strong `ETag` becomes weak.

- `HEAD` gets the same compression headers as `GET`, with an empty body. The router records the length of the stripped body, so the filter decides as it would for `GET`.
- `Accept-Encoding` names are case-insensitive. A malformed entry is skipped. When a name is listed twice, the first entry wins. A codec that is not listed takes the `q` of `*`, and is not acceptable when `*` is absent. The best `q` above zero wins; ties go to zstd, then brotli, then gzip. `x-gzip` is not recognised.
- A client that sends `identity;q=0` and cannot use any compiled codec gets 406, but only for a response that would otherwise have been compressed. `*;q=0` alone rules out the codecs and does not refuse the identity.
- Encoding runs synchronously inside the request. Level `best` on a large body can take seconds; keep `max-body-bytes` and the level in proportion.
- Level `best` costs memory too: brotli quality 11 with a 4 MiB window and zstd level 19 can use tens of MB per encode, and there is no limit on how many encodes run at once.
- Error answers rendered by the error filter are not compressed: the error filter (`order::ERROR_HANDLING`) is outside the compression filter, so the compression filter never sees the rendered body. A body that a handler builds itself is compressed like any other.

**BREACH.** Compressing a response that mixes a secret (a CSRF token, a session value) with input the attacker controls, served over TLS, can leak the secret through the compressed length. Do not compress such responses: leave their type out of `mime-types`, or send `Cache-Control: no-transform`. Compression is off until you enable it.
