//! The OAuth2 client: tokens for the application itself, with the client credentials grant,
//! and the logged-in user's tokens from the session, refreshed with the refresh token grant.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use oxbloom_config::Secret;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_http_client::HttpClient;
use serde_json::Value;

use super::cache::TokenCache;
use super::refresh::{RefreshLocks, Refreshed};
use super::stored::StoredTokens;
use super::{ClientAuthentication, ClientRegistration, MetadataCache, OAuth2Error, OAuth2Settings};
use super::{OAUTH2_PRINCIPAL_KEY, OAUTH2_REGISTRATION_KEY, constant_time_eq};
use super::{ProviderMetadata, TokenEndpoint, TokenResponse};
use crate::form_login::epoch_seconds;
use crate::{Authentication, RequestSecurityExt, RequestSessionExt, Session};

/// How long a token without `expires_in` is used.
const DEFAULT_VALIDITY: Duration = Duration::from_secs(60);
/// How long before its expiry a cached token is no longer used, by default.
const DEFAULT_SKEW: Duration = Duration::from_secs(30);

/// An access token and the time it expires. `Debug` shows the expiry and never the token.
///
/// The struct is `#[non_exhaustive]`: read the fields, and get a token from
/// [`OAuth2Client::client_credentials`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_security::{
///     ClientAuthentication, ClientRegistration, OAuth2Client, OAuth2Settings, ProviderConfig,
/// };
/// use oxbloom_config::Secret;
/// use oxbloom_test::FakeProvider;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let provider = ProviderConfig {
///     issuer_uri: Some("https://idp.example.test".into()),
///     ..ProviderConfig::default()
/// };
/// let registration = ClientRegistration {
///     id: "billing".into(),
///     client_id: "orders-service".into(),
///     client_secret: Some(Secret::new("orders-secret".into())),
///     scopes: Vec::new(),
///     redirect_uri: String::new(),
///     client_authentication: ClientAuthentication::Basic,
///     provider: "idp".into(),
/// };
/// let settings = OAuth2Settings {
///     registrations: vec![registration],
///     providers: [("idp".to_owned(), provider)].into(),
/// };
/// let token = OAuth2Client::new(settings, fake.client()).client_credentials("billing").await?;
/// // Use `value.expose()` for the `Authorization: Bearer` header only.
/// let header = format!("Bearer {}", token.value.expose());
/// assert!(header.len() > "Bearer ".len());
/// assert!(token.expires_at > std::time::SystemTime::now());
/// assert!(!format!("{token:?}").contains(token.value.expose()));
/// # Ok::<(), oxbloom_security::TokenError>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct AccessToken {
    /// The token, to be sent as `Authorization: Bearer <value>`.
    pub value: Secret<String>,
    /// When the provider says the token expires.
    pub expires_at: SystemTime,
}

