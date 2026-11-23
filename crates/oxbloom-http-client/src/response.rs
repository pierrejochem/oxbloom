use std::fmt;

use http::{HeaderMap, StatusCode};
use serde::de::DeserializeOwned;

use crate::HttpClientError;

/// A received HTTP response: status, headers and the body bytes.
///
/// A status that is not 2xx is not an error; call [`Response::error_for_status`] to turn it
/// into one. `Debug` prints the status and the body length only.
///
/// # Examples
///
/// ```
/// use oxbloom_http_client::Response;
///
/// let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), br#"{"ok":true}"#.to_vec());
/// assert_eq!(response.status(), http::StatusCode::OK);
/// let value: serde_json::Value = response.json()?;
/// assert_eq!(value["ok"], true);
/// # Ok::<(), oxbloom_http_client::HttpClientError>(())
/// ```
#[derive(Clone)]
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Response {
    /// Creates a response from its parts.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Response;
    ///
    /// let response = Response::new(http::StatusCode::CREATED, http::HeaderMap::new(), Vec::new());
    /// assert_eq!(response.status(), http::StatusCode::CREATED);
    /// ```
    pub fn new(status: StatusCode, headers: HeaderMap, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body,
        }
    }

    /// Returns the status.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Response;
    ///
    /// let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), Vec::new());
    /// assert!(response.status().is_success());
    /// ```
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// Returns the headers.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Response;
    ///
    /// let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), Vec::new());
    /// assert!(response.headers().is_empty());
    /// ```
    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    /// Returns the body bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Response;
    ///
    /// let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), b"hi".to_vec());
    /// assert_eq!(response.bytes(), b"hi");
    /// ```
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    /// Returns the body as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::Decode`] when the body is not valid UTF-8.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Response;
    ///
    /// let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), b"hello".to_vec());
    /// assert_eq!(response.text()?, "hello");
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    pub fn text(&self) -> Result<String, HttpClientError> {
        String::from_utf8(self.body.clone()).map_err(|_| HttpClientError::Decode)
    }

    /// Decodes the body as JSON.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::Decode`], with fixed text, when the body is not valid JSON
    /// for `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::Response;
    ///
    /// let response = Response::new(http::StatusCode::OK, http::HeaderMap::new(), b"[1,2]".to_vec());
    /// let numbers: Vec<u32> = response.json()?;
    /// assert_eq!(numbers, vec![1, 2]);
    /// # Ok::<(), oxbloom_http_client::HttpClientError>(())
    /// ```
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, HttpClientError> {
        serde_json::from_slice(&self.body).map_err(|_| HttpClientError::Decode)
    }

    /// Returns the response when its status is 2xx.
    ///
    /// # Errors
    ///
    /// Returns [`HttpClientError::BadStatus`] for any other status.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_http_client::{HttpClientError, Response};
    ///
    /// let ok = Response::new(http::StatusCode::OK, http::HeaderMap::new(), Vec::new());
    /// assert!(ok.error_for_status().is_ok());
    /// let bad = Response::new(http::StatusCode::NOT_FOUND, http::HeaderMap::new(), Vec::new());
    /// assert_eq!(bad.error_for_status().unwrap_err(), HttpClientError::BadStatus(http::StatusCode::NOT_FOUND));
    /// ```
    pub fn error_for_status(self) -> Result<Self, HttpClientError> {
        if self.status.is_success() {
            Ok(self)
        } else {
            Err(HttpClientError::BadStatus(self.status))
        }
    }
}

impl fmt::Debug for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpClientError;
    use http::{HeaderMap, StatusCode};

    fn response(status: StatusCode, body: &[u8]) -> Response {
        Response::new(status, HeaderMap::new(), body.to_vec())
    }

    #[test]
    fn response_text_and_json_decode() {
        let r = response(StatusCode::OK, br#"{"n":7}"#);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.bytes(), br#"{"n":7}"#);
        assert_eq!(r.text().unwrap(), r#"{"n":7}"#);
        let value: serde_json::Value = r.json().unwrap();
        assert_eq!(value["n"], 7);
        assert!(r.headers().is_empty());
        assert_eq!(
            response(StatusCode::OK, &[0xff]).text(),
            Err(HttpClientError::Decode)
        );
    }

    #[test]
    fn response_json_decode_error_is_fixed_text() {
        let r = response(StatusCode::OK, b"not json with secret-token");
        let error = r.json::<serde_json::Value>().unwrap_err();
        assert_eq!(error, HttpClientError::Decode);
        assert!(!error.to_string().contains("secret-token"));
        assert!(!format!("{r:?}").contains("secret-token"));
    }

    #[test]
    fn error_for_status_passes_2xx_and_fails_others_with_bad_status() {
        assert!(
            response(StatusCode::NO_CONTENT, b"")
                .error_for_status()
                .is_ok()
        );
        assert_eq!(
            response(StatusCode::BAD_GATEWAY, b"")
                .error_for_status()
                .unwrap_err(),
            HttpClientError::BadStatus(StatusCode::BAD_GATEWAY)
        );
        assert!(response(StatusCode::FOUND, b"").error_for_status().is_err());
    }
}
