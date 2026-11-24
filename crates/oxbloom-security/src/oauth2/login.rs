//! The OAuth2 login endpoints of the security chain: `GET /oauth2/authorization/{id}` starts a
//! login, `GET /login/oauth2/code/{id}` is the callback.
//!
//! Every failure redirects to the failure URL with no detail in the response, and logs the
//! [`OAuth2Error`] at `debug` on the target `oxbloom::security`. No token, code, state, nonce,
//! secret, URL or response body reaches a log line, an error or `Debug` output.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use http::{HeaderValue, StatusCode, header};
use oxbloom_core::{Clock, HttpMethod};
use oxbloom_http_client::HttpClient;
use oxbloom_web::{Request, Response};
use serde_json::{Map, Value};

use super::authorization::authorization_url;
use super::error::allowed_authorization_code;
use super::pending::Pending;
use super::pkce::{Verifier, random_token};
use super::settings::{
    is_acceptable_endpoint, is_scope_token, validate_provider, validate_registration,
};
use super::stored::StoredTokens;
use super::{
    ClientRegistration, IdTokenValidator, MetadataCache, OAuth2Error, ProviderConfig,
    ProviderMetadata, TokenEndpoint, TokenResponse, UserInfo, authentication_from,
    constant_time_eq,
};
use crate::form_login::{
    FormLoginHandlers, OAUTH2_TOKENS_PREFIX, SAVED_REQUEST_KEY, take_saved_request,
    urlencoded_pairs,
};
use crate::{RequestSessionExt, SecurityBuildError, Session, is_safe_redirect_target};

/// The path prefix of the start of a login.
const START_PREFIX: &str = "/oauth2/authorization/";
/// The path prefix of the callback.
const CALLBACK_PREFIX: &str = "/login/oauth2/code/";
/// The longest `state` accepted from the query string.
const MAX_STATE_LEN: usize = 64;
/// How long a started login may take by default.
const DEFAULT_PENDING_MAX_AGE: Duration = Duration::from_secs(600);

/// The OAuth2 and OpenID Connect login settings inside
/// [`HttpSecurity::oauth2_login`](crate::HttpSecurity::oauth2_login).
///
/// A login needs an [`HttpClient`] for the provider requests, at least one
/// [`ClientRegistration`] with its [`ProviderConfig`], and the base URL of the application,
/// from which the redirect URI is built (it is never guessed from the `Host` header).
/// `Debug` shows the registration ids and the provider names, never a secret.
///
/// Notes on the result of a login:
///
/// - The callback path is fixed: `/login/oauth2/code/{id}`. The redirect URI of every
///   registration must be exactly `{base-url}/login/oauth2/code/{id}`, or the build fails.
/// - The `SCOPE_*` authorities come from the scopes the provider granted and are not tied to a
///   registration: a provider of low trust that grants a scope named `admin` produces
///   `SCOPE_admin`. Rules that must trust one provider should also check the
///   `oauth2.registration` detail.
/// - The session limit and the per-user session index count an OAuth2 user under the key
///   `oauth2:<registration>:<principal>`, never under the bare principal, so an OAuth2 user
///   and a password user with the same name never evict each other's sessions. An application
///   that calls [`SessionRegistry::invalidate_all`](crate::SessionRegistry::invalidate_all) or
///   `sessions_of` for an OAuth2 user passes that key, for example `"oauth2:github:alice"`.
///   Usernames that start with `oauth2:` are therefore reserved: a password user literally
///   named `oauth2:github:alice` would share that key with the OAuth2 user, so a user store
///   should refuse such names.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{ClientAuthentication, ClientRegistration, OAuth2Login, ProviderConfig};
/// use oxbloom_test::FakeProvider;
///
/// let fake = FakeProvider::new("https://idp.example.test");
/// let login = OAuth2Login::new(fake.client())
///     .registration(ClientRegistration {
///         id: "idp".into(),
///         client_id: "app".into(),
///         client_secret: Some(Secret::new("app-secret".into())),
///         scopes: vec!["email".into()],
///         redirect_uri: "{base-url}/login/oauth2/code/{id}".into(),
///         client_authentication: ClientAuthentication::Basic,
///         provider: "idp".into(),
///     })
///     .provider("idp", ProviderConfig {
///         issuer_uri: Some("https://idp.example.test".into()),
///         ..ProviderConfig::default()
///     })
///     .base_url("https://app.example.test")
///     .failure_url("/login?error")
///     .pending_max_age(Duration::from_secs(300));
/// assert!(!format!("{login:?}").contains("app-secret"));
/// ```
pub struct OAuth2Login {
    client: Option<Arc<dyn HttpClient>>,
    registrations: Vec<ClientRegistration>,
    providers: BTreeMap<String, ProviderConfig>,
    base_url: Option<String>,
    failure_url: Option<String>,
    clock: Option<Arc<dyn Clock>>,
    pending_max_age: Duration,
}

