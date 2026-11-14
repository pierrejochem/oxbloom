use std::collections::HashMap;
use std::fmt;

use bytes::Bytes;
use http::{HeaderValue, StatusCode, header};
use oxbloom_core::HttpMethod;

use crate::{Handler, HttpError, IntoResponse, Request, Response, Route};

/// Marker in the response extensions: the router dropped the body of a `GET` answer for `HEAD`,
/// so an empty error body there is the handler's own, not a framework answer. It carries the
/// length of the dropped body, which the compression filter needs to treat `HEAD` like `GET`.
#[derive(Clone)]
pub(crate) struct HeadBodyStripped(pub(crate) usize);

/// Router behavior switches. Secure by default.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{App, RouterConfig};
///
/// // Default: TRACE and CONNECT requests get 405.
/// let strict = RouterConfig::default();
/// assert!(!strict.allow_trace && !strict.allow_connect);
///
/// // Opt in explicitly.
/// let app = App::with_config(RouterConfig { allow_trace: true, ..RouterConfig::default() });
/// # let _ = app;
/// ```
#[derive(Debug, Clone, Default)]
pub struct RouterConfig {
    /// Dispatch TRACE requests to routes. Default `false` (405).
    pub allow_trace: bool,
    /// Dispatch CONNECT requests to routes. Default `false` (405).
    pub allow_connect: bool,
}

/// Error registering a route.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, Route, RouteError, Router, RouterConfig};
///
/// let mut router = Router::new(RouterConfig::default());
/// let get = || Route::new(HttpMethod::Get, "/x", |_| async { "x" });
/// router.add(get())?;
///
/// // Same method and path twice.
/// assert!(matches!(router.add(get()), Err(RouteError::Duplicate(..))));
///
/// // Same path position with a different parameter name.
/// router.add(Route::new(HttpMethod::Get, "/u/{id}", |_| async { "a" }))?;
/// let clash = Route::new(HttpMethod::Put, "/u/{name}", |_| async { "b" });
/// assert!(matches!(router.add(clash), Err(RouteError::Path(..))));
/// # Ok::<(), RouteError>(())
/// ```
#[derive(Debug)]
pub enum RouteError {
    /// Invalid or conflicting path pattern.
    Path(String, matchit::InsertError),
    /// Same method registered twice for one path.
    Duplicate(HttpMethod, String),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(p, e) => write!(f, "invalid route path {p:?}: {e}"),
            Self::Duplicate(m, p) => write!(f, "duplicate route {m} {p}"),
        }
    }
}

impl std::error::Error for RouteError {}

type MethodMap = HashMap<HttpMethod, Handler>;

/// Method + path dispatcher.
///
/// `App` wraps one; use `Router` directly to test routing without a server.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, Request, Route, Router, RouterConfig};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut router = Router::new(RouterConfig::default());
/// router.add(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
/// let res = router.handle(Request::new(HttpMethod::Get, "/ping")).await;
/// assert_eq!(res.body(), "pong");
/// # Ok(())
/// # }
/// ```
pub struct Router {
    config: RouterConfig,
    paths: matchit::Router<usize>,
    entries: Vec<MethodMap>,
    patterns: Vec<String>,
}

impl Router {
    /// Creates an empty router.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{Router, RouterConfig};
    ///
    /// let router = Router::new(RouterConfig::default());
    /// # let _ = router;
    /// ```
    pub fn new(config: RouterConfig) -> Self {
        Self {
            config,
            paths: matchit::Router::new(),
            entries: Vec::new(),
            patterns: Vec::new(),
        }
    }

    /// Registers a route. Fails on an invalid or conflicting path, or on the same method and
    /// path twice.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{HttpMethod, Route, Router, RouterConfig};
    ///
    /// let mut router = Router::new(RouterConfig::default());
    /// router.add(Route::new(HttpMethod::Get, "/users/{id}", |_| async { "user" }))?;
    /// router.add(Route::new(HttpMethod::Delete, "/users/{id}", |_| async { "gone" }))?;
    /// # Ok::<(), oxbloom_web::RouteError>(())
    /// ```
    pub fn add(&mut self, route: Route) -> Result<(), RouteError> {
        // Same pattern text only. Matching the pattern as a concrete path would miss a pattern
        // with escaped braces (`{{`, `}}`), whose stored form differs from its text.
        let idx = self.patterns.iter().position(|p| *p == route.path);
        let idx = match idx {
            Some(i) => i,
            None => {
                let i = self.entries.len();
                self.paths
                    .insert(route.path.clone(), i)
                    .map_err(|e| RouteError::Path(route.path.clone(), e))?;
                self.entries.push(MethodMap::new());
                self.patterns.push(route.path.clone());
                i
            }
        };
        if self.entries[idx].contains_key(&route.method) {
            return Err(RouteError::Duplicate(route.method, route.path));
        }
        self.entries[idx].insert(route.method, route.handler);
        Ok(())
    }

