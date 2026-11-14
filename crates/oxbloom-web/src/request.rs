use std::collections::HashMap;

use bytes::Bytes;
use http::HeaderMap;
use oxbloom_core::HttpMethod;

/// An incoming HTTP request as seen by handlers.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, Request};
///
/// let mut req = Request::new(HttpMethod::Post, "/users");
/// req.query = Some("notify=true".into());
/// req.body = "name=Ada".into();
/// assert_eq!(req.method, HttpMethod::Post);
/// assert_eq!(req.path, "/users");
/// ```
#[derive(Debug, Clone)]
pub struct Request {
    pub method: HttpMethod,
    pub path: String,
    pub query: Option<String>,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub(crate) params: HashMap<String, String>,
    pub(crate) extensions: http::Extensions,
}

impl Request {
    /// Builds a request with no query, headers, body or path parameters.
    ///
    /// The server builds real requests itself; use this in tests with `App::handle`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let req = Request::new(HttpMethod::Get, "/ping");
    /// assert!(req.query.is_none());
    /// assert!(req.body.is_empty());
    /// ```
    pub fn new(method: HttpMethod, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: None,
            headers: HeaderMap::new(),
            body: Bytes::new(),
            params: HashMap::new(),
            extensions: http::Extensions::new(),
        }
    }

    /// Typed per-request data attached by filters, e.g. the authenticated user.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let mut req = Request::new(HttpMethod::Get, "/");
    /// assert_eq!(req.extensions().get::<u32>(), None);
    /// req.extensions_mut().insert(7u32);
    /// assert_eq!(req.extensions().get::<u32>(), Some(&7));
    /// ```
    pub fn extensions(&self) -> &http::Extensions {
        &self.extensions
    }

    /// Mutable access to the per-request extensions; filters use it to pass data on.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let mut req = Request::new(HttpMethod::Get, "/");
    /// req.extensions_mut().insert("alice".to_string());
    /// assert_eq!(req.extensions().get::<String>().map(String::as_str), Some("alice"));
    /// ```
    pub fn extensions_mut(&mut self) -> &mut http::Extensions {
        &mut self.extensions
    }

    /// Value of a path parameter, e.g. `id` for the route `/users/{id}`.
    ///
    /// Parameters are filled in by the router, so a hand-built request has none.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{HttpMethod, IntoResponse, Request, Route, Router, RouterConfig};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut router = Router::new(RouterConfig::default());
    /// router.add(Route::new(HttpMethod::Get, "/users/{id}", |req: Request| async move {
    ///     format!("user {}", req.param("id").unwrap_or("?"))
    /// }))?;
    ///
    /// let res = router.handle(Request::new(HttpMethod::Get, "/users/42")).await;
    /// assert_eq!(res.body(), "user 42");
    ///
    /// // A request that never went through the router has no params.
    /// assert_eq!(Request::new(HttpMethod::Get, "/users/42").param("id"), None);
    /// # Ok(())
    /// # }
    /// ```
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }

    /// Address of the client: the TCP peer, or the client named by a trusted proxy (see
    /// [`App::forwarded`](crate::App::forwarded)). `None` for a request that did not come
    /// through [`App::serve`](crate::App::serve), such as one passed to `App::handle`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{ClientInfo, HttpMethod, Request, Scheme};
    ///
    /// let mut req = Request::new(HttpMethod::Get, "/");
    /// assert_eq!(req.client_addr(), None);
    /// req.extensions_mut().insert(ClientInfo {
    ///     addr: "192.0.2.1:40000".parse().unwrap(),
    ///     scheme: Scheme::Http,
    ///     host: None,
    /// });
    /// assert_eq!(req.client_addr(), Some("192.0.2.1:40000".parse().unwrap()));
    /// ```
    pub fn client_addr(&self) -> Option<std::net::SocketAddr> {
        self.client_info().map(|c| c.addr)
    }

    /// Scheme the client used, as reported by a trusted proxy; [`Scheme::Http`](crate::Scheme::Http) otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{ClientInfo, HttpMethod, Request, Scheme};
    ///
    /// let mut req = Request::new(HttpMethod::Get, "/");
    /// assert_eq!(req.scheme(), Scheme::Http);
    /// req.extensions_mut().insert(ClientInfo {
    ///     addr: "192.0.2.1:0".parse().unwrap(),
    ///     scheme: Scheme::Https,
    ///     host: None,
    /// });
    /// assert_eq!(req.scheme(), Scheme::Https);
    /// ```
    pub fn scheme(&self) -> crate::Scheme {
        self.client_info().map(|c| c.scheme).unwrap_or_default()
    }

    /// Host (authority) the client asked for, as reported by a trusted proxy; `None`
    /// otherwise. The `Host` header itself stays in [`Request::headers`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{ClientInfo, HttpMethod, Request, Scheme};
    ///
    /// let mut req = Request::new(HttpMethod::Get, "/");
    /// assert_eq!(req.host(), None);
    /// req.extensions_mut().insert(ClientInfo {
    ///     addr: "192.0.2.1:0".parse().unwrap(),
    ///     scheme: Scheme::Https,
    ///     host: Some("example.com".into()),
    /// });
    /// assert_eq!(req.host(), Some("example.com"));
    /// ```
    pub fn host(&self) -> Option<&str> {
        self.client_info().and_then(|c| c.host.as_deref())
    }

    /// Whether the server cut the body of this request because it was larger than
    /// `max_body_bytes` (`server.max-body-bytes` in `oxbloom-boot`). The body is then empty.
    ///
    /// The built-in body limit filter (slot [`order::BODY_LIMIT`](crate::order::BODY_LIMIT))
    /// answers such a request with 413, so only a filter ordered before it sees one; this
    /// tells such a filter why the body is empty. A request that did not come through
    /// [`App::serve`](crate::App::serve), such as one passed to `App::handle`, always gives
    /// `false`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, Filter, HttpMethod, IntoResponse, Next, Request, Response, order};
    /// use tokio::io::{AsyncReadExt, AsyncWriteExt};
    /// use tokio::net::{TcpListener, TcpStream};
    ///
    /// /// Ordered before the body limit: answers with what it saw.
    /// struct Probe;
    ///
    /// #[async_trait::async_trait]
    /// impl Filter for Probe {
    ///     async fn filter(&self, req: Request, next: Next<'_>) -> Response {
    ///         if req.body_too_large() {
    ///             return format!("cut, {} bytes left", req.body.len()).into_response();
    ///         }
    ///         next.run(req).await
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// assert!(!Request::new(HttpMethod::Post, "/").body_too_large());
    ///
    /// let app = App::new().max_body_bytes(4).filter_ordered(order::BODY_LIMIT - 50, Probe);
    /// let listener = TcpListener::bind("127.0.0.1:0").await?;
    /// let addr = listener.local_addr()?;
    /// let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    /// let server = tokio::spawn(app.serve(listener, async { let _ = stopped.await; }));
    ///
    /// let mut conn = TcpStream::connect(addr).await?;
    /// conn.write_all(b"POST /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 5\r\n\r\n12345").await?;
    /// let mut reply = String::new();
    /// conn.read_to_string(&mut reply).await?;
    /// assert!(reply.ends_with("cut, 0 bytes left"), "{reply}");
    ///
    /// stop.send(()).unwrap();
    /// server.await??;
    /// # Ok(())
    /// # }
    /// ```
    pub fn body_too_large(&self) -> bool {
        self.extensions
            .get::<crate::filter::BodyTooLarge>()
            .is_some()
    }

    fn client_info(&self) -> Option<&crate::ClientInfo> {
        self.extensions.get::<crate::ClientInfo>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::BodyTooLarge;

    #[test]
    fn body_too_large_reads_the_marker() {
        let mut req = Request::new(HttpMethod::Post, "/");
        assert!(!req.body_too_large());
        req.extensions_mut().insert(BodyTooLarge);
        assert!(req.body_too_large());
    }
}
