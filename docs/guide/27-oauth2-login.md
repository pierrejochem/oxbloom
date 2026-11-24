# OAuth2 login

Oxbloom can log users in through an external identity provider with the OAuth2 authorization code flow and PKCE. It supports OpenID Connect providers, whose id token says who the user is, and plain OAuth2 providers such as GitHub, whose user-info endpoint does. A user who comes back from the provider is an ordinary session user, exactly like after a form login: the session id changes, the `Authentication` is stored in the session, logout and the saved request work, and the session limit applies. This is security Slice 9, part B. It builds on [Form login](21-form-login.md#form-login), [Sessions](20-sessions.md#sessions) and the [HTTP client](26-http-client.md#http-client). To call other services with the tokens of the logged-in user, or with tokens of the application itself, see [OAuth2 client](28-oauth2-client.md#oauth2-client).

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["web", "oauth2"] }        # SecurityAutoConfiguration reads security.oauth2.*
oxbloom-security = { version = "0.1", features = ["oauth2"] }           # HttpSecurity::oauth2_login in code
oxbloom-test = { version = "0.1", features = ["oauth2"] }               # FakeProvider (dev-dependency)
```

The boot feature `oauth2` turns on `security` and `http-client`, so the provider requests go through the `dyn HttpClient` bean of `HttpClientAutoConfiguration` (see [Configuration and the bean](26-http-client.md#configuration-and-the-bean)). Without the feature, boot does not read `security.oauth2`; when the configuration has that section anyway, `SecurityAutoConfiguration` logs one warning at startup (target `oxbloom::boot`) that names the key and the feature. The decisions are in [ADR 0094](../adr/0094-oauth2-login.md) and the public items are in the [API reference](../API.md#415-oauth2-login). The design is a local file under `docs/superpowers`, so this page names it without a link.

## How a login runs

1. A link or a button sends the browser to `GET /oauth2/authorization/{id}`, where `{id}` is the registration id, for example `/oauth2/authorization/github`.
2. The chain saves a pending login in the session (a random `state`, a PKCE verifier and, for OpenID Connect, a `nonce`) and redirects to the provider's authorization endpoint.
3. The user logs in at the provider, which redirects back to `GET /login/oauth2/code/{id}` with a `code` and the `state`.
4. The chain takes the pending login (a state works once), checks the state, exchanges the code at the token endpoint, and validates the id token or loads the user info.
5. The user is logged in as with form login and redirected to the saved request, or to the default success URL of form login.

Every failure redirects to the failure URL (by default the failure URL of form login, `/login?error`) with no detail in the response. The cause is logged at debug level on the target `oxbloom::security`, with fixed text and no token, code or secret.

## Configuration

Boot turns OAuth2 login on when `security.oauth2.client.registration` has at least one entry. Form login and the session table must be configured too, and form login needs no password provider then.

```toml
[security.form-login]
enabled = true                      # required: OAuth2 login uses the form login session

[security.session]
store = "memory"                    # a server-side store keeps the tokens out of the cookie

[security.oauth2]
base-url    = "https://app.example" # required; the {base-url} of every redirect URI
failure-url = "/signin?failed"      # optional; default: the failure URL of form login

# An OpenID Connect provider, configured through its issuer (discovery).
[security.oauth2.client.registration.corp]
client-id     = "billing"
client-secret = "${CORP_CLIENT_SECRET}"   # a secret: keep it in a file or the environment
scope         = ["email", "profile"]      # `openid` is added for an OpenID Connect provider
# provider = "corp"                       # default: the registration id
# client-authentication = "basic"         # default; "post" or "none" (public client with PKCE)

[security.oauth2.client.provider.corp]
issuer-uri = "https://login.corp.example"

# A plain OAuth2 provider (GitHub-like), configured with its endpoints.
[security.oauth2.client.registration.github]
client-id     = "Iv1.0123456789abcdef"
client-secret = "${GITHUB_CLIENT_SECRET}"
scope         = ["read:user"]