    fn enabled(&self, m: &HttpMethod) -> bool {
        match m {
            HttpMethod::Trace => self.config.allow_trace,
            HttpMethod::Connect => self.config.allow_connect,
            _ => true,
        }
    }

    fn allow_header(&self, methods: &MethodMap) -> HeaderValue {
        let mut names: Vec<&str> = methods
            .keys()
            .filter(|m| self.enabled(m))
            .map(HttpMethod::as_str)
            .collect();
        if methods.contains_key(&HttpMethod::Get) {
            names.push("HEAD");
        }
        names.push("OPTIONS");
        names.sort_unstable();
        names.dedup();
        HeaderValue::from_str(&names.join(", ")).expect("method tokens are valid header values")
    }

    /// Dispatch a request to its handler.
    ///
    /// Unknown path: 404. Known path, wrong method: 405 with `Allow`. `HEAD` falls back to `GET`
    /// with the body stripped. `OPTIONS` answers 204 with `Allow`. `TRACE` and `CONNECT` get 405
    /// unless enabled in [`RouterConfig`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{HttpMethod, Request, Route, Router, RouterConfig};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut router = Router::new(RouterConfig::default());
    /// router.add(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
    ///
    /// let res = router.handle(Request::new(HttpMethod::Get, "/missing")).await;
    /// assert_eq!(res.status(), 404);
    ///
    /// let res = router.handle(Request::new(HttpMethod::Post, "/ping")).await;
    /// assert_eq!(res.status(), 405);
    /// assert_eq!(res.headers()["allow"], "GET, HEAD, OPTIONS");
    ///
    /// let res = router.handle(Request::new(HttpMethod::Head, "/ping")).await;
    /// assert_eq!(res.status(), 200);
    /// assert!(res.body().is_empty());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn handle(&self, mut req: Request) -> Response {
        let Ok(m) = self.paths.at(&req.path) else {
            return HttpError::new(StatusCode::NOT_FOUND).into_response();
        };
        let methods = &self.entries[*m.value];
        req.params = m
            .params
            .iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();

        let not_allowed = || {
            let mut res = HttpError::new(StatusCode::METHOD_NOT_ALLOWED).into_response();
            res.headers_mut()
                .insert(header::ALLOW, self.allow_header(methods));
            res
        };