impl fmt::Debug for OAuth2Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ids: Vec<&str> = self.registrations.iter().map(|r| r.id.as_str()).collect();
        f.debug_struct("OAuth2Login")
            .field("client", &self.client.is_some())
            .field("registrations", &ids)
            .field("providers", &self.providers.keys().collect::<Vec<_>>())
            .field("base_url", &self.base_url.is_some())
            .field("failure_url", &self.failure_url)
            .field("clock", &self.clock.is_some())
            .field("pending_max_age", &self.pending_max_age)
            .finish()
    }
}

impl OAuth2Login {
    /// Settings with no client yet: what [`HttpSecurity::oauth2_login`](crate::HttpSecurity::oauth2_login)
    /// passes on its first call.
    pub(crate) fn empty() -> Self {
        Self {
            client: None,
            registrations: Vec::new(),
            providers: BTreeMap::new(),
            base_url: None,
            failure_url: None,
            clock: None,
            pending_max_age: DEFAULT_PENDING_MAX_AGE,
        }
    }

    /// Settings that send the provider requests (discovery, the token exchange, the key set
    /// and the user info) through `client`, with no registration yet, the failure URL of form
    /// login and a pending login that is valid for 10 minutes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::OAuth2Login;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let login = OAuth2Login::new(std::sync::Arc::new(MockHttpClient::new()));
    /// assert!(format!("{login:?}").contains("client: true"));
    /// ```
    pub fn new(client: Arc<dyn HttpClient>) -> Self {
        Self::empty().client(client)
    }

    /// Replaces the client of the provider requests, for example when a later
    /// `oauth2_login` call tunes settings that an earlier one created.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::HttpSecurity;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let security = HttpSecurity::new().oauth2_login(|login| login.client(Arc::new(MockHttpClient::new())));
    /// # let _ = security;
    /// ```
    #[must_use]
    pub fn client(mut self, client: Arc<dyn HttpClient>) -> Self {
        self.client = Some(client);
        self
    }

    /// Adds a registration; one with the same id replaces it. Its `provider` must name a
    /// provider added with [`provider`](Self::provider), and its id may hold only `a` to `z`,
    /// `0` to `9` and `-`, because it appears in the login and callback paths.
    ///
    /// The callback path is always `/login/oauth2/code/{id}`, so a custom `redirect_uri`
    /// must point at it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{ClientAuthentication, ClientRegistration, OAuth2Login};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let login = OAuth2Login::new(std::sync::Arc::new(MockHttpClient::new())).registration(
    ///     ClientRegistration {
    ///         id: "spa".into(),
    ///         client_id: "public-client".into(),
    ///         client_secret: None,
    ///         scopes: vec![],
    ///         redirect_uri: "{base-url}/login/oauth2/code/{id}".into(),
    ///         client_authentication: ClientAuthentication::None,
    ///         provider: "idp".into(),
    ///     },
    /// );
    /// assert!(format!("{login:?}").contains("\"spa\""));
    /// ```
    #[must_use]
    pub fn registration(mut self, registration: ClientRegistration) -> Self {
        self.registrations.retain(|r| r.id != registration.id);
        self.registrations.push(registration);
        self
    }

