# HTTP behavior

- Unknown path: 404. Known path, wrong method: 405 with `Allow`.
- `HEAD` falls back to the `GET` handler, body stripped. `OPTIONS` answers 204 with `Allow`.
- `TRACE` and `CONNECT` return 405 unless enabled in `RouterConfig`.
- Request bodies are capped (default 1 MiB, 413 above); header read timeout 10 s. These limits, the shutdown timeout and forwarded headers are `server.*` keys; see [Server properties](09-errors-and-filter-order.md#server-properties).
- Error answers are `application/problem+json`; see [Errors and filter order](09-errors-and-filter-order.md#errors-and-filter-order).
- `Json<T>` answers, `Request::json` and the `Accept` check (415, 400, 406) are described in [JSON and content negotiation](10-json-and-content-negotiation.md#json-and-content-negotiation).
- CORS headers, compression and static files are described in [CORS](11-cors.md#cors), [Compression](12-compression.md#compression) and [Static resources](13-static-resources.md#static-resources).
