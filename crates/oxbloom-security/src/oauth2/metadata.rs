//! Provider metadata: OpenID Connect discovery, explicit endpoints, and a cache.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use jsonwebtoken::Algorithm;
use oxbloom_http_client::{HttpClient, Request};
use serde::Deserialize;

use super::settings::is_acceptable_endpoint;
use super::{MAX_BODY_BYTES, OAuth2Error, ProviderConfig};

/// The suffix of the discovery document, appended to the issuer without its trailing slash.
const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";
/// The id token algorithms accepted at all: asymmetric ones whose keys the JWKS source can
/// match (`PS256` is left out because no key in a set is ever matched to it).
pub(crate) const ASYMMETRIC: [Algorithm; 5] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::ES256,
    Algorithm::ES384,
];
/// The algorithms used when the provider does not list any.
const DEFAULT_ALGORITHMS: [Algorithm; 2] = [Algorithm::RS256, Algorithm::ES256];
/// How long resolved metadata is used by default.
const DEFAULT_TTL: Duration = Duration::from_secs(3600);
/// How long a failed resolution is answered from memory by default.
const DEFAULT_FAILURE_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct Discovery {
    issuer: Option<String>,
    authorization_endpoint: Option<String>,
    token_endpoint: Option<String>,
    userinfo_endpoint: Option<String>,
    jwks_uri: Option<String>,
    id_token_signing_alg_values_supported: Option<Vec<String>>,
}

/// The endpoints of a provider and its id token algorithms, from the configuration and, for
/// an `issuer-uri` provider, from its discovery document.
///
/// `Debug` shows which endpoints are present and the algorithms, never a URL.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
/// use oxbloom_security::ProviderConfig;
/// use oxbloom_security::__test_support::ProviderMetadata;
/// use oxbloom_test::FakeProvider;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let provider = ProviderConfig {
///     issuer_uri: Some("https://idp.example.test".into()),
///     ..ProviderConfig::default()
/// };
/// let metadata = ProviderMetadata::resolve(&provider, fake.client().as_ref(), SystemTime::now()).await?;
/// assert_eq!(metadata.token_endpoint, "https://idp.example.test/token");
/// assert!(metadata.is_oidc());
/// # Ok::<(), oxbloom_security::__test_support::OAuth2Error>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderMetadata {
    /// The issuer, exactly as configured; `None` for a provider without `issuer-uri`.
    pub issuer: Option<String>,
    /// The authorization endpoint.
    pub authorization_endpoint: String,
    /// The token endpoint.
    pub token_endpoint: String,
    /// The user-info endpoint.
    pub userinfo_endpoint: Option<String>,
    /// The key set that signs id tokens; present for an OpenID Connect provider.
    pub jwks_uri: Option<String>,
    /// The algorithms an id token may use: the provider's list (default `RS256` and `ES256`)
    /// narrowed to `RS256`, `RS384`, `RS512`, `ES256` and `ES384`.
    pub id_token_algorithms: Vec<Algorithm>,
    /// When the metadata was resolved.
    pub resolved_at: SystemTime,
}

