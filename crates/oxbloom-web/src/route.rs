use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use oxbloom_core::HttpMethod;

use crate::{IntoResponse, Request, Response};

/// A boxed, sendable future. The return type of a [`Handler`].
///
/// # Examples
///
/// ```
/// use oxbloom_web::BoxFuture;
///
/// let fut: BoxFuture<u8> = Box::pin(async { 7 });
/// # drop(fut);
/// ```
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Type-erased request handler. [`Route::new`] builds one from a closure.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, Route};
///
/// let route = Route::new(HttpMethod::Get, "/ping", |_req| async { "pong" });
/// let _handler = route.handler.clone();
/// ```
pub type Handler = Arc<dyn Fn(Request) -> BoxFuture<Response> + Send + Sync>;

/// A method + path pattern + handler. Path params use `{name}` syntax.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, Route};
///
/// let route = Route::new(HttpMethod::Delete, "/users/{id}", |_req| async {
///     http::StatusCode::NO_CONTENT
/// });
/// assert_eq!(route.method, HttpMethod::Delete);
/// assert_eq!(route.path, "/users/{id}");
/// ```
#[derive(Clone)]
pub struct Route {
    pub method: HttpMethod,
    pub path: String,
    pub handler: Handler,
}

impl Route {
    /// Builds a route from an async closure, without macros.
    ///
    /// The closure takes a [`Request`] and returns a future of anything that implements
    /// [`IntoResponse`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{HttpMethod, Request, Route};
    ///
    /// let ping = Route::new(HttpMethod::Get, "/ping", |_req| async { "pong" });
    /// let hello = Route::new(HttpMethod::Get, "/hello/{name}", |req: Request| async move {
    ///     format!("hello {}", req.param("name").unwrap_or("stranger"))
    /// });
    /// # let _ = (ping, hello);
    /// ```
    pub fn new<F, Fut, R>(method: HttpMethod, path: impl Into<String>, f: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoResponse,
    {
        Self {
            method,
            path: path.into(),
            handler: Arc::new(move |req| {
                let fut = f(req);
                Box::pin(async move { fut.await.into_response() })
            }),
        }
    }
}