    /// Adds the provider `name`; one with the same name replaces it. A provider with
    /// `issuer_uri` or `jwk_set_uri` is an OpenID Connect provider: the login asks for the
    /// scope `openid`, sends a `nonce` and validates the id token. Any other provider is a
    /// plain OAuth2 provider, whose user comes from `user_info_uri`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{OAuth2Login, ProviderConfig};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let login = OAuth2Login::new(std::sync::Arc::new(MockHttpClient::new())).provider(
    ///     "github",
    ///     ProviderConfig {
    ///         authorization_uri: Some("https://github.example.test/login/oauth/authorize".into()),
    ///         token_uri: Some("https://github.example.test/login/oauth/access_token".into()),
    ///         user_info_uri: Some("https://api.github.example.test/user".into()),
    ///         user_name_attribute: Some("login".into()),
    ///         ..ProviderConfig::default()
    ///     },
    /// );
    /// assert!(format!("{login:?}").contains("\"github\""));
    /// ```
    #[must_use]
    pub fn provider(mut self, name: &str, provider: ProviderConfig) -> Self {
        self.providers.insert(name.to_owned(), provider);
        self
    }

    /// The external base URL of the application, such as `https://app.example`: the
    /// `{base-url}` of the redirect URI. It must be an absolute `https` URL (`http` only for a
    /// loopback host) without a path beyond an optional `/`, a query or a fragment, because the
    /// callback path is fixed; a trailing slash is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::OAuth2Login;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let login = OAuth2Login::new(std::sync::Arc::new(MockHttpClient::new()))
    ///     .base_url("https://app.example.test");
    /// assert!(format!("{login:?}").contains("base_url: true"));
    /// ```
    #[must_use]
    pub fn base_url(mut self, base_url: &str) -> Self {
        self.base_url = Some(base_url.to_owned());
        self
    }

    /// Where every failed login is redirected. Without it, the failure URL of form login is
    /// used (`/login?error` by default). It must be a safe local redirect target (see
    /// [`is_safe_redirect_target`]) and must not be one of the two OAuth2 endpoints, which
    /// would loop.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::OAuth2Login;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let login = OAuth2Login::new(std::sync::Arc::new(MockHttpClient::new()))
    ///     .failure_url("/signin?failed");
    /// assert!(format!("{login:?}").contains("/signin?failed"));
    /// ```
    #[must_use]
    pub fn failure_url(mut self, failure_url: &str) -> Self {
        self.failure_url = Some(failure_url.to_owned());
        self
    }

    /// The clock for the age of a pending login, the id token times and the token expiry.
    /// Without it the clock of [`HttpSecurity::clock`](crate::HttpSecurity::clock) is used.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::OAuth2Login;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let login = OAuth2Login::new(Arc::new(MockHttpClient::new())).clock(Arc::new(SystemClock));
    /// assert!(format!("{login:?}").contains("clock: true"));
    /// ```
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// How long a started login stays valid, 10 minutes by default. A callback after that is
    /// a failure. Zero fails the build.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::OAuth2Login;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let login = OAuth2Login::new(std::sync::Arc::new(MockHttpClient::new()))
    ///     .pending_max_age(Duration::from_secs(120));
    /// assert!(format!("{login:?}").contains("120s"));
    /// ```
    #[must_use]
    pub fn pending_max_age(mut self, max_age: Duration) -> Self {
        self.pending_max_age = max_age;
        self
    }

