# OAuth2 client

Oxbloom can get OAuth2 access tokens for outgoing calls: tokens for the application itself, with the client credentials grant, and the tokens of the logged-in user, which the [OAuth2 login](27-oauth2-login.md#oauth2-login) stored in the session and the client refreshes with the refresh token grant. Interceptors for the [HTTP client](26-http-client.md#http-client) put the token into the `Authorization: Bearer` header of each request. This is security Slice 9, part C.

```toml
# Cargo.toml
oxbloom-boot = { version = "0.1", features = ["web", "oauth2"] }        # OAuth2ClientAutoConfiguration
oxbloom-security = { version = "0.1", features = ["oauth2"] }           # OAuth2Client and the interceptors
oxbloom-test = { version = "0.1", features = ["oauth2"] }               # FakeProvider and MockHttpClient (dev-dependency)
```

The decisions are in [ADR 0095](../adr/0095-oauth2-client-credentials.md) and the public items are in the [API reference](../API.md#415-oauth2-login). The design is a local file under `docs/superpowers`, so this page names it without a link.

## Registrations

The client reads the same `security.oauth2.client.*` keys as the login (see [Configuration](27-oauth2-login.md#configuration)). Each registration names a client id, its secret, the scopes and a provider, and the provider is given by its issuer (discovery) or by its endpoints. With the feature `oauth2` of `oxbloom-boot`, `OAuth2ClientAutoConfiguration` registers one `OAuth2Client` bean (fetched as `Arc<OAuth2Client>`) whenever `security.oauth2.client.registration` has an entry:

```toml
[security.form-login]
enabled = true                       # needed while SecurityAutoConfiguration runs (see Limits)

[security.session]

[security.oauth2]
base-url = "https://app.example"     # needed while SecurityAutoConfiguration runs (see Limits)

# The users log in with `corp`; the client refreshes their tokens.
[security.oauth2.client.registration.corp]
client-id     = "billing-web"
client-secret = "${CORP_CLIENT_SECRET}"
scope         = ["email", "orders.read"]

# The application calls the orders service as itself with `billing`.
[security.oauth2.client.registration.billing]
client-id     = "billing-service"
client-secret = "${BILLING_CLIENT_SECRET}"
scope         = ["orders.read"]
provider      = "corp"

[security.oauth2.client.provider.corp]
issuer-uri = "https://login.corp.example"
```

The bean:

- reads the same parsed settings as the login of `SecurityAutoConfiguration`: the section is parsed once per application, and a bad registration fails the start with an error that names the full key (for example `security.oauth2.client.registration.billing.client-id`) and never the value or the secret;
- sends every provider request through the `dyn HttpClient` bean, from `HttpClientAutoConfiguration` or from the application, so the `http.client.*` keys apply; without such a bean the start fails with a report that says the `HttpClient` bean is missing;
- takes its time from the `dyn Clock` bean when there is one, and replaces a token 30 seconds (the skew) before it expires;
- steps aside when the application registers an `OAuth2Client` of its own, for example one built with `OAuth2Client::new(settings, client).skew(..)`.

## Client credentials

`OAuth2Client::client_credentials(registration_id)` returns an `AccessToken` for the application itself. The client sends `grant_type=client_credentials`, the scopes of the registration and its client authentication, keeps the token per registration until the skew before its expiry, and lets callers that arrive while a request is in flight wait for that request, so many tasks share one token and one request per expiry. A public registration (`client-authentication = "none"`) cannot use this grant and gets `TokenError::Invalid`.

Two wrappers over the shared `dyn HttpClient` add the token to outgoing requests:

- `ClientCredentialsInterceptor::new(oauth2, registration_id)` is an `Interceptor` for an `InterceptedClient`. It never retries.
- `BearerRetryClient::new(inner, oauth2, registration_id)` is an `HttpClient`. When the service answers 401, it drops the refused token from the cache (only if it is still the cached one, so a late 401 cannot discard a fresh token) and sends the request once more with a new token.

```rust
use std::sync::Arc;
use oxbloom_http_client::{HttpClient, HttpClientError, Request};
use oxbloom_security::{BearerRetryClient, OAuth2Client};

/// A component that calls the orders service as the application itself.
struct Orders {
    http: BearerRetryClient,
}

impl Orders {
    /// `client` is the `dyn HttpClient` bean and `oauth2` the `OAuth2Client` bean.
    fn new(client: Arc<dyn HttpClient>, oauth2: Arc<OAuth2Client>) -> Orders {
        Orders { http: BearerRetryClient::new(client, oauth2, "billing") }
    }

    async fn open_orders(&self) -> Result<String, HttpClientError> {
        let response = self
            .http
            .execute(Request::get("https://orders.example/api/orders?state=open"))
            .await?
            .error_for_status()?;
        response.text()
    }
}
```

When no token can be had, the wrappers do not send the request and return `HttpClientError::InvalidRequest`; the reason is logged at debug level on the target `oxbloom::security`. Call `client_credentials` yourself first when you need the `TokenError`.

## The logged-in user's tokens

`OAuth2Client::access_token_for(&request, registration_id)` returns the access token of the user of an incoming request: the token that the OAuth2 login stored in the session, refreshed when it is due. The call checks first that the current authentication is the OAuth2 login of that registration; a password user, an anonymous request or a user of another registration gets `TokenError::ReauthenticationRequired`, so a token never reaches a principal it was not issued to. The same error means that the session holds no usable tokens any more (no refresh token, or the provider refused it): send the user to the login again.

A handler that calls a downstream service with the logged-in user's token:

```rust
use std::sync::Arc;
use http::StatusCode;
use oxbloom_http_client::{HttpClient, InterceptedClient, Request as OutRequest};
use oxbloom_security::{OAuth2Client, TokenError, UserTokenInterceptor};
use oxbloom_web::{HttpError, IntoResponse, Request, controller};

struct Profile {
    oauth2: Arc<OAuth2Client>,
    http: Arc<dyn HttpClient>,
}

#[controller("/profile")]
impl Profile {
    #[get("")]
    async fn show(&self, req: Request) -> Result<impl IntoResponse, HttpError> {
        // Ask for the token first: then a missing login can be told apart from other failures.
        match self.oauth2.access_token_for(&req, "corp").await {
            Ok(_) => {}
            Err(TokenError::ReauthenticationRequired) => {
                return Err(HttpError::new(StatusCode::UNAUTHORIZED).detail("log in again"));
            }
            Err(_) => return Err(HttpError::new(StatusCode::BAD_GATEWAY)),
        }
        // The interceptor takes the session and the authentication of this request; build one
        // per request.
        let http = InterceptedClient::new(self.http.clone()).with(Arc::new(
            UserTokenInterceptor::for_request(self.oauth2.clone(), &req, "corp"),
        ));
        let response = http
            .execute(OutRequest::get("https://orders.example/api/me/orders"))
            .await
            .map_err(|_| HttpError::new(StatusCode::BAD_GATEWAY))?;
        Ok(response.text().unwrap_or_default())
    }
}
```

How a refresh works:

- The stored access token is used until 30 seconds before its expiry. A token the login stored without an expiry (the provider sent no `expires_in`) is used as it is and never refreshed early.
- A due token is refreshed with `grant_type=refresh_token` and the stored refresh token. The new tokens go to the session (a new refresh token replaces the old one), and the session filter stores them when the request ends.
- Refreshes of one refresh token are serialized, and each successful refresh is remembered for 10 minutes: a concurrent request, or a stale copy of the session that still holds the spent refresh token, adopts the new tokens instead of sending the spent token again. With providers that rotate refresh tokens and refuse a spent one, this keeps overlapping requests of one browser from logging the user out.
- The refresh runs in a task of its own, so a request that is dropped mid-refresh still records the rotated token for the next request.

## Testing with `FakeProvider` and `MockHttpClient`

`FakeProvider` (feature `oauth2` of `oxbloom-test`) serves discovery, the key set and the token endpoint through a `MockHttpClient`, with the grants `authorization_code`, `client_credentials` and `refresh_token`. Register its client as the `dyn HttpClient` bean, and give the component under test another `MockHttpClient` as the downstream service:

```rust
use std::sync::Arc;
use http::Method;
use oxbloom_boot::OAuth2ClientAutoConfiguration;
use oxbloom_http_client::{HttpClient, Request};
use oxbloom_security::{BearerRetryClient, OAuth2Client};
use oxbloom_test::{ContextRunner, FakeProvider, MockHttpClient, MockResponse};

#[tokio::test]
async fn the_orders_call_carries_the_application_token() {
    let fake = FakeProvider::new("https://idp.example.test");
    fake.expect_client_secret("test-secret");
    let provider = fake.client();
    let seen = Arc::new(std::sync::Mutex::new(None));
    let out = seen.clone();
    ContextRunner::new()
        .config_str(
            "[security.oauth2.client.registration.billing]\nclient-id = \"billing-service\"\n\
             client-secret = \"test-secret\"\nprovider = \"idp\"\n\
             [security.oauth2.client.provider.idp]\nissuer-uri = \"https://idp.example.test\"\n",
        )
        .auto_configuration::<OAuth2ClientAutoConfiguration>()
        .configure(move |b| {
            b.instance(provider)
                .provides(|c: Arc<Arc<MockHttpClient>>| -> Arc<dyn HttpClient> { (*c).clone() })
        })
        .run(move |context| *out.lock().unwrap() = Some(context.get::<OAuth2Client>().unwrap()))
        .await;
    let oauth2 = seen.lock().unwrap().clone().unwrap();

    let orders = Arc::new(MockHttpClient::new());
    orders.on(Method::GET, "https://orders.example/api/orders", MockResponse::status(200));
    let http = BearerRetryClient::new(orders.clone(), oauth2, "billing");
    for _ in 0..2 {
        http.execute(Request::get("https://orders.example/api/orders")).await.unwrap();
    }
    let sent = orders.requests();
    assert!(sent[0].headers()["authorization"].to_str().unwrap().starts_with("Bearer "));
    assert_eq!(fake.token_requests().len(), 1); // one token for both calls
}
```

For the user's tokens, log in through the fake as in [Testing with `FakeProvider`](27-oauth2-login.md#testing-with-fakeprovider), give the application and the fake one `ManualClock` (`TestApp::clock` and `FakeProvider::clock`), advance it past the token's expiry, and call a route that uses `access_token_for`. `fake.access_token_lifetime(Some(600))` keeps the lifetime below the idle timeout of the session, and `fake.rotate_refresh_tokens(false)` imitates a provider that does not rotate. The boot tests in `crates/oxbloom-boot/tests/oauth2_client.rs` run both flows through a booted application.

## Limits

- **A client-credentials-only registration still needs login fields.** The configuration checks treat every registration like a login registration: a plain provider needs `authorization-uri`, `user-info-uri` and `user-name-attribute`, and a discovered provider must announce an authorization endpoint. For a provider that you only use for client credentials, supply the login fields anyway, even though no login will use them. A per-registration grant types key is parked for a later slice (ruling R24).
- **The registrations also turn on OAuth2 login.** With `SecurityAutoConfiguration` in the list (as in `auto_configurations()`), every registration is also offered for login, so form login, the `security.session` table and `security.oauth2.base-url` are required even in an application that only uses the client, and `/oauth2/authorization/<id>` starts a login for a client-credentials registration too. An application without a login can register its own `SecurityFilterChain` bean (then boot does not configure OAuth2 login) or exclude `SecurityAutoConfiguration`.
- **Provider calls use the shared HTTP client.** Every token and discovery request goes through the `dyn HttpClient` bean, so `http.client.*` applies (redirects, body limit, proxy, SSL bundle), and each request has its own timeout of 10 seconds, which replaces `http.client.timeout`. With `http.client.retry.max-attempts` above 1, the discovery GET can be retried; the token request is a POST and is not.
- **The skew.** A token is replaced 30 seconds before it expires. A token whose lifetime is no longer than the skew is never reused, so every call asks the provider again; keep the skew (`OAuth2Client::skew` on your own bean) below the lifetime the provider issues. Clamping the skew to the lifetime is parked.
- **Backoff.** A failed discovery is remembered for 5 seconds, so calls within that window fail without a request. A failed token request has no backoff: callers that waited for it get its failure, and the next caller asks again at once.
- **The rotation record is per process and in memory.** It keeps at most 256 records for 10 minutes; an older record is no longer used but stays in memory until the next refresh after the 10 minutes. Each record holds copies of the renewed tokens (access, refresh and id token), so a memory dump can show them for that long. Instances behind a load balancer do not share it: use sticky sessions, or a provider that does not rotate refresh tokens, or overlapping requests on two instances can spend a refresh token twice and log the user out. A logout does not purge the records.
- **The binding check.** `access_token_for` works only for a request whose authentication has `oauth2.principal = true` and `oauth2.registration` equal to the requested registration. A user who logged in with another registration, or with a password, gets `ReauthenticationRequired`.
- **Tokens without `expires_in`.** A login token stored without an expiry is reported to expire 60 seconds from now and is never refreshed early; it is used until the downstream service answers 401.
- **`UserTokenInterceptor` is per request.** It takes the session and the authentication of one request when it is built; kept past its request it fails every call. It maps every token error to `HttpClientError::InvalidRequest`; call `access_token_for` first to tell `ReauthenticationRequired` apart, as in the example above.
- **One retry after a 401, and only for client credentials.** `BearerRetryClient` retries once, also a `POST`, because a 401 means the service refused the request before processing it; do not put a service that processes a request and then answers 401 behind it. `UserTokenInterceptor` and `ClientCredentialsInterceptor` never retry.
- **Not supported:** the device code grant, token exchange (RFC 8693), token revocation, and a persistent token store. Client credentials tokens live in memory per process; the user's tokens live in the session and end with it.
- **Two metadata caches.** The login and the client each keep their own discovery cache, so an application with both fetches the discovery document of each provider twice per hour. Sharing one cache is parked.
- **`TokenError` is `Copy`.** Its variants carry no data beyond a fixed error code, so adding a variant with a message or a source would be a breaking change.
