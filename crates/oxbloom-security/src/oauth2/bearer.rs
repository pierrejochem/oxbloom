//! Interceptors for `oxbloom-http-client` that add `Authorization: Bearer <token>` to
//! outgoing requests: with the application's own token (client credentials), with a retry on
//! 401, and with the logged-in user's token.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use oxbloom_http_client::{HttpClient, HttpClientError, Interceptor, Request, Response};

use super::client::{AccessToken, OAuth2Client, TokenError};
use crate::{Authentication, RequestSecurityExt, RequestSessionExt, Session};

/// The error of an interceptor whose token could not be had. The interceptor trait can only
/// return an `HttpClientError`, so every `TokenError` becomes the fixed-text
/// `InvalidRequest`: the request cannot be sent as asked, because it cannot be authorized. The
/// `TokenError` is logged at debug level, with its fixed text and the registration id only.
fn token_failure(registration_id: &str, error: TokenError) -> HttpClientError {
    tracing::debug!(
        target: "oxbloom::security",
        registration = %registration_id,
        error = %error,
        "no OAuth2 access token for an outgoing request"
    );
    HttpClientError::InvalidRequest
}

/// Sets `Authorization: Bearer <token>` on `request`, replacing an `Authorization` header that
/// is already there.
fn authorize(request: &mut Request, token: &AccessToken) -> Result<(), HttpClientError> {
    request.set_header("authorization", &format!("Bearer {}", token.value.expose()))
}

/// An [`Interceptor`] that adds the application's own access token, from
/// [`OAuth2Client::client_credentials`], as `Authorization: Bearer <token>`, replacing an
/// `Authorization` header that the request already has.
///
/// It shares the token cache of the client, so many requests share one token and one token
/// request per expiry. It never retries; for a retry with a fresh token when the service
/// answers 401, wrap the client in a [`BearerRetryClient`] instead.
///
/// When no token can be had, the request is not sent and the error is
/// [`HttpClientError::InvalidRequest`] with its fixed text; the reason is logged at debug
/// level. Callers that need the [`TokenError`] call
/// [`client_credentials`](OAuth2Client::client_credentials) themselves first.
///
/// `Debug` shows the registration id only.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use http::Method;
/// use oxbloom_config::Secret;
/// use oxbloom_http_client::{HttpClient, InterceptedClient, Request};
/// use oxbloom_security::{
///     ClientAuthentication, ClientCredentialsInterceptor, ClientRegistration, OAuth2Client,
///     OAuth2Settings, ProviderConfig,
/// };
/// use oxbloom_test::{FakeProvider, MockHttpClient, MockResponse};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let mut settings = OAuth2Settings::default();
/// settings.providers.insert(
///     "idp".into(),
///     ProviderConfig { issuer_uri: Some("https://idp.example.test".into()), ..ProviderConfig::default() },
/// );
/// settings.registrations.push(ClientRegistration {
///     id: "billing".into(),
///     client_id: "orders-service".into(),
///     client_secret: Some(Secret::new("orders-secret".into())),
///     scopes: vec!["invoices.read".into()],
///     redirect_uri: String::new(),
///     client_authentication: ClientAuthentication::Basic,
///     provider: "idp".into(),
/// });
/// let oauth2 = Arc::new(OAuth2Client::new(settings, fake.client()));
///
/// let api = Arc::new(MockHttpClient::new());
/// api.on(Method::GET, "https://billing.example.test/invoices", MockResponse::status(200));
/// let http = InterceptedClient::new(api.clone())
///     .with(Arc::new(ClientCredentialsInterceptor::new(oauth2, "billing")));
/// http.execute(Request::get("https://billing.example.test/invoices")).await?;
///
/// let sent = &api.requests()[0];
/// assert!(sent.headers()["authorization"].to_str()?.starts_with("Bearer "));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct ClientCredentialsInterceptor {
    client: Arc<OAuth2Client>,
    registration_id: String,
}

impl ClientCredentialsInterceptor {
    /// An interceptor that adds the client credentials token of `registration_id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{ClientCredentialsInterceptor, OAuth2Client, OAuth2Settings};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let oauth2 = Arc::new(OAuth2Client::new(OAuth2Settings::default(), Arc::new(MockHttpClient::new())));
    /// let interceptor = ClientCredentialsInterceptor::new(oauth2, "billing");
    /// assert_eq!(format!("{interceptor:?}"), r#"ClientCredentialsInterceptor { registration: "billing" }"#);
    /// ```
    #[must_use]
    pub fn new(client: Arc<OAuth2Client>, registration_id: &str) -> ClientCredentialsInterceptor {
        ClientCredentialsInterceptor {
            client,
            registration_id: registration_id.to_owned(),
        }
    }
}

