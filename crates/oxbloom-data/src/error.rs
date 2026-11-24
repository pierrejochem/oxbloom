//! [`DataError`], the error of the data core, and [`SourceError`], which names the source.

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

/// A [`DataError`] of one data source, with the name of that source.
///
/// [`DataSources::build`](crate::DataSources::build) returns it, so a failure report can say
/// which source and which configuration key to fix. The `Display` text is fixed apart from
/// the source name, which is a configuration name and not a secret; it never contains the
/// URL. There is no [`source`](std::error::Error::source) chain.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use oxbloom_config::Config;
/// use oxbloom_data::{DataError, DataSettings, DataSources};
///
/// // sqlx refuses the value of `mode`; the error names the source, never the value.
/// let config = Config::from_str(
///     "[data]\nurl = \"sqlite::memory:\"\n[data.sources.cache]\nurl = \"sqlite:c.db?mode=fast\"",
/// )?;
/// let err = DataSources::build(&DataSettings::from_config(&config)?, &|_| Err(DataError::Tls))
///     .unwrap_err();
/// assert_eq!(err.source_name(), "cache");
/// assert_eq!(err.error(), DataError::Config);
/// assert_eq!(err.config_prefix(), "data.sources.cache");
/// assert_eq!(err.to_string(), "data source `cache`: the data configuration is invalid");
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "sqlite"))]
/// # fn main() {}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct SourceError {
    /// The name of the failing source: `default` or the name under `data.sources`.
    pub source_name: String,
    /// What went wrong.
    pub error: DataError,
}

impl SourceError {
    pub(crate) fn new(source_name: &str, error: DataError) -> Self {
        Self {
            source_name: source_name.to_string(),
            error,
        }
    }

    /// The name of the failing source.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// // `sqlite:` without the `sqlite` feature, or a URL sqlx refuses: either way it fails.
    /// let config = Config::from_str("[data.sources.cache]\nurl = \"sqlite:c.db?mode=fast\"")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// # let _rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    /// # let _guard = _rt.enter();
    /// let err = DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap_err();
    /// assert_eq!(err.source_name(), "cache");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn source_name(&self) -> &str {
        &self.source_name
    }

    /// The error of the source.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite:c.db?mode=fast\"")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// # let _rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    /// # let _guard = _rt.enter();
    /// let err = DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap_err();
    /// // `Config` with the `sqlite` feature, `Unsupported` without it.
    /// assert!(matches!(err.error(), DataError::Config | DataError::Unsupported));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn error(&self) -> DataError {
        self.error
    }

    /// The configuration key of the failing source: `data` for the default source and
    /// `data.sources.<name>` for a named one.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let config = Config::from_str("[data]\nurl = \"sqlite:c.db?mode=fast\"")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// # let _rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    /// # let _guard = _rt.enter();
    /// let err = DataSources::build(&settings, &|_| Err(DataError::Tls)).unwrap_err();
    /// assert_eq!(err.config_prefix(), "data");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn config_prefix(&self) -> String {
        crate::settings::config_prefix(&self.source_name)
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "data source `{}`: {}", self.source_name, self.error)
    }
}

impl std::error::Error for SourceError {}
