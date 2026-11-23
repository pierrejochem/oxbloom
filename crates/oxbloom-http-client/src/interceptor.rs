use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::{HttpClient, HttpClientError, Request, Response};

/// A hook that runs before a request is sent and may change it, for example to add an
/// `Authorization` header.
///
/// # Examples
///
/// ```
/// use oxbloom_http_client::{HttpClientError, Interceptor, Request};
///
/// struct Bearer(String);
///
/// #[async_trait::async_trait]
/// impl Interceptor for Bearer {
///     async fn before(&self, request: &mut Request) -> Result<(), HttpClientError> {
///         request.set_header("authorization", &format!("Bearer {}", self.0))
///     }
/// }
///
/// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// let mut request = Request::get("https://api.example.test/");
/// rt.block_on(Bearer("t0ken".into()).before(&mut request))?;
/// assert_eq!(request.headers()["authorization"], "Bearer t0ken");
/// # Ok::<(), HttpClientError>(())
/// ```
#[async_trait]
pub trait Interceptor: Send + Sync {
    /// Runs before the request is sent. An error stops the request and is returned to the
    /// caller.
    async fn before(&self, request: &mut Request) -> Result<(), HttpClientError>;
}

/// An [`HttpClient`] that runs its [`Interceptor`]s, in registration order, and then
/// delegates to an inner client.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_http_client::{
///     HttpClient, HttpClientError, InterceptedClient, Interceptor, Request, Response,
/// };
///
/// struct Stub;
///
/// #[async_trait::async_trait]
/// impl HttpClient for Stub {
///     async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
///         let seen = request.headers()["x-trace"].as_bytes().to_vec();
///         Ok(Response::new(http::StatusCode::OK, http::HeaderMap::new(), seen))
///     }
/// }
///
/// struct Trace;
///
/// #[async_trait::async_trait]
/// impl Interceptor for Trace {
///     async fn before(&self, request: &mut Request) -> Result<(), HttpClientError> {
///         request.set_header("x-trace", "on")
///     }
/// }
///
/// let client = InterceptedClient::new(Arc::new(Stub)).with(Arc::new(Trace));
/// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
/// let response = rt.block_on(client.execute(Request::get("https://api.example.test/")))?;
/// assert_eq!(response.text()?, "on");
/// # Ok::<(), HttpClientError>(())
/// ```
pub struct InterceptedClient {
    inner: Arc<dyn HttpClient>,
    interceptors: Vec<Arc<dyn Interceptor>>,
}

impl InterceptedClient {
    /// Wraps a client, with no interceptors yet.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_http_client::{HttpClient, HttpClientError, InterceptedClient, Request, Response};
    ///
    /// struct Stub;
    ///
    /// #[async_trait::async_trait]
    /// impl HttpClient for Stub {
    ///     async fn execute(&self, _: Request) -> Result<Response, HttpClientError> {
    ///         Ok(Response::new(http::StatusCode::OK, http::HeaderMap::new(), Vec::new()))
    ///     }
    /// }
    ///
    /// let client = InterceptedClient::new(Arc::new(Stub));
    /// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// let response = rt.block_on(client.execute(Request::get("https://api.example.test/")))?;
    /// assert!(response.status().is_success());
    /// # Ok::<(), HttpClientError>(())
    /// ```
    pub fn new(inner: Arc<dyn HttpClient>) -> Self {
        Self {
            inner,
            interceptors: Vec::new(),
        }
    }

    /// Adds an interceptor that runs after the ones added before it.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_http_client::{
    ///     HttpClient, HttpClientError, InterceptedClient, Interceptor, Request, Response,
    /// };
    ///
    /// struct Stub;
    ///
    /// #[async_trait::async_trait]
    /// impl HttpClient for Stub {
    ///     async fn execute(&self, _: Request) -> Result<Response, HttpClientError> {
    ///         Ok(Response::new(http::StatusCode::OK, http::HeaderMap::new(), Vec::new()))
    ///     }
    /// }
    ///
    /// struct Deny;
    ///
    /// #[async_trait::async_trait]
    /// impl Interceptor for Deny {
    ///     async fn before(&self, _: &mut Request) -> Result<(), HttpClientError> {
    ///         Err(HttpClientError::InvalidRequest)
    ///     }
    /// }
    ///
    /// let client = InterceptedClient::new(Arc::new(Stub)).with(Arc::new(Deny));
    /// let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// let result = rt.block_on(client.execute(Request::get("https://api.example.test/")));
    /// assert_eq!(result.unwrap_err(), HttpClientError::InvalidRequest);
    /// ```
    #[must_use]
    pub fn with(mut self, interceptor: Arc<dyn Interceptor>) -> Self {
        self.interceptors.push(interceptor);
        self
    }
}

impl fmt::Debug for InterceptedClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InterceptedClient")
            .field("interceptors", &self.interceptors.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl HttpClient for InterceptedClient {
    async fn execute(&self, mut request: Request) -> Result<Response, HttpClientError> {
        for interceptor in &self.interceptors {
            interceptor.before(&mut request).await?;
        }
        self.inner.execute(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpClient, HttpClientError, Request, Response};
    use std::sync::{Arc, Mutex};

    struct Stub(Arc<Mutex<Vec<String>>>);

    #[async_trait::async_trait]
    impl HttpClient for Stub {
        async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
            let order = request.headers()["x-order"].to_str().unwrap().to_owned();
            self.0.lock().unwrap().push(format!("sent {order}"));
            Ok(Response::new(
                http::StatusCode::OK,
                http::HeaderMap::new(),
                Vec::new(),
            ))
        }
    }

    struct Append(&'static str, bool);

    #[async_trait::async_trait]
    impl Interceptor for Append {
        async fn before(&self, request: &mut Request) -> Result<(), HttpClientError> {
            if self.1 {
                return Err(HttpClientError::Build);
            }
            let old = request
                .headers()
                .get("x-order")
                .map(|v| v.to_str().unwrap().to_owned())
                .unwrap_or_default();
            request.set_header("x-order", &format!("{old}{}", self.0))
        }
    }

    #[tokio::test]
    async fn interceptors_run_in_registration_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let client = InterceptedClient::new(Arc::new(Stub(log.clone())))
            .with(Arc::new(Append("a", false)))
            .with(Arc::new(Append("b", false)));
        client
            .execute(Request::get("https://x.test"))
            .await
            .unwrap();
        assert_eq!(*log.lock().unwrap(), vec!["sent ab".to_owned()]);
    }

    #[tokio::test]
    async fn an_interceptor_error_stops_the_request() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let client = InterceptedClient::new(Arc::new(Stub(log.clone())))
            .with(Arc::new(Append("a", false)))
            .with(Arc::new(Append("b", true)));
        let error = client
            .execute(Request::get("https://x.test"))
            .await
            .unwrap_err();
        assert_eq!(error, HttpClientError::Build);
        assert!(log.lock().unwrap().is_empty());
    }
}
