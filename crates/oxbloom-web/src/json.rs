//! JSON request bodies and responses.

use bytes::Bytes;
use http::{HeaderValue, StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{HttpError, IntoResponse, Request, Response};

const JSON: &str = "application/json";

/// A value that is sent as a JSON response body.
///
/// The response has status 200, the header `content-type: application/json` and the serialized
/// body; wrap it in `(StatusCode, Json<T>)` for another status. It carries a [`JsonBody`] marker
/// in its extensions, which the `Accept` check looks for. If serialization fails the answer is a
/// 500 problem without a `detail`; the error text is logged at level `error` on target
/// `oxbloom::web` and never sent to the client.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{IntoResponse, Json};
///
/// let res = Json(vec![1, 2, 3]).into_response();
/// assert_eq!(res.status(), 200);
/// assert_eq!(res.headers()["content-type"], "application/json");
/// assert_eq!(res.body(), "[1,2,3]");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Json<T>(pub T);

/// Marker in the extensions of a response that was built by [`Json`].
///
/// The `Accept` check compares the request with this marker after the handler ran, so only
/// JSON answers can become a 406; other responses are never touched.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{IntoResponse, Json, JsonBody};
///
/// let res = Json("hi").into_response();
/// assert_eq!(res.extensions().get::<JsonBody>(), Some(&JsonBody));
/// assert!("plain".into_response().extensions().get::<JsonBody>().is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonBody;

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(bytes) => {
                let mut res = Response::new(Bytes::from(bytes));
                res.headers_mut()
                    .insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON));
                res.extensions_mut().insert(JsonBody);
                res
            }
            Err(err) => {
                tracing::error!(
                    target: "oxbloom::web",
                    error = %err,
                    "JSON serialization failed"
                );
                HttpError::new(StatusCode::INTERNAL_SERVER_ERROR).into_response()
            }
        }
    }
}

impl Request {
    /// Parses the body as JSON into `T`.
    ///
    /// The `Content-Type` header must be `application/json` or a `+json` type such as
    /// `application/vnd.api+json`; parameters are allowed and case is ignored. A missing or other
    /// type is a 415 error with the member `accepted = "application/json"`. A body that is empty
    /// or not valid JSON for `T` is a 400 error whose `detail` names only the error class
    /// (`syntax`, `data` or `eof`) and the line and column, never the body or a value from it.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::{HeaderValue, header};
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let mut req = Request::new(HttpMethod::Post, "/numbers");
    /// req.headers
    ///     .insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    /// req.body = "[1,2]".into();
    /// let numbers: Vec<u8> = req.json().unwrap();
    /// assert_eq!(numbers, [1, 2]);
    ///
    /// req.body = "[1,".into();
    /// assert!(req.json::<Vec<u8>>().is_err());
    /// ```
    #[allow(clippy::result_large_err)] // The error type is `HttpError` so `?` works in handlers.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, HttpError> {
        let is_json = self
            .headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_json_media_type);
        if !is_json {
            return Err(HttpError::new(StatusCode::UNSUPPORTED_MEDIA_TYPE).with("accepted", JSON));
        }
        serde_json::from_slice(&self.body).map_err(|err| {
            let class = match err.classify() {
                serde_json::error::Category::Eof => "eof",
                serde_json::error::Category::Data => "data",
                serde_json::error::Category::Syntax => "syntax",
                serde_json::error::Category::Io => "io",
            };
            HttpError::new(StatusCode::BAD_REQUEST).detail(format!(
                "invalid JSON body: {class} error at line {} column {}",
                err.line(),
                err.column()
            ))
        })
    }
}

/// Whether a `Content-Type` value names JSON: type `application`, subtype `json` or a non-empty
/// prefix followed by `+json`. Parameters are ignored and ASCII case does not matter.
fn is_json_media_type(value: &str) -> bool {
    let essence = value.split(';').next().unwrap_or("").trim();
    let Some((ty, subtype)) = essence.split_once('/') else {
        return false;
    };
    if !ty.eq_ignore_ascii_case("application") {
        return false;
    }
    let subtype = subtype.to_ascii_lowercase();
    subtype == "json"
        || subtype
            .strip_suffix("+json")
            .is_some_and(|prefix| !prefix.is_empty())
}