impl AccessToken {
    /// A token with this value and expiry; the client builds its tokens with it.
    pub(crate) fn new(value: Secret<String>, expires_at: SystemTime) -> AccessToken {
        AccessToken { value, expires_at }
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccessToken")
            .field("value", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Why an access token could not be had. The text is fixed: no variant carries a URL, a
/// token, a code or a response body. [`Provider`](Self::Provider) holds an RFC 6749 error code
/// from an allow-list, or `unknown`.
///
/// The enum is `Copy` today, so a variant that carries data that is not `Copy` (a message, a
/// source error) would be a breaking change; none is planned.
///
/// # Examples
///
/// ```
/// use oxbloom_security::TokenError;
///
/// assert_eq!(
///     TokenError::Provider("invalid_client").to_string(),
///     "the OAuth2 provider answered with the error code invalid_client"
/// );
/// // The enum is non-exhaustive: keep a wildcard arm.
/// let retryable = match TokenError::Http {
///     TokenError::Http | TokenError::Discovery => true,
///     _ => false,
/// };
/// assert!(retryable);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenError {
    /// No registration has the requested id.
    UnknownRegistration,
    /// The token endpoint answered with this error code (`unknown` for a code that is not an
    /// RFC 6749 section 5.2 code).
    Provider(&'static str),
    /// A request to the provider failed before an answer arrived.
    Http,
    /// The provider metadata could not be loaded or is not usable.
    Discovery,
    /// The stored refresh token no longer works and the user has to log in again.
    ReauthenticationRequired,
    /// The registration cannot be used for this grant, or the answer of the provider is not a
    /// usable token response.
    Invalid,
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRegistration => f.write_str("no OAuth2 client registration has this id"),
            Self::Provider(code) => {
                write!(f, "the OAuth2 provider answered with the error code {code}")
            }
            Self::Http => f.write_str("the request to the OAuth2 provider failed"),
            Self::Discovery => f.write_str("the OAuth2 provider metadata could not be loaded"),
            Self::ReauthenticationRequired => {
                f.write_str("the user has to log in again to get a new access token")
            }
            Self::Invalid => {
                f.write_str("the OAuth2 client registration or the token response is not usable")
            }
        }
    }
}

impl std::error::Error for TokenError {}

/// Maps the error of a provider call to the error of the client. A registration that cannot
/// make the request (`Config`) and every unusable answer are `Invalid`.
fn token_error(error: OAuth2Error) -> TokenError {
    match error {
        OAuth2Error::Http => TokenError::Http,
        OAuth2Error::Discovery | OAuth2Error::IssuerMismatch => TokenError::Discovery,
        OAuth2Error::Provider(code) => TokenError::Provider(code),
        _ => TokenError::Invalid,
    }
}

/// Whether `authentication` is the OAuth2 login of `registration_id`: its details hold the
/// boolean `oauth2.principal = true` and `oauth2.registration = registration_id`, which only
/// the OAuth2 login sets (provider attributes with those names are dropped at the login).
fn is_oauth2_login_of(authentication: Option<&Authentication>, registration_id: &str) -> bool {
    authentication.is_some_and(|authentication| {
        let details = authentication.details();
        details.get(OAUTH2_PRINCIPAL_KEY) == Some(&Value::Bool(true))
            && details.get(OAUTH2_REGISTRATION_KEY).and_then(Value::as_str) == Some(registration_id)
    })
}

/// The expiry of a stored access token, when the login or the refresh recorded one.
fn stored_expiry(stored: &StoredTokens) -> Option<SystemTime> {
    stored
        .expires_at
        .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)))
}

/// The stored access token as an `AccessToken`. A token without an expiry is reported with
/// one 60 seconds from `now`.
fn stored_access_token(stored: &StoredTokens, now: SystemTime) -> AccessToken {
    let expires_at = stored_expiry(stored)
        .or_else(|| now.checked_add(DEFAULT_VALIDITY))
        .unwrap_or(now);
    AccessToken::new(Secret::new(stored.access_token.clone()), expires_at)
}

/// An OAuth2 client: it gets access tokens at the providers of the registrations in
/// [`OAuth2Settings`], for the application itself with the client credentials grant (kept in
/// a cache), and for the logged-in user of a request from the tokens that the OAuth2 login
/// stored in the session, refreshed with the refresh token grant (see
/// [`access_token_for`](Self::access_token_for)). The interceptors
/// [`ClientCredentialsInterceptor`](crate::ClientCredentialsInterceptor),
/// [`BearerRetryClient`](crate::BearerRetryClient) and
/// [`UserTokenInterceptor`](crate::UserTokenInterceptor) add those tokens to outgoing
/// requests.
///
/// Cloning is cheap and the clones share one cache, so one client can be built at startup and
/// handed to every component that calls a protected service.
///
/// `Debug` shows the number of registrations and the skew, never a secret or a token.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{
///     ClientAuthentication, ClientRegistration, OAuth2Client, OAuth2Settings, ProviderConfig,
/// };
/// use oxbloom_test::FakeProvider;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let mut settings = OAuth2Settings::default();
/// settings.providers.insert(
///     "idp".into(),
///     ProviderConfig {
///         issuer_uri: Some("https://idp.example.test".into()),
///         ..ProviderConfig::default()
///     },
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
///
/// let client = OAuth2Client::new(settings, fake.client());
/// let token = client.client_credentials("billing").await?;
/// // The second call is answered from the cache.
/// client.client_credentials("billing").await?;
/// assert_eq!(fake.token_requests().len(), 1);
/// assert!(!token.value.expose().is_empty());
/// # Ok::<(), oxbloom_security::TokenError>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct OAuth2Client {
    settings: Arc<OAuth2Settings>,
    endpoint: TokenEndpoint,
    client: Arc<dyn HttpClient>,
    clock: Arc<dyn Clock>,
    skew: Duration,
    metadata: Arc<MetadataCache>,
    cache: Arc<TokenCache>,
    refresh_locks: Arc<RefreshLocks>,
}

