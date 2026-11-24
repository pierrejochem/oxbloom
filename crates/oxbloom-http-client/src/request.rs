use std::fmt;
use std::time::Duration;

use http::{HeaderMap, HeaderName, HeaderValue, Method};

use crate::HttpClientError;

/// The body of a [`Request`].
///
/// `Debug` prints the kind only, never the content.
///
/// # Examples
///
/// ```
/// use oxbloom_http_client::Body;
///
/// let body = Body::Form(vec![("grant_type".into(), "client_credentials".into())]);
/// assert_eq!(format!("{body:?}"), "Body::Form");
/// ```
#[derive(Clone, PartialEq, Eq, Default)]
pub enum Body {
    /// No body.
    #[default]
    Empty,
    /// Raw bytes, sent as given.
    Bytes(Vec<u8>),
    /// Key and value pairs, sent form-URL-encoded.
    Form(Vec<(String, String)>),
    /// A JSON value (feature `json`).
    #[cfg(feature = "json")]
    Json(serde_json::Value),
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Empty => "Body::Empty",
            Self::Bytes(_) => "Body::Bytes",
            Self::Form(_) => "Body::Form",
            #[cfg(feature = "json")]
            Self::Json(_) => "Body::Json",
        })
    }
}

/// An outgoing HTTP request: method, URL, headers, body and an optional timeout.
///
/// `Debug` prints the method and the body kind only, never the URL or a header value.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_http_client::{Body, Request};
///
/// let request = Request::post("https://auth.example.test/token")
///     .header("accept", "application/json")
///     .body(Body::Form(vec![("grant_type".into(), "client_credentials".into())]))
///     .timeout(Duration::from_secs(5));
/// assert_eq!(request.method(), http::Method::POST);
/// assert_eq!(request.url(), "https://auth.example.test/token");
/// assert_eq!(request.timeout_value(), Some(Duration::from_secs(5)));
/// ```
#[derive(Clone)]
pub struct Request {
    method: Method,
    url: String,
    headers: HeaderMap,
    body: Body,
    timeout: Option<Duration>,
    invalid: bool,
}

