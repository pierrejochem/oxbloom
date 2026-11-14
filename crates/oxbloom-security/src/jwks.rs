//! JWKS key source: fetches a JSON Web Key Set through a pluggable [`JwksFetcher`] and caches it.
//!
//! Cache rules:
//!
//! - A fetched set is fresh for the TTL (default 5 minutes).
//! - A lookup for an unknown `kid`, an expired set or an empty cache starts a refresh.
//! - Refresh attempts are rate-limited: a new attempt starts only when the minimum refresh
//!   interval (default 30 seconds) has passed since the previous attempt *started*, whatever its
//!   outcome (success, failure, timeout or a panicking fetcher). Random `kid` values and a
//!   failing or slow issuer therefore cause at most one fetch per interval.
//! - A fetch runs as its own Tokio task, bounded by the fetch timeout, so a caller that goes
//!   away (e.g. a client that disconnects) cannot abandon it. Outside a Tokio runtime no fetch
//!   can run: a lookup that needs one is answered as if the refresh had failed (stale set, or
//!   [`KeyLookupError::Unavailable`] without one); an attempt that never started (no runtime, or a runtime that drops the task unpolled
//!   while shutting down) does not count for the rate limit.
//! - Concurrent lookups that need a refresh share one in-flight fetch. Lookups that a fresh set
//!   answers never wait for it.
//! - When a refresh fails, the previous set is served until it is older than twice the TTL.
//!   After that lookups fail closed with [`KeyLookupError::Unavailable`].
//! - Only keys whose `kty`, curve and `alg` fit the requested algorithms are used, and a present
//!   `use` must be `sig`.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey};
use tokio::sync::watch;
use tokio::time::Instant;

/// Loads a JSON Web Key Set from a URI.
///
/// [`JwksSource`] calls it at most once per refresh interval and caches the result. The
/// implementation enforces its own limits (body size, timeout, no redirects) and reports
/// problems as a [`FetchError`], never with the response body.
///
/// # Examples
///
/// ```
/// use async_trait::async_trait;
/// use jsonwebtoken::jwk::JwkSet;
/// use oxbloom_security::{FetchError, JwksFetcher};
///
/// /// Serves a fixed key set, e.g. in tests.
/// struct StaticJwks(JwkSet);
///
/// #[async_trait]
/// impl JwksFetcher for StaticJwks {
///     async fn fetch(&self, _uri: &str) -> Result<JwkSet, FetchError> {
///         Ok(self.0.clone())
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let fetcher = StaticJwks(JwkSet { keys: Vec::new() });
/// let set = fetcher.fetch("https://issuer.example/jwks.json").await;
/// assert_eq!(set.map(|s| s.keys.len()), Ok(0));
/// # }
/// ```
#[async_trait]
pub trait JwksFetcher: Send + Sync {
    /// Fetches and parses the key set at `uri`.
    async fn fetch(&self, uri: &str) -> Result<JwkSet, FetchError>;
}

/// Why a [`JwksFetcher`] could not load a key set. Fixed text, never the response body.
///
/// # Examples
///
/// ```
/// use oxbloom_security::FetchError;
///
/// assert_eq!(FetchError::Timeout.to_string(), "JWKS request timed out");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FetchError {
    /// The connection failed or the request could not be sent.
    Network,
    /// The request did not finish in time.
    Timeout,
    /// The response body exceeds the size cap.
    TooLarge,
    /// The endpoint answered with a non-success status.
    BadStatus,
    /// The body is not a valid JWK set.
    Malformed,
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Network => "JWKS request failed",
            Self::Timeout => "JWKS request timed out",
            Self::TooLarge => "JWKS response too large",
            Self::BadStatus => "JWKS endpoint returned an error status",
            Self::Malformed => "JWKS response is not a valid key set",
        })
    }
}

impl std::error::Error for FetchError {}

/// Why [`JwksSource::key_for`] returned no key. Fixed text.
///
/// # Examples
///
/// ```
/// use oxbloom_security::KeyLookupError;
///
/// assert_eq!(KeyLookupError::Unavailable.to_string(), "JWKS keys unavailable");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyLookupError {
    /// The set has no usable key with this `kid` for the requested algorithms.
    UnknownKid,
    /// No key set could be loaded, or the cached one is older than twice the TTL.
    Unavailable,
}

