//! The OAuth2 client: tokens for the application itself, with the client credentials grant.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use oxbloom_config::Secret;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_http_client::HttpClient;

use super::cache::TokenCache;
use super::{ClientAuthentication, ClientRegistration, MetadataCache, OAuth2Error, OAuth2Settings};
use super::{TokenEndpoint, TokenResponse};

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

/// An OAuth2 client for the application itself: it gets access tokens at the providers of
/// the registrations in [`OAuth2Settings`], with the client credentials grant, and keeps them
/// in a cache.
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
        let registration = self
            .settings
            .registrations
            .iter()
            .find(|registration| registration.id == registration_id)
            .ok_or(TokenError::UnknownRegistration)?;
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

    /// One token request: resolves the provider metadata, then asks the token endpoint.
    async fn fetch_client_credentials(
        &self,
        registration: &ClientRegistration,
    ) -> Result<AccessToken, TokenError> {
        let provider = self
            .settings
            .providers
            .get(&registration.provider)
            .ok_or(TokenError::Invalid)?;
        let metadata = self
            .metadata
            .get_or_resolve(
                &registration.provider,
                provider,
                self.client.as_ref(),
                self.clock.now(),
            )
            .await
            .map_err(token_error)?;
        let tokens = self
            .endpoint
            .client_credentials(registration, &metadata)
            .await
            .map_err(token_error)?;
        Ok(self.access_token(tokens))
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
