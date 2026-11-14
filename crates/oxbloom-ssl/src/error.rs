//! The error type of SSL bundle loading.

use std::fmt;
use std::io::ErrorKind;

use oxbloom_config::ConfigError;

/// Error returned while loading an SSL bundle or building a rustls configuration from it.
///
/// Every text is fixed. It names the bundle and the configuration key to fix, and never
/// contains a file path, file contents, a private key or a password: a path may itself be
/// a misplaced secret, so even [`Io`](SslError::Io) names the key instead of the path.
///
/// # Examples
///
/// ```
/// use oxbloom_ssl::SslError;
///
/// let err = SslError::KeyMismatch { bundle: "web".into() };
/// assert_eq!(
///     err.to_string(),
///     "the private key of SSL bundle `web` does not match its certificate"
/// );
/// ```
///
/// The enum is `#[non_exhaustive]`: a `match` outside this crate needs a wildcard arm.
///
/// ```
/// use oxbloom_ssl::SslError;
///
/// fn bundle_name(err: &SslError) -> Option<&str> {
///     match err {
///         SslError::NoIdentity { bundle } | SslError::KeyMismatch { bundle } => Some(bundle),
///         _ => None,
///     }
/// }
/// assert_eq!(bundle_name(&SslError::NoIdentity { bundle: "client".into() }), Some("client"));
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum SslError {
    /// A file named by `key` could not be read. `kind` is the I/O error kind.
    Io {
        /// The bundle name.
        bundle: String,
        /// The full configuration key whose file failed, such as `ssl.bundle.pem.web.certificate`.
        key: String,
        /// The kind of the I/O error, such as [`ErrorKind::NotFound`].
        kind: ErrorKind,
    },
    /// The value or file named by `key` holds no usable PEM data: it is empty, malformed,
    /// encrypted, or of an unsupported key type.
    Pem {
        /// The bundle name.
        bundle: String,
        /// The full configuration key, such as `ssl.bundle.pem.web.private-key`.
        key: String,
    },
    /// A server configuration was requested from a bundle without a certificate and key. A
    /// reload also reports it when the new files would remove the identity the bundle has.
    NoIdentity {
        /// The bundle name.
        bundle: String,
    },
    /// The private key does not belong to the first certificate of the chain.
    KeyMismatch {
        /// The bundle name.
        bundle: String,
    },
    /// The PKCS#12 file could not be decoded: a wrong password, a damaged file, or an
    /// unsupported format.
    Pkcs12 {
        /// The bundle name.
        bundle: String,
    },
    /// The bundle has neither an identity nor a trust store. A reload also reports it when
    /// the new files would remove the trust store the bundle has.
    EmptyBundle {
        /// The bundle name.
        bundle: String,
    },
    /// A reload would add an identity or a trust store the bundle did not have. The parts a
    /// bundle has are fixed when it is first loaded; changing them needs a restart.
    ///
    /// ```
    /// use oxbloom_ssl::SslError;
    ///
    /// let err = SslError::ShapeChanged { bundle: "web".into() };
    /// assert_eq!(
    ///     err.to_string(),
    ///     "the new files of SSL bundle `web` add an identity or a trust store it did not \
    ///      have; restart the application to change the parts of a bundle"
    /// );
    /// ```
    ShapeChanged {
        /// The bundle name.
        bundle: String,
    },
    /// The same bundle name is used under both `ssl.bundle.pem` and `ssl.bundle.pkcs12`.
    DuplicateName {
        /// The bundle name, as written under `ssl.bundle.pem`.
        name: String,
    },
    /// No bundle has the requested name.
    UnknownBundle {
        /// The requested name.
        name: String,
        /// The names of the loaded bundles, sorted.
        available: Vec<String>,
    },
    /// An `ssl.*` key has an invalid value or is missing; the error names the key.
    Config(ConfigError),
}

/// `Config` shows the configuration error unchanged; the other variants have fixed texts.
///
/// ```
/// use std::io::ErrorKind;
/// use oxbloom_ssl::SslError;
///
/// let io = SslError::Io {
///     bundle: "web".into(),
///     key: "ssl.bundle.pem.web.certificate".into(),
///     kind: ErrorKind::NotFound,
/// };
/// assert_eq!(
///     io.to_string(),
///     "cannot read `ssl.bundle.pem.web.certificate` of SSL bundle `web`: entity not found"
/// );
/// let unknown = SslError::UnknownBundle { name: "api".into(), available: vec![] };
/// assert_eq!(unknown.to_string(), "unknown SSL bundle `api`; available bundles: none");
/// ```
impl fmt::Display for SslError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { bundle, key, kind } => {
                write!(f, "cannot read `{key}` of SSL bundle `{bundle}`: {kind}")
            }
            Self::Pem { bundle, key } => write!(
                f,
                "`{key}` of SSL bundle `{bundle}` holds no usable unencrypted PEM data"
            ),
            Self::NoIdentity { bundle } => write!(
                f,
                "SSL bundle `{bundle}` has no certificate and private key"
            ),
            Self::KeyMismatch { bundle } => write!(
                f,
                "the private key of SSL bundle `{bundle}` does not match its certificate"
            ),
            Self::Pkcs12 { bundle } => write!(
                f,
                "cannot decode the PKCS#12 file of SSL bundle `{bundle}`: wrong password, \
                 damaged file or unsupported format"
            ),
            Self::EmptyBundle { bundle } => write!(
                f,
                "SSL bundle `{bundle}` has neither an identity nor a trust store"
            ),
            Self::ShapeChanged { bundle } => write!(
                f,
                "the new files of SSL bundle `{bundle}` add an identity or a trust store it did \
                 not have; restart the application to change the parts of a bundle"
            ),
            Self::DuplicateName { name } => write!(
                f,
                "SSL bundle name `{name}` is used under both `ssl.bundle.pem` and \
                 `ssl.bundle.pkcs12`"
            ),
            Self::UnknownBundle { name, available } => {
                let list = if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                };
                write!(f, "unknown SSL bundle `{name}`; available bundles: {list}")
            }
            // Transparent: the configuration error already names the key.
            Self::Config(e) => fmt::Display::fmt(e, f),
        }
    }
}

/// `Config` is transparent, so its source is the configuration error's own source. The other
/// variants have no source.
///
/// ```
/// use std::error::Error;
/// use oxbloom_ssl::SslError;
///
/// assert!(SslError::EmptyBundle { bundle: "web".into() }.source().is_none());
/// ```
impl std::error::Error for SslError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => e.source(),
            _ => None,
        }
    }
}

/// Wraps a configuration error, so `?` works on `Config::get` in SSL code.
///
/// ```
/// use oxbloom_config::ConfigError;
/// use oxbloom_ssl::SslError;
///
/// let err = SslError::from(ConfigError::Missing { key: "ssl.bundle.pem.web.certificate".into() });
/// assert!(matches!(err, SslError::Config(ConfigError::Missing { .. })));
/// ```
impl From<ConfigError> for SslError {
    fn from(e: ConfigError) -> Self {
        Self::Config(e)
    }
}
