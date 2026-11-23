//! [`JwksFetcher`] over `oxbloom-http-client` (cargo feature `jwks`, rustls only).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use oxbloom_http_client::{ClientBuilder, HttpClient, HttpClientError, Request};

use crate::{FetchError, JwksFetcher};

/// Largest accepted response body: 1 MiB.
const MAX_BODY_BYTES: usize = 1_048_576;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Fetches a JWKS over HTTP(S) through an `oxbloom_http_client::HttpClient` (by default a `ReqwestClient` with rustls and the platform
/// certificate verifier), or
/// with a given rustls client configuration ([`ReqwestJwksFetcher::with_tls`]).
///
/// Limits: the whole request (connect, headers and body) must finish within the timeout
/// (default 5 seconds); redirects are never followed, so a 3xx answer is
/// [`FetchError::BadStatus`]; a non-2xx status is [`FetchError::BadStatus`]; the body is read
/// incrementally and the fetch fails with [`FetchError::TooLarge`] as soon as it exceeds
/// 1 MiB (a larger `Content-Length` is refused before reading). No cookies, credentials or
/// `Referer` are sent. Errors carry fixed text only, never the body, URL or headers.
///
/// Proxies: the `reqwest` default of the built client is kept, so the `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and
/// `NO_PROXY` environment variables are honored.
///
/// # Examples
///
/// ```no_run
/// use std::sync::Arc;
/// use oxbloom_security::{JwksSource, ReqwestJwksFetcher};
///
/// let fetcher = ReqwestJwksFetcher::new()?;
/// let source = JwksSource::builder(Arc::new(fetcher), "https://issuer.example/jwks.json")
///     .build()?;
/// # let _ = source;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct ReqwestJwksFetcher {
    client: Arc<dyn HttpClient>,
    /// The outer deadline of a fetch; `None` when the caller's client decides.
    timeout: Option<Duration>,
}

impl ReqwestJwksFetcher {
    /// A fetcher with the default 5 second timeout.
    ///
    /// # Errors
    ///
    /// [`FetchError::Network`] if the HTTP client cannot be built (e.g. no usable TLS roots).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::ReqwestJwksFetcher;
    ///
    /// let fetcher = ReqwestJwksFetcher::new();
    /// assert!(fetcher.is_ok());
    /// ```
    pub fn new() -> Result<Self, FetchError> {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }

    /// A fetcher whose requests must finish, body included, within `timeout`.
    ///
    /// # Errors
    ///
    /// [`FetchError::Network`] if the HTTP client cannot be built.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::ReqwestJwksFetcher;
    ///
    /// let fetcher = ReqwestJwksFetcher::with_timeout(Duration::from_secs(2));
    /// assert!(fetcher.is_ok());
    /// ```
    pub fn with_timeout(timeout: Duration) -> Result<Self, FetchError> {
        Self::build(ClientBuilder::new(), timeout)
    }

    /// A fetcher over a client the caller already has, for example the application's shared
    /// `HttpClient` bean.
    ///
    /// The caller's client then decides the timeout, redirects, proxies, TLS and its own body
    /// limit; the fetcher adds no deadline of its own. It still refuses a body over 1 MiB
    /// itself (by checking `Response::bytes().len()`, so the answer is
    /// [`FetchError::TooLarge`]) and treats any non-2xx status, a 3xx answer included, as
    /// [`FetchError::BadStatus`]. A client that follows redirects therefore follows them for
    /// the fetch too.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_http_client::{ClientBuilder, HttpClient};
    /// use oxbloom_security::ReqwestJwksFetcher;
    ///
    /// let client: Arc<dyn HttpClient> = Arc::new(ClientBuilder::new().build()?);
    /// let fetcher = ReqwestJwksFetcher::with_client(client);
    /// # let _ = fetcher;
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    #[must_use]
    pub fn with_client(client: Arc<dyn HttpClient>) -> Self {
        Self {
            client,
            timeout: None,
        }
    }

