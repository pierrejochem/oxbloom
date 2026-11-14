use std::sync::Arc;

use http::header;

use crate::catch::{CatchUnwind, panic_response, request_info, strip_for_head};
use crate::router::HeadBodyStripped;
use crate::{
    ErrorAttributes, ErrorRenderer, IntoResponse, Request, RequestInfo, Response, Router, order,
};

/// A request interceptor that runs before routing, in the order of [`Filter::order`] (lower
/// first; equal orders keep registration order).
///
/// A filter either calls [`Next::run`] to continue the chain (and may inspect or modify the
/// response), or returns a response itself, in which case the router is never reached.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{App, Filter, HttpMethod, Next, Request, Response};
/// use http::StatusCode;
///
/// struct RequireKey;
///
/// #[async_trait::async_trait]
/// impl Filter for RequireKey {
///     async fn filter(&self, req: Request, next: Next<'_>) -> Response {
///         if req.headers.contains_key("x-key") {
///             next.run(req).await
///         } else {
///             let mut res = Response::new(Default::default());
///             *res.status_mut() = StatusCode::UNAUTHORIZED;
///             res
///         }
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let app = App::new().filter(RequireKey);
/// let res = app.handle(Request::new(HttpMethod::Get, "/")).await;
/// assert_eq!(res.status(), 401);
/// # }
/// ```
#[async_trait::async_trait]
pub trait Filter: Send + Sync {
    /// Handles `req`, usually delegating to `next`.
    async fn filter(&self, req: Request, next: Next<'_>) -> Response;

    /// The slot of this filter in the chain; lower runs first. Defaults to [`order::DEFAULT`].
    ///
    /// A filter ordered below [`order::SECURITY`] runs before authentication: it sees
    /// unauthenticated requests (no `Authentication` in the extensions) and a response it
    /// returns itself is sent without any authorization check. A filter ordered below
    /// [`order::BODY_LIMIT`] sees an empty body for a request whose body was over the limit, and
    /// can tell why only by calling [`Request::body_too_large`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{Filter, Next, Request, Response, order};
    ///
    /// struct Early;
    ///
    /// #[async_trait::async_trait]
    /// impl Filter for Early {
    ///     async fn filter(&self, req: Request, next: Next<'_>) -> Response {
    ///         next.run(req).await
    ///     }
    ///     fn order(&self) -> i32 {
    ///         order::SECURITY
    ///     }
    /// }
    ///
    /// assert_eq!(Early.order(), -100);
    /// ```
    fn order(&self) -> i32 {
        order::DEFAULT
    }
}

#[async_trait::async_trait]
impl<F: Filter + ?Sized> Filter for Arc<F> {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        (**self).filter(req, next).await
    }

    fn order(&self) -> i32 {
        (**self).order()
    }
}

/// Wraps a [`Filter`] and overrides its order.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{Filter, Next, Ordered, Request, Response, order};
///
/// struct Pass;
///
/// #[async_trait::async_trait]
/// impl Filter for Pass {
///     async fn filter(&self, req: Request, next: Next<'_>) -> Response {
///         next.run(req).await
///     }
/// }
///
/// let early = Ordered(order::SECURITY, Pass);
/// assert_eq!(early.order(), -100);
/// ```
pub struct Ordered<F>(pub i32, pub F);

#[async_trait::async_trait]
impl<F: Filter> Filter for Ordered<F> {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        self.1.filter(req, next).await
    }

    fn order(&self) -> i32 {
        self.0
    }
}

/// Marker in the request extensions: the body exceeded the limit and was dropped.
#[derive(Clone)]
pub(crate) struct BodyTooLarge;

/// Answers 413 when the server dropped an oversized body.
pub(crate) struct BodyLimitFilter;

#[async_trait::async_trait]
impl Filter for BodyLimitFilter {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        if req.extensions().get::<BodyTooLarge>().is_some() {
            return crate::HttpError::new(http::StatusCode::PAYLOAD_TOO_LARGE).into_response();
        }
        next.run(req).await
    }

    fn order(&self) -> i32 {
        order::BODY_LIMIT
    }
}

/// Renders error answers through the app's [`ErrorRenderer`] and turns a panic further down the
/// chain into a 500.
pub(crate) struct ErrorHandlingFilter {
    pub(crate) renderer: Arc<dyn ErrorRenderer>,
}