[security.oauth2.client.provider.github]
authorization-uri   = "https://github.com/login/oauth/authorize"
token-uri           = "https://github.com/login/oauth/access_token"
user-info-uri       = "https://api.github.com/user"
user-name-attribute = "id"          # GitHub's stable numeric id; `login` can be renamed
```

The keys:

| Key | Meaning |
|---|---|
| `security.oauth2.base-url` | The external URL of the application: an absolute `https` URL (`http` only for `localhost`, a `127.0.0.0/8` address or `::1`) with no path beyond an optional `/`, no query and no fragment. Required when a registration exists. |
| `security.oauth2.failure-url` | Where a failed login goes: a local path outside `/oauth2/authorization/` and `/login/oauth2/code/`. Optional. |
| `security.oauth2.client.registration.<id>.client-id` | The client id. Required. |
| `security.oauth2.client.registration.<id>.client-secret` | The client secret, required unless `client-authentication` is `none`. Never printed. |
| `security.oauth2.client.registration.<id>.scope` | The scopes to ask for. |
| `security.oauth2.client.registration.<id>.redirect-uri` | Default `{base-url}/login/oauth2/code/{id}`, and it must resolve to exactly that (see [Limits](#limits)). |
| `security.oauth2.client.registration.<id>.client-authentication` | `basic` (default), `post` or `none`. |
| `security.oauth2.client.registration.<id>.provider` | The provider id; default the registration id. |
| `security.oauth2.client.provider.<id>.issuer-uri` | The issuer of an OpenID Connect provider; its discovery document gives the other endpoints. |
| `security.oauth2.client.provider.<id>.authorization-uri`, `token-uri` | The endpoints of a provider without an issuer; both required then. |
| `security.oauth2.client.provider.<id>.user-info-uri` | The user-info endpoint. A plain OAuth2 provider (no `issuer-uri`, no `jwk-set-uri`) needs it and `user-name-attribute`. |
| `security.oauth2.client.provider.<id>.jwk-set-uri` | The key set of the id tokens; a provider with it is an OpenID Connect provider. |
| `security.oauth2.client.provider.<id>.user-name-attribute` | The attribute that names the user (default `sub` for OpenID Connect; required for a plain OAuth2 provider). |

Registration and provider ids hold only `a` to `z`, `0` to `9` and `-`, because they appear in the URLs. Every provider endpoint must be an absolute `https` URL, with `http` allowed for loopback hosts only. The keys are checked at startup even when a customizer configures OAuth2 login itself: an unknown key under `security.oauth2`, a missing client id, a missing secret, a plain provider without both `user-info-uri` and `user-name-attribute`, a bad URL or a redirect URI that does not lead to the callback fails the start with an error that names the full key (for example `security.oauth2.client.registration.corp.redirect-uri`) and never the value or the secret. A login built in code with `OAuth2Login` gets the same shape checks at `build()`, which fails with `SecurityBuildError::OAuth2LoginWithoutRegistration`. Without form login the startup failure report says to set `security.form-login.enabled = true`; without an `HttpClient` bean (for example when `HttpClientAutoConfiguration` is excluded) it says that the bean is missing.

## A login link and a protected route

The default login page of form login does not list the registrations, so add your own links, for example on a custom login page:

```html
<a href="/oauth2/authorization/corp">Sign in with Corp</a>
<a href="/oauth2/authorization/github">Sign in with GitHub</a>
```

A protected route reads the user like any other: the principal is the value of `user-name-attribute`, the authorities are `OIDC_USER` or `OAUTH2_USER` plus `SCOPE_<scope>` for each granted scope, and the details hold the user attributes, `oauth2.registration` (the registration id) and `oauth2.principal = true`. Provider attributes whose names start with `oxbloom.` or `oauth2.` are dropped, so a provider cannot set a framework detail such as `oxbloom.remembered`.

```rust
use oxbloom_security::RequestSecurityExt;
use oxbloom_web::{IntoResponse, Request, controller};

struct Profile;