impl fmt::Display for KeyLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownKid => "no usable key for the key id",
            Self::Unavailable => "JWKS keys unavailable",
        })
    }
}

impl std::error::Error for KeyLookupError {}

/// Why [`JwksSourceBuilder::build`] refused a configuration. Fixed text.
///
/// # Examples
///
/// ```
/// use oxbloom_security::JwksError;
///
/// assert_eq!(JwksError::InsecureUri.to_string(), "JWKS URI must use https");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JwksError {
    /// The URI cannot be parsed, has no host, or uses a scheme other than `https`/`http`.
    InvalidUri,
    /// The URI uses `http` and [`JwksSourceBuilder::allow_insecure_http`] was not called.
    InsecureUri,
    /// The TTL is zero.
    ZeroTtl,
    /// The fetch timeout is zero.
    ZeroTimeout,
}

impl fmt::Display for JwksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidUri => "invalid JWKS URI",
            Self::InsecureUri => "JWKS URI must use https",
            Self::ZeroTtl => "JWKS TTL must be greater than zero",
            Self::ZeroTimeout => "JWKS fetch timeout must be greater than zero",
        })
    }
}

impl std::error::Error for JwksError {}

const DEFAULT_TTL: Duration = Duration::from_secs(5 * 60);
const DEFAULT_MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Configures a [`JwksSource`].
///
/// Defaults: TTL 5 minutes, at most one refresh per 30 seconds, fetch timeout 5 seconds,
/// `https` required.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// # use async_trait::async_trait;
/// # use jsonwebtoken::jwk::JwkSet;
/// use oxbloom_security::{FetchError, JwksFetcher, JwksSource};
/// # struct StaticJwks;
/// # #[async_trait]
/// # impl JwksFetcher for StaticJwks {
/// #     async fn fetch(&self, _uri: &str) -> Result<JwkSet, FetchError> {
/// #         Ok(JwkSet { keys: Vec::new() })
/// #     }
/// # }
///
/// let source = JwksSource::builder(Arc::new(StaticJwks), "https://issuer.example/jwks.json")
///     .ttl(Duration::from_secs(600))
///     .min_refresh_interval(Duration::from_secs(60))
///     .build()?;
/// # let _ = source;
/// # Ok::<(), oxbloom_security::JwksError>(())
/// ```
pub struct JwksSourceBuilder {
    fetcher: Arc<dyn JwksFetcher>,
    uri: String,
    ttl: Duration,
    min_refresh_interval: Duration,
    fetch_timeout: Duration,
    allow_insecure_http: bool,
}

impl JwksSourceBuilder {
    /// How long a fetched set is fresh (default 5 minutes). A set that cannot be refreshed is
    /// served until it is twice this old. Zero is a [`JwksError::ZeroTtl`] at build time.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::Arc;
    /// use std::time::Duration;
    /// # use async_trait::async_trait;
    /// # use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::{JwksError, JwksSource};
    /// # struct StaticJwks;
    /// # #[async_trait]
    /// # impl oxbloom_security::JwksFetcher for StaticJwks {
    /// #     async fn fetch(&self, _: &str) -> Result<JwkSet, oxbloom_security::FetchError> {
    /// #         Ok(JwkSet { keys: Vec::new() })
    /// #     }
    /// # }
    ///
    /// let builder = JwksSource::builder(Arc::new(StaticJwks), "https://issuer.example/jwks");
    /// let err = builder.ttl(Duration::ZERO).build().unwrap_err();
    /// assert_eq!(err, JwksError::ZeroTtl);
    /// ```
    pub fn ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// The minimum time between the starts of two fetches (default 30 seconds), whatever caused
    /// them and whether they succeeded. Zero disables the rate limit; do not do that in
    /// production.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::Arc;
    /// use std::time::Duration;
    /// # use async_trait::async_trait;
    /// # use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::JwksSource;
    /// # struct StaticJwks;
    /// # #[async_trait]
    /// # impl oxbloom_security::JwksFetcher for StaticJwks {
    /// #     async fn fetch(&self, _: &str) -> Result<JwkSet, oxbloom_security::FetchError> {
    /// #         Ok(JwkSet { keys: Vec::new() })
    /// #     }
    /// # }
    ///
    /// let source = JwksSource::builder(Arc::new(StaticJwks), "https://issuer.example/jwks")
    ///     .min_refresh_interval(Duration::from_secs(60))
    ///     .build();
    /// assert!(source.is_ok());
    /// ```
    pub fn min_refresh_interval(mut self, interval: Duration) -> Self {
        self.min_refresh_interval = interval;
        self
    }