    /// Checks the settings and builds the handlers. `default_clock` is the clock of the
    /// builder and `form_failure_url` the failure URL of form login. Logs one warning per
    /// registration whose provider has a key set and no issuer.
    pub(crate) fn into_handlers(
        self,
        default_clock: Arc<dyn Clock>,
        form_failure_url: &str,
    ) -> Result<OAuth2LoginHandlers, SecurityBuildError> {
        let client = self
            .client
            .ok_or(SecurityBuildError::OAuth2LoginWithoutClient)?;
        // The same shape checks as the configuration keys, so a registration built in code
        // cannot pass a shape that fails every login after the code exchange.
        let usable = |r: &ClientRegistration| {
            validate_registration(r).is_ok()
                && self
                    .providers
                    .get(&r.provider)
                    .is_some_and(|p| validate_provider(&r.provider, p).is_ok())
        };
        if self.registrations.is_empty() || !self.registrations.iter().all(usable) {
            return Err(SecurityBuildError::OAuth2LoginWithoutRegistration);
        }
        let base_url = self
            .base_url
            .filter(|url| is_acceptable_endpoint(url, false) && has_no_path(url))
            .ok_or(SecurityBuildError::OAuth2LoginWithoutBaseUrl)?;
        let base_url = base_url.trim_end_matches('/').to_owned();
        // The callback path is fixed, so the redirect URI must lead to it exactly. With unique
        // ids this also keeps two registrations from sharing one redirect URI.
        let leads_to_callback = |r: &ClientRegistration| {
            r.resolved_redirect_uri(&base_url) == format!("{base_url}{CALLBACK_PREFIX}{}", r.id)
        };
        if !self.registrations.iter().all(leads_to_callback) || self.pending_max_age.is_zero() {
            return Err(SecurityBuildError::InvalidOAuth2Login);
        }
        let failure_url = self
            .failure_url
            .unwrap_or_else(|| form_failure_url.to_owned());
        let on_an_endpoint = |url: &str| {
            let path = crate::form_login::path_of(url);
            path.starts_with(START_PREFIX) || path.starts_with(CALLBACK_PREFIX)
        };
        if !is_safe_redirect_target(&failure_url)
            || HeaderValue::from_str(&failure_url).is_err()
            || on_an_endpoint(&failure_url)
        {
            return Err(SecurityBuildError::InvalidFormLoginUrl);
        }
        for registration in &self.registrations {
            let provider = &self.providers[&registration.provider];
            if provider.jwk_set_uri.is_some() && provider.issuer_uri.is_none() {
                tracing::warn!(
                    target: "oxbloom::security",
                    registration = %registration.id,
                    "the OAuth2 provider of this registration has a key set and no issuer, so \
                     the issuer of its id tokens is not checked against a configured value"
                );
            }
        }
        Ok(OAuth2LoginHandlers {
            registrations: self
                .registrations
                .into_iter()
                .map(|r| (r.id.clone(), r))
                .collect(),
            providers: self.providers,
            base_url,
            failure_url,
            clock: self.clock.unwrap_or(default_clock),
            max_age: self.pending_max_age,
            metadata: MetadataCache::new(),
            tokens: TokenEndpoint::new(client.clone()),
            id_tokens: IdTokenValidator::new(client.clone()),
            client,
        })
    }
}

/// Whether `url` has nothing after its authority but an optional `/`: the callback path is
/// fixed, so a base URL with a path prefix could never be reached (ruling R17).
fn has_no_path(url: &str) -> bool {
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let tail = rest.find(['/', '?', '#']).map_or("", |i| &rest[i..]);
    tail.is_empty() || tail == "/"
}

/// The OAuth2 login part of a chain.
pub(crate) struct OAuth2LoginHandlers {
    client: Arc<dyn HttpClient>,
    registrations: HashMap<String, ClientRegistration>,
    providers: BTreeMap<String, ProviderConfig>,
    base_url: String,
    failure_url: String,
    clock: Arc<dyn Clock>,
    max_age: Duration,
    metadata: MetadataCache,
    tokens: TokenEndpoint,
    id_tokens: IdTokenValidator,
}

/// The future of an answer of the OAuth2 endpoints, boxed so the chain future stays small.
pub(crate) type Answer<'a> = Pin<Box<dyn Future<Output = Response> + Send + 'a>>;

/// Logs that OAuth2 login found no session, which means the application has no session filter.
fn no_session_error() {
    tracing::error!(
        target: "oxbloom::security",
        "OAuth2 login has no session: add a SessionFilter to the application"
    );
}

