use std::sync::Arc;
use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use oxbloom_tasks::{Backoff, Jitter, RetryPolicy};
use serde::Deserialize;

use crate::{ClientBuilder, HttpClient, HttpClientError, RetryingClient};

const TIMEOUT: &str = "http.client.timeout";
const CONNECT_TIMEOUT: &str = "http.client.connect-timeout";
const MAX_BODY_BYTES: &str = "http.client.max-body-bytes";
const USER_AGENT: &str = "http.client.user-agent";
const SSL_BUNDLE: &str = "http.client.ssl-bundle";
const MAX_ATTEMPTS: &str = "http.client.retry.max-attempts";
const BACKOFF: &str = "http.client.retry.backoff";
const INITIAL: &str = "http.client.retry.initial";
const MULTIPLIER: &str = "http.client.retry.multiplier";
const MAX_DELAY: &str = "http.client.retry.max-delay";
const JITTER: &str = "http.client.retry.jitter";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Raw {
    timeout: Option<Duration>,
    connect_timeout: Option<Duration>,
    max_body_bytes: Option<usize>,
    follow_redirects: Option<usize>,
    user_agent: Option<String>,
    ssl_bundle: Option<String>,
    retry: RawRetry,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRetry {
    max_attempts: Option<u32>,
    backoff: Option<String>,
    initial: Option<Duration>,
    multiplier: Option<f64>,
    max_delay: Option<Duration>,
    jitter: Option<String>,
}

/// The settings of the `http.client` keys: the limits of the [`ReqwestClient`](crate::ReqwestClient),
/// the name of an SSL bundle and the retry policy.
///
/// Read with [`from_config`](Self::from_config) and turned into a client with
/// [`build`](Self::build). The keys are `http.client.timeout` (default 10 seconds),
/// `connect-timeout` (default none), `max-body-bytes` (default 1 MiB), `follow-redirects`
/// (default 0), `user-agent`, `ssl-bundle` and `retry.max-attempts`, `retry.backoff`,
/// `retry.initial`, `retry.multiplier`, `retry.max-delay` and `retry.jitter`. The `retry`
/// keys are the fields of [`RetryPolicy`] with its defaults. Durations take the form of the
/// other duration keys, such as `250ms` or `2s`.
///
/// Available with the cargo feature `config`.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_config::Config;
/// use oxbloom_http_client::HttpClientSettings;
///
/// let config = Config::from_str("[http.client]\ntimeout = \"3s\"\nmax-body-bytes = 4096")?;
/// let settings = HttpClientSettings::from_config(&config)?;
/// assert_eq!(settings.timeout, Duration::from_secs(3));
/// assert_eq!(settings.max_body_bytes, 4096);
/// assert!(settings.retry.is_none());
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct HttpClientSettings {
    /// The total timeout of a request. Default 10 seconds.
    pub timeout: Duration,
    /// The timeout of connecting. Default none.
    pub connect_timeout: Option<Duration>,
    /// The size limit of a response body in bytes. Default 1 MiB.
    pub max_body_bytes: usize,
    /// How many redirects are followed. Default 0.
    pub follow_redirects: usize,
    /// The `User-Agent` header. Default none.
    pub user_agent: Option<String>,
    /// The name of the SSL bundle whose client configuration is used. Default none. The
    /// caller resolves the name and passes the result to [`build`](Self::build).
    pub ssl_bundle: Option<String>,
    /// The retry policy, set when `retry.max-attempts` is configured. [`build`](Self::build)
    /// retries only when its `max_attempts` is above 1.
    pub retry: Option<RetryPolicy>,
}

impl Default for HttpClientSettings {
    /// The defaults of every key.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_http_client::HttpClientSettings;
    ///
    /// let settings = HttpClientSettings::default();
    /// assert_eq!(settings.timeout, Duration::from_secs(10));
    /// assert_eq!(settings.max_body_bytes, 1_048_576);
    /// assert_eq!(settings.follow_redirects, 0);
    /// ```
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            connect_timeout: None,
            max_body_bytes: 1_048_576,
            follow_redirects: 0,
            user_agent: None,
            ssl_bundle: None,
            retry: None,
        }
    }
}