    /// A fetcher that verifies the server with `client_config` instead of the platform
    /// certificate verifier, with the default 5 second timeout.
    ///
    /// The client configuration decides which CAs are trusted and whether a client
    /// certificate is sent, for example one built from an SSL bundle with
    /// `oxbloom_ssl::SslBundle::client_config`. Everything else is as in
    /// [`ReqwestJwksFetcher::new`]: no redirects, no `Referer`, the same timeout and the same
    /// 1 MiB body limit. ALPN is left as `client_config` sets it.
    ///
    /// Trust follows `client_config` alone: a configuration with its own root store trusts only
    /// those CAs, so a server certificate from a public CA is then rejected. A configuration
    /// from an SSL bundle without a trust store keeps the platform verifier and only adds the
    /// bundle's client certificate.
    ///
    /// The HTTP client is built once, here, from the configuration as it is now. A fetcher
    /// built from an SSL bundle therefore does not follow a later reload of the bundle's
    /// files; build a new fetcher (in an application: restart it) to use new certificates.
    ///
    /// # Errors
    ///
    /// [`FetchError::Network`] if the HTTP client cannot be built.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::sync::Arc;
    /// use oxbloom_security::{JwksSource, ReqwestJwksFetcher};
    /// use rustls::{ClientConfig, RootCertStore};
    ///
    /// # fn corporate_ca() -> rustls::pki_types::CertificateDer<'static> { unimplemented!() }
    /// // Trust only the corporate CA that signed the issuer's certificate.
    /// let mut roots = RootCertStore::empty();
    /// roots.add(corporate_ca())?;
    /// let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    /// let client_config = ClientConfig::builder_with_provider(provider)
    ///     .with_safe_default_protocol_versions()?
    ///     .with_root_certificates(roots)
    ///     .with_no_client_auth();
    ///
    /// let fetcher = ReqwestJwksFetcher::with_tls(Arc::new(client_config))?;
    /// let source = JwksSource::builder(Arc::new(fetcher), "https://issuer.corp.example/jwks.json")
    ///     .build()?;
    /// # let _ = source;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_tls(client_config: Arc<rustls::ClientConfig>) -> Result<Self, FetchError> {
        Self::build(ClientBuilder::new().tls(client_config), DEFAULT_TIMEOUT)
    }

    /// Applies the settings every built fetcher shares and builds the client.
    fn build(builder: ClientBuilder, timeout: Duration) -> Result<Self, FetchError> {
        let client = builder
            .timeout(timeout)
            .max_body_bytes(MAX_BODY_BYTES)
            .follow_redirects(0)
            .build()
            .map_err(|_| FetchError::Network)?;
        Ok(Self {
            client: Arc::new(client),
            timeout: Some(timeout),
        })
    }

    async fn fetch_inner(&self, uri: &str) -> Result<JwkSet, FetchError> {
        let request = Request::get(uri).header("accept", "application/json");
        let response = self.client.execute(request).await.map_err(map_error)?;
        if !response.status().is_success() {
            return Err(FetchError::BadStatus);
        }
        if response.bytes().len() > MAX_BODY_BYTES {
            return Err(FetchError::TooLarge);
        }
        serde_json::from_slice(response.bytes()).map_err(|_| FetchError::Malformed)
    }
}

fn map_error(err: HttpClientError) -> FetchError {
    match err {
        HttpClientError::Timeout => FetchError::Timeout,
        HttpClientError::TooLarge => FetchError::TooLarge,
        HttpClientError::BadStatus(_) => FetchError::BadStatus,
        _ => FetchError::Network,
    }
}

#[async_trait]
impl JwksFetcher for ReqwestJwksFetcher {
    async fn fetch(&self, uri: &str) -> Result<JwkSet, FetchError> {
        match self.timeout {
            // The client's own timeout covers the whole exchange; this outer deadline is a
            // backstop so a slow-drip body can never outlive it.
            Some(timeout) => tokio::time::timeout(timeout, self.fetch_inner(uri))
                .await
                .unwrap_or(Err(FetchError::Timeout)),
            None => self.fetch_inner(uri).await,
        }
    }
}

