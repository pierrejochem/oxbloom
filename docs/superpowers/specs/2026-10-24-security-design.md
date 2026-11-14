# oxbloom-security design (Slice 5)

Date: 2026-10-24

## Purpose

Give Oxbloom applications a security layer modeled on Spring Security: a filter chain in front of the router, an authentication pipeline, an authorization step that denies by default, and a JWT resource server with static keys and JWKS. Slice 5 delivers the web filter layer, the security core and the JWT resource server. Later slices add the rest of Spring Security's surface.

## Roadmap

| Slice | Content |
|---|---|
| 5 (this spec) | Web filter layer, security core, JWT bearer resource server with static keys and JWKS |
| 6 | `PasswordEncoder`, `UserDetailsService`, HTTP Basic, form login with sessions |
| 7 | CSRF, session management, security headers |
| 8 | Method security (`@PreAuthorize` equivalent), CORS |
| 9 | OAuth2 login and client |

## Scope of this slice

In scope:

- A `Filter` layer in `oxbloom-web`.
- The new crate `oxbloom-security`: authentication model, `ProviderManager`, authorization rules, `HttpSecurity` builder, `SecurityFilterChain`, 401 and 403 handlers.
- Bearer token extraction, `JwtDecoder`, JWKS source with caching, principal mapping.
- Integration with `oxbloom-config` (a normal dependency, because `Secret<T>` appears in core types) and `oxbloom-di` (optional feature `di`).

Out of scope, each deferred to the slice shown in the roadmap:

- Several filter chains with different matchers. One chain per application. This is a deliberate YAGNI cut.
- Sessions, password login, HTTP Basic, CSRF, method security, CORS, security headers, OAuth2 login.
- Opaque-token introspection.
- Writing any cryptographic code. All signature work uses `jsonwebtoken`.

## Part 1: filter layer in `oxbloom-web`

- `Filter` is an async trait with `filter(&self, req: Request, next: Next<'_>) -> Response`.
- `Next::run(self, req: Request) -> Response` calls the rest of the chain. The last element is the router.
- `App::filter(self, f: impl Filter + 'static) -> Self` appends a filter. Filters run in registration order and run **before routing**, so a chain applies to unknown paths too. Under a deny-by-default chain an unknown path returns 401 or 403, never 404, and route existence is not leaked.
- `Request` gains a typed extensions map: `extensions(&self)` and `extensions_mut(&mut self)`, backed by `http::Extensions`.
- A filter returns a `Response`, not a `Result`. Filters render their own error responses.
- With no filters registered, the request path and every existing behavior stay identical. All existing tests must pass unchanged.

## Part 2: security core

### Authentication model

- `GrantedAuthority` is a string newtype such as `ROLE_ADMIN` or `SCOPE_read`.
- `Authentication` holds a `principal` (`String`), a set of `GrantedAuthority`, a `details` map of claims, and is always authenticated. An unauthenticated state is a different type.
- `AuthenticationRequest` is an enum of unauthenticated inputs. This slice has one variant, `BearerToken(Secret<String>)`.
- `SecurityContext` wraps `Option<Authentication>` and is stored in the request extensions. `Request` gets an accessor `authentication() -> Option<&Authentication>` through an extension trait in `oxbloom-security`. This replaces Spring's `SecurityContextHolder`, because thread-locals are unsafe across `.await`.
- `Debug` for `Authentication` and `SecurityContext` is written by hand. It prints the principal and authority names and never claim values.

### Authentication pipeline

- `AuthenticationProvider` is an async trait with `supports(&AuthenticationRequest) -> bool` and `authenticate(&AuthenticationRequest) -> Result<Authentication, AuthenticationError>`.
- `ProviderManager` holds an ordered list of providers. It uses the first provider that supports the request and returns that provider's result. A definitive error such as `BadCredentials` or `InvalidToken` is returned as is. It never falls through to another provider.
- `AuthenticationError` has these variants: `BadCredentials`, `Expired`, `InvalidToken`, `ProviderUnavailable`. `Display` text is fixed and never contains token contents, keys or claim values.

### Authorization

- `AuthorizationRule` is a closed enum: `PermitAll`, `DenyAll`, `Authenticated`, `HasRole(String)`, `HasAnyRole(Vec<String>)`, `HasAuthority(String)`, `HasAnyAuthority(Vec<String>)`, and `Custom(Arc<dyn AuthorizationManager>)`.
- `HasRole("ADMIN")` checks the authority `ROLE_ADMIN`, as in Spring.
- `RequestMatcher` matches an HTTP method (or any) and an Ant-style path pattern: `*` for one segment, `**` for the rest.
- Rules are evaluated in declaration order and the first matching matcher wins. **If no matcher matches, the request is denied.**

### Configuration builder

```rust
let chain = HttpSecurity::new()
    .authorize(|a| a
        .get("/public/**").permit_all()
        .path("/admin/**").has_role("ADMIN")
        .any_request().authenticated())
    .oauth2_resource_server(|r| r.jwt(decoder))
    .build()?;
let app = App::new().filter(chain);
```