    /// Upper bound for one fetch (default 5 seconds), applied on top of the fetcher's own
    /// timeout so a hanging fetcher cannot stall lookups. A fetch that runs out of time counts
    /// as a failed refresh. Zero is a [`JwksError::ZeroTimeout`] at build time.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::Arc;
    /// use std::time::Duration;
    /// # use async_trait::async_trait;
    /// # use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::JwksSource;
    /// # struct StaticJwks;
    /// # #[async_trait]
    /// # impl oxbloom_security::JwksFetcher for StaticJwks {
    /// #     async fn fetch(&self, _: &str) -> Result<JwkSet, oxbloom_security::FetchError> {
    /// #         Ok(JwkSet { keys: Vec::new() })
    /// #     }
    /// # }
    ///
    /// let source = JwksSource::builder(Arc::new(StaticJwks), "https://issuer.example/jwks")
    ///     .fetch_timeout(Duration::from_secs(2))
    ///     .build();
    /// assert!(source.is_ok());
    /// ```
    pub fn fetch_timeout(mut self, timeout: Duration) -> Self {
        self.fetch_timeout = timeout;
        self
    }

    /// Permits a plain `http` URI. Meant for loopback tests only: keys fetched over `http` can
    /// be replaced by anyone on the network path.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use async_trait::async_trait;
    /// # use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::JwksSource;
    /// # struct StaticJwks;
    /// # #[async_trait]
    /// # impl oxbloom_security::JwksFetcher for StaticJwks {
    /// #     async fn fetch(&self, _: &str) -> Result<JwkSet, oxbloom_security::FetchError> {
    /// #         Ok(JwkSet { keys: Vec::new() })
    /// #     }
    /// # }
    ///
    /// let source = JwksSource::builder(Arc::new(StaticJwks), "http://127.0.0.1:8080/jwks")
    ///     .allow_insecure_http()
    ///     .build();
    /// assert!(source.is_ok());
    /// ```
    pub fn allow_insecure_http(mut self) -> Self {
        self.allow_insecure_http = true;
        self
    }

    /// Builds the source. Nothing is fetched until the first lookup.
    ///
    /// # Errors
    ///
    /// [`JwksError::InvalidUri`], [`JwksError::InsecureUri`] for `http` without
    /// [`allow_insecure_http`](Self::allow_insecure_http), [`JwksError::ZeroTtl`] and
    /// [`JwksError::ZeroTimeout`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use async_trait::async_trait;
    /// # use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::{JwksError, JwksSource};
    /// # struct StaticJwks;
    /// # #[async_trait]
    /// # impl oxbloom_security::JwksFetcher for StaticJwks {
    /// #     async fn fetch(&self, _: &str) -> Result<JwkSet, oxbloom_security::FetchError> {
    /// #         Ok(JwkSet { keys: Vec::new() })
    /// #     }
    /// # }
    ///
    /// let err = JwksSource::builder(Arc::new(StaticJwks), "http://issuer.example/jwks")
    ///     .build()
    ///     .unwrap_err();
    /// assert_eq!(err, JwksError::InsecureUri);
    /// ```
    pub fn build(self) -> Result<Arc<JwksSource>, JwksError> {
        let uri: http::Uri = self.uri.parse().map_err(|_| JwksError::InvalidUri)?;
        if uri.host().is_none_or(str::is_empty) {
            return Err(JwksError::InvalidUri);
        }
        match uri.scheme_str() {
            Some(s) if s.eq_ignore_ascii_case("https") => {}
            Some(s) if s.eq_ignore_ascii_case("http") => {
                if !self.allow_insecure_http {
                    return Err(JwksError::InsecureUri);
                }
            }
            _ => return Err(JwksError::InvalidUri),
        }
        if self.ttl.is_zero() {
            return Err(JwksError::ZeroTtl);
        }
        if self.fetch_timeout.is_zero() {
            return Err(JwksError::ZeroTimeout);
        }
        Ok(Arc::new(JwksSource {
            inner: Arc::new(Inner {
                fetcher: self.fetcher,
                uri: self.uri,
                ttl: self.ttl,
                min_refresh_interval: self.min_refresh_interval,
                fetch_timeout: self.fetch_timeout,
                state: Mutex::new(State::default()),
            }),
        }))
    }
}