impl HttpClientSettings {
    /// Reads the settings from the `http.client` section of `config`. A missing section gives
    /// [`HttpClientSettings::default`].
    ///
    /// The `retry` field is set only when `retry.max-attempts` is configured; the other
    /// `retry` keys are still checked, and are used only together with it.
    ///
    /// # Errors
    ///
    /// Fails with a [`ConfigError`] naming the full key for an unknown key under
    /// `http.client` or `http.client.retry`, a value of the wrong type, a bad duration, a
    /// zero `timeout`, `max-body-bytes`, an empty `ssl-bundle`, an empty `user-agent` or one
    /// that is not a valid header value, a
    /// `max-attempts` of zero, a `multiplier` below 1.0 or not finite, an `initial` above
    /// `max-delay`, and an unknown `backoff` or `jitter`. The error never shows the value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_http_client::HttpClientSettings;
    ///
    /// let config = Config::from_str("[http.client.retry]\nmax-attempts = 4\ninitial = \"50ms\"")?;
    /// let settings = HttpClientSettings::from_config(&config)?;
    /// assert_eq!(settings.retry.unwrap().max_attempts, 4);
    ///
    /// let config = Config::from_str("[http.client]\ntimeout = \"soon\"")?;
    /// let err = HttpClientSettings::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("http.client.timeout"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let raw = match config.get::<Raw>("http.client") {
            Ok(raw) => raw,
            Err(ConfigError::Missing { .. }) => Raw::default(),
            Err(e) => return Err(e),
        };
        let defaults = Self::default();
        let timeout = match raw.timeout {
            Some(d) if d.is_zero() => {
                return Err(ConfigError::invalid(TIMEOUT, "must be above zero"));
            }
            Some(d) => d,
            None => defaults.timeout,
        };
        if raw.connect_timeout.is_some_and(|d| d.is_zero()) {
            return Err(ConfigError::invalid(CONNECT_TIMEOUT, "must be above zero"));
        }
        let max_body_bytes = match raw.max_body_bytes {
            Some(0) => return Err(ConfigError::invalid(MAX_BODY_BYTES, "must be at least 1")),
            Some(n) => n,
            None => defaults.max_body_bytes,
        };
        if raw.user_agent.as_deref().is_some_and(str::is_empty) {
            return Err(ConfigError::invalid(USER_AGENT, "must not be empty"));
        }
        if raw
            .user_agent
            .as_deref()
            .is_some_and(|agent| http::HeaderValue::from_str(agent).is_err())
        {
            return Err(ConfigError::invalid(
                USER_AGENT,
                "must be a valid header value (visible ASCII, no line breaks)",
            ));
        }
        if raw.ssl_bundle.as_deref().is_some_and(str::is_empty) {
            return Err(ConfigError::invalid(SSL_BUNDLE, "must not be empty"));
        }
        Ok(Self {
            timeout,
            connect_timeout: raw.connect_timeout,
            max_body_bytes,
            follow_redirects: raw.follow_redirects.unwrap_or(0),
            user_agent: raw.user_agent,
            ssl_bundle: raw.ssl_bundle,
            retry: retry_policy(raw.retry)?,
        })
    }

    /// Builds the client: a [`ReqwestClient`](crate::ReqwestClient) with these settings,
    /// wrapped in a [`RetryingClient`] when `retry.max-attempts` is set and above 1.
    ///
    /// `tls` is the client configuration of the SSL bundle that `ssl-bundle` names, or `None`
    /// for the platform roots; the caller resolves the bundle.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::Build`] when the client cannot be built, for example for a
    /// `User-Agent` value that is not a valid header value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_http_client::HttpClientSettings;
    ///
    /// let config = Config::from_str("[http.client]\nuser-agent = \"demo/1\"")?;
    /// let client = HttpClientSettings::from_config(&config)?.build(None)?;
    /// # let _ = client;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(
        &self,
        tls: Option<Arc<rustls::ClientConfig>>,
    ) -> Result<Arc<dyn HttpClient>, HttpClientError> {
        let mut builder = ClientBuilder::new()
            .timeout(self.timeout)
            .max_body_bytes(self.max_body_bytes)
            .follow_redirects(self.follow_redirects);
        if let Some(timeout) = self.connect_timeout {
            builder = builder.connect_timeout(timeout);
        }
        if let Some(agent) = &self.user_agent {
            builder = builder.user_agent(agent);
        }
        if let Some(tls) = tls {
            builder = builder.tls(tls);
        }
        let client: Arc<dyn HttpClient> = Arc::new(builder.build()?);
        Ok(match &self.retry {
            Some(policy) if policy.max_attempts > 1 => {
                Arc::new(RetryingClient::new(client, policy.clone()))
            }
            _ => client,
        })
    }
}

fn retry_policy(raw: RawRetry) -> Result<Option<RetryPolicy>, ConfigError> {
    let defaults = RetryPolicy::default();
    let max_attempts = match raw.max_attempts {
        Some(0) => return Err(ConfigError::invalid(MAX_ATTEMPTS, "must be at least 1")),
        other => other,
    };
    let backoff = match raw.backoff {
        Some(name) => name
            .parse::<Backoff>()
            .map_err(|_| ConfigError::invalid(BACKOFF, "must be `fixed` or `exponential`"))?,
        None => defaults.backoff,
    };
    let jitter = match raw.jitter {
        Some(name) => name
            .parse::<Jitter>()
            .map_err(|_| ConfigError::invalid(JITTER, "must be `none`, `full` or `equal`"))?,
        None => defaults.jitter,
    };
    let multiplier = match raw.multiplier {
        Some(m) if !m.is_finite() || m < 1.0 => {
            return Err(ConfigError::invalid(
                MULTIPLIER,
                "must be a finite number of at least 1.0",
            ));
        }
        Some(m) => m,
        None => defaults.multiplier,
    };
    let initial = raw.initial.unwrap_or(defaults.initial);
    let max_delay = raw.max_delay.unwrap_or(defaults.max_delay);
    if initial > max_delay {
        // Name the key the user set: `max-delay` when `initial` is the default.
        let key = if raw.initial.is_some() {
            INITIAL
        } else {
            MAX_DELAY
        };
        return Err(ConfigError::invalid(
            key,
            "initial must not be greater than max-delay",
        ));
    }
    Ok(max_attempts.map(|max_attempts| RetryPolicy {
        max_attempts,
        backoff,
        initial,
        multiplier,
        max_delay,
        jitter,
    }))
}