#[controller("/me")]
impl Profile {
    #[get("")]
    async fn me(&self, req: Request) -> impl IntoResponse {
        let Some(user) = req.authentication() else {
            return "anonymous".to_owned();
        };
        let detail = |key: &str| user.details().get(key).and_then(|v| v.as_str());
        let provider = detail("oauth2.registration").unwrap_or("password");
        let email = detail("email").unwrap_or("-");
        format!("{} via {provider} ({email})", user.principal())
    }
}
```

A rule can require an OAuth2 user (`has_authority("OIDC_USER")`) or a granted scope (`has_authority("SCOPE_read:user")`). A `SCOPE_*` authority does not say which provider granted it; when that matters, check `oauth2.registration` too.

## Configuring OAuth2 login in code

Without boot, or to set what the keys cannot, call `HttpSecurity::oauth2_login`. It needs `form_login`, an `HttpClient`, at least one registration with its provider, and the base URL.

```rust
use std::sync::Arc;
use oxbloom_config::Secret;
use oxbloom_http_client::{ClientBuilder, HttpClient};
use oxbloom_security::{
    ClientAuthentication, ClientRegistration, HttpSecurity, OAuth2Login, ProviderConfig,
    SecurityFilterChain,
};

fn chain(secret: Secret<String>) -> Result<SecurityFilterChain, Box<dyn std::error::Error>> {
    let client: Arc<dyn HttpClient> = Arc::new(ClientBuilder::new().build()?);
    Ok(HttpSecurity::new()
        .form_login(|f| f)
        .oauth2_login(|_| {
            OAuth2Login::new(client)
                .registration(ClientRegistration {
                    id: "corp".into(),
                    client_id: "billing".into(),
                    client_secret: Some(secret),
                    scopes: vec!["email".into()],
                    redirect_uri: "{base-url}/login/oauth2/code/{id}".into(),
                    client_authentication: ClientAuthentication::Basic,
                    provider: "corp".into(),
                })
                .provider("corp", ProviderConfig {
                    issuer_uri: Some("https://login.corp.example".into()),
                    ..ProviderConfig::default()
                })
                .base_url("https://app.example")
                .failure_url("/signin?failed")
        })
        .authorize(|a| a.get("/").permit_all().any_request().authenticated())
        .build()?)
}
```

In a boot application, a `SecurityCustomizer` that calls `oauth2_login` wins: `SecurityAutoConfiguration` then does not configure OAuth2 login from the keys and does not require `security.oauth2.base-url`. `OAuth2Settings::from_config` reads the registrations and providers of the configuration, if the customizer wants to start from them.

## Testing with `FakeProvider`

`FakeProvider` (feature `oauth2` of `oxbloom-test`) is an OpenID Connect provider served through a `MockHttpClient`: discovery, the key set (a fresh ES256 key), `POST /token` and `/userinfo` under its issuer. Register its client as the `dyn HttpClient` bean, follow the redirect to the provider by hand, let the fake issue a code, and call the callback with the `state` from the authorization URL. `expect_pkce`, `expect_redirect_uri`, `expect_nonce` and `expect_client_secret` make the token endpoint check what the login sent, and `misbehave` drives negative tests. `TestApp::secured()` has no `HttpClientAutoConfiguration`, so the fake's client is the only client:

```rust
use std::sync::Arc;
use oxbloom_http_client::HttpClient;
use oxbloom_test::{FakeProvider, MockHttpClient, TestApp};

