use bytes::Bytes;
use http::StatusCode;

use crate::{IntoResponse, Response};

/// A value of an extension member of a problem body: a string, an integer or a boolean.
///
/// The enum is `#[non_exhaustive]`, so more kinds of values can follow: a `match` outside
/// `oxbloom-web` needs a wildcard arm. Build values with `From` (or the variants).
///
/// # Examples
///
/// ```
/// use oxbloom_web::ErrorValue;
///
/// assert_eq!(ErrorValue::from("x"), ErrorValue::Str("x".into()));
/// assert_eq!(ErrorValue::from(5_i64), ErrorValue::Int(5));
/// assert_eq!(ErrorValue::from(true), ErrorValue::Bool(true));
///
/// let text = match ErrorValue::from(5_i64) {
///     ErrorValue::Str(s) => s,
///     ErrorValue::Int(i) => i.to_string(),
///     ErrorValue::Bool(b) => b.to_string(),
///     _ => String::new(), // required: the enum is non-exhaustive
/// };
/// assert_eq!(text, "5");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorValue {
    /// A JSON string.
    Str(String),
    /// A JSON integer.
    Int(i64),
    /// A JSON boolean.
    Bool(bool),
}

impl From<&str> for ErrorValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_owned())
    }
}

impl From<String> for ErrorValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}

impl From<i64> for ErrorValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<i32> for ErrorValue {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<u32> for ErrorValue {
    fn from(v: u32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<bool> for ErrorValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

/// Everything an [`ErrorRenderer`](crate::ErrorRenderer) needs to describe an error answer.
///
/// Travels in the response extensions of an [`HttpError`] response. `internal` is set only by
/// the framework (for example the panic message) and is shown to clients only when the renderer
/// is told to include it.
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::ErrorAttributes;
///
/// let attrs = ErrorAttributes::from_status(StatusCode::NOT_FOUND);
/// assert_eq!(attrs.status, StatusCode::NOT_FOUND);
/// assert!(attrs.title.is_none() && attrs.internal.is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorAttributes {
    /// Response status.
    pub status: StatusCode,
    /// Author's title; the status reason phrase is used when `None`.
    pub title: Option<String>,
    /// Author's detail, always shown to the client.
    pub detail: Option<String>,
    /// Problem type URI; `about:blank` when `None`.
    pub type_uri: Option<String>,
    /// Extension members, in order.
    pub extensions: Vec<(String, ErrorValue)>,
    /// Framework-only message, hidden from clients unless the renderer includes it.
    pub internal: Option<String>,
}

impl ErrorAttributes {
    /// Attributes with only a status set.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::ErrorAttributes;
    ///
    /// let attrs = ErrorAttributes::from_status(StatusCode::BAD_GATEWAY);
    /// assert_eq!(attrs.status, 502);
    /// assert!(attrs.extensions.is_empty());
    /// ```
    pub fn from_status(status: StatusCode) -> Self {
        Self {
            status,
            title: None,
            detail: None,
            type_uri: None,
            extensions: Vec::new(),
            internal: None,
        }
    }
}

/// An error a handler can return; it becomes a problem+json answer.
///
/// Converting it gives an empty-bodied response with the status and an [`ErrorAttributes`]
/// value in the response extensions; the built-in error filter renders it.
///
/// # Examples
///
/// ```
/// use http::StatusCode;
/// use oxbloom_web::{ErrorAttributes, HttpError, IntoResponse};
///
/// let res = HttpError::new(StatusCode::CONFLICT)
///     .title("Taken")
///     .detail("name is taken")
///     .type_uri("https://example.com/taken")
///     .with("limit", 5)
///     .into_response();
/// assert_eq!(res.status(), 409);
/// let attrs = res.extensions().get::<ErrorAttributes>().unwrap();
/// assert_eq!(attrs.title.as_deref(), Some("Taken"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    attrs: ErrorAttributes,
}

impl HttpError {
    /// An error with the given status.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::{HttpError, IntoResponse};
    ///
    /// assert_eq!(HttpError::new(StatusCode::GONE).into_response().status(), 410);
    /// ```
    pub fn new(status: StatusCode) -> Self {
        Self {
            attrs: ErrorAttributes::from_status(status),
        }
    }

    /// Sets the title.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::HttpError;
    ///
    /// let _ = HttpError::new(StatusCode::GONE).title("Gone for good");
    /// ```
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.attrs.title = Some(title.into());
        self
    }

    /// Sets the detail, which is always shown to the client.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::HttpError;
    ///
    /// let _ = HttpError::new(StatusCode::GONE).detail("removed yesterday");
    /// ```
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.attrs.detail = Some(detail.into());
        self
    }

    /// Sets the problem type URI.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::HttpError;
    ///
    /// let _ = HttpError::new(StatusCode::GONE).type_uri("https://example.com/gone");
    /// ```
    #[must_use]
    pub fn type_uri(mut self, type_uri: impl Into<String>) -> Self {
        self.attrs.type_uri = Some(type_uri.into());
        self
    }

    /// Adds an extension member. A key equal to a built-in member (`type`, `title`, `status`,
    /// `detail`, `instance`) is dropped when rendering.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::StatusCode;
    /// use oxbloom_web::HttpError;
    ///
    /// let _ = HttpError::new(StatusCode::BAD_REQUEST)
    ///     .with("field", "name")
    ///     .with("max", 10)
    ///     .with("retry", false);
    /// ```
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<ErrorValue>) -> Self {
        self.attrs.extensions.push((key.into(), value.into()));
        self
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let mut res = Response::new(Bytes::new());
        *res.status_mut() = self.attrs.status;
        res.extensions_mut().insert(self.attrs);
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_error_response_carries_attributes_and_no_body() {
        let res = HttpError::new(StatusCode::NOT_FOUND)
            .title("t")
            .detail("d")
            .type_uri("u")
            .with("k", "v")
            .into_response();
        assert_eq!(res.status(), 404);
        assert!(res.body().is_empty());
        let attrs = res.extensions().get::<ErrorAttributes>().unwrap();
        assert_eq!(attrs.title.as_deref(), Some("t"));
        assert_eq!(attrs.detail.as_deref(), Some("d"));
        assert_eq!(attrs.type_uri.as_deref(), Some("u"));
        assert_eq!(
            attrs.extensions,
            vec![("k".into(), ErrorValue::Str("v".into()))]
        );
        assert!(attrs.internal.is_none());
    }

    #[test]
    fn result_ok_and_err_map_through_into_response() {
        let ok: Result<&'static str, HttpError> = Ok("fine");
        assert_eq!(ok.into_response().body(), "fine");
        let err: Result<&'static str, HttpError> = Err(HttpError::new(StatusCode::IM_A_TEAPOT));
        let res = err.into_response();
        assert_eq!(res.status(), 418);
        assert!(res.extensions().get::<ErrorAttributes>().is_some());
    }

    #[test]
    fn error_value_conversions() {
        assert_eq!(ErrorValue::from(7_u32), ErrorValue::Int(7));
        assert_eq!(
            ErrorValue::from(String::from("a")),
            ErrorValue::Str("a".into())
        );
    }
}