#[async_trait::async_trait]
impl Filter for ErrorHandlingFilter {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let info = request_info(&req);
        match CatchUnwind::new(Box::pin(next.run(req))).await {
            Ok(res) => render_error(&*self.renderer, res, &info),
            Err(payload) => panic_response(&*self.renderer, &*payload, &info),
        }
    }

    fn order(&self) -> i32 {
        order::ERROR_HANDLING
    }
}

/// Headers of the original answer that describe its body, so they are not copied onto the
/// rendered one.
const BODY_HEADERS: [header::HeaderName; 5] = [
    header::CONTENT_TYPE,
    header::CONTENT_LENGTH,
    header::CONTENT_ENCODING,
    header::CONTENT_RANGE,
    header::TRANSFER_ENCODING,
];

/// Renders a bodiless `res` when it carries [`ErrorAttributes`] or has a 4xx/5xx status. A
/// response with a body (also one the router emptied for `HEAD`) is the handler's own answer and
/// is returned untouched, even when it carries [`ErrorAttributes`].
fn render_error(renderer: &dyn ErrorRenderer, res: Response, info: &RequestInfo) -> Response {
    if !res.body().is_empty() || res.extensions().get::<HeadBodyStripped>().is_some() {
        return res;
    }
    let status = res.status();
    let mut attrs = match res.extensions().get::<ErrorAttributes>() {
        Some(a) => a.clone(),
        None if status.is_client_error() || status.is_server_error() => {
            ErrorAttributes::from_status(status)
        }
        None => return res,
    };
    // `(StatusCode, HttpError)` changes the status after the attributes were made.
    attrs.status = status;
    let mut out = renderer.render(&attrs, info);
    let headers = out.headers_mut();
    let kept = || {
        res.headers()
            .iter()
            .filter(|(n, _)| !BODY_HEADERS.contains(n))
    };
    for (name, _) in kept() {
        headers.remove(name);
    }
    for (name, value) in kept() {
        headers.append(name.clone(), value.clone());
    }
    strip_for_head(&mut out, info);
    out
}

/// The rest of the chain after the current [`Filter`]: remaining filters, then the router.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{App, Filter, HttpMethod, Next, Request, Response, Route};
///
/// struct PassThrough;
///
/// #[async_trait::async_trait]
/// impl Filter for PassThrough {
///     async fn filter(&self, req: Request, next: Next<'_>) -> Response {
///         next.run(req).await
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let app = App::new()
///     .filter(PassThrough)
///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
/// let res = app.handle(Request::new(HttpMethod::Get, "/ping")).await;
/// assert_eq!(res.body(), "pong");
/// # Ok(())
/// # }
/// ```
pub struct Next<'a> {
    filters: &'a [Arc<dyn Filter>],
    router: &'a Router,
}

impl<'a> Next<'a> {
    /// Runs the remaining filters and then the router with `req`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::{App, Filter, HttpMethod, Next, Request, Response};
    ///
    /// struct Mark;
    ///
    /// #[async_trait::async_trait]
    /// impl Filter for Mark {
    ///     async fn filter(&self, mut req: Request, next: Next<'_>) -> Response {
    ///         req.extensions_mut().insert(1u8);
    ///         next.run(req).await
    ///     }
    /// }
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let res = App::new().filter(Mark).handle(Request::new(HttpMethod::Get, "/x")).await;
    /// assert_eq!(res.status(), 404);
    /// # }
    /// ```
    pub async fn run(self, req: Request) -> Response {
        match self.filters.split_first() {
            Some((first, rest)) => {
                let next = Next {
                    filters: rest,
                    router: self.router,
                };
                first.filter(req, next).await
            }
            None => self.router.handle(req).await,
        }
    }
}

