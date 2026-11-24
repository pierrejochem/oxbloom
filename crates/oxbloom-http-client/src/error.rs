use std::fmt;

/// The failure of an HTTP call.
///
/// The text of every variant is fixed. It never contains the URL, a header, the body or a
/// secret, so it is safe to log and to show to a user.
///
/// # Examples
///
/// ```
/// use oxbloom_http_client::HttpClientError;
///
/// let error = HttpClientError::Timeout;
/// assert_eq!(error.to_string(), "the HTTP request timed out");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HttpClientError {
    /// The request ran into its total or connect timeout.
    Timeout,
    /// The connection failed or broke off.
    Network,
    /// The response body is larger than the configured limit.
    TooLarge,
    /// The server answered with a status that is not 2xx (see `Response::error_for_status`).
    BadStatus(http::StatusCode),
    /// The client could not be built from its settings.
    Build,
    /// The response body could not be decoded.
    Decode,
    /// The request is not valid, for example because of a bad header name, a malformed URL
    /// or a scheme other than `http` or `https`.
    ///
    /// It does not always mean the request must be fixed: when redirects are followed, a
    /// server-controlled redirect whose `Location` is malformed or names a scheme other
    /// than `http` or `https` (for example `ftp:`) also surfaces as this variant.
    InvalidRequest,
}

impl fmt::Display for HttpClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Timeout => "the HTTP request timed out",
            Self::Network => "the HTTP request failed because of a network error",
            Self::TooLarge => "the HTTP response body exceeds the size limit",
            Self::BadStatus(_) => "the HTTP response has an error status",
            Self::Build => "the HTTP client could not be built",
            Self::Decode => "the HTTP response body could not be decoded",
            Self::InvalidRequest => "the HTTP request is not valid",
        })
    }
}

impl std::error::Error for HttpClientError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_never_contains_a_url() {
        let all = [
            HttpClientError::Timeout,
            HttpClientError::Network,
            HttpClientError::TooLarge,
            HttpClientError::BadStatus(http::StatusCode::NOT_FOUND),
            HttpClientError::Build,
            HttpClientError::Decode,
            HttpClientError::InvalidRequest,
        ];
        for error in all {
            let text = format!("{error} {error:?}");
            assert!(
                !text.contains("http://") && !text.contains("https://"),
                "{text}"
            );
            assert!(!text.is_empty());
        }
        assert_eq!(
            HttpClientError::Timeout.to_string(),
            "the HTTP request timed out"
        );
    }
}