#[tokio::test]
async fn logs_in_through_the_provider() {
    let fake = FakeProvider::new("https://idp.example.test");
    let client = fake.client();
    let app = TestApp::secured()
        .config_str(
            "[security.form-login]\nenabled = true\n[security.session]\n\
             [security.oauth2]\nbase-url = \"https://app.example.test\"\n\
             [security.oauth2.client.registration.idp]\nclient-id = \"app\"\n\
             client-secret = \"test-secret\"\n\
             [security.oauth2.client.provider.idp]\nissuer-uri = \"https://idp.example.test\"\n",
        )
        .configure(move |b| {
            b.instance(client)
                .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
        })
        .start()
        .await;
    let start = app.client().get("/oauth2/authorization/idp").send().await;
    assert_eq!(start.status(), 302);
    assert!(start.header("set-cookie").is_some()); // the session that holds the pending login
    // Read `state` and `nonce` from the `location` header, keep the session cookie, then:
    // fake.expect_nonce(nonce);
    // let code = fake.issue_code(FakeUser::new("alice").scope("openid email"));
    // app.client().get(&format!("/login/oauth2/code/idp?code={code}&state={state}"))
    //     .header("cookie", cookie).send().await  -> 302 to "/"
    app.stop().await;
}
```

The boot tests in `crates/oxbloom-boot/tests/oauth2_login.rs` run the whole flow this way, for an OpenID Connect and a plain provider.

## Limits

- **Provider calls use the shared HTTP client.** Discovery, the key set, the token exchange and the user info go through the application's one `HttpClient` bean, so the `http.client.*` keys apply to them: `follow-redirects` (default 0, so a provider endpoint that answers with a redirect fails the login), `max-body-bytes`, the proxy and the SSL bundle. Discovery, token and user info requests carry their own timeout of 10 seconds, which replaces `http.client.timeout` (a shorter client timeout does not apply to them, a longer one is cut to 10 seconds); the key set fetch waits for the shorter of the client timeout and 10 seconds. The 10 seconds are per attempt: with `http.client.retry.max-attempts` above 1 the client retries idempotent GET requests, so a call can take several times 10 seconds plus the backoff. If you enable `follow-redirects` for other calls, provider calls follow redirects too, within the hop rules of [ADR 0093](../adr/0093-http-client.md).
- **Tokens in the session.** The access token, the refresh token and the id token are kept in the session. A cookie-backed session (`store = "cookie"`) works, but the tokens can make the cookie larger than browsers accept; use a server-side store.
- **`SameSite=Strict` loses the login.** The browser does not send a `Strict` session cookie on the provider's redirect back, so the pending login is not found and the login fails. Keep the default `Lax`.
- **No RP-initiated logout.** Logout ends the local session only; the user stays logged in at the provider.
- **Form login is required**, also in an application without passwords.
- **The callback path is fixed** at `/login/oauth2/code/{id}`. A `redirect-uri` must resolve to `{base-url}/login/oauth2/code/{id}`; anything else fails the start on that registration's `redirect-uri` key.
- **No path prefix.** `security.oauth2.base-url` has no path, so the redirect URI is always `https://<host>/login/oauth2/code/{id}` and the two endpoints live at the root of the external host. Deploying the application under a path prefix (`https://example.com/app/`) is not supported for OAuth2 login in this slice (ruling R15 keeps the base URL free of a path and the callback path fixed). Behind a reverse proxy, the proxy must route `/oauth2/authorization/` and `/login/oauth2/code/` at the root of the external host to the application, and `base-url` is that external origin.
- **`SCOPE_*` authorities are not tied to a registration.** A provider of low trust that grants a scope named `admin` produces `SCOPE_admin`. Check `oauth2.registration` when it matters.
- **A provider without an issuer.** A provider with `jwk-set-uri` and no `issuer-uri` is accepted; its id token `iss` is only required to be non-empty. The startup logs a warning for each such registration. Configure `issuer-uri` when the provider has one.
- **Numeric principals.** A JSON integer in `user-name-attribute` becomes a string principal (`4242`). A boolean, a float, an array, an object, `null` or an empty string fails the login.
- **GitHub's 200 answer with an `error` field.** GitHub answers some token endpoint failures with status 200 and an `error` field instead of a token. Oxbloom treats such an answer as a generic token error, without the provider's description.
- **A token without `kid` is refused**, even from a provider that has a single key (a documented gap).
- **Fractional times are refused.** An id token whose `exp` or `iat` is not a whole number of seconds fails.
- **HEAD is 405.** The two endpoints answer only GET; any other method, HEAD included, is a 405.
- **The default login page does not list the registrations.** Add the links yourself.
- **`invalidate_all` needs the qualified key.** The session limit and the session registry count an OAuth2 user under `oauth2:<registration>:<principal>`, so `SessionRegistry::invalidate_all("oauth2:github:4242")` ends that user's sessions, and the bare principal does not.
- **Usernames that start with `oauth2:` are reserved, and this is enforced.** A password user literally named `oauth2:github:4242` would share the OAuth2 user's session-limit key, so with the feature `oauth2` a password or remember-me login of such a user fails like a wrong password (logged at debug level). Refuse such names in your user store as well, so the user is told early.
- **PS256 is not supported**, and neither are symmetric algorithms or `none`. The accepted algorithms are RS256, RS384, RS512, ES256 and ES384.
- **No provider presets.** Configure each provider with its issuer or its endpoints.