impl ProviderMetadata {
    /// Resolves the metadata of `provider` at the time `now`.
    ///
    /// For a provider with `issuer-uri` it fetches `<issuer>/.well-known/openid-configuration`
    /// (the issuer without its trailing slash, then the path) through `client`. The `issuer`
    /// of the document must equal `issuer-uri` exactly: a trailing slash is significant. An
    /// explicit URI in the configuration overrides the discovered one. Every endpoint must be
    /// an absolute `https` URL (`http` only for a loopback host). A provider without
    /// `issuer-uri` takes its endpoints from the configuration and needs no request.
    ///
    /// # Errors
    ///
    /// [`OAuth2Error::Http`] when the request fails; [`OAuth2Error::Discovery`] for a non-2xx
    /// answer, a body over 1 MiB, a document that is not a JSON object, a missing
    /// authorization or token endpoint, or an endpoint that is not acceptable;
    /// [`OAuth2Error::IssuerMismatch`] when the document names another issuer.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::ProviderConfig;
    /// use oxbloom_security::__test_support::{OAuth2Error, ProviderMetadata};
    /// use oxbloom_test::{FakeProvider, Fault};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// fake.misbehave(Fault::DiscoveryIssuerMismatch);
    /// let provider = ProviderConfig {
    ///     issuer_uri: Some("https://idp.example.test".into()),
    ///     ..ProviderConfig::default()
    /// };
    /// let err = ProviderMetadata::resolve(&provider, fake.client().as_ref(), SystemTime::now())
    ///     .await
    ///     .unwrap_err();
    /// assert_eq!(err, OAuth2Error::IssuerMismatch);
    /// # })
    /// # ;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn resolve(
        provider: &ProviderConfig,
        client: &dyn HttpClient,
        now: SystemTime,
    ) -> Result<ProviderMetadata, OAuth2Error> {
        let discovered = match &provider.issuer_uri {
            Some(issuer) => Some(discover(issuer, client).await?),
            None => None,
        };
        let pick = |explicit: &Option<String>, found: Option<&Option<String>>| {
            explicit.clone().or_else(|| found.cloned().flatten())
        };
        let d = discovered.as_ref();
        let authorization_endpoint = pick(
            &provider.authorization_uri,
            d.map(|d| &d.authorization_endpoint),
        )
        .ok_or(OAuth2Error::Discovery)?;
        let token_endpoint = pick(&provider.token_uri, d.map(|d| &d.token_endpoint))
            .ok_or(OAuth2Error::Discovery)?;
        let userinfo_endpoint = pick(&provider.user_info_uri, d.map(|d| &d.userinfo_endpoint));
        let jwks_uri = pick(&provider.jwk_set_uri, d.map(|d| &d.jwks_uri));
        let all = [
            Some(&authorization_endpoint),
            Some(&token_endpoint),
            userinfo_endpoint.as_ref(),
            jwks_uri.as_ref(),
        ];
        if !all
            .into_iter()
            .flatten()
            .all(|endpoint| is_acceptable_endpoint(endpoint, true))
        {
            return Err(OAuth2Error::Discovery);
        }
        let id_token_algorithms =
            match d.and_then(|d| d.id_token_signing_alg_values_supported.as_ref()) {
                Some(names) => ASYMMETRIC
                    .into_iter()
                    .filter(|alg| {
                        names
                            .iter()
                            .any(|name| name.parse::<Algorithm>().ok() == Some(*alg))
                    })
                    .collect(),
                None => DEFAULT_ALGORITHMS.to_vec(),
            };
        Ok(ProviderMetadata {
            issuer: provider.issuer_uri.clone(),
            authorization_endpoint,
            token_endpoint,
            userinfo_endpoint,
            jwks_uri,
            id_token_algorithms,
            resolved_at: now,
        })
    }

    /// Whether the provider is an OpenID Connect provider: it has a key set for id tokens.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_security::__test_support::ProviderMetadata;
    ///
    /// let plain = ProviderMetadata {
    ///     issuer: None,
    ///     authorization_endpoint: "https://github.example.test/authorize".into(),
    ///     token_endpoint: "https://github.example.test/token".into(),
    ///     userinfo_endpoint: Some("https://api.github.example.test/user".into()),
    ///     jwks_uri: None,
    ///     id_token_algorithms: Vec::new(),
    ///     resolved_at: SystemTime::UNIX_EPOCH,
    /// };
    /// assert!(!plain.is_oidc());
    /// ```
    #[must_use]
    pub fn is_oidc(&self) -> bool {
        self.jwks_uri.is_some()
    }
}