        if !self.enabled(&req.method) {
            return not_allowed();
        }
        if let Some(h) = methods.get(&req.method) {
            return h(req).await;
        }
        match req.method {
            HttpMethod::Head => match methods.get(&HttpMethod::Get) {
                Some(h) => {
                    let mut res = h(req).await;
                    if !res.body().is_empty() {
                        let len = res.body().len();
                        *res.body_mut() = Bytes::new();
                        res.extensions_mut().insert(HeadBodyStripped(len));
                    }
                    res
                }
                None => not_allowed(),
            },
            HttpMethod::Options => {
                let mut res = StatusCode::NO_CONTENT.into_response();
                res.headers_mut()
                    .insert(header::ALLOW, self.allow_header(methods));
                res
            }
            _ => not_allowed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(m: HttpMethod, path: &str, body: &'static str) -> Route {
        Route::new(m, path, move |_req| async move { body })
    }

    fn router(config: RouterConfig) -> Router {
        let mut r = Router::new(config);
        r.add(route(HttpMethod::Get, "/hello", "hi")).unwrap();
        r.add(route(HttpMethod::Post, "/hello", "posted")).unwrap();
        r.add(Route::new(
            HttpMethod::Get,
            "/users/{id}",
            |req| async move { req.param("id").unwrap().to_owned() },
        ))
        .unwrap();
        r.add(route(HttpMethod::Trace, "/hello", "traced")).unwrap();
        r
    }

    async fn call(r: &Router, m: HttpMethod, path: &str) -> Response {
        r.handle(Request::new(m, path)).await
    }

    fn allow(res: &Response) -> &str {
        res.headers()[header::ALLOW].to_str().unwrap()
    }

    #[tokio::test]
    async fn dispatches_and_extracts_params() {
        let r = router(RouterConfig::default());
        let res = call(&r, HttpMethod::Get, "/hello").await;
        assert_eq!(res.body(), "hi");
        let res = call(&r, HttpMethod::Get, "/users/42").await;
        assert_eq!(res.body(), "42");
    }

    #[tokio::test]
    async fn unknown_path_is_404() {
        let r = router(RouterConfig::default());
        let res = call(&r, HttpMethod::Get, "/nope").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert!(res.body().is_empty());
        let attrs = res.extensions().get::<crate::ErrorAttributes>().unwrap();
        assert_eq!(
            *attrs,
            crate::ErrorAttributes::from_status(StatusCode::NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn wrong_method_is_405_with_allow() {
        let r = router(RouterConfig::default());
        let res = call(&r, HttpMethod::Delete, "/hello").await;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(allow(&res), "GET, HEAD, OPTIONS, POST");
        assert!(res.body().is_empty());
        let attrs = res.extensions().get::<crate::ErrorAttributes>().unwrap();
        assert_eq!(attrs.status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn head_uses_get_without_body() {
        let r = router(RouterConfig::default());
        let res = call(&r, HttpMethod::Head, "/hello").await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.body().is_empty());
        assert!(res.headers().contains_key(header::CONTENT_TYPE));
    }

    #[tokio::test]
    async fn head_without_get_is_405() {
        let mut r = Router::new(RouterConfig::default());
        r.add(route(HttpMethod::Post, "/p", "x")).unwrap();
        let res = call(&r, HttpMethod::Head, "/p").await;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn options_auto_allow() {
        let r = router(RouterConfig::default());
        let res = call(&r, HttpMethod::Options, "/hello").await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        assert_eq!(allow(&res), "GET, HEAD, OPTIONS, POST");
    }

    #[tokio::test]
    async fn trace_disabled_by_default_even_if_registered() {
        let r = router(RouterConfig::default());
        let res = call(&r, HttpMethod::Trace, "/hello").await;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert!(!allow(&res).contains("TRACE"));
    }

    #[tokio::test]
    async fn trace_enabled_by_config() {
        let r = router(RouterConfig {
            allow_trace: true,
            ..Default::default()
        });
        let res = call(&r, HttpMethod::Trace, "/hello").await;
        assert_eq!(res.body(), "traced");
    }

    #[tokio::test]
    async fn connect_disabled_by_default() {
        let mut r = Router::new(RouterConfig::default());
        r.add(route(HttpMethod::Connect, "/c", "x")).unwrap();
        let res = call(&r, HttpMethod::Connect, "/c").await;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[test]
    fn duplicate_route_rejected() {
        let mut r = Router::new(RouterConfig::default());
        r.add(route(HttpMethod::Get, "/a", "1")).unwrap();
        let err = r.add(route(HttpMethod::Get, "/a", "2")).unwrap_err();
        assert!(matches!(err, RouteError::Duplicate(..)));
    }

    #[tokio::test]
    async fn a_pattern_with_escaped_braces_takes_several_methods() {
        let mut r = Router::new(RouterConfig::default());
        r.add(route(HttpMethod::Get, "/a{{b}}", "get")).unwrap();
        r.add(route(HttpMethod::Head, "/a{{b}}", "head")).unwrap();
        let err = r
            .add(route(HttpMethod::Get, "/a{{b}}", "again"))
            .unwrap_err();
        assert!(matches!(err, RouteError::Duplicate(..)));
        assert_eq!(call(&r, HttpMethod::Get, "/a{b}").await.body(), "get");
    }

    #[test]
    fn conflicting_path_rejected() {
        let mut r = Router::new(RouterConfig::default());
        r.add(route(HttpMethod::Get, "/u/{id}", "1")).unwrap();
        let err = r.add(route(HttpMethod::Get, "/u/{name}", "2")).unwrap_err();
        assert!(matches!(err, RouteError::Path(..)));
    }
}
