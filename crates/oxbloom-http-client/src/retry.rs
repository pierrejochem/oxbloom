//! [`RetryingClient`] over the retry policy of `oxbloom-tasks` (cargo feature `retry`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use http::{Method, StatusCode};
use oxbloom_tasks::RetryPolicy;

use crate::{HttpClient, HttpClientError, Request, Response};

/// The label of the retry events on target `oxbloom::tasks`.
const LABEL: &str = "http-client";

/// An [`HttpClient`] that runs a request again with a [`RetryPolicy`] when the inner client
/// fails with [`HttpClientError::Network`] or [`HttpClientError::Timeout`], or answers 502,
/// 503 or 504.
///
/// Only the idempotent methods `GET`, `HEAD`, `PUT`, `DELETE` and `OPTIONS` are retried; any
/// other method, `POST` included, is sent once. When the attempts run out on a retryable
/// status, the last response is returned, not an error. When they run out on an error, the
/// last error is returned. Every other error and status is returned at once.
///
/// # Examples
///
/// ```no_run
/// use std::sync::Arc;
/// use std::time::Duration;
/// use oxbloom_http_client::{HttpClient, Request, RetryingClient};
/// use oxbloom_tasks::RetryPolicy;
///
/// // `inner` is any client, for example a `ReqwestClient` (cargo feature `reqwest`).
/// async fn stock(inner: Arc<dyn HttpClient>) -> Result<(), oxbloom_http_client::HttpClientError> {
///     let policy = RetryPolicy {
///         max_attempts: 4,
///         initial: Duration::from_millis(200),
///         ..RetryPolicy::default()
///     };
///     let client = RetryingClient::new(inner, policy);
///     let response = client.execute(Request::get("https://api.example.test/stock")).await?;
///     # let _ = response;
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct RetryingClient {
    inner: Arc<dyn HttpClient>,
    policy: RetryPolicy,
}

impl RetryingClient {
    /// Wraps `inner` so that its retryable failures are retried with `policy`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response, RetryingClient};
    /// use oxbloom_tasks::{Jitter, RetryPolicy};
    ///
    /// struct AlwaysBusy;
    ///
    /// #[async_trait::async_trait]
    /// impl HttpClient for AlwaysBusy {
    ///     async fn execute(&self, _request: Request) -> Result<Response, HttpClientError> {
    ///         Ok(Response::new(http::StatusCode::SERVICE_UNAVAILABLE, http::HeaderMap::new(), Vec::new()))
    ///     }
    /// }
    ///
    /// let policy = RetryPolicy { max_attempts: 2, jitter: Jitter::None, ..RetryPolicy::default() };
    /// let client = RetryingClient::new(Arc::new(AlwaysBusy), policy);
    /// let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    /// // After the last attempt the 503 response itself is returned.
    /// let response = rt.block_on(client.execute(Request::get("https://api.example.test/")))?;
    /// assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    /// # Ok::<(), HttpClientError>(())
    /// ```
    pub fn new(inner: Arc<dyn HttpClient>, policy: RetryPolicy) -> Self {
        Self { inner, policy }
    }
}

/// The failure of one attempt, as `RetryPolicy::run` sees it: an error of the inner client,
/// or a response with a retryable status, carried so it can be returned after the last
/// attempt.
enum Attempt {
    Failed(HttpClientError),
    Status(Response),
}

impl Attempt {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Failed(error) => {
                matches!(error, HttpClientError::Network | HttpClientError::Timeout)
            }
            Self::Status(_) => true,
        }
    }
}

impl fmt::Display for Attempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(error) => error.fmt(f),
            Self::Status(_) => f.write_str("the HTTP response has a retryable status"),
        }
    }
}

/// Whether a request with `method` may be sent again without changing the outcome.
fn is_idempotent(method: &Method) -> bool {
    [
        Method::GET,
        Method::HEAD,
        Method::PUT,
        Method::DELETE,
        Method::OPTIONS,
    ]
    .contains(method)
}

/// Whether a response with `status` is worth another attempt.
fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
    )
}

#[async_trait]
impl HttpClient for RetryingClient {
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
        request.validate()?;
        if !is_idempotent(&request.method()) {
            return self.inner.execute(request).await;
        }
        let result = self
            .policy
            .run(
                LABEL,
                || {
                    let inner = Arc::clone(&self.inner);
                    let request = request.clone();
                    async move {
                        match inner.execute(request).await {
                            Ok(response) if is_retryable_status(response.status()) => {
                                Err(Attempt::Status(response))
                            }
                            Ok(response) => Ok(response),
                            Err(error) => Err(Attempt::Failed(error)),
                        }
                    }
                },
                Attempt::is_retryable,
            )
            .await;
        match result {
            Ok(response) | Err(Attempt::Status(response)) => Ok(response),
            Err(Attempt::Failed(error)) => Err(error),
        }
    }
}

impl fmt::Debug for RetryingClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryingClient")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}