impl fmt::Debug for ProviderMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderMetadata")
            .field("issuer", &self.issuer.is_some())
            .field("userinfo_endpoint", &self.userinfo_endpoint.is_some())
            .field("jwks_uri", &self.jwks_uri.is_some())
            .field("id_token_algorithms", &self.id_token_algorithms)
            .field("resolved_at", &self.resolved_at)
            .finish_non_exhaustive()
    }
}

/// Fetches and checks the discovery document of `issuer`.
async fn discover(issuer: &str, client: &dyn HttpClient) -> Result<Discovery, OAuth2Error> {
    let url = format!("{}{DISCOVERY_PATH}", issuer.trim_end_matches('/'));
    let request = Request::get(url).header("accept", "application/json");
    let response = client
        .execute(request)
        .await
        .map_err(|_| OAuth2Error::Http)?;
    if !response.status().is_success() || response.bytes().len() > MAX_BODY_BYTES {
        return Err(OAuth2Error::Discovery);
    }
    let document: Discovery =
        serde_json::from_slice(response.bytes()).map_err(|_| OAuth2Error::Discovery)?;
    if document.issuer.as_deref() != Some(issuer) {
        return Err(OAuth2Error::IssuerMismatch);
    }
    Ok(document)
}

/// What one cache slot remembers: the last metadata and the last failure.
#[derive(Default)]
struct SlotState {
    cached: Option<Arc<ProviderMetadata>>,
    /// The error of the last attempt and the time passed to it; cleared by a success.
    failure: Option<(OAuth2Error, SystemTime)>,
}

/// One cache slot per provider. Its async lock is held while the metadata is resolved, so
/// concurrent callers wait for one resolution instead of starting their own. `attempts`
/// counts the finished attempts; a caller that sees it change while it waited takes the
/// outcome of that attempt.
#[derive(Default)]
struct Slot {
    attempts: AtomicU64,
    state: tokio::sync::Mutex<SlotState>,
}

