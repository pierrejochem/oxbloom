//! [`DataError`], the error of the data core.

use std::fmt;

/// The failure of the data core.
///
/// Every variant has a fixed message and no payload, so an error can be logged and shown
/// without leaking a connection URL, a host name or a password. The cause is logged at the
/// place it happens, with the name of the source.
///
/// # Examples
///
/// ```
/// use oxbloom_data::DataError;
///
/// assert_eq!(DataError::Connect.to_string(), "could not connect to the database");
/// let boxed: Box<dyn std::error::Error> = Box::new(DataError::Tls);
/// assert_eq!(boxed.to_string(), "the TLS configuration of the database is unusable");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DataError {
    /// The `data.*` configuration is invalid.
    Config,
    /// A connection could not be made.
    Connect,
    /// The connection pool failed, for example it timed out or is closed.
    Pool,
    /// The driver of the URL is not compiled in, or the feature is not supported.
    Unsupported,
    /// The TLS settings of a source cannot be applied.
    Tls,
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Config => "the data configuration is invalid",
            Self::Connect => "could not connect to the database",
            Self::Pool => "the database connection pool failed",
            Self::Unsupported => "the database driver is not available in this build",
            Self::Tls => "the TLS configuration of the database is unusable",
        })
    }
}

impl std::error::Error for DataError {}
