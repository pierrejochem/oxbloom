//! The error type of the session module.

use std::fmt;

/// A session operation failed.
///
/// The texts are fixed strings: they never contain a session id, a value, a key or a cookie.
///
/// # Examples
///
/// ```
/// use oxbloom_security::SessionError;
///
/// assert_eq!(SessionError::TooLarge.to_string(), "the session data is too large");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// The session store failed.
    Store,
    /// Sealing or opening the session cookie failed.
    Codec,
    /// The session data does not fit the configured limit.
    TooLarge,
    /// The session settings are not valid.
    InvalidSettings,
    /// A session value could not be serialized or deserialized.
    Serialization,
    /// The key starts with the prefix `oxbloom.security.`, which Oxbloom reserves for itself.
    ReservedKey,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Store => "the session store failed",
            Self::Codec => "the session cookie could not be sealed or opened",
            Self::TooLarge => "the session data is too large",
            Self::InvalidSettings => "the session settings are not valid",
            Self::Serialization => "a session value could not be serialized",
            Self::ReservedKey => "the session key is reserved for Oxbloom",
        })
    }
}

impl std::error::Error for SessionError {}