impl OAuth2Client {
    /// A client for the registrations in `settings` that sends its requests through `client`
    /// (in an application, the `HttpClient` bean, so the `http.client.*` keys apply). The
    /// clock is the system clock and the skew is 30 seconds; the endpoints come from the
    /// settings or the discovery document of the provider, as for the login.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{OAuth2Client, OAuth2Settings};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let client = OAuth2Client::new(OAuth2Settings::default(), std::sync::Arc::new(MockHttpClient::new()));
    /// assert!(format!("{client:?}").contains("registrations: 0"));
    /// ```
    #[must_use]
    pub fn new(settings: OAuth2Settings, client: Arc<dyn HttpClient>) -> OAuth2Client {
        OAuth2Client {
            settings: Arc::new(settings),
            endpoint: TokenEndpoint::new(client.clone()),
            client,
            clock: Arc::new(SystemClock),
            skew: DEFAULT_SKEW,
            metadata: Arc::new(MetadataCache::new()),
            cache: Arc::new(TokenCache::default()),
            refresh_locks: Arc::new(RefreshLocks::default()),
        }
    }

    /// Takes the time from `clock` instead of the system time: for the expiry of the cached
    /// tokens and the age of the provider metadata. Tests pass a `ManualClock`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::SystemTime;
    /// use oxbloom_security::{OAuth2Client, OAuth2Settings};
    /// use oxbloom_test::{ManualClock, MockHttpClient};
    ///
    /// let clock = ManualClock::at(SystemTime::now());
    /// let client = OAuth2Client::new(OAuth2Settings::default(), Arc::new(MockHttpClient::new()))
    ///     .clock(Arc::new(clock.clone()));
    /// # let _ = client;
    /// ```
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> OAuth2Client {
        self.clock = clock;
        self
    }

    /// Sets how long before its expiry a cached token is replaced (default 30 seconds), so
    /// that a token does not expire while a request that carries it is on its way. A token
    /// whose lifetime is no longer than the skew is never reused.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::Duration;
    /// use oxbloom_security::{OAuth2Client, OAuth2Settings};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let client = OAuth2Client::new(OAuth2Settings::default(), Arc::new(MockHttpClient::new()))
    ///     .skew(Duration::from_secs(10));
    /// assert!(format!("{client:?}").contains("10s"));
    /// ```
    #[must_use]
    pub fn skew(mut self, skew: Duration) -> OAuth2Client {
        self.skew = skew;
        self
    }

