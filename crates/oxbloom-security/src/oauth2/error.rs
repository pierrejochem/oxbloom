//! The error of every OAuth2 login step, with fixed text.

use std::fmt;

/// Why an OAuth2 login step failed. The text is fixed: no variant carries a URL, a token, a
/// code or a response body. [`Provider`](Self::Provider) holds an RFC 6749 error code from an
/// allow-list, or `unknown`.
///
/// # Examples
///
/// ```
/// use oxbloom_security::__test_support::OAuth2Error;
///
/// assert_eq!(
///     OAuth2Error::Provider("invalid_grant").to_string(),
///     "the OAuth2 provider answered with the error code invalid_grant"
/// );
/// assert_eq!(OAuth2Error::InvalidIdToken.to_string(), "the id token is invalid");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OAuth2Error {
    /// The registration or the provider is not usable, for example an unknown registration.
    Config,
    /// The provider metadata could not be loaded or is not usable.
    Discovery,
    /// The issuer in the discovery document is not the configured issuer.
    IssuerMismatch,
    /// A request to the provider failed before an answer arrived.
    Http,
    /// The token endpoint answered with this error code.
    Provider(&'static str),
    /// The token endpoint answered with an error status or an unusable body.
    Token,
    /// The id token failed validation.
    InvalidIdToken,
    /// The user info could not be loaded or is not a JSON object.
    UserInfo,
    /// The login state is missing, expired or does not match.
    Pending,
}

impl fmt::Display for OAuth2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config => f.write_str("the OAuth2 client configuration is not usable"),
            Self::Discovery => f.write_str("the OAuth2 provider metadata could not be loaded"),
            Self::IssuerMismatch => {
                f.write_str("the issuer of the provider metadata is not the configured issuer")
            }
            Self::Http => f.write_str("the request to the OAuth2 provider failed"),
            Self::Provider(code) => {
                write!(f, "the OAuth2 provider answered with the error code {code}")
            }
            Self::Token => f.write_str("the token response is invalid"),
            Self::InvalidIdToken => f.write_str("the id token is invalid"),
            Self::UserInfo => f.write_str("the user info could not be loaded"),
            Self::Pending => f.write_str("the login state is missing, expired or does not match"),
        }
    }
}

impl std::error::Error for OAuth2Error {}

/// The RFC 6749 section 5.2 error codes that are surfaced; every other code is `unknown`.
const ALLOWED_CODES: [&str; 6] = [
    "invalid_request",
    "invalid_client",
    "invalid_grant",
    "unauthorized_client",
    "unsupported_grant_type",
    "invalid_scope",
];

/// The allow-listed code equal to `code`, or `unknown`.
pub(crate) fn allowed_code(code: &str) -> &'static str {
    ALLOWED_CODES
        .iter()
        .find(|allowed| **allowed == code)
        .copied()
        .unwrap_or("unknown")
}

/// The RFC 6749 section 4.1.2.1 error codes of an authorization response that are logged;
/// every other code is `unknown`.
const AUTHORIZATION_CODES: [&str; 7] = [
    "invalid_request",
    "unauthorized_client",
    "access_denied",
    "unsupported_response_type",
    "invalid_scope",
    "server_error",
    "temporarily_unavailable",
];

/// The allow-listed authorization response code equal to `code`, or `unknown`.
pub(crate) fn allowed_authorization_code(code: &str) -> &'static str {
    AUTHORIZATION_CODES
        .iter()
        .find(|allowed| **allowed == code)
        .copied()
        .unwrap_or("unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allow_listed_authorization_codes_are_kept() {
        assert_eq!(allowed_authorization_code("access_denied"), "access_denied");
        assert_eq!(
            allowed_authorization_code("temporarily_unavailable"),
            "temporarily_unavailable"
        );
        assert_eq!(allowed_authorization_code("invalid_grant"), "unknown");
        assert_eq!(allowed_authorization_code("ACCESS_DENIED"), "unknown");
        assert_eq!(allowed_authorization_code("<script>"), "unknown");
    }

    #[test]
    fn only_allow_listed_codes_are_kept() {
        assert_eq!(allowed_code("invalid_scope"), "invalid_scope");
        assert_eq!(allowed_code("INVALID_SCOPE"), "unknown");
        assert_eq!(allowed_code("access_denied"), "unknown");
        assert_eq!(allowed_code(""), "unknown");
    }
}