#[async_trait]
impl Interceptor for ClientCredentialsInterceptor {
    async fn before(&self, request: &mut Request) -> Result<(), HttpClientError> {
        let token = self
            .client
            .client_credentials(&self.registration_id)
            .await
            .map_err(|error| token_failure(&self.registration_id, error))?;
        authorize(request, &token)
    }
}

impl fmt::Debug for ClientCredentialsInterceptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientCredentialsInterceptor")
            .field("registration", &self.registration_id)
            .finish()
    }
}

/// An [`HttpClient`] that sends each request through `inner` with the application's own access
/// token, from [`OAuth2Client::client_credentials`], and retries once with a fresh token when
/// the service answers 401.
///
/// The request is cloned before it is sent. When the answer is `401 Unauthorized`, its body is
/// dropped, the refused token is dropped from the cache with
/// [`invalidate_token`](OAuth2Client::invalidate_token) (only if it is still the cached one, so
/// a late 401 never discards a token that another caller already replaced, and many callers
/// refused with the same token cause one token request between them), and the clone is sent
/// once more with a new token. The answer of that retry is returned as it is, also when it is
/// a 401 again: there is never a second retry. Any other answer, and every error of `inner`,
/// is returned as it is.
///
/// The retry also covers requests with a method that is not idempotent, such as `POST`: a 401
/// means the service refused the request before processing it, so sending it again does not
/// repeat an effect. A service that processes a request and then answers 401 breaks that
/// assumption; do not put such a service behind this client.
///
/// When no token can be had, the request is not sent and the error is
/// [`HttpClientError::InvalidRequest`] with its fixed text; the reason is logged at debug
/// level. This also holds for the retry: when the new token request after a 401 fails, the
/// caller gets `InvalidRequest` and never sees the 401. An invalid request is refused with the
/// same error before a token is fetched. The request is cloned before the first send, so a
/// request with a large body costs a copy of it.
///
/// `Debug` shows the registration id only.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use http::Method;
/// use oxbloom_config::Secret;
/// use oxbloom_http_client::{HttpClient, Request};
/// use oxbloom_security::{
///     BearerRetryClient, ClientAuthentication, ClientRegistration, OAuth2Client, OAuth2Settings,
///     ProviderConfig,
/// };
/// use oxbloom_test::{FakeProvider, MockHttpClient, MockResponse};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let mut settings = OAuth2Settings::default();
/// settings.providers.insert(
///     "idp".into(),
///     ProviderConfig { issuer_uri: Some("https://idp.example.test".into()), ..ProviderConfig::default() },
/// );
/// settings.registrations.push(ClientRegistration {
///     id: "billing".into(),
///     client_id: "orders-service".into(),
///     client_secret: Some(Secret::new("orders-secret".into())),
///     scopes: Vec::new(),
///     redirect_uri: String::new(),
///     client_authentication: ClientAuthentication::Basic,
///     provider: "idp".into(),
/// });
/// let oauth2 = Arc::new(OAuth2Client::new(settings, fake.client()));
///
/// // The service refuses the first token (revoked early), then accepts the new one.
/// let api = Arc::new(MockHttpClient::new());
/// let url = "https://billing.example.test/invoices";
/// api.on(Method::GET, url, MockResponse::status(401));
/// api.on(Method::GET, url, MockResponse::status(200));
///
/// let http = BearerRetryClient::new(api.clone(), oauth2, "billing");
/// let response = http.execute(Request::get(url)).await?;
/// assert_eq!(response.status(), 200);
/// assert_eq!(api.request_count(), 2);
/// assert_eq!(fake.token_requests().len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct BearerRetryClient {
    inner: Arc<dyn HttpClient>,
    client: Arc<OAuth2Client>,
    registration_id: String,
}

impl BearerRetryClient {
    /// A client that sends through `inner` with the client credentials token of
    /// `registration_id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{BearerRetryClient, OAuth2Client, OAuth2Settings};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let api = Arc::new(MockHttpClient::new());
    /// let oauth2 = Arc::new(OAuth2Client::new(OAuth2Settings::default(), api.clone()));
    /// let http = BearerRetryClient::new(api, oauth2, "billing");
    /// assert_eq!(format!("{http:?}"), r#"BearerRetryClient { registration: "billing" }"#);
    /// ```
    #[must_use]
    pub fn new(
        inner: Arc<dyn HttpClient>,
        client: Arc<OAuth2Client>,
        registration_id: &str,
    ) -> BearerRetryClient {
        BearerRetryClient {
            inner,
            client,
            registration_id: registration_id.to_owned(),
        }
    }

    async fn token(&self) -> Result<AccessToken, HttpClientError> {
        self.client
            .client_credentials(&self.registration_id)
            .await
            .map_err(|error| token_failure(&self.registration_id, error))
    }
}

