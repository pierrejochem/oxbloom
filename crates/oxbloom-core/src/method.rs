use std::fmt;
use std::str::FromStr;

/// HTTP request method.
///
/// Method names are case-sensitive (RFC 9110 §9.1): `"get"` is not `GET`
/// and parses as [`HttpMethod::Other`].
///
/// # Examples
///
/// ```
/// use oxbloom_core::HttpMethod;
///
/// let method: HttpMethod = "PATCH".parse()?;
/// assert_eq!(method, HttpMethod::Patch);
/// assert_eq!(HttpMethod::Delete.to_string(), "DELETE");
///
/// // Case matters: lowercase is a custom method, not GET.
/// assert_eq!("get".parse::<HttpMethod>()?, HttpMethod::Other("get".into()));
/// # Ok::<(), oxbloom_core::InvalidHttpMethod>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Trace,
    Connect,
    /// Any other valid method token.
    Other(String),
}

/// Error returned when a string is not a valid HTTP method token.
///
/// # Examples
///
/// ```
/// use oxbloom_core::{HttpMethod, InvalidHttpMethod};
///
/// let err: InvalidHttpMethod = "NOT VALID".parse::<HttpMethod>().unwrap_err();
/// assert_eq!(err.to_string(), "invalid HTTP method token: \"NOT VALID\"");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidHttpMethod(String);

impl fmt::Display for InvalidHttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid HTTP method token: {:?}", self.0)
    }
}

impl std::error::Error for InvalidHttpMethod {}

impl HttpMethod {
    /// Canonical method name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_core::HttpMethod;
    ///
    /// assert_eq!(HttpMethod::Get.as_str(), "GET");
    /// assert_eq!(HttpMethod::Other("PURGE".into()).as_str(), "PURGE");
    /// ```
    pub fn as_str(&self) -> &str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
            Self::Connect => "CONNECT",
            Self::Other(s) => s,
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

/// Parses a method token. Standard names map to their variants; any other valid token becomes
/// [`HttpMethod::Other`]; an empty string or a token with illegal characters is an error.
///
/// # Examples
///
/// ```
/// use oxbloom_core::HttpMethod;
///
/// assert_eq!("POST".parse::<HttpMethod>()?, HttpMethod::Post);
/// assert_eq!("PURGE".parse::<HttpMethod>()?, HttpMethod::Other("PURGE".into()));
/// assert!("".parse::<HttpMethod>().is_err());
/// assert!("GE T".parse::<HttpMethod>().is_err());
/// # Ok::<(), oxbloom_core::InvalidHttpMethod>(())
/// ```
impl FromStr for HttpMethod {
    type Err = InvalidHttpMethod;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "GET" => Self::Get,
            "POST" => Self::Post,
            "PUT" => Self::Put,
            "PATCH" => Self::Patch,
            "DELETE" => Self::Delete,
            "HEAD" => Self::Head,
            "OPTIONS" => Self::Options,
            "TRACE" => Self::Trace,
            "CONNECT" => Self::Connect,
            other if !other.is_empty() && other.bytes().all(is_tchar) => {
                Self::Other(other.to_owned())
            }
            other => return Err(InvalidHttpMethod(other.to_owned())),
        })
    }
}

/// Converts the `http` crate's method, which is how the server maps Hyper requests.
///
/// # Examples
///
/// ```
/// use oxbloom_core::HttpMethod;
///
/// assert_eq!(HttpMethod::from(&http::Method::PUT), HttpMethod::Put);
/// ```
impl From<&http::Method> for HttpMethod {
    fn from(m: &http::Method) -> Self {
        // `http::Method` only holds valid tokens, so this cannot fail.
        m.as_str().parse().expect("http::Method is a valid token")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_methods() {
        for (s, m) in [
            ("GET", HttpMethod::Get),
            ("POST", HttpMethod::Post),
            ("PUT", HttpMethod::Put),
            ("PATCH", HttpMethod::Patch),
            ("DELETE", HttpMethod::Delete),
            ("HEAD", HttpMethod::Head),
            ("OPTIONS", HttpMethod::Options),
            ("TRACE", HttpMethod::Trace),
            ("CONNECT", HttpMethod::Connect),
        ] {
            assert_eq!(s.parse::<HttpMethod>().unwrap(), m);
            assert_eq!(m.to_string(), s);
        }
    }

    #[test]
    fn is_case_sensitive() {
        assert_eq!(
            "get".parse::<HttpMethod>().unwrap(),
            HttpMethod::Other("get".into())
        );
    }

    #[test]
    fn accepts_custom_token() {
        let m: HttpMethod = "PURGE".parse().unwrap();
        assert_eq!(m, HttpMethod::Other("PURGE".into()));
        assert_eq!(m.to_string(), "PURGE");
    }

    #[test]
    fn rejects_invalid_tokens() {
        for s in ["", "GE T", "GET\r\n", "PO/ST", "ÄÖ"] {
            assert!(s.parse::<HttpMethod>().is_err(), "{s:?} should be rejected");
        }
    }

    #[test]
    fn converts_from_http_method() {
        assert_eq!(HttpMethod::from(&http::Method::PATCH), HttpMethod::Patch);
        let custom = http::Method::from_bytes(b"PURGE").unwrap();
        assert_eq!(HttpMethod::from(&custom), HttpMethod::Other("PURGE".into()));
    }
}