/// Caches [`ProviderMetadata`] per provider id for a time limit (default one hour).
///
/// Concurrent calls for the same provider are coalesced, so one request is made and the
/// others wait for its result, success or failure. A failure is not cached as metadata, but
/// it is remembered for a short backoff window (default 5 seconds, measured on the time passed
/// in): a call within the window gets the same error without a request, and the first call
/// after it tries again. A success clears the failure. During a provider outage logins
/// therefore fail fast instead of queueing for one request each.
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
/// use oxbloom_security::ProviderConfig;
/// use oxbloom_security::__test_support::MetadataCache;
/// use oxbloom_test::FakeProvider;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let provider = ProviderConfig {
///     issuer_uri: Some("https://idp.example.test".into()),
///     ..ProviderConfig::default()
/// };
/// let cache = MetadataCache::new();
/// let now = SystemTime::now();
/// cache.get_or_resolve("idp", &provider, fake.client().as_ref(), now).await?;
/// cache.get_or_resolve("idp", &provider, fake.client().as_ref(), now).await?;
/// assert_eq!(fake.client().request_count(), 1);
/// # Ok::<(), oxbloom_security::__test_support::OAuth2Error>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct MetadataCache {
    ttl: Duration,
    failure_backoff: Duration,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl MetadataCache {
    /// A cache whose entries are used for one hour.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::__test_support::MetadataCache;
    ///
    /// let cache = MetadataCache::new();
    /// # let _ = cache;
    /// ```
    #[must_use]
    pub fn new() -> MetadataCache {
        MetadataCache::with_ttl(DEFAULT_TTL)
    }

    /// A cache whose entries are used for `ttl`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::__test_support::MetadataCache;
    ///
    /// let cache = MetadataCache::with_ttl(Duration::from_secs(600));
    /// # let _ = cache;
    /// ```
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> MetadataCache {
        MetadataCache {
            ttl,
            failure_backoff: DEFAULT_FAILURE_BACKOFF,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// Sets how long a failed resolution is answered from memory before the next call tries
    /// again (default 5 seconds). Zero retries on every call that did not wait for the failed
    /// attempt.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::__test_support::MetadataCache;
    ///
    /// let cache = MetadataCache::new().failure_backoff(Duration::from_secs(30));
    /// # let _ = cache;
    /// ```
    #[must_use]
    pub fn failure_backoff(mut self, backoff: Duration) -> MetadataCache {
        self.failure_backoff = backoff;
        self
    }

    /// Returns the cached metadata of `provider_id` when it was resolved less than the TTL
    /// before `now`, otherwise resolves it with [`ProviderMetadata::resolve`] and caches it.
    ///
    /// A caller that waited while another call resolved the same provider takes that call's
    /// outcome. A caller that arrives within the backoff window of a failure gets that
    /// failure without a request.
    ///
    /// # Errors
    ///
    /// The errors of [`ProviderMetadata::resolve`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use oxbloom_security::ProviderConfig;
    /// use oxbloom_security::__test_support::{MetadataCache, OAuth2Error};
    /// use oxbloom_test::MockHttpClient;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let provider = ProviderConfig {
    ///     issuer_uri: Some("https://idp.example.test".into()),
    ///     ..ProviderConfig::default()
    /// };
    /// // The mock answers nothing, so the resolution fails.
    /// let mock = MockHttpClient::new();
    /// let cache = MetadataCache::new();
    /// let now = SystemTime::now();
    /// assert_eq!(cache.get_or_resolve("idp", &provider, &mock, now).await.unwrap_err(), OAuth2Error::Http);
    /// // Within the backoff window the failure is answered without a request ...
    /// assert!(cache.get_or_resolve("idp", &provider, &mock, now).await.is_err());
    /// assert_eq!(mock.request_count(), 1);
    /// // ... and after it the next call tries again.
    /// let later = now + Duration::from_secs(5);
    /// assert!(cache.get_or_resolve("idp", &provider, &mock, later).await.is_err());
    /// assert_eq!(mock.request_count(), 2);
    /// # })
    /// # ;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn get_or_resolve(
        &self,
        provider_id: &str,
        provider: &ProviderConfig,
        client: &dyn HttpClient,
        now: SystemTime,
    ) -> Result<Arc<ProviderMetadata>, OAuth2Error> {
        let slot = self
            .slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(provider_id.to_owned())
            .or_default()
            .clone();
        let seen = slot.attempts.load(Ordering::Acquire);
        let mut state = slot.state.lock().await;
        // An attempt finished while this caller waited: take its failure. Its success is in
        // `cached` and is found below.
        if slot.attempts.load(Ordering::Acquire) != seen
            && let Some((error, _)) = state.failure
        {
            return Err(error);
        }
        // A clock that went back counts as age zero.
        let age = |since: SystemTime| now.duration_since(since).unwrap_or(Duration::ZERO);
        if let Some(metadata) = &state.cached
            && age(metadata.resolved_at) < self.ttl
        {
            return Ok(metadata.clone());
        }
        if let Some((error, failed_at)) = state.failure
            && age(failed_at) < self.failure_backoff
        {
            return Err(error);
        }
        let result = ProviderMetadata::resolve(provider, client, now)
            .await
            .map(Arc::new);
        match &result {
            Ok(metadata) => {
                state.cached = Some(metadata.clone());
                state.failure = None;
            }
            Err(error) => state.failure = Some((*error, now)),
        }
        // Counted while the lock is still held, so every waiter sees it.
        slot.attempts.fetch_add(1, Ordering::Release);
        result
    }
}

impl Default for MetadataCache {
    /// The same as [`MetadataCache::new`].
    fn default() -> MetadataCache {
        MetadataCache::new()
    }
}

impl fmt::Debug for MetadataCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MetadataCache")
            .field("ttl", &self.ttl)
            .field("failure_backoff", &self.failure_backoff)
            .finish_non_exhaustive()
    }
}