impl Request {
    /// Creates a request with the given method and URL, no headers and no body.
    ///
    /// The URL is checked when the request is sent: a malformed URL or a scheme other than
    /// `http` or `https` gives [`HttpClientError::InvalidRequest`]. Credentials in the URL
    /// (`https://user:password@host/`) are turned into a `Basic` authorization header by
    /// `ReqwestClient`; prefer headers.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// let request = Request::new(http::Method::DELETE, "https://api.example.test/item/1");
    /// assert_eq!(request.method(), http::Method::DELETE);
    /// ```
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: HeaderMap::new(),
            body: Body::Empty,
            timeout: None,
            invalid: false,
        }
    }

    /// Creates a `GET` request.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// assert_eq!(Request::get("https://api.example.test/").method(), http::Method::GET);
    /// ```
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::GET, url)
    }

    /// Creates a `POST` request.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// assert_eq!(Request::post("https://api.example.test/").method(), http::Method::POST);
    /// ```
    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::POST, url)
    }

    /// Sets a header, replacing an earlier value of the same name.
    ///
    /// A bad name or value does not panic. It marks the request as invalid, and
    /// [`Request::validate`] then returns [`HttpClientError::InvalidRequest`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// let request = Request::get("https://api.example.test/").header("accept", "text/plain");
    /// assert_eq!(request.headers()["accept"], "text/plain");
    /// assert!(Request::get("https://api.example.test/").header("bad name", "x").validate().is_err());
    /// ```
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        if self.set_header(name, value).is_err() {
            self.invalid = true;
        }
        self
    }

    /// Sets a header on a request in place, for example from an interceptor.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::InvalidRequest`] for a bad header name or value. The
    /// request is left unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// let mut request = Request::get("https://api.example.test/");
    /// request.set_header("authorization", "Bearer token")?;
    /// assert!(request.set_header("bad name", "x").is_err());
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    pub fn set_header(&mut self, name: &str, value: &str) -> Result<(), HttpClientError> {
        let name =
            HeaderName::from_bytes(name.as_bytes()).map_err(|_| HttpClientError::InvalidRequest)?;
        let value = HeaderValue::from_str(value).map_err(|_| HttpClientError::InvalidRequest)?;
        self.headers.insert(name, value);
        Ok(())
    }

    /// Sets the body.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::{Body, Request};
    ///
    /// let request = Request::post("https://api.example.test/").body(Body::Bytes(vec![1, 2, 3]));
    /// assert_eq!(request.body_ref(), &Body::Bytes(vec![1, 2, 3]));
    /// ```
    #[must_use]
    pub fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    /// Sets a timeout for this request that replaces the client default.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_http_client::Request;
    ///
    /// let request = Request::get("https://api.example.test/").timeout(Duration::from_secs(2));
    /// assert_eq!(request.timeout_value(), Some(Duration::from_secs(2)));
    /// ```
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Returns the method.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// assert_eq!(Request::get("https://api.example.test/").method(), http::Method::GET);
    /// ```
    pub fn method(&self) -> Method {
        self.method.clone()
    }

    /// Returns the URL.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// assert_eq!(Request::get("https://api.example.test/").url(), "https://api.example.test/");
    /// ```
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the headers.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// assert!(Request::get("https://api.example.test/").headers().is_empty());
    /// ```
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns the body.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::{Body, Request};
    ///
    /// assert_eq!(Request::get("https://api.example.test/").body_ref(), &Body::Empty);
    /// ```
    pub fn body_ref(&self) -> &Body {
        &self.body
    }

    /// Returns the timeout of this request, if one was set.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// assert_eq!(Request::get("https://api.example.test/").timeout_value(), None);
    /// ```
    pub fn timeout_value(&self) -> Option<Duration> {
        self.timeout
    }

    /// Checks that no earlier [`Request::header`] call was given a bad name or value.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::InvalidRequest`] when one was.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Request;
    ///
    /// assert!(Request::get("https://api.example.test/").validate().is_ok());
    /// ```
    pub fn validate(&self) -> Result<(), HttpClientError> {
        if self.invalid {
            Err(HttpClientError::InvalidRequest)
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("body", &self.body)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn request_builder_sets_method_url_headers_body_timeout() {
        let request = Request::new(http::Method::PUT, "https://example.test/a")
            .header("accept", "application/json")
            .body(Body::Bytes(vec![1, 2]))
            .timeout(Duration::from_secs(3));
        assert_eq!(request.method(), http::Method::PUT);
        assert_eq!(request.url(), "https://example.test/a");
        assert_eq!(request.headers()["accept"], "application/json");
        assert_eq!(request.body_ref(), &Body::Bytes(vec![1, 2]));
        assert_eq!(request.timeout_value(), Some(Duration::from_secs(3)));
        assert_eq!(Request::get("u").method(), http::Method::GET);
        assert_eq!(Request::post("u").method(), http::Method::POST);
        assert_eq!(Request::get("u").timeout_value(), None);
        assert!(Request::get("u").validate().is_ok());
    }

    #[test]
    fn invalid_header_makes_the_request_invalid() {
        let request = Request::get("u").header("bad name", "v");
        assert_eq!(
            request.validate(),
            Err(crate::HttpClientError::InvalidRequest)
        );
    }

    #[test]
    fn request_debug_hides_url_and_header_values() {
        let request = Request::post("https://secret.test/token?code=abc")
            .header("authorization", "Bearer topsecret")
            .body(Body::Form(vec![("client_secret".into(), "hunter2".into())]));
        let text = format!("{request:?}");
        for hidden in [
            "secret.test",
            "abc",
            "topsecret",
            "hunter2",
            "client_secret",
        ] {
            assert!(!text.contains(hidden), "{text}");
        }
        assert!(text.contains("POST") && text.contains("Form"));
    }
}