impl fmt::Debug for ReqwestJwksFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestJwksFetcher")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use oxbloom_web::{App, HttpMethod, Route};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use crate::{FetchError, JwksFetcher, JwksSource};

    const MARKER: &str = "SECRET-MARKER-9f3a";
    const MAX: usize = 1_048_576;

    fn jwks_json() -> String {
        r#"{"keys":[{"kty":"oct","kid":"k1","k":"c2VjcmV0"}]}"#.to_owned()
    }

    /// Serves `app` on a loopback port and returns its base URL.
    async fn spawn(app: App) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(app.serve(listener, std::future::pending::<()>()));
        format!("http://{addr}")
    }

    async fn serve_body(status: u16, body: Vec<u8>) -> String {
        let route = Route::new(HttpMethod::Get, "/jwks", move |_| {
            let body = body.clone();
            async move {
                let mut res = http::Response::new(body.into());
                *res.status_mut() = http::StatusCode::from_u16(status).unwrap();
                res
            }
        });
        spawn(App::new().route(route).unwrap()).await + "/jwks"
    }

    /// A JSON body of exactly `len` bytes: a valid key set padded with whitespace.
    fn padded(len: usize) -> Vec<u8> {
        let mut body = jwks_json().into_bytes();
        body.resize(len, b' ');
        body
    }

    #[tokio::test]
    async fn fetches_and_parses_jwks() {
        let uri = serve_body(200, jwks_json().into_bytes()).await;
        let fetcher = Arc::new(ReqwestJwksFetcher::new().unwrap());
        let set = fetcher.fetch(&uri).await.unwrap();
        assert_eq!(set.keys.len(), 1);
        // And through a JwksSource over plain http.
        let source = JwksSource::builder(fetcher, uri)
            .allow_insecure_http()
            .build();
        assert!(source.is_ok());
    }

    #[tokio::test]
    async fn non_2xx_is_bad_status() {
        for status in [404, 500, 401] {
            let uri = serve_body(status, jwks_json().into_bytes()).await;
            let err = ReqwestJwksFetcher::new().unwrap().fetch(&uri).await;
            assert_eq!(err.unwrap_err(), FetchError::BadStatus, "status {status}");
        }
    }

    #[tokio::test]
    async fn oversized_body_is_too_large() {
        let uri = serve_body(200, padded(MAX + 1)).await;
        let err = ReqwestJwksFetcher::new().unwrap().fetch(&uri).await;
        assert_eq!(err.unwrap_err(), FetchError::TooLarge);
    }

    #[tokio::test]
    async fn oversized_chunked_body_without_content_length_is_too_large() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = s.read(&mut request).await;
            let head =
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(head).await;
            let chunk = vec![b' '; 65_536];
            for _ in 0..40 {
                let _ = s.write_all(b"10000\r\n").await;
                if s.write_all(&chunk).await.is_err() || s.write_all(b"\r\n").await.is_err() {
                    return;
                }
            }
        });
        let err = ReqwestJwksFetcher::new()
            .unwrap()
            .fetch(&format!("http://{addr}/jwks"))
            .await;
        assert_eq!(err.unwrap_err(), FetchError::TooLarge);
    }

    #[tokio::test]
    async fn exactly_one_mib_body_is_accepted() {
        let uri = serve_body(200, padded(MAX)).await;
        let set = ReqwestJwksFetcher::new().unwrap().fetch(&uri).await;
        assert_eq!(set.unwrap().keys.len(), 1);
    }

    #[tokio::test]
    async fn slow_server_times_out() {
        // Never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(s);
        });
        let fetcher = ReqwestJwksFetcher::with_timeout(Duration::from_millis(200)).unwrap();
        let err = fetcher.fetch(&format!("http://{addr}/jwks")).await;
        assert_eq!(err.unwrap_err(), FetchError::Timeout);
    }

    #[tokio::test]
    async fn slow_drip_body_cannot_exceed_total_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = s.read(&mut request).await;
            let head = b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\n";
            let _ = s.write_all(head).await;
            loop {
                if s.write_all(b" ").await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let fetcher = ReqwestJwksFetcher::with_timeout(Duration::from_millis(400)).unwrap();
        let started = std::time::Instant::now();
        let err = fetcher.fetch(&format!("http://{addr}/jwks")).await;
        assert_eq!(err.unwrap_err(), FetchError::Timeout);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&hits);
        let app = App::new()
            .route(Route::new(HttpMethod::Get, "/jwks", |_| async {
                let mut res = http::Response::new(Vec::new().into());
                *res.status_mut() = http::StatusCode::FOUND;
                res.headers_mut()
                    .insert(http::header::LOCATION, "/other".parse().unwrap());
                res
            }))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/other", move |_| {
                let seen = Arc::clone(&seen);
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    jwks_json()
                }
            }))
            .unwrap();
        let uri = spawn(app).await + "/jwks";
        let err = ReqwestJwksFetcher::new().unwrap().fetch(&uri).await;
        assert_eq!(err.unwrap_err(), FetchError::BadStatus);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn malformed_json_is_malformed() {
        for body in ["not json", "{\"keys\": 5}", "[]", ""] {
            let uri = serve_body(200, body.as_bytes().to_vec()).await;
            let err = ReqwestJwksFetcher::new().unwrap().fetch(&uri).await;
            assert_eq!(err.unwrap_err(), FetchError::Malformed, "body {body:?}");
        }
    }

    #[tokio::test]
    async fn connection_refused_is_network() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let err = ReqwestJwksFetcher::new()
            .unwrap()
            .fetch(&format!("http://{addr}/jwks"))
            .await;
        assert_eq!(err.unwrap_err(), FetchError::Network);
    }

    #[tokio::test]
    async fn errors_do_not_contain_body() {
        let bodies = [
            format!("{{\"keys\":\"{MARKER}\"}}"),
            format!("{MARKER} not json"),
        ];
        for body in bodies {
            let uri = serve_body(200, body.into_bytes()).await;
            let err = ReqwestJwksFetcher::new()
                .unwrap()
                .fetch(&uri)
                .await
                .unwrap_err();
            assert!(!err.to_string().contains(MARKER));
            assert!(!format!("{err:?}").contains(MARKER));
        }
        let uri = serve_body(500, MARKER.as_bytes().to_vec()).await;
        let err = ReqwestJwksFetcher::new()
            .unwrap()
            .fetch(&uri)
            .await
            .unwrap_err();
        assert!(!err.to_string().contains(MARKER));
        assert!(!format!("{err:?}").contains(MARKER));
    }
}