    /// Returns an access token for the registration `registration_id`, got with the client
    /// credentials grant (RFC 6749 section 4.4).
    ///
    /// The request carries `grant_type=client_credentials`, the scopes of the registration
    /// separated by spaces (no `scope` field when there are none) and the client
    /// authentication of the registration, and has a 10 second timeout. The token is cached
    /// per registration until 30 seconds (see [`skew`](Self::skew)) before it expires; a
    /// token without `expires_in` counts as valid for 60 seconds, and one that is within the
    /// skew of its expiry when it arrives (for example `expires_in = 0`) is returned but not
    /// kept, so the next call asks again. Callers that arrive while a request is in flight
    /// wait for it, so there is one request at a time per registration; a failed token
    /// request is not cached and has no backoff, so the next call tries again at once. The
    /// provider metadata is different: a failed discovery is remembered for 5 seconds by the
    /// metadata cache of the client, so within 5 seconds of a failed discovery a call can
    /// return [`TokenError::Discovery`] (or `Http`) without any request.
    ///
    /// # Errors
    ///
    /// [`TokenError::UnknownRegistration`] for an id that is not configured;
    /// [`TokenError::Invalid`] for a public registration (`client-authentication = none`,
    /// which has nothing to authenticate with), a registration whose provider or secret is
    /// missing, a non-2xx answer without an `error` code, or an answer that is not a bearer
    /// token response; [`TokenError::Discovery`] and [`TokenError::Http`] when the provider
    /// metadata or the token endpoint cannot be reached; [`TokenError::Provider`] with the
    /// RFC 6749 error code (`unknown` for any other) when the provider refuses.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{OAuth2Client, OAuth2Settings, TokenError};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let client = OAuth2Client::new(OAuth2Settings::default(), Arc::new(MockHttpClient::new()));
    /// assert_eq!(
    ///     client.client_credentials("billing").await.unwrap_err(),
    ///     TokenError::UnknownRegistration
    /// );
    /// # })
    /// # ;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn client_credentials(
        &self,
        registration_id: &str,
    ) -> Result<AccessToken, TokenError> {
        let registration = self.registration(registration_id)?;
        // Refused before anything is cached or requested.
        if registration.client_authentication == ClientAuthentication::None {
            return Err(TokenError::Invalid);
        }
        self.cache
            .get_or_fetch(registration_id, self.clock.as_ref(), self.skew, || {
                self.fetch_client_credentials(registration)
            })
            .await
    }

    /// Drops the cached token of `registration_id` whatever it is, so the next call to
    /// [`client_credentials`](Self::client_credentials) asks the provider for a new one. Use it
    /// for an explicit full reset; to react to a service that refused a token, use
    /// [`invalidate_token`](Self::invalidate_token). A request that is in flight still answers
    /// its callers, but its token is not kept. An unknown id is ignored.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{OAuth2Client, OAuth2Settings};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let client = OAuth2Client::new(OAuth2Settings::default(), Arc::new(MockHttpClient::new()));
    /// client.invalidate("billing");
    /// ```
    pub fn invalidate(&self, registration_id: &str) {
        self.cache.invalidate(registration_id);
    }

    /// Drops the cached token of `registration_id` only if it is `refused`: the same expiry and
    /// the same value (compared in constant time). Call it when a service answered 401 to a
    /// request that carried `refused` (an HTTP interceptor that adds the token to requests is the
    /// natural place).
    ///
    /// When the cached token is a different one, because another caller already replaced it,
    /// or there is none, nothing happens. So a late 401 for an old token cannot discard a
    /// fresh token, and many callers that were refused with the same token cause one new
    /// request between them. An unknown id is ignored.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{
    ///     ClientAuthentication, ClientRegistration, OAuth2Client, OAuth2Settings, ProviderConfig,
    /// };
    /// use oxbloom_test::FakeProvider;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// let provider = ProviderConfig {
    ///     issuer_uri: Some("https://idp.example.test".into()),
    ///     ..ProviderConfig::default()
    /// };
    /// let registration = ClientRegistration {
    ///     id: "billing".into(),
    ///     client_id: "orders-service".into(),
    ///     client_secret: Some(Secret::new("orders-secret".into())),
    ///     scopes: Vec::new(),
    ///     redirect_uri: String::new(),
    ///     client_authentication: ClientAuthentication::Basic,
    ///     provider: "idp".into(),
    /// };
    /// let settings = OAuth2Settings {
    ///     registrations: vec![registration],
    ///     providers: [("idp".to_owned(), provider)].into(),
    /// };
    /// let client = OAuth2Client::new(settings, fake.client());
    /// let refused = client.client_credentials("billing").await?;
    /// // The service answered 401 to `refused`: the next call gets a new token.
    /// client.invalidate_token("billing", &refused);
    /// let fresh = client.client_credentials("billing").await?;
    /// assert_ne!(refused.value.expose(), fresh.value.expose());
    /// // A second, late 401 for the old token does not discard the fresh one.
    /// client.invalidate_token("billing", &refused);
    /// let again = client.client_credentials("billing").await?;
    /// assert_eq!(fresh.value.expose(), again.value.expose());
    /// assert_eq!(fake.token_requests().len(), 2);
    /// # Ok::<(), oxbloom_security::TokenError>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn invalidate_token(&self, registration_id: &str, refused: &AccessToken) {
        self.cache.invalidate_token(registration_id, refused);
    }

    /// The number of callers of `registration_id` waiting for a token request: a test hook,
    /// not a stable API.
    #[doc(hidden)]
    #[must_use]
    pub fn __waiting_callers(&self, registration_id: &str) -> usize {
        self.cache.waiting(registration_id)
    }

    /// Returns the access token of the user of `request` for the registration
    /// `registration_id`: the token that the OAuth2 login stored in the session, refreshed with
    /// the refresh token grant when it is due.
    ///
    /// The call is request-based because sessions are: it reads the session and the
    /// authentication that the session filter and the security chain put into `request`.
    ///
    /// - The current authentication must be the OAuth2 login of this registration: its
    ///   `details` hold `oauth2.principal = true` and `oauth2.registration = registration_id`.
    ///   Otherwise, for example after a later password login in the same session or for a
    ///   user who logged in with another registration, the call refuses without looking at the
    ///   session, so tokens never reach a principal they were not issued to.
    /// - The stored access token is returned while its expiry minus the skew (30 seconds by
    ///   default, see [`skew`](Self::skew)) is in the future. A token stored without an expiry,
    ///   because the provider sent no `expires_in` at the login, is used as it is and reported
    ///   with an expiry 60 seconds from now.
    /// - A due token is refreshed: `grant_type=refresh_token` with the stored refresh token and
    ///   no `scope` (the provider grants the scope of the login), with the client
    ///   authentication of the registration (a public client sends its `client_id` only) and a
    ///   10 second timeout. The new tokens are written to the session: a new refresh token
    ///   replaces the old one, an answer without one keeps the old one, and the id token and
    ///   the scope of the login are kept. A new token without `expires_in` counts as valid for
    ///   60 seconds; one that is already due when it arrives (`expires_in = 0`) is returned once,
    ///   and the next call refreshes again, so one call makes at most one request.
    /// - Refreshes of one session and registration are serialized. A call that waited while
    ///   another call of the same session refreshed does not refresh again: it takes the new
    ///   tokens, also when it runs in another request with its own copy of the session. So a
    ///   provider that rotates refresh tokens never sees one used twice.
    ///
    /// The session filter writes the session back at the end of the request, so the refreshed
    /// tokens are stored when the request that refreshed them ends.
    ///
    /// # Errors
    ///
    /// [`TokenError::ReauthenticationRequired`] when the current authentication is not the
    /// OAuth2 login of the registration, when the request has no session, when the session
    /// holds no tokens of the registration, when a due token has no refresh token (the
    /// useless token is removed), or when the provider answers the refresh with
    /// `invalid_grant` (the tokens are removed from the session): the user has to log in
    /// again. [`TokenError::UnknownRegistration`] for an id that is not configured. Any other
    /// failure of the refresh leaves the stored tokens as they are and is reported as for
    /// [`client_credentials`](Self::client_credentials): [`TokenError::Provider`],
    /// [`TokenError::Http`], [`TokenError::Discovery`] or [`TokenError::Invalid`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{
    ///     Authentication, ClientAuthentication, ClientRegistration, OAuth2Client, OAuth2Settings,
    ///     SecurityContext, TokenError,
    /// };
    /// use oxbloom_test::MockHttpClient;
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// // In a handler: the `Authorization` value for a call on behalf of the logged-in user.
    /// async fn authorization(client: &OAuth2Client, req: &Request) -> Result<String, TokenError> {
    ///     let token = client.access_token_for(req, "idp").await?;
    ///     Ok(format!("Bearer {}", token.value.expose()))
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let mut settings = OAuth2Settings::default();
    /// settings.registrations.push(ClientRegistration {
    ///     id: "idp".into(),
    ///     client_id: "app".into(),
    ///     client_secret: Some(Secret::new("app-secret".into())),
    ///     scopes: vec!["openid".into()],
    ///     redirect_uri: "https://app.example.test/login/oauth2/code/idp".into(),
    ///     client_authentication: ClientAuthentication::Basic,
    ///     provider: "idp".into(),
    /// });
    /// let client = OAuth2Client::new(settings, Arc::new(MockHttpClient::new()));
    ///
    /// // A request of a user who logged in with `idp`: in an application the login stored the
    /// // tokens in the session, and the session filter and the security chain put the session
    /// // and the authentication into the request. Here they are built by hand.
    /// let mut req = Request::new(HttpMethod::Get, "/orders");
    /// # let session = oxbloom_security::__test_support::session();
    /// # session.set_reserved("oxbloom.security.oauth2.tokens.idp", &serde_json::json!({
    /// #     "v": 1, "access_token": "the-access-token", "token_type": "Bearer",
    /// #     "expires_at": 4_102_444_800u64, "refresh_token": "the-refresh-token",
    /// #     "id_token": null, "scope": "openid",
    /// # }))?;
    /// # req.extensions_mut().insert(session);
    /// let mut details = serde_json::Map::new();
    /// details.insert("oauth2.registration".into(), "idp".into());
    /// details.insert("oauth2.principal".into(), true.into());
    /// req.extensions_mut().insert(SecurityContext::authenticated(
    ///     Authentication::new("alice", []).with_details(details),
    /// ));
    /// assert_eq!(authorization(&client, &req).await?, "Bearer the-access-token");
    ///
    /// // A request without that login gets no token.
    /// let anonymous = Request::new(HttpMethod::Get, "/orders");
    /// assert_eq!(
    ///     authorization(&client, &anonymous).await.unwrap_err(),
    ///     TokenError::ReauthenticationRequired
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn access_token_for(
        &self,
        request: &oxbloom_web::Request,
        registration_id: &str,
    ) -> Result<AccessToken, TokenError> {
        let session = request.session();
        self.user_access_token(session.as_ref(), request.authentication(), registration_id)
            .await
    }

    /// The number of calls waiting for the refresh of a user's token: a test hook, not a
    /// stable API.
    #[doc(hidden)]
    #[must_use]
    pub fn __refresh_waiters(&self) -> usize {
        self.refresh_locks.waiting()
    }

    /// The number of refresh lock entries, one per session and registration with a call in
    /// flight: a test hook, not a stable API.
    #[doc(hidden)]
    #[must_use]
    pub fn __refresh_locks(&self) -> usize {
        self.refresh_locks.len()
    }

    /// [`access_token_for`](Self::access_token_for) with the session and the authentication
    /// taken out of the request already; the user token interceptor calls it.
    pub(crate) async fn user_access_token(
        &self,
        session: Option<&Session>,
        authentication: Option<&Authentication>,
        registration_id: &str,
    ) -> Result<AccessToken, TokenError> {
        let registration = self.registration(registration_id)?;
        if !is_oauth2_login_of(authentication, registration_id) {
            return Err(TokenError::ReauthenticationRequired);
        }
        let session = session.ok_or(TokenError::ReauthenticationRequired)?;
        let stored = StoredTokens::load(session, registration_id)
            .ok_or(TokenError::ReauthenticationRequired)?;
        if let Some(token) = self.usable(&stored) {
            return Ok(token);
        }

        let entry = self.refresh_locks.entry(session.id(), registration_id);
        let mut last = entry.lock().await;
        // Another call with this session handle may have refreshed while this one waited.
        let stored = StoredTokens::load(session, registration_id)
            .ok_or(TokenError::ReauthenticationRequired)?;
        if let Some(token) = self.usable(&stored) {
            return Ok(token);
        }
        let Some(refresh_token) = stored.refresh_token.clone() else {
            StoredTokens::remove(session, registration_id);
            return Err(TokenError::ReauthenticationRequired);
        };
        // Or a call of another request of this session, with its own copy of the session: the
        // refresh token this call holds is the one that call spent.
        if let Some(done) = &*last
            && constant_time_eq(&done.spent, &refresh_token)
        {
            done.tokens
                .store(session, registration_id)
                .map_err(|_| TokenError::Invalid)?;
            return Ok(stored_access_token(&done.tokens, self.clock.now()));
        }

        let response = match self.fetch_refresh(registration, &refresh_token).await {
            Ok(response) => response,
            Err(TokenError::Provider("invalid_grant")) => {
                StoredTokens::remove(session, registration_id);
                tracing::debug!(
                    target: "oxbloom::security",
                    registration = %registration_id,
                    "the OAuth2 provider refused the refresh token; the user has to log in again"
                );
                return Err(TokenError::ReauthenticationRequired);
            }
            Err(error) => return Err(error),
        };
        let now = self.clock.now();
        let expires_at = response
            .expires_at(now)
            .or_else(|| now.checked_add(DEFAULT_VALIDITY))
            .unwrap_or(now);
        let renewed = StoredTokens {
            access_token: response.access_token.expose().clone(),
            token_type: response.token_type.clone(),
            expires_at: Some(epoch_seconds(expires_at)),
            refresh_token: response
                .refresh_token
                .as_ref()
                .map(|token| token.expose().clone())
                .or(stored.refresh_token),
            ..stored
        };
        renewed
            .store(session, registration_id)
            .map_err(|_| TokenError::Invalid)?;
        tracing::debug!(
            target: "oxbloom::security",
            registration = %registration_id,
            "the OAuth2 access token of the user was refreshed"
        );
        let token = AccessToken::new(response.access_token, expires_at);
        *last = Some(Refreshed {
            spent: refresh_token,
            tokens: renewed,
        });
        Ok(token)
    }

    /// The stored access token, when it can still be used now.
    fn usable(&self, stored: &StoredTokens) -> Option<AccessToken> {
        let now = self.clock.now();
        let fresh = match stored.expires_at {
            None => true,
            Some(_) => stored_expiry(stored)
                .and_then(|expires_at| expires_at.checked_sub(self.skew))
                .is_some_and(|limit| now < limit),
        };
        fresh.then(|| stored_access_token(stored, now))
    }

    /// One refresh request: resolves the provider metadata, then asks the token endpoint.
    async fn fetch_refresh(
        &self,
        registration: &ClientRegistration,
        refresh_token: &str,
    ) -> Result<TokenResponse, TokenError> {
        let metadata = self.metadata_of(registration).await?;
        self.endpoint
            .refresh(registration, &metadata, refresh_token)
            .await
            .map_err(token_error)
    }

    /// One token request: resolves the provider metadata, then asks the token endpoint.
    async fn fetch_client_credentials(
        &self,
        registration: &ClientRegistration,
    ) -> Result<AccessToken, TokenError> {
        let metadata = self.metadata_of(registration).await?;
        let tokens = self
            .endpoint
            .client_credentials(registration, &metadata)
            .await
            .map_err(token_error)?;
        Ok(self.access_token(tokens))
    }

    /// The registration with this id.
    fn registration(&self, registration_id: &str) -> Result<&ClientRegistration, TokenError> {
        self.settings
            .registrations
            .iter()
            .find(|registration| registration.id == registration_id)
            .ok_or(TokenError::UnknownRegistration)
    }

    /// The provider metadata of `registration`, from the metadata cache of the client.
    async fn metadata_of(
        &self,
        registration: &ClientRegistration,
    ) -> Result<Arc<ProviderMetadata>, TokenError> {
        let provider = self
            .settings
            .providers
            .get(&registration.provider)
            .ok_or(TokenError::Invalid)?;
        self.metadata
            .get_or_resolve(
                &registration.provider,
                provider,
                self.client.as_ref(),
                self.clock.now(),
            )
            .await
            .map_err(token_error)
    }

    /// The access token of a token response, with the expiry counted from now.
    fn access_token(&self, tokens: TokenResponse) -> AccessToken {
        let now = self.clock.now();
        let expires_at = tokens
            .expires_at(now)
            .or_else(|| now.checked_add(DEFAULT_VALIDITY))
            .unwrap_or(now);
        AccessToken::new(tokens.access_token, expires_at)
    }
}

impl fmt::Debug for OAuth2Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth2Client")
            .field("registrations", &self.settings.registrations.len())
            .field("skew", &self.skew)
            .finish_non_exhaustive()
    }
}