/// The URI for `Debug` output: scheme, host, port and path only. Userinfo and the query string
/// may hold credentials and are dropped; an unparseable URI gives a fixed placeholder.
fn redacted_uri(uri: &str) -> String {
    let Ok(uri) = uri.parse::<http::Uri>() else {
        return "<unparseable uri>".to_owned();
    };
    match (uri.scheme_str(), uri.host()) {
        (Some(scheme), Some(host)) => {
            let port = uri.port().map(|p| format!(":{p}")).unwrap_or_default();
            format!("{scheme}://{host}{port}{}", uri.path())
        }
        _ => "<unparseable uri>".to_owned(),
    }
}

impl fmt::Debug for JwksSourceBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwksSourceBuilder")
            .field("uri", &redacted_uri(&self.uri))
            .field("ttl", &self.ttl)
            .field("min_refresh_interval", &self.min_refresh_interval)
            .field("fetch_timeout", &self.fetch_timeout)
            .field("allow_insecure_http", &self.allow_insecure_http)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct State {
    /// The last successfully fetched set and when it arrived.
    cached: Option<(Arc<JwkSet>, Instant)>,
    /// When the last fetch attempt started (any outcome).
    last_attempt: Option<Instant>,
    /// Set while a fetch task runs; becomes `true` (or closes) when it has finished.
    in_flight: Option<watch::Receiver<bool>>,
}

/// A cached JWKS, looked up by `kid`. Build it with [`JwksSource::builder`] and pass it to
/// [`JwtDecoderBuilder::with_jwks`](crate::JwtDecoderBuilder::with_jwks).
///
/// See the [module rules](self) for TTL, rate limit, stale serving and key filtering.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use async_trait::async_trait;
/// use jsonwebtoken::jwk::JwkSet;
/// use oxbloom_security::{
///     Algorithm, FetchError, JwksFetcher, JwksSource, JwtDecoderBuilder,
/// };
///
/// struct Issuer;
///
/// #[async_trait]
/// impl JwksFetcher for Issuer {
///     async fn fetch(&self, _uri: &str) -> Result<JwkSet, FetchError> {
///         Err(FetchError::Network) // a real fetcher loads the set over https
///     }
/// }
///
/// let source = JwksSource::builder(Arc::new(Issuer), "https://issuer.example/jwks.json").build()?;
/// let decoder = JwtDecoderBuilder::with_jwks(source, vec![Algorithm::RS256]).build()?;
/// # let _ = decoder;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct JwksSource {
    inner: Arc<Inner>,
}

/// Shared with the spawned fetch task, so a fetch outlives the request that started it.
struct Inner {
    fetcher: Arc<dyn JwksFetcher>,
    uri: String,
    ttl: Duration,
    min_refresh_interval: Duration,
    fetch_timeout: Duration,
    state: Mutex<State>,
}

impl JwksSource {
    /// Starts configuring a source that loads `uri` through `fetcher`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use async_trait::async_trait;
    /// # use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::JwksSource;
    /// # struct StaticJwks;
    /// # #[async_trait]
    /// # impl oxbloom_security::JwksFetcher for StaticJwks {
    /// #     async fn fetch(&self, _: &str) -> Result<JwkSet, oxbloom_security::FetchError> {
    /// #         Ok(JwkSet { keys: Vec::new() })
    /// #     }
    /// # }
    ///
    /// let source = JwksSource::builder(Arc::new(StaticJwks), "https://issuer.example/jwks").build();
    /// assert!(source.is_ok());
    /// ```
    pub fn builder(fetcher: Arc<dyn JwksFetcher>, uri: impl Into<String>) -> JwksSourceBuilder {
        JwksSourceBuilder {
            fetcher,
            uri: uri.into(),
            ttl: DEFAULT_TTL,
            min_refresh_interval: DEFAULT_MIN_REFRESH_INTERVAL,
            fetch_timeout: DEFAULT_FETCH_TIMEOUT,
            allow_insecure_http: false,
        }
    }

