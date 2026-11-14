use std::fmt;
use std::path::PathBuf;

use oxbloom_config::ConfigError;

/// Error returned while reading the logging configuration or installing the subscriber.
///
/// Messages name configuration keys, never their values.
///
/// # Examples
///
/// ```
/// use oxbloom_config::ConfigError;
/// use oxbloom_logging::LoggingError;
///
/// let err = LoggingError::from(ConfigError::invalid("logging.level.app", "not a level"));
/// assert!(matches!(err, LoggingError::Config(ConfigError::Invalid { .. })));
/// assert_eq!(
///     err.to_string(),
///     "invalid configuration key `logging.level.app`: not a level"
/// );
/// ```
///
/// The enum is `#[non_exhaustive]`: a `match` outside this crate needs a wildcard arm.
///
/// ```
/// use oxbloom_logging::LoggingError;
///
/// fn fatal(err: &LoggingError) -> bool {
///     match err {
///         LoggingError::AlreadyInstalled => false,
///         _ => true,
///     }
/// }
/// assert!(!fatal(&LoggingError::AlreadyInstalled));
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum LoggingError {
    /// A global `tracing` subscriber or `log` logger is already installed in this process.
    AlreadyInstalled,
    /// A `logging.*` key has an invalid value; the error names the key.
    Config(ConfigError),
    /// The level filter could not be built or replaced. The text describes the problem
    /// without configuration values.
    Filter(String),
    /// A log directory or file could not be created or opened. The message names the path;
    /// [`source`](std::error::Error::source) is the I/O error.
    Io {
        /// The directory or file that failed.
        path: PathBuf,
        /// The I/O error.
        source: std::io::Error,
    },
}

/// `Config` shows the configuration error unchanged; the other variants have fixed texts.
///
/// ```
/// use oxbloom_logging::LoggingError;
///
/// assert_eq!(
///     LoggingError::AlreadyInstalled.to_string(),
///     "a global tracing subscriber or `log` logger is already installed"
/// );
/// assert_eq!(
///     LoggingError::Filter("reload failed".into()).to_string(),
///     "invalid logging filter: reload failed"
/// );
/// let io = LoggingError::Io {
///     path: "logs".into(),
///     source: std::io::Error::other("denied"),
/// };
/// assert_eq!(io.to_string(), "cannot write log file at `logs`");
/// ```
impl fmt::Display for LoggingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInstalled => {
                f.write_str("a global tracing subscriber or `log` logger is already installed")
            }
            // Transparent: the configuration error already names the key.
            Self::Config(e) => fmt::Display::fmt(e, f),
            Self::Filter(message) => write!(f, "invalid logging filter: {message}"),
            Self::Io { path, .. } => write!(f, "cannot write log file at `{}`", path.display()),
        }
    }
}

/// `Config` is transparent, so its source is the configuration error's own source.
///
/// ```
/// use std::error::Error;
/// use oxbloom_config::ConfigError;
/// use oxbloom_logging::LoggingError;
///
/// let err = LoggingError::from(ConfigError::invalid("logging.level.root", "not a level"));
/// assert!(err.source().is_none());
///
/// let io = LoggingError::Io {
///     path: "logs".into(),
///     source: std::io::Error::other("denied"),
/// };
/// assert_eq!(io.source().unwrap().to_string(), "denied");
/// ```
impl std::error::Error for LoggingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(e) => e.source(),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Wraps a configuration error, so `?` works on `Config::get` in logging code.
///
/// ```
/// use oxbloom_config::{Config, ConfigError};
/// use oxbloom_logging::{Level, LoggingError};
///
/// fn root(config: &Config) -> Result<Level, LoggingError> {
///     Ok(config.get("logging.level.root")?)
/// }
/// let config = Config::from_str("[logging.level]\nroot = \"loud\"")?;
/// assert!(matches!(root(&config), Err(LoggingError::Config(ConfigError::Invalid { .. }))));
/// # Ok::<(), ConfigError>(())
/// ```
impl From<ConfigError> for LoggingError {
    fn from(e: ConfigError) -> Self {
        Self::Config(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    #[test]
    fn error_is_send_sync_static() {
        assert_send_sync_static::<LoggingError>();
    }
}