- `HttpSecurity::build()` returns `Result<SecurityFilterChain, SecurityBuildError>`. It is an error to build without an `authorize` section, so there is no way to build an unprotected chain by accident. A matcher placed after `any_request()` is also a build error, because it could never match. Calling `authorize` more than once is a build error (`DuplicateAuthorizeSection`), because a later call would silently replace earlier rules. A pattern with a segment that contains `{`, `}` or `?`, or contains `*` without being exactly `*` or `**`, is a build error (`InvalidPattern`), because such a rule would never match and a permissive rule after it would then apply.
- `SecurityFilterChain` implements `Filter`. For every request it does the following in order: extract credentials, authenticate (when credentials exist), store the `SecurityContext`, authorize, then call `next` on success.
- Missing credentials mean anonymous. The matching rule decides. `Authenticated` on an anonymous request produces the entry point response. `HasRole` for an authenticated user without that role produces the access-denied response. A `HasRole` rule on an anonymous request produces the entry point response.

### Failure responses

- `AuthenticationEntryPoint` and `AccessDeniedHandler` are traits. Defaults:
  - Anonymous request, or any invalid token: status 401 with `WWW-Authenticate: Bearer`. When a token was supplied and failed, the header also carries `error="invalid_token"` (RFC 6750). No description text is derived from the token. An empty body.
  - Authenticated but not authorized: status 403, empty body.
  - A JWKS or provider outage that prevents validation (`ProviderUnavailable`): status 503, empty body, no `WWW-Authenticate` challenge.

## Part 3: JWT resource server

Signature verification and parsing use the `jsonwebtoken` crate.

### Token extraction

- The filter reads `Authorization: Bearer <token>`. The scheme match is case-insensitive.
- More than one `Authorization` header, or a header whose value is not a well-formed `Bearer` credential, is an invalid request: 401 with `error="invalid_request"`.
- Tokens in the query string or in the body are not supported.
- A missing header means anonymous, not an error.

### Decoder

- `JwtDecoder` is an async trait: `decode(&self, token: &str) -> Result<Jwt, AuthenticationError>`.
- `Jwt` holds the header, the claims map and the raw token (as `Secret<String>`). Its `Debug` prints neither the token nor claim values.
- Constructors on a `JwtDecoderBuilder`:
  - `with_secret(alg, Secret<Vec<u8>>)` for HS256, HS384, HS512.
  - `with_public_key_pem(alg, &str)` for RS256, RS384, RS512, ES256, ES384.
  - `with_jwks(source)` for JWKS.
- The algorithm allow-list is fixed when the decoder is built. The token's `alg` header is checked against the list and never selects the key type. `alg: none` is always rejected. A decoder built for symmetric algorithms never accepts asymmetric ones and the other way around. This prevents algorithm confusion.
- Validation: signature; `exp` required by default (an explicit opt-out exists); `nbf`; optional `iss`; optional `aud`; clock skew, default 60 seconds.
- The decoder maps failures to `AuthenticationError::Expired` or `InvalidToken`. A fixed `error="invalid_token"` is all the client sees. A hook `on_failure(FailureReason)` lets the application log the cause. `FailureReason` is an enum (`Expired`, `NotYetValid`, `BadSignature`, `AudienceMismatch`, `IssuerMismatch`, `UnknownKid`, `Malformed`, `AlgorithmNotAllowed`) and carries no token text.

### JWKS

- `JwksFetcher` is an async trait: `fetch(&self, uri: &str) -> Result<JwkSet, FetchError>`.
- `ReqwestJwksFetcher` is the default implementation. It lives behind the cargo feature `jwks` and uses `reqwest` with rustls.
- `JwksSource` wraps a fetcher with a cache:
  - Entry TTL: 5 minutes by default.
  - Lookup is by `kid`. An unknown `kid` triggers one refresh. Refreshes are rate-limited to one per 30 seconds, so random `kid` values cannot make the service hammer the issuer.
  - Concurrent lookups share a single in-flight fetch.
  - If a refresh fails and a cached set exists, the cached set is served until it is older than twice the TTL. After that the decoder fails closed with `ProviderUnavailable`.
  - Only keys whose `kty` and `alg` fit the decoder's allow-list are used. If `use` is present it must be `sig`.
- The JWKS URI must use `https`. A plain `http` URI is rejected unless the application calls `allow_insecure_http()`, which exists for loopback tests.
- The response body is capped at 1 MiB and the request timeout defaults to 5 seconds.
- `FetchError` has fixed text and never contains the response body.

### Principal mapping

- `principal` comes from `sub`. A token without `sub` is invalid.
- Authorities are `SCOPE_<x>` for each entry of `scope` (a space-delimited string) or `scp` (an array), as in Spring.
- `JwtAuthenticationConverter` lets an application map other claims, such as `roles`, to authorities with a chosen prefix.

### Configuration