    /// Returns the key with id `kid` that fits one of `allowed`, refreshing the set when needed.
    ///
    /// A key fits when its `kty` (and, for EC, its curve) matches an allowed algorithm, its
    /// `alg` (if present) is allowed, its `use` (if present) is `sig`, and its key material is
    /// well-formed. HMAC (`oct`) and other key types never fit.
    ///
    /// A needed refresh is spawned as a Tokio task. Outside a Tokio runtime no refresh runs and
    /// the lookup is answered as if it had failed (stale set within twice the TTL, otherwise
    /// [`KeyLookupError::Unavailable`]).
    ///
    /// # Errors
    ///
    /// [`KeyLookupError::UnknownKid`] when the (possibly refreshed) set has no fitting key, and
    /// [`KeyLookupError::Unavailable`] when no set could be loaded or the cached set is older
    /// than twice the TTL.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use async_trait::async_trait;
    /// use jsonwebtoken::jwk::JwkSet;
    /// use oxbloom_security::{Algorithm, FetchError, JwksFetcher, JwksSource, KeyLookupError};
    ///
    /// struct Down;
    ///
    /// #[async_trait]
    /// impl JwksFetcher for Down {
    ///     async fn fetch(&self, _uri: &str) -> Result<JwkSet, FetchError> {
    ///         Err(FetchError::Network)
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let source = JwksSource::builder(Arc::new(Down), "https://issuer.example/jwks").build()?;
    /// let err = source.key_for("key-1", &[Algorithm::RS256]).await.unwrap_err();
    /// assert_eq!(err, KeyLookupError::Unavailable);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn key_for(&self, kid: &str, allowed: &[Algorithm]) -> Result<Jwk, KeyLookupError> {
        let inner = &self.inner;
        // One short, synchronous look at the state: answer, join the running fetch, or start one.
        let (mut done, start) = {
            let mut state = inner.state();
            let now = Instant::now();
            if let Some((set, fetched_at)) = &state.cached
                && now.saturating_duration_since(*fetched_at) < inner.ttl
                && let Some(jwk) = find(set, kid, allowed)
            {
                return Ok(jwk);
            }
            match &state.in_flight {
                Some(done) => (done.clone(), None),
                None if inner.may_refresh(&state, now) => {
                    let previous = state.last_attempt.replace(now);
                    let (tx, rx) = watch::channel(false);
                    state.in_flight = Some(rx.clone());
                    let finish = Finish {
                        inner: inner.clone(),
                        done: tx,
                        unstarted: Some(previous),
                    };
                    (rx, Some(finish))
                }
                // Nothing running and the rate limit forbids a new attempt.
                None => return inner.resolve(&state, now, kid, allowed),
            }
        };
        // The state lock is released here: dropping `finish` below locks it again.
        if let Some(finish) = start {
            // The fetch runs as its own task, so a caller that goes away (client disconnect)
            // cannot abandon it. `finish` moves into the task: if the task is dropped before
            // its first poll (runtime shutting down), or there is no runtime at all, dropping
            // it clears the in-flight marker and gives back the unused rate-limit slot.
            match tokio::runtime::Handle::try_current() {
                Ok(runtime) => drop(runtime.spawn(Inner::refresh(finish))),
                Err(_) => drop(finish),
            }
        }
        // `Err` means the task ended without reporting (panic, or dropped unpolled): its
        // `Finish` guard has already cleared the in-flight marker, so the outcome is in the
        // state either way.
        let _ = done.wait_for(|finished| *finished).await;
        inner.resolve(&inner.state(), Instant::now(), kid, allowed)
    }
}

