//! [`ReqwestClient`] and its [`ClientBuilder`] (cargo feature `reqwest`, rustls only).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use http::HeaderValue;
use http::header::CONTENT_TYPE;

use crate::{Body, HttpClient, HttpClientError, Request, Response};

/// The default total timeout of a request: 10 seconds.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// The default largest accepted response body: 1 MiB.
const DEFAULT_MAX_BODY_BYTES: usize = 1_048_576;

/// The settings of a [`ReqwestClient`].
///
/// The defaults are a total timeout of 10 seconds, no separate connect timeout, a body limit
/// of 1 MiB, no redirects (a 3xx answer is returned as a response), no `User-Agent` header,
/// and the platform certificate verifier.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_http_client::ClientBuilder;
///
/// let client = ClientBuilder::new()
///     .timeout(Duration::from_secs(5))
///     .connect_timeout(Duration::from_secs(2))
///     .max_body_bytes(64 * 1024)
///     .follow_redirects(3)
///     .user_agent("inventory-service/1.4")
///     .build()?;
/// # let _ = client;
/// # Ok::<(), oxbloom_http_client::HttpClientError>(())
/// ```
#[derive(Clone)]
pub struct ClientBuilder {
    timeout: Duration,
    connect_timeout: Option<Duration>,
    max_body_bytes: usize,
    follow_redirects: usize,
    user_agent: Option<String>,
    tls: Option<Arc<rustls::ClientConfig>>,
}

impl Default for ClientBuilder {
    /// The same as [`ClientBuilder::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::ClientBuilder;
    ///
    /// let client = ClientBuilder::default().build()?;
    /// # let _ = client;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    fn default() -> Self {
        Self::new()
    }
}

impl ClientBuilder {
    /// Creates a builder with the default settings.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::ClientBuilder;
    ///
    /// let client = ClientBuilder::new().build()?;
    /// # let _ = client;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    pub fn new() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            connect_timeout: None,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            follow_redirects: 0,
            user_agent: None,
            tls: None,
        }
    }

    /// Sets the total timeout of a request: connecting, sending, the response headers and
    /// the whole body must finish within it. The default is 10 seconds. A timeout set on a
    /// [`Request`] replaces it for that request.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_http_client::ClientBuilder;
    ///
    /// let client = ClientBuilder::new().timeout(Duration::from_secs(3)).build()?;
    /// # let _ = client;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets a separate timeout for opening the connection. Without it, only the total
    /// timeout applies.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_http_client::ClientBuilder;
    ///
    /// let client = ClientBuilder::new().connect_timeout(Duration::from_secs(1)).build()?;
    /// # let _ = client;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = Some(timeout);
        self
    }

    /// Sets the largest accepted response body in bytes. The default is 1 MiB.
    ///
    /// A response whose `Content-Length` is larger is refused with
    /// [`HttpClientError::TooLarge`] before its body is read. Without a `Content-Length`,
    /// reading stops with the same error as soon as the limit is exceeded.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::ClientBuilder;
    ///
    /// let client = ClientBuilder::new().max_body_bytes(16 * 1024).build()?;
    /// # let _ = client;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    #[must_use]
    pub fn max_body_bytes(mut self, max: usize) -> Self {
        self.max_body_bytes = max;
        self
    }

    /// Follows up to `max` redirects. The default is 0: a 3xx answer is returned as the
    /// response. When a request would need more than `max` redirects, the last 3xx answer
    /// is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::ClientBuilder;
    ///
    /// let client = ClientBuilder::new().follow_redirects(5).build()?;
    /// # let _ = client;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    #[must_use]
    pub fn follow_redirects(mut self, max: usize) -> Self {
        self.follow_redirects = max;
        self
    }

    /// Sends this `User-Agent` header with every request. By default none is sent.
    ///
    /// A value that is not a valid header value makes [`ClientBuilder::build`] fail.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::ClientBuilder;
    ///
    /// let client = ClientBuilder::new().user_agent("billing/2.0").build()?;
    /// # let _ = client;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    #[must_use]
    pub fn user_agent(mut self, user_agent: &str) -> Self {
        self.user_agent = Some(user_agent.to_owned());
        self
    }

    /// Verifies servers with `client_config` instead of the platform certificate verifier.
    ///
    /// The configuration decides which CAs are trusted and whether a client certificate is
    /// sent, for example one built from an SSL bundle with
    /// `oxbloom_ssl::SslBundle::client_config`. A configuration with its own root store
    /// trusts only those CAs. Build it with the `aws-lc-rs` provider; the client is built
    /// once from the configuration as it is at [`ClientBuilder::build`].
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use oxbloom_http_client::ClientBuilder;
    /// use rustls::{ClientConfig, RootCertStore};
    ///
    /// # fn corporate_ca() -> rustls::pki_types::CertificateDer<'static> { unimplemented!() }
    /// // Trust only the corporate CA.
    /// let mut roots = RootCertStore::empty();
    /// roots.add(corporate_ca())?;
    /// let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    /// let client_config = ClientConfig::builder_with_provider(provider)
    ///     .with_safe_default_protocol_versions()?
    ///     .with_root_certificates(roots)
    ///     .with_no_client_auth();
    ///
    /// let client = ClientBuilder::new().tls(Arc::new(client_config)).build()?;
    /// # let _ = client;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn tls(mut self, client_config: Arc<rustls::ClientConfig>) -> Self {
        self.tls = Some(client_config);
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::Build`] when the settings cannot be applied, for example
    /// for a bad `User-Agent` value or when no usable TLS roots are found.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::{ClientBuilder, HttpClientError};
    ///
    /// assert!(ClientBuilder::new().build().is_ok());
    /// assert_eq!(
    ///     ClientBuilder::new().user_agent("bad\nvalue").build().unwrap_err(),
    ///     HttpClientError::Build,
    /// );
    /// ```
    pub fn build(self) -> Result<ReqwestClient, HttpClientError> {
        let mut builder = reqwest::Client::builder();
        if let Some(config) = self.tls {
            // reqwest takes the configuration by value; the clone shares the root store, the
            // verifier and the certificate resolver through their `Arc`s.
            builder = builder.tls_backend_preconfigured(Arc::unwrap_or_clone(config));
        }
        let max = self.follow_redirects;
        let redirect = if max == 0 {
            reqwest::redirect::Policy::none()
        } else {
            // `previous` holds the URLs requested so far, the first one included. Stopping
            // returns the 3xx answer instead of an error.
            reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() > max {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            })
        };
        builder = builder
            .redirect(redirect)
            .timeout(self.timeout)
            .referer(false);
        if let Some(connect) = self.connect_timeout {
            builder = builder.connect_timeout(connect);
        }
        if let Some(user_agent) = self.user_agent {
            builder = builder.user_agent(user_agent);
        }
        let client = builder.build().map_err(|_| HttpClientError::Build)?;
        Ok(ReqwestClient {
            client,
            timeout: self.timeout,
            max_body_bytes: self.max_body_bytes,
        })
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("timeout", &self.timeout)
            .field("connect_timeout", &self.connect_timeout)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("follow_redirects", &self.follow_redirects)
            .field("custom_tls", &self.tls.is_some())
            .finish_non_exhaustive()
    }
}