#[async_trait]
impl HttpClient for BearerRetryClient {
    async fn execute(&self, mut request: Request) -> Result<Response, HttpClientError> {
        request.validate()?;
        let retry = request.clone();
        let refused = self.token().await?;
        authorize(&mut request, &refused)?;
        let response = self.inner.execute(request).await?;
        if response.status() != http::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        drop(response);
        self.client
            .invalidate_token(&self.registration_id, &refused);
        let fresh = self.token().await?;
        let mut retry = retry;
        authorize(&mut retry, &fresh)?;
        self.inner.execute(retry).await
    }
}

impl fmt::Debug for BearerRetryClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BearerRetryClient")
            .field("registration", &self.registration_id)
            .finish()
    }
}

/// An [`Interceptor`] that adds the access token of the logged-in user of one incoming
/// request, from [`OAuth2Client::access_token_for`], as `Authorization: Bearer <token>`,
/// replacing an `Authorization` header that the request already has.
///
/// Build one per incoming request with [`for_request`](Self::for_request) and use it while
/// that request is handled: it keeps the session handle and the authentication of that
/// request. After the request has ended, the session filter has taken the session back, so a
/// kept interceptor finds no tokens and every call fails. A token that is due is refreshed
/// and written to the session of the request, as `access_token_for` does, and the same check
/// applies: the authentication must be the OAuth2 login of the registration.
///
/// It never retries: a refused user token usually means the user has to log in again, which
/// only the user can do.
///
/// When no token can be had, the outgoing request is not sent and the error is
/// [`HttpClientError::InvalidRequest`] with its fixed text; the reason is logged at debug
/// level. Callers that need to tell [`TokenError::ReauthenticationRequired`] apart, to send the
/// user to the login, call [`access_token_for`](OAuth2Client::access_token_for) first.
///
/// `Debug` shows the registration id only.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_http_client::{HttpClient, HttpClientError, InterceptedClient, Request as OutRequest};
/// use oxbloom_security::{OAuth2Client, UserTokenInterceptor};
/// use oxbloom_web::Request;
///
/// // In a handler: call the provider's API on behalf of the logged-in user.
/// async fn profile(
///     oauth2: Arc<OAuth2Client>,
///     api: Arc<dyn HttpClient>,
///     req: &Request,
/// ) -> Result<String, HttpClientError> {
///     let http = InterceptedClient::new(api)
///         .with(Arc::new(UserTokenInterceptor::for_request(oauth2, req, "idp")));
///     http.execute(OutRequest::get("https://api.example.test/me")).await?.text()
/// }
///
/// # use oxbloom_security::OAuth2Settings;
/// # use oxbloom_test::MockHttpClient;
/// # use oxbloom_web::HttpMethod;
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let api = Arc::new(MockHttpClient::new());
/// let oauth2 = Arc::new(OAuth2Client::new(OAuth2Settings::default(), api.clone()));
/// // A request of nobody: no token, and nothing is sent.
/// let anonymous = Request::new(HttpMethod::Get, "/profile");
/// assert_eq!(
///     profile(oauth2, api.clone(), &anonymous).await.unwrap_err(),
///     HttpClientError::InvalidRequest
/// );
/// assert_eq!(api.request_count(), 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct UserTokenInterceptor {
    client: Arc<OAuth2Client>,
    session: Option<Session>,
    authentication: Option<Authentication>,
    registration_id: String,
}

impl UserTokenInterceptor {
    /// An interceptor for the user of `request`, with the tokens of `registration_id`. It takes
    /// the session handle and the authentication of `request` now.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{OAuth2Client, OAuth2Settings, UserTokenInterceptor};
    /// use oxbloom_test::MockHttpClient;
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let oauth2 = Arc::new(OAuth2Client::new(OAuth2Settings::default(), Arc::new(MockHttpClient::new())));
    /// let req = Request::new(HttpMethod::Get, "/profile");
    /// let interceptor = UserTokenInterceptor::for_request(oauth2, &req, "idp");
    /// assert_eq!(format!("{interceptor:?}"), r#"UserTokenInterceptor { registration: "idp" }"#);
    /// ```
    #[must_use]
    pub fn for_request(
        client: Arc<OAuth2Client>,
        request: &oxbloom_web::Request,
        registration_id: &str,
    ) -> UserTokenInterceptor {
        UserTokenInterceptor {
            client,
            session: request.session(),
            authentication: request.authentication().cloned(),
            registration_id: registration_id.to_owned(),
        }
    }
}

#[async_trait]
impl Interceptor for UserTokenInterceptor {
    async fn before(&self, request: &mut Request) -> Result<(), HttpClientError> {
        let token = self
            .client
            .user_access_token(
                self.session.as_ref(),
                self.authentication.as_ref(),
                &self.registration_id,
            )
            .await
            .map_err(|error| token_failure(&self.registration_id, error))?;
        authorize(request, &token)
    }
}

impl fmt::Debug for UserTokenInterceptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserTokenInterceptor")
            .field("registration", &self.registration_id)
            .finish()
    }
}