impl Inner {
    /// One fetch attempt, run as a spawned task. Bounded by the fetch timeout.
    async fn refresh(mut finish: Finish) {
        // From here on the attempt counts for the rate limit, whatever its outcome.
        finish.unstarted = None;
        let inner = finish.inner.clone();
        let fetched =
            tokio::time::timeout(inner.fetch_timeout, inner.fetcher.fetch(&inner.uri)).await;
        if let Ok(Ok(set)) = fetched {
            inner.state().cached = Some((Arc::new(set), Instant::now()));
        }
        // A failed fetch keeps the previous set; `resolve` decides whether it is still usable.
        drop(finish);
    }

    /// Whether the rate limit allows a fetch attempt to start at `now`.
    fn may_refresh(&self, state: &State, now: Instant) -> bool {
        state
            .last_attempt
            .is_none_or(|last| now.saturating_duration_since(last) >= self.min_refresh_interval)
    }

    /// Answers from the cached set without fetching: stale sets are served up to twice the TTL.
    fn resolve(
        &self,
        state: &State,
        now: Instant,
        kid: &str,
        allowed: &[Algorithm],
    ) -> Result<Jwk, KeyLookupError> {
        let Some((set, fetched_at)) = &state.cached else {
            return Err(KeyLookupError::Unavailable);
        };
        if now.saturating_duration_since(*fetched_at) > self.ttl.saturating_mul(2) {
            return Err(KeyLookupError::Unavailable);
        }
        find(set, kid, allowed).ok_or(KeyLookupError::UnknownKid)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for JwksSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwksSource")
            .field("uri", &redacted_uri(&self.inner.uri))
            .field("ttl", &self.inner.ttl)
            .field("min_refresh_interval", &self.inner.min_refresh_interval)
            .field("fetch_timeout", &self.inner.fetch_timeout)
            .finish_non_exhaustive()
    }
}

/// Ends a fetch attempt: clears the in-flight marker, then wakes the waiters.
///
/// It is created before the task is spawned and moved into it, so it runs in every case: normal
/// completion, timeout, a panicking fetcher (unwind), the task being dropped unpolled by a
/// runtime that is shutting down, and no runtime being available. Waiters therefore never hang
/// and a later attempt can always start. If the fetch never started, the rate-limit stamp is
/// restored, so an attempt that never ran does not block the next one.
struct Finish {
    inner: Arc<Inner>,
    done: watch::Sender<bool>,
    /// `Some(previous last_attempt)` until the task starts the fetch.
    unstarted: Option<Option<Instant>>,
}

impl Drop for Finish {
    fn drop(&mut self) {
        {
            let mut state = self.inner.state();
            state.in_flight = None;
            if let Some(previous) = self.unstarted.take() {
                state.last_attempt = previous;
            }
        }
        self.done.send_replace(true);
    }
}

/// The first key with id `kid` that fits one of `allowed`.
fn find(set: &JwkSet, kid: &str, allowed: &[Algorithm]) -> Option<Jwk> {
    set.keys
        .iter()
        .find(|jwk| jwk.common.key_id.as_deref() == Some(kid) && fits(jwk, allowed))
        .cloned()
}

/// Whether `jwk` may verify a token signed with one of `allowed`.
fn fits(jwk: &Jwk, allowed: &[Algorithm]) -> bool {
    if jwk
        .common
        .public_key_use
        .as_ref()
        .is_some_and(|u| *u != PublicKeyUse::Signature)
    {
        return false;
    }
    // The algorithms this key's type and curve can verify.
    let by_type: &[Algorithm] = match &jwk.algorithm {
        AlgorithmParameters::RSA(_) => &[Algorithm::RS256, Algorithm::RS384, Algorithm::RS512],
        AlgorithmParameters::EllipticCurve(ec) => match ec.curve {
            EllipticCurve::P256 => &[Algorithm::ES256],
            EllipticCurve::P384 => &[Algorithm::ES384],
            _ => &[],
        },
        // HMAC (`oct`), EdDSA (`OKP`) and unknown key types are never used.
        _ => &[],
    };
    let usable = |alg: &Algorithm| by_type.contains(alg) && allowed.contains(alg);
    let alg_fits = match jwk.common.key_algorithm {
        None => by_type.iter().any(usable),
        Some(key_alg) => key_alg
            .to_string()
            .parse::<Algorithm>()
            .is_ok_and(|alg| usable(&alg)),
    };
    alg_fits && DecodingKey::from_jwk(jwk).is_ok()
}

#[cfg(test)]
mod tests;