```toml
[security.jwt]
issuer = "https://issuer.example"
jwk-set-uri = "https://issuer.example/.well-known/jwks.json"
algorithms = ["RS256"]
audience = ["my-api"]
clock-skew-secs = 60
```

- The symmetric `secret` is a `Secret<String>` and comes from an environment variable, for example `APP_SECURITY__JWT__SECRET`.
- A function builds a `JwtDecoder` from a `oxbloom-config` `Config`. A missing or contradictory combination is a startup error that names the key: both `secret` and `jwk-set-uri` set, no `algorithms`, a symmetric algorithm without a `secret`, an asymmetric algorithm without a key source.

### DI integration (optional `di` feature)

- `ContainerBuilder` gets an extension that registers a built `SecurityFilterChain` as a singleton, so an application can resolve it and pass it to `App::filter`.

## Errors and logging

- `AuthenticationError`, `FetchError` and `SecurityBuildError` use fixed `Display` text. They never contain a token, a key, a claim value or a JWKS body.
- Raw tokens and symmetric keys are `Secret` values.

## Dependencies

- `jsonwebtoken` for JWT. `reqwest` (rustls) only behind the `jwks` feature, so `oxbloom-security` without that feature pulls in no HTTP client.
- `async-trait`, unless native async functions in traits suffice for the object-safe cases. The plan decides.
- Each new dependency is pinned to a version at least two weeks old with no known advisory that applies. Versions stay consistent across the workspace.
- `oxbloom-security` depends on `oxbloom-web`, `oxbloom-core` and `oxbloom-config`. `jwks` and `di` are its only optional cargo features.

## Testing

All tests run offline.

Web filter layer:

- Filters run in registration order and before routing.
- A filter can short-circuit.
- Extensions set by a filter reach the handler.
- With no filters, behavior is identical to today. The existing tests pass unchanged.

Authorization:

- Deny by default when no matcher matches.
- First match wins.
- `HasRole` maps to the `ROLE_` authority.
- An unknown path returns 401 or 403 under an `Authenticated` catch-all, never 404.
- A build without an `authorize` section fails, and so does a matcher after `any_request()`, a second `authorize` call, and an invalid pattern.
- A `HEAD` request is authorized as `GET`: the chain also applies a rule whose method is `GET` to `HEAD`, because the router runs the GET handler for HEAD. The matcher itself stays strict. Credential headers (`authorization`, `proxy-authorization`, `cookie`) are marked sensitive so `Debug` output of a request does not print them.

JWT, with tokens signed in the tests by `jsonwebtoken` using generated keys:

- Valid token.
- Expired token.
- `nbf` in the future.
- Missing `exp`.
- Wrong `iss` and wrong `aud`.
- Clock skew boundary on both sides.
- `alg: none`.
- Algorithm confusion: an RS256 public key presented as an HS256 secret.
- Tampered payload.
- Duplicate and malformed `Authorization` headers, and a lowercase `bearer` scheme.
- Missing `sub`.
- `scope` as a string and `scp` as an array.

JWKS, with an in-memory fetcher that counts calls:

- Cache hit.
- TTL expiry.
- Key rotation through an unknown `kid`.
- The refresh rate limit under many random `kid` values.
- Concurrent lookups share one fetch.
- Stale serve, then fail closed with 503 after twice the TTL.
- A non-https URI is rejected, and `allow_insecure_http()` permits loopback.
- An oversized body is rejected.
- A key whose `alg` is outside the allow-list is ignored.

Responses and hygiene:

- 401 carries `WWW-Authenticate: Bearer` and, for a bad token, `error="invalid_token"` with no token text.
- 403 has an empty body.
- 503 on provider outage.
- No token, secret or claim value appears in any error's `Display` or `Debug`, in `Jwt`, or in `Authentication`.

Integration:

- One test starts the real server on a local port with a JWT-protected route and checks 200, 401 and 403 over HTTP.
- `ReqwestJwksFetcher` is tested against a local hyper server on loopback with `allow_insecure_http()`.
- Configuration: valid configuration builds a decoder, and each contradictory combination is a startup error that names the key.
- With the `di` feature: the chain resolves from the container.

Done means: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets` and `cargo test --workspace` pass, each also with the `jwks` and `di` features on `oxbloom-security`. Public items have doc examples (see `AGENT.md`), the README has a Security section, `docs/API.md` has an entry, and `graphify update .` has run.

## Risks

- Adding a filter layer changes the request path in `oxbloom-web`. The no-filter equivalence test and the unchanged existing suite are the guard.
- Request-extension storage requires `Request` to be mutable in the chain. The plan verifies `Request`'s current shape before changing it.
- The JWKS cache has concurrency subtleties (shared in-flight fetch, rate limit, stale serve). It gets the most tests and its own task.
- `reqwest` brings a large transitive tree. It stays behind the `jwks` feature.
- `jsonwebtoken` API details (key formats, validation options) are checked against its documentation when the plan is written, not assumed.