/// A 302 to `target`, with `Cache-Control: no-store`.
fn found(target: HeaderValue) -> Response {
    let mut res = Response::new(Default::default());
    *res.status_mut() = StatusCode::FOUND;
    res.headers_mut().insert(header::LOCATION, target);
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn method_not_allowed() -> Response {
    let mut res = Response::new(Default::default());
    *res.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    res.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET"));
    res
}

/// Whether the provider is an OpenID Connect provider: it names an issuer or a key set.
fn is_oidc(provider: &ProviderConfig) -> bool {
    provider.issuer_uri.is_some() || provider.jwk_set_uri.is_some()
}

/// The scopes the login asks for: those of the registration, with `openid` first for an OpenID
/// Connect provider. Tokens with characters outside RFC 6749 section 3.3 are left out.
fn requested_scopes(registration: &ClientRegistration, oidc: bool) -> Vec<String> {
    let mut scopes = Vec::new();
    if oidc {
        scopes.push("openid".to_owned());
    }
    for scope in &registration.scopes {
        if is_scope_token(scope) && !scopes.contains(scope) {
            scopes.push(scope.clone());
        }
    }
    scopes
}

/// The scopes that become `SCOPE_*` authorities: the `scope` of the token response split on
/// whitespace, without empty entries, duplicates and tokens with characters outside RFC 6749
/// section 3.3; the requested scopes when the response has no `scope`.
pub(crate) fn granted_scopes(granted: Option<&str>, requested: &[String]) -> Vec<String> {
    let mut scopes: Vec<String> = Vec::new();
    let candidates: Vec<&str> = match granted {
        Some(text) => text.split_whitespace().collect(),
        None => requested.iter().map(String::as_str).collect(),
    };
    for scope in candidates {
        if is_scope_token(scope) && !scopes.iter().any(|s| s == scope) {
            scopes.push(scope.to_owned());
        }
    }
    scopes
}

/// Whether a `state` from the query string may be used in a session key: 1 to 64 characters of
/// `A-Z`, `a-z`, `0-9`, `_` and `-`.
pub(crate) fn is_valid_state(state: &str) -> bool {
    !state.is_empty()
        && state.len() <= MAX_STATE_LEN
        && state
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The parameters of a callback that the login reads.
#[derive(Default)]
struct CallbackParams {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
    iss: Option<String>,
}

impl CallbackParams {
    /// Reads the query. `None` when it is not valid form encoding or when one of the four
    /// parameters appears more than once.
    fn parse(query: Option<&str>) -> Option<Self> {
        let mut params = Self::default();
        for (name, value) in urlencoded_pairs(query.unwrap_or_default().as_bytes())? {
            let slot = match name.as_str() {
                "state" => &mut params.state,
                "code" => &mut params.code,
                "error" => &mut params.error,
                "iss" => &mut params.iss,
                _ => continue,
            };
            if slot.replace(value).is_some() {
                return None;
            }
        }
        Some(params)
    }
}

/// The two endpoints, with the registration id from the path.
enum Endpoint<'a> {
    Start(&'a str),
    Callback(&'a str),
}

fn endpoint(path: &str) -> Option<Endpoint<'_>> {
    if let Some(id) = path.strip_prefix(START_PREFIX) {
        Some(Endpoint::Start(id))
    } else {
        path.strip_prefix(CALLBACK_PREFIX).map(Endpoint::Callback)
    }
}

impl OAuth2LoginHandlers {
    /// Answers the two endpoints, or `None` for any other path. Any method other than `GET` on
    /// them is a 405.
    pub(crate) fn handle<'a>(
        &'a self,
        req: &'a Request,
        form: &'a FormLoginHandlers,
    ) -> Option<Answer<'a>> {
        let endpoint = endpoint(&req.path)?;
        if req.method != HttpMethod::Get {
            return Some(Box::pin(async { method_not_allowed() }));
        }
        Some(match endpoint {
            Endpoint::Start(id) => Box::pin(self.start(req, id)),
            Endpoint::Callback(id) => Box::pin(self.callback(req, form, id)),
        })
    }

    fn failure_redirect(&self) -> Response {
        found(
            HeaderValue::from_str(&self.failure_url)
                .unwrap_or_else(|_| HeaderValue::from_static("/")),
        )
    }

    /// Logs `error` at debug and redirects to the failure URL.
    fn fail(&self, error: OAuth2Error) -> Response {
        tracing::debug!(
            target: "oxbloom::security",
            error = %error,
            "OAuth2 login failed"
        );
        self.failure_redirect()
    }

    async fn metadata_for(
        &self,
        registration: &ClientRegistration,
    ) -> Result<(&ProviderConfig, Arc<ProviderMetadata>), OAuth2Error> {
        let provider = self
            .providers
            .get(&registration.provider)
            .ok_or(OAuth2Error::Config)?;
        let metadata = self
            .metadata
            .get_or_resolve(
                &registration.provider,
                provider,
                self.client.as_ref(),
                self.clock.now(),
            )
            .await?;
        Ok((provider, metadata))
    }

    async fn start(&self, req: &Request, id: &str) -> Response {
        // The id is checked before anything else, so an unknown id makes no request and no
        // cache slot.
        let Some(registration) = self.registrations.get(id) else {
            return self.fail(OAuth2Error::Config);
        };
        match self.try_start(req, registration).await {
            Ok(res) => res,
            Err(error) => self.fail(error),
        }
    }

    async fn try_start(
        &self,
        req: &Request,
        registration: &ClientRegistration,
    ) -> Result<Response, OAuth2Error> {
        let Some(session) = req.session() else {
            no_session_error();
            return Err(OAuth2Error::Pending);
        };
        let (provider, metadata) = self.metadata_for(registration).await?;
        let oidc = is_oidc(provider);
        let verifier = Verifier::generate();
        let state = random_token(32);
        let nonce = oidc.then(|| random_token(32));
        let saved_request = session
            .get::<String>(SAVED_REQUEST_KEY)
            .ok()
            .flatten()
            .filter(|target| is_safe_redirect_target(target));
        let redirect_uri = registration.resolved_redirect_uri(&self.base_url);
        let url = authorization_url(
            &metadata.authorization_endpoint,
            registration,
            &redirect_uri,
            &requested_scopes(registration, oidc),
            &state,
            nonce.as_deref(),
            &verifier.challenge_s256(),
        );
        let location = HeaderValue::from_str(&url).map_err(|_| OAuth2Error::Config)?;
        Pending {
            registration: registration.id.clone(),
            state,
            nonce,
            verifier: verifier.as_str().to_owned(),
            saved_request,
            created_at: self.clock.now(),
        }
        .store(&session)
        .map_err(|_| OAuth2Error::Pending)?;
        Ok(found(location))
    }

    async fn callback(&self, req: &Request, form: &FormLoginHandlers, id: &str) -> Response {
        let Some(registration) = self.registrations.get(id) else {
            return self.fail(OAuth2Error::Config);
        };
        match self.try_callback(req, form, registration).await {
            Ok(res) => res,
            Err(error) => self.fail(error),
        }
    }

    async fn try_callback(
        &self,
        req: &Request,
        form: &FormLoginHandlers,
        registration: &ClientRegistration,
    ) -> Result<Response, OAuth2Error> {
        // The state is checked before it becomes part of a session key, and before the session
        // is touched.
        let params = CallbackParams::parse(req.query.as_deref()).ok_or(OAuth2Error::Pending)?;
        let state = params
            .state
            .as_deref()
            .filter(|state| is_valid_state(state))
            .ok_or(OAuth2Error::Pending)?;
        let Some(session) = req.session() else {
            no_session_error();
            return Err(OAuth2Error::Pending);
        };
        // The pending login is removed before anything else, so a state works once.
        let pending = Pending::take(&session, state, self.clock.now(), self.max_age)
            .ok_or(OAuth2Error::Pending)?;
        if !constant_time_eq(&pending.state, state) || pending.registration != registration.id {
            return Err(OAuth2Error::Pending);
        }
        if let Some(code) = &params.error {
            return Err(OAuth2Error::Provider(allowed_authorization_code(code)));
        }
        let provider = self
            .providers
            .get(&registration.provider)
            .ok_or(OAuth2Error::Config)?;
        // RFC 9207: an `iss` must be the issuer of the provider; one without an issuer
        // matches none.
        if let Some(iss) = &params.iss
            && provider.issuer_uri.as_deref() != Some(iss.as_str())
        {
            return Err(OAuth2Error::IssuerMismatch);
        }
        let code = params
            .code
            .as_deref()
            .filter(|code| !code.is_empty())
            .ok_or(OAuth2Error::Pending)?;
        let (provider, metadata) = self.metadata_for(registration).await?;
        let redirect_uri = registration.resolved_redirect_uri(&self.base_url);
        let tokens = self
            .tokens
            .exchange_code(
                registration,
                &metadata,
                code,
                &redirect_uri,
                &pending.verifier,
            )
            .await?;
        let oidc = is_oidc(provider);
        let attributes = self
            .attributes(registration, provider, &metadata, &tokens, &pending, oidc)
            .await?;
        let scopes = granted_scopes(
            tokens.scope.as_deref(),
            &requested_scopes(registration, oidc),
        );
        let authentication =
            authentication_from(registration, provider, oidc, attributes, &scopes)?;
        Ok(self
            .finish(
                req,
                form,
                &session,
                registration,
                &tokens,
                &scopes,
                pending,
                authentication,
            )
            .await)
    }

    /// The user attributes: the id token claims of an OpenID Connect login, merged with the
    /// user info when the provider has `user_info_uri`; the user info of a plain OAuth2 login.
    async fn attributes(
        &self,
        registration: &ClientRegistration,
        provider: &ProviderConfig,
        metadata: &ProviderMetadata,
        tokens: &TokenResponse,
        pending: &Pending,
        oidc: bool,
    ) -> Result<Map<String, Value>, OAuth2Error> {
        let user_info = |endpoint: Option<&str>| {
            let endpoint = endpoint.map(str::to_owned);
            async move {
                let endpoint = endpoint.ok_or(OAuth2Error::Config)?;
                UserInfo
                    .fetch(&endpoint, &tokens.access_token, self.client.as_ref())
                    .await
            }
        };
        if !oidc {
            return user_info(metadata.userinfo_endpoint.as_deref()).await;
        }
        let id_token = tokens
            .id_token
            .as_ref()
            .ok_or(OAuth2Error::InvalidIdToken)?;
        let nonce = pending.nonce.as_deref().ok_or(OAuth2Error::Pending)?;
        let claims = self
            .id_tokens
            .validate(
                id_token.expose(),
                registration,
                metadata,
                nonce,
                self.clock.now(),
            )
            .await?;
        let subject = claims.subject().to_owned();
        let mut attributes = claims.into_claims();
        if provider.user_info_uri.is_some() {
            let info = user_info(metadata.userinfo_endpoint.as_deref()).await?;
            // OpenID Connect Core 5.3.2: the user info must be about the same subject.
            if info.get("sub").and_then(Value::as_str) != Some(subject.as_str()) {
                return Err(OAuth2Error::UserInfo);
            }
            // The signed claims of the id token win over the user info.
            for (name, value) in info {
                attributes.entry(name).or_insert(value);
            }
        }
        Ok(attributes)
    }

    /// Logs the user in and redirects: the session id is rotated and the authentication stored
    /// (through form login, which also applies the session limit), the tokens are stored, and
    /// the browser goes to the saved request or the default success URL.
    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        req: &Request,
        form: &FormLoginHandlers,
        session: &Session,
        registration: &ClientRegistration,
        tokens: &TokenResponse,
        scopes: &[String],
        pending: Pending,
        authentication: crate::Authentication,
    ) -> Response {
        // Tokens of an earlier login in this session never stay next to the new login, also
        // those of a registration that is no longer configured.
        session.remove_reserved_prefix(OAUTH2_TOKENS_PREFIX);
        // The session's saved request is spent; the one captured at the start is used.
        let _ = take_saved_request(session);
        if form
            .establish(session, &authentication, Some(&registration.id))
            .await
            .is_err()
        {
            // `establish` logged the failure and ended the session.
            return self.failure_redirect();
        }
        let stored = StoredTokens::new(tokens, scopes, self.clock.now());
        if stored.store(session, &registration.id).is_err() {
            tracing::error!(
                target: "oxbloom::security",
                "the OAuth2 tokens could not be stored in the session"
            );
            session.invalidate();
            return self.failure_redirect();
        }
        tracing::debug!(
            target: "oxbloom::security",
            registration = %registration.id,
            principal = ?authentication.principal(),
            "OAuth2 login succeeded"
        );
        let default = &form.settings().default_success_url;
        let target = pending
            .saved_request
            .filter(|target| is_safe_redirect_target(target))
            .unwrap_or_else(|| default.clone());
        let location = HeaderValue::from_str(&target)
            .or_else(|_| HeaderValue::from_str(default))
            .unwrap_or_else(|_| HeaderValue::from_static("/"));
        let mut res = found(location);
        // An OAuth2 login never issues a remember-me cookie, and ends one left in the browser.
        if let Some(cookie) = form.stale_remember_me_cookie(req) {
            res.headers_mut().append(header::SET_COOKIE, cookie);
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granted_scopes_are_split_checked_and_deduplicated() {
        let requested = vec!["openid".to_owned(), "email".to_owned()];
        assert_eq!(
            granted_scopes(Some("  openid\temail  profile email "), &requested),
            ["openid", "email", "profile"]
        );
        assert_eq!(
            granted_scopes(Some("ok bad\"one back\\slash \u{7f}del é"), &requested),
            ["ok"]
        );
        assert!(granted_scopes(Some(""), &requested).is_empty());
        assert!(granted_scopes(Some("   "), &requested).is_empty());
    }

    #[test]
    fn without_a_granted_scope_the_requested_scopes_are_used() {
        let requested = vec![
            "openid".to_owned(),
            "bad scope".to_owned(),
            "read".to_owned(),
        ];
        assert_eq!(granted_scopes(None, &requested), ["openid", "read"]);
    }

    #[test]
    fn requested_scopes_put_openid_first_for_oidc_only() {
        let mut registration = ClientRegistration {
            id: "idp".into(),
            client_id: "app".into(),
            client_secret: None,
            scopes: vec!["email".into(), "openid".into(), "bad\"scope".into()],
            redirect_uri: String::new(),
            client_authentication: crate::ClientAuthentication::None,
            provider: "idp".into(),
        };
        assert_eq!(requested_scopes(&registration, true), ["openid", "email"]);
        assert_eq!(requested_scopes(&registration, false), ["email", "openid"]);
        registration.scopes.clear();
        assert_eq!(requested_scopes(&registration, true), ["openid"]);
        assert!(requested_scopes(&registration, false).is_empty());
    }

    #[test]
    fn a_state_is_1_to_64_url_safe_characters() {
        assert!(is_valid_state("abcXYZ019_-"));
        assert!(is_valid_state(&"a".repeat(64)));
        assert!(!is_valid_state(&"a".repeat(65)));
        assert!(!is_valid_state(""));
        for bad in ["a b", "a.b", "a/b", "a%b", "a+b", "é", "a\0"] {
            assert!(!is_valid_state(bad), "{bad:?}");
        }
    }

    #[test]
    fn callback_params_refuse_repeats_of_the_read_names_only() {
        let p =
            CallbackParams::parse(Some("state=s&code=c%2Bd&iss=https%3A%2F%2Fi&x=1&x=2")).unwrap();
        assert_eq!(p.state.as_deref(), Some("s"));
        assert_eq!(p.code.as_deref(), Some("c+d"));
        assert_eq!(p.iss.as_deref(), Some("https://i"));
        assert!(p.error.is_none());
        for repeated in [
            "state=a&state=b",
            "code=a&code=a",
            "error=a&error=b",
            "iss=a&iss=a",
        ] {
            assert!(
                CallbackParams::parse(Some(repeated)).is_none(),
                "{repeated}"
            );
        }
        assert!(CallbackParams::parse(None).unwrap().state.is_none());
        assert!(CallbackParams::parse(Some("state=%FF")).is_none());
    }

    #[test]
    fn endpoints_are_found_by_prefix() {
        assert!(matches!(
            endpoint("/oauth2/authorization/idp"),
            Some(Endpoint::Start("idp"))
        ));
        assert!(matches!(
            endpoint("/login/oauth2/code/idp"),
            Some(Endpoint::Callback("idp"))
        ));
        assert!(matches!(
            endpoint("/oauth2/authorization/"),
            Some(Endpoint::Start(""))
        ));
        assert!(endpoint("/oauth2/authorization").is_none());
        assert!(endpoint("/login").is_none());
    }
}