/// An [`HttpClient`] over `reqwest` with rustls (`aws-lc-rs`, never `ring`), built with a
/// [`ClientBuilder`].
///
/// Behavior: the `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` environment
/// variables are honored (read when the client is built); there is no cookie store; no
/// `Referer` header is sent; the body is read incrementally within the size limit; a
/// timeout is [`HttpClientError::Timeout`]; a bad URL is
/// [`HttpClientError::InvalidRequest`]; every other transport failure is
/// [`HttpClientError::Network`]. Clones share one connection pool.
///
/// # Examples
///
/// ```no_run
/// use oxbloom_http_client::{ClientBuilder, HttpClient, Request};
///
/// # async fn demo() -> Result<(), oxbloom_http_client::HttpClientError> {
/// let client = ClientBuilder::new().build()?;
/// let response = client
///     .execute(Request::get("https://api.example.test/status").header("accept", "application/json"))
///     .await?
///     .error_for_status()?;
/// let status: serde_json::Value = response.json()?;
/// # let _ = status;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct ReqwestClient {
    client: reqwest::Client,
    timeout: Duration,
    max_body_bytes: usize,
}

impl ReqwestClient {
    /// Sends the request and reads the response within the size limit.
    async fn send(&self, request: Request, timeout: Duration) -> Result<Response, HttpClientError> {
        let mut builder = self
            .client
            .request(request.method(), request.url())
            .headers(request.headers().clone())
            .timeout(timeout);
        if let Some((bytes, content_type)) = encode(request.body_ref())? {
            if let Some(content_type) = content_type
                && !request.headers().contains_key(CONTENT_TYPE)
            {
                builder = builder.header(CONTENT_TYPE, HeaderValue::from_static(content_type));
            }
            builder = builder.body(bytes);
        }
        let mut response = builder.send().await.map_err(map_error)?;
        if response
            .content_length()
            .is_some_and(|len| len > self.max_body_bytes as u64)
        {
            return Err(HttpClientError::TooLarge);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(map_error)? {
            if body.len() + chunk.len() > self.max_body_bytes {
                return Err(HttpClientError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response::new(
            response.status(),
            response.headers().clone(),
            body,
        ))
    }
}

/// The bytes of an encoded body and the content type that goes with them, if any.
type Encoded = (Vec<u8>, Option<&'static str>);

/// Encodes a body, or returns `None` for no body.
fn encode(body: &Body) -> Result<Option<Encoded>, HttpClientError> {
    Ok(match body {
        Body::Empty => None,
        Body::Bytes(bytes) => Some((bytes.clone(), None)),
        Body::Form(pairs) => {
            let encoded = form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish();
            Some((
                encoded.into_bytes(),
                Some("application/x-www-form-urlencoded"),
            ))
        }
        #[cfg(feature = "json")]
        Body::Json(value) => Some((
            serde_json::to_vec(value).map_err(|_| HttpClientError::InvalidRequest)?,
            Some("application/json"),
        )),
    })
}

/// Maps a `reqwest` error to fixed text: a timeout, a request that could not be built (for
/// example a bad URL), or any other transport failure.
fn map_error(error: reqwest::Error) -> HttpClientError {
    if error.is_timeout() {
        HttpClientError::Timeout
    } else if error.is_builder() {
        HttpClientError::InvalidRequest
    } else {
        HttpClientError::Network
    }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        request.validate()?;
        let timeout = request.timeout_value().unwrap_or(self.timeout);
        // reqwest's own timeout covers the whole exchange; this outer deadline is a backstop
        // so a slow-drip body can never outlive it.
        tokio::time::timeout(timeout, self.send(request, timeout))
            .await
            .unwrap_or(Err(HttpClientError::Timeout))
    }
}

impl fmt::Debug for ReqwestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestClient")
            .field("timeout", &self.timeout)
            .field("max_body_bytes", &self.max_body_bytes)
            .finish_non_exhaustive()
    }
}
