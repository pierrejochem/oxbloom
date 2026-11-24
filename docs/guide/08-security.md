# Security

`oxbloom-security` is a Spring-Security-style layer for `oxbloom-web`: a `SecurityFilterChain` filter reads `Authorization: Bearer <token>`, authenticates it, then authorizes the request against ordered rules. Bearer JWTs (resource server), password login with HTTP Basic, sessions and form login are implemented; Basic has its own chapter, [Password login](19-password-login.md#password-login), sessions have [Sessions](20-sessions.md#sessions), and form login has [Form login](21-form-login.md#form-login).

```rust
let chain = HttpSecurity::new()
    .authorize(|a| a
        .get("/public/**").permit_all()
        .path("/admin/**").has_role("ADMIN")
        .any_request().authenticated())
    .oauth2_resource_server(|r| r
        .jwt(Arc::new(decoder))
        .converter(JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_")))
    .build()?;                       // no or a second `authorize` section, an invalid pattern, or a rule after `any_request()`, is an error
let app = App::new().filter(chain);  // handlers read `req.authentication()` (trait `RequestSecurityExt`)
```

- Deny by default: a request that matches no rule is denied. One chain per application; `HttpSecurity::build()` fails without an `authorize` section and when `authorize` is called more than once (`DuplicateAuthorizeSection`; sections are never merged, so put every rule in one call). The first matching rule wins. `has_role("X")` checks the authority `ROLE_X`.
- Filters run before routing, so an unknown path is denied (401/403) unless a rule permits it, so anonymous clients cannot probe which routes exist (a `permit_all` rule, or a caller the rules allow, still reaches the router's 404). The exception: a request body over the limit gets 413 before the security chain runs (the body limit has the slot `order::BODY_LIMIT`, ahead of `order::SECURITY`).
- Matching uses the raw, undecoded path and Ant patterns: `*` is exactly one segment, `**` is zero or more. Literals are case-sensitive; a trailing slash on the request is ignored. Any other `*` and any `{`, `}` or `?` in a segment (`/users/{id}`, `/files/*.pdf`, `a*`, `***`) is rejected by `build()` with `InvalidPattern`; write `/users/*` instead.
- A `HEAD` request is authorized as `GET`: a `get(..)` rule also governs `HEAD`, and rules are tried in order with both views, so an earlier `get(..)` rule shadows a later `head(..)` rule and vice versa.
- Responses: anonymous or bad token is 401 with `WWW-Authenticate: Bearer` (plus `error="invalid_token"` when a token failed), authenticated but not allowed is 403, and a key-source outage (`ProviderUnavailable`) is 503 with no challenge. The answers are `application/problem+json` bodies with the headers kept (see [Errors and filter order](09-errors-and-filter-order.md#errors-and-filter-order)). A malformed `Authorization` header (several headers, a value that is not valid text, `Bearer` without a token, whitespace inside the token) is answered 401 by the chain itself (`error="invalid_request"` with a bearer provider; in a chain with HTTP Basic and no bearer provider, the Basic challenge only) and bypasses a custom `entry_point`. Another scheme is treated as anonymous, and `Basic` is too unless `http_basic` is on (see [Password login](19-password-login.md#http-basic)); so is every `Authorization` header when neither a bearer provider nor HTTP Basic is set up. Errors, responses and the `Debug` output of the security types never contain tokens, keys or claim values; `JwksSource` prints its URI without userinfo or query string. The server marks `Authorization`, `Proxy-Authorization` and `Cookie` header values sensitive (and the chain marks `Authorization` sensitive again, also for hand-built requests), so `Debug` of a `Request` or its headers shows `Sensitive` for them. Other headers, including custom credential headers, and the query and body are printed as sent.
- If `oauth2_resource_server` is called while another bearer provider is registered, the first bearer provider wins.
- Sessions (slice 6b) are a separate filter at `SESSION_ORDER` (-101) that runs before the chain; see [Sessions](20-sessions.md#sessions). With form login on, the chain also checks a CSRF token on every unsafe request that the session authenticates; see [CSRF protection](22-csrf.md#csrf-protection).
- CSRF protection (slice 6d) is a synchronizer token in the session, on whenever form login is on. `Basic` and bearer requests, and anonymous requests, are not checked.
- Security headers (slice 7a) are added to every response of the chain by default: `X-Content-Type-Options`, `Referrer-Policy`, `X-Frame-Options`, `Cache-Control: no-store` for authenticated requests and HSTS on `https` requests. A header that the response already has is kept. See [Session management and security headers](23-session-management.md#security-headers).
- Session management (slice 7a): `security.session.max-sessions-per-user` evicts the oldest session of a user at login, `SessionRegistry` lists and ends the sessions of a user (in-memory store only), and `security.session.revalidate-interval` (default 5 minutes) checks the stored login against the user service again. See [Session management and security headers](23-session-management.md#session-management-and-security-headers).
- Remember-me is built (slice 7b, see [Remember-me](24-remember-me.md#remember-me)), so is method security (slice 8, see [Method security](25-method-security.md#method-security)), and so are OAuth2 and OpenID Connect login (slice 9 part B, feature `oauth2`, see [OAuth2 login](27-oauth2-login.md#oauth2-login)) and the OAuth2 client (slice 9 part C, feature `oauth2`, see [OAuth2 client](28-oauth2-client.md#oauth2-client)). Not implemented (deliberate cuts): a default content security policy, RP-initiated logout, the device code grant, token exchange, token revocation, a persistent token store, opaque-token introspection, several chains per app.

## JWT

- The algorithm allow-list is fixed when the decoder is built; the token's `alg` header never selects the key type, and `alg: none` is rejected. Build with `JwtDecoderBuilder::with_secret` (HS256/384/512, as `Secret<Vec<u8>>`; the secret must be at least as long as the hash output: 32, 48 or 64 bytes), `with_public_key_pem` (RS256/384/512, ES256/384) or `with_jwks`.
- `exp` is required (opt out with `require_exp(false)`), `nbf` is checked, clock skew is 60 s (at most 1 h), `sub` is required. `iss` is checked when an issuer is configured. **`aud` is not checked unless you configure an audience; set it.**
- Authorities: `SCOPE_<x>` from `scope` (space-delimited string) or `scp` (array). `authorities_claim("roles", "ROLE_")` adds a claim mapping; it does not replace the default one.
- `on_failure(|reason| ..)` receives a `FailureReason` for logging; it carries no token text. It is not called for JWKS outages.

## JWKS (feature `jwks`)

```rust
let source = JwksSource::builder(Arc::new(ReqwestJwksFetcher::new()?), "https://issuer.example/.well-known/jwks.json").build()?;
let decoder = JwtDecoderBuilder::with_jwks(source, vec![Algorithm::RS256]).issuer("https://issuer.example").audience(["my-api"]).build()?;
```

- The URI must be `https` unless `allow_insecure_http()` is called (loopback tests only). The body is capped at 1 MiB, the request times out after 5 s, redirects are not followed.
- Keys are cached for 5 min and looked up by `kid`. An unknown `kid` triggers one refresh, and refreshes happen at most once per 30 s however many tokens arrive. Concurrent lookups share one fetch. After a failed refresh the stale set is served until it is older than 2x the TTL, then requests fail with 503. A token with a brand-new `kid` can therefore be rejected for up to 30 s after a key rotation. A key the issuer has removed stops being accepted at the first successful refresh after the TTL (up to 5 min plus 30 s with a healthy issuer); while refreshes keep failing it can be accepted for up to 2x the TTL (10 min).
- Only keys whose `kty`/`alg` fit the decoder's allow-list are used; `use`, if present, must be `sig`.
- The feature uses `reqwest` with rustls (aws-lc-rs), so building needs a C compiler. `HTTP_PROXY`/`HTTPS_PROXY` environment variables are honored. Without the feature there is no HTTP client in the dependency tree.
- Supply-chain note: the `jsonwebtoken` backend `rust_crypto` pulls in `rsa` 0.9, which carries advisory RUSTSEC-2023-0071 (a timing leak in private-key operations). This crate only verifies signatures and never uses private keys, but `cargo audit` will report it.

## Security configuration

`jwt_decoder_from_config(&config, fetcher)` builds a decoder from `security.jwt`. Register the secret key when you build the `Config`; the decoder also refuses a command-line secret on its own:

```rust
use oxbloom_security::{SECRET_KEY, jwt_decoder_from_config};

let config = Config::builder().dir("config").secret_key(SECRET_KEY).build()?;
let decoder = jwt_decoder_from_config(&config, None)?;
```

The section:

```toml
[security.jwt]
issuer = "https://issuer.example"
jwk-set-uri = "https://issuer.example/.well-known/jwks.json"   # needs a fetcher argument
algorithms = ["RS256"]
audience = ["my-api"]
clock-skew-secs = 60
# secret = "..."         # HMAC only; set it from APP_SECURITY__JWT__SECRET
# public-key-pem = "..." # alternative to jwk-set-uri for RS*/ES*
```

- Empty or whitespace-only items in `audience` and `algorithms` are an error.
- Exactly one key source: `secret`, `public-key-pem` or `jwk-set-uri`. A missing or contradictory combination is an error naming the key.
- Unknown keys under `security.jwt` are rejected.
- `jwk-set-uri` must be an `https` URI; the configuration has no way to allow plain `http` (only code can, with `allow_insecure_http()`).
- `algorithms` and `audience` are lists; from environment variables set them comma-separated (`APP_SECURITY__JWT__AUDIENCE=a,b`) or indexed (`APP_SECURITY__JWT__AUDIENCE__0=a`).
- Keys match case-insensitively ignoring `-` and `_`, so `jwk-set-uri`, `jwk_set_uri`, `jwkSetUri` and `JWK_SET_URI` (environment) are one key. Two spellings of one key in the same file are a `DuplicateKey` error.
- Register `oxbloom_security::SECRET_KEY` (`"security.jwt.secret"`) with `Config::builder().secret_key(SECRET_KEY)` so the HMAC secret cannot be passed as a command-line argument.

Features of `oxbloom-security`: `jwks` (`ReqwestJwksFetcher`) and `di` (`ContainerBuilder::security_chain(chain)`, resolve `Arc<SecurityFilterChain>`).