pub(crate) async fn run_pipeline(
    filters: &[Arc<dyn Filter>],
    router: &Router,
    req: Request,
) -> Response {
    Next { filters, router }.run(req).await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use http::StatusCode;

    use super::*;
    use crate::{App, HttpMethod, Route};

    struct Record(&'static str, Arc<Mutex<Vec<&'static str>>>);

    #[async_trait::async_trait]
    impl Filter for Record {
        async fn filter(&self, req: Request, next: Next<'_>) -> Response {
            self.1.lock().unwrap().push(self.0);
            next.run(req).await
        }
    }

    struct Stop;

    #[async_trait::async_trait]
    impl Filter for Stop {
        async fn filter(&self, _req: Request, _next: Next<'_>) -> Response {
            let mut res = Response::new(Default::default());
            *res.status_mut() = StatusCode::IM_A_TEAPOT;
            res
        }
    }

    struct Insert(u32);

    #[async_trait::async_trait]
    impl Filter for Insert {
        async fn filter(&self, mut req: Request, next: Next<'_>) -> Response {
            req.extensions_mut().insert(self.0);
            next.run(req).await
        }
    }

    fn get(path: &str) -> Request {
        Request::new(HttpMethod::Get, path)
    }

    fn ext_route() -> Route {
        Route::new(HttpMethod::Get, "/ext", |req: Request| async move {
            format!("{:?}", req.extensions().get::<u32>())
        })
    }

    #[tokio::test]
    async fn filters_run_in_registration_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let app = App::new()
            .filter(Record("a", log.clone()))
            .filter(Record("b", log.clone()))
            .filter(Record("c", log.clone()));
        app.handle(get("/")).await;
        assert_eq!(*log.lock().unwrap(), ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn filter_runs_before_routing() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let app = App::new().filter(Record("a", log.clone()));
        let res = app.handle(get("/unknown")).await;
        assert_eq!(res.status(), 404);
        assert_eq!(*log.lock().unwrap(), ["a"]);
    }

    #[tokio::test]
    async fn filter_can_short_circuit_without_calling_router() {
        let hit = Arc::new(Mutex::new(false));
        let h = hit.clone();
        let app = App::new()
            .filter(Stop)
            .route(Route::new(HttpMethod::Get, "/p", move |_| {
                let h = h.clone();
                async move {
                    *h.lock().unwrap() = true;
                    "p"
                }
            }))
            .unwrap();
        let res = app.handle(get("/p")).await;
        assert_eq!(res.status(), 418);
        assert!(!*hit.lock().unwrap());
    }

    #[tokio::test]
    async fn extensions_set_by_filter_reach_handler() {
        let app = App::new().filter(Insert(5)).route(ext_route()).unwrap();
        let res = app.handle(get("/ext")).await;
        assert_eq!(res.body(), "Some(5)");
    }

    #[tokio::test]
    async fn extensions_are_per_request() {
        let app = App::new().route(ext_route()).unwrap();
        let mut first = get("/ext");
        first.extensions_mut().insert(9u32);
        assert_eq!(app.handle(first).await.body(), "Some(9)");
        assert_eq!(app.handle(get("/ext")).await.body(), "None");
    }

    #[tokio::test]
    async fn no_filters_behaves_as_before() {
        let app = App::new().route(ext_route()).unwrap();
        assert_eq!(app.handle(get("/ext")).await.body(), "None");
        assert_eq!(app.handle(get("/nope")).await.status(), 404);
    }

    #[tokio::test]
    async fn arc_filter_is_a_filter() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let shared: Arc<dyn Filter> = Arc::new(Record("a", log.clone()));
        let app = App::new().filter(shared.clone()).filter(Arc::new(Stop));
        let res = app.handle(get("/")).await;
        assert_eq!(res.status(), 418);
        assert_eq!(*log.lock().unwrap(), ["a"]);
    }

    #[tokio::test]
    async fn filter_runs_on_the_serve_path() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        let app = App::new().filter(Stop);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(app.serve(listener, async {
            let _ = stopped.await;
        }));

        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /anything HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut reply = String::new();
        conn.read_to_string(&mut reply).await.unwrap();
        assert!(reply.starts_with("HTTP/1.1 418"), "{reply}");

        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    struct Declared(i32, &'static str, Arc<Mutex<Vec<&'static str>>>);

    #[async_trait::async_trait]
    impl Filter for Declared {
        async fn filter(&self, req: Request, next: Next<'_>) -> Response {
            self.2.lock().unwrap().push(self.1);
            next.run(req).await
        }
        fn order(&self) -> i32 {
            self.0
        }
    }

    #[test]
    fn constants_are_in_the_documented_order() {
        use crate::order::*;
        const { assert!(REQUEST_LOG < ERROR_HANDLING) };
        const { assert!(ERROR_HANDLING < BODY_LIMIT) };
        const { assert!(BODY_LIMIT < SECURITY) };
        const { assert!(SECURITY < DEFAULT) };
        assert_eq!(
            [REQUEST_LOG, ERROR_HANDLING, BODY_LIMIT, SECURITY, DEFAULT],
            [-400, -300, -200, -100, 0]
        );
    }

    #[tokio::test]
    async fn plain_filter_runs_at_default_and_keeps_registration_order_on_ties() {
        let log = Arc::new(Mutex::new(Vec::new()));
        assert_eq!(Record("a", log.clone()).order(), crate::order::DEFAULT);
        let app = App::new()
            .filter(Record("a", log.clone()))
            .filter(Ordered(0, Record("b", log.clone())))
            .filter(Record("c", log.clone()));
        app.handle(get("/")).await;
        assert_eq!(*log.lock().unwrap(), ["a", "b", "c"]);
    }

    #[tokio::test]
    async fn filter_ordered_runs_before_default_filters() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let app = App::new()
            .filter(Record("a", log.clone()))
            .filter_ordered(crate::order::SECURITY, Record("s", log.clone()))
            .filter(Record("b", log.clone()))
            .filter_ordered(crate::order::REQUEST_LOG, Record("l", log.clone()));
        app.handle(get("/")).await;
        assert_eq!(*log.lock().unwrap(), ["l", "s", "a", "b"]);
    }

    #[tokio::test]
    async fn a_filter_declaring_order_via_the_trait_is_sorted() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let app = App::new()
            .filter(Record("a", log.clone()))
            .filter(Declared(-50, "d", log.clone()))
            .filter(Declared(5, "e", log.clone()));
        app.handle(get("/")).await;
        assert_eq!(*log.lock().unwrap(), ["d", "a", "e"]);
    }

    #[tokio::test]
    async fn arc_filter_forwards_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let shared: Arc<dyn Filter> = Arc::new(Declared(-7, "d", log.clone()));
        assert_eq!(shared.order(), -7);
        let app = App::new()
            .filter(Record("a", log.clone()))
            .filter(shared.clone());
        app.handle(get("/")).await;
        assert_eq!(*log.lock().unwrap(), ["d", "a"]);
    }

    struct Seen(Arc<Mutex<Vec<u16>>>);

    #[async_trait::async_trait]
    impl Filter for Seen {
        async fn filter(&self, req: Request, next: Next<'_>) -> Response {
            let res = next.run(req).await;
            self.0.lock().unwrap().push(res.status().as_u16());
            res
        }
    }

    async fn post_over_serve(app: App, request: &'static [u8]) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(app.serve(listener, async {
            let _ = stopped.await;
        }));
        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(request).await.unwrap();
        let mut reply = String::new();
        conn.read_to_string(&mut reply).await.unwrap();
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
        reply
    }

    const BIG: &[u8] =
        b"POST /x HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 9\r\n\r\n123456789";

    #[tokio::test]
    async fn oversized_body_is_answered_413_by_the_body_limit_filter() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::new(Mutex::new(Vec::new()));
        let app = App::new()
            .max_body_bytes(8)
            .filter_ordered(crate::order::REQUEST_LOG, Seen(seen.clone()))
            .filter(Record("default", log.clone()));
        let reply = post_over_serve(app, BIG).await;
        assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
        assert_eq!(*seen.lock().unwrap(), [413]);
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn oversized_body_gets_413_before_security_style_filter() {
        struct Deny;
        #[async_trait::async_trait]
        impl Filter for Deny {
            async fn filter(&self, _req: Request, _next: Next<'_>) -> Response {
                let mut res = Response::new(Default::default());
                *res.status_mut() = StatusCode::UNAUTHORIZED;
                res
            }
            fn order(&self) -> i32 {
                crate::order::SECURITY
            }
        }
        let app = App::new().max_body_bytes(8).filter(Deny);
        let reply = post_over_serve(app, BIG).await;
        assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
    }

    #[tokio::test]
    async fn a_filter_before_the_body_limit_sees_an_empty_body_and_the_flag() {
        struct Early(Arc<Mutex<Vec<(usize, bool)>>>);
        #[async_trait::async_trait]
        impl Filter for Early {
            async fn filter(&self, req: Request, next: Next<'_>) -> Response {
                self.0
                    .lock()
                    .unwrap()
                    .push((req.body.len(), req.body_too_large()));
                next.run(req).await
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = App::new()
            .max_body_bytes(8)
            .filter_ordered(crate::order::BODY_LIMIT - 50, Early(seen.clone()));
        let reply = post_over_serve(app, BIG).await;
        assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
        assert_eq!(*seen.lock().unwrap(), [(0, true)]);
    }

    #[tokio::test]
    async fn handle_with_a_body_over_the_limit_is_not_limited() {
        let app = App::new()
            .max_body_bytes(8)
            .route(Route::new(
                HttpMethod::Post,
                "/x",
                |r: Request| async move { r.body.len().to_string() },
            ))
            .unwrap();
        let mut req = Request::new(HttpMethod::Post, "/x");
        req.body = bytes::Bytes::from_static(b"123456789");
        let res = app.handle(req).await;
        assert_eq!(res.status(), 200);
        assert_eq!(res.body(), "9");
    }
}
