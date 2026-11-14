use bytes::Bytes;
use http::{HeaderValue, StatusCode, header};

/// Response type produced by the router: an `http::Response` with a `Bytes` body.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{IntoResponse, Response};
///
/// let res: Response = "hello".into_response();
/// assert_eq!(res.status(), 200);
/// assert_eq!(res.body(), "hello");
/// ```
pub type Response = http::Response<Bytes>;

/// Conversion into a [`Response`]. Handlers return anything that implements it.
///
/// Implemented for [`Response`], `StatusCode`, `&'static str`, `String` and `(StatusCode, T)`.
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::{IntoResponse, Response};
///
/// struct Teapot;
///
/// impl IntoResponse for Teapot {
///     fn into_response(self) -> Response {
///         (StatusCode::IM_A_TEAPOT, "short and stout").into_response()
///     }
/// }
///
/// assert_eq!(Teapot.into_response().status(), 418);
/// ```
pub trait IntoResponse {
    /// Converts `self` into a [`Response`].
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::IntoResponse;
    ///
    /// let res = (StatusCode::CREATED, "made").into_response();
    /// assert_eq!(res.status(), 201);
    /// assert_eq!(res.body(), "made");
    /// ```
    fn into_response(self) -> Response;
}

/// A finished [`Response`] is returned unchanged.
///
/// ```
/// use oxbloom_web::{IntoResponse, Response};
///
/// let res = Response::new("raw".into()).into_response();
/// assert_eq!(res.body(), "raw");
/// ```
impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

/// A bare status code gives an empty body.
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::IntoResponse;
///
/// let res = StatusCode::NOT_FOUND.into_response();
/// assert_eq!(res.status(), 404);
/// assert!(res.body().is_empty());
/// ```
impl IntoResponse for StatusCode {
    fn into_response(self) -> Response {
        let mut res = Response::new(Bytes::new());
        *res.status_mut() = self;
        res
    }
}

fn text(body: Bytes) -> Response {
    let mut res = Response::new(body);
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    res
}

/// Plain text, `200 OK`, `text/plain; charset=utf-8`.
///
/// ```
/// use oxbloom_web::IntoResponse;
///
/// let res = "pong".into_response();
/// assert_eq!(res.headers()["content-type"], "text/plain; charset=utf-8");
/// ```
impl IntoResponse for &'static str {
    fn into_response(self) -> Response {
        text(Bytes::from_static(self.as_bytes()))
    }
}

/// Plain text, `200 OK`, `text/plain; charset=utf-8`.
///
/// ```
/// use oxbloom_web::IntoResponse;
///
/// let res = format!("hello {}", "Ada").into_response();
/// assert_eq!(res.body(), "hello Ada");
/// ```
impl IntoResponse for String {
    fn into_response(self) -> Response {
        text(Bytes::from(self))
    }
}

/// Converts the body, then replaces the status code.
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::IntoResponse;
///
/// let res = (StatusCode::ACCEPTED, "queued").into_response();
/// assert_eq!(res.status(), 202);
/// assert_eq!(res.headers()["content-type"], "text/plain; charset=utf-8");
/// ```
impl<T: IntoResponse> IntoResponse for (StatusCode, T) {
    fn into_response(self) -> Response {
        let mut res = self.1.into_response();
        *res.status_mut() = self.0;
        res
    }
}

/// `Ok` and `Err` each convert through their own [`IntoResponse`].
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::{HttpError, IntoResponse};
///
/// let ok: Result<&'static str, HttpError> = Ok("fine");
/// assert_eq!(ok.into_response().body(), "fine");
/// let err: Result<&'static str, HttpError> = Err(HttpError::new(StatusCode::GONE));
/// assert_eq!(err.into_response().status(), 410);
/// ```
impl<T: IntoResponse, E: IntoResponse> IntoResponse for Result<T, E> {
    fn into_response(self) -> Response {
        match self {
            Ok(v) => v.into_response(),
            Err(e) => e.into_response(),
        }
    }
}
