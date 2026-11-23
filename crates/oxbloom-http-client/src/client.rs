use async_trait::async_trait;

use crate::{HttpClientError, Request, Response};

/// Sends a [`Request`] and returns the [`Response`].
///
/// The trait is object safe, so applications hold an `Arc<dyn HttpClient>`. A status that is
/// not 2xx is returned as a normal response, not as an error.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response};
///
/// struct Stub;
///
/// #[async_trait::async_trait]
/// impl HttpClient for Stub {
///     async fn execute(&self, _request: Request) -> Result<Response, HttpClientError> {
///         Ok(Response::new(http::StatusCode::OK, http::HeaderMap::new(), b"ok".to_vec()))
///     }
/// }
///
/// let client: Arc<dyn HttpClient> = Arc::new(Stub);
/// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// let response = rt.block_on(client.execute(Request::get("https://api.example.test/")))?;
/// assert_eq!(response.text()?, "ok");
/// # Ok::<(), HttpClientError>(())
/// ```
#[async_trait]
pub trait HttpClient: Send + Sync {
    /// Sends the request.
    ///
    /// # Errors
    ///
    /// Returns an [`HttpClientError`] when the request cannot be completed.
    async fn execute(&self, request: Request) -> Result<Response, HttpClientError>;
}
