//! Logging for Oxbloom applications: levels, groups and console options read from the
//! `logging.*` configuration keys.
//!
//! Levels come from `logging.level.root` and `logging.level.<target>`; `logging.group.<name>`
//! lists targets that share the level of `logging.level.<name>`. Invalid values are a
//! [`ConfigError::Invalid`](oxbloom_config::ConfigError::Invalid) that names the key, never the
//! value. [`PROPERTIES`] lists every key this crate reads.
//!
//! Besides the console, the crate writes rotating log files (`logging.file.*`, rotation
//! [`FileRotation`], layout [`FileFormat`]) and JSON lines (`logging.console.format` or
//! `logging.file.format` set to `json`, with the fields of [`JsonField`] switched on or off and
//! renamed through `logging.json.*`). The file writer is non-blocking and drops events when its
//! buffer is full; buffered lines are flushed when the last [`LoggingSystem`] clone drops. Pruning
//! with `logging.file.max-history` considers every file in the directory whose name starts with
//! the file name prefix, so use a dedicated directory. A directory or file that cannot be opened is
//! a [`LoggingError::Io`]. The output layers are fixed at install time; the level methods of
//! [`LoggingSystem`] change levels only. The `logging.request.*` keys of the HTTP request log
//! belong to `oxbloom-web` and `oxbloom-boot`.
//!
//! ```
//! use oxbloom_config::Config;
//! use oxbloom_logging::{ConsoleFormat, Level, PROPERTIES};
//!
//! let config = Config::from_str(
//!     "[logging.level]\nroot = \"warn\"\n\"oxbloom_web::router\" = \"debug\"\n\
//!      [logging.console]\nformat = \"compact\"",
//! )?;
//! assert_eq!(config.get::<Level>("logging.level.root")?, Level::Warn);
//! assert_eq!(config.get::<ConsoleFormat>("logging.console.format")?, ConsoleFormat::Compact);
//! assert_eq!(PROPERTIES.len(), 13);
//! # Ok::<(), oxbloom_config::ConfigError>(())
//! ```

mod error;
mod file;
mod json;
mod level;
mod settings;
mod system;

pub use error::LoggingError;
pub use level::{Level, ParseLevelError};
pub use settings::{ConsoleColor, ConsoleFormat, FileFormat, FileRotation, JsonField, PROPERTIES};
pub use system::LoggingSystem;

/// Support for this crate's integration tests; not a public API.
#[doc(hidden)]
pub mod __private {
    use std::sync::Arc;

    use oxbloom_config::Config;
    use oxbloom_core::Clock;
    use tracing::Dispatch;
    use tracing_subscriber::fmt::MakeWriter;

    pub use crate::settings::Settings;
    use crate::{LoggingError, LoggingSystem};

    /// A [`LoggingSystem`] whose subscriber is returned instead of installed, for use with
    /// `tracing::dispatcher::with_default`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::Level;
    /// use oxbloom_logging::__private::scoped;
    ///
    /// let (dispatch, logging) = scoped(&Config::from_str("")?, std::io::sink)?;
    /// logging.set_level("app", Level::Debug)?;
    /// tracing::dispatcher::with_default(&dispatch, || tracing::debug!(target: "app", "kept"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn scoped<W>(config: &Config, writer: W) -> Result<(Dispatch, LoggingSystem), LoggingError>
    where
        W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    {
        LoggingSystem::build(config, writer)
    }

    /// [`scoped`] with the JSON timestamps read from `clock` instead of the system time.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::SystemTime;
    /// use oxbloom_config::Config;
    /// use oxbloom_core::Clock;
    /// use oxbloom_logging::__private::scoped_with_clock;
    ///
    /// struct AtEpoch;
    ///
    /// impl Clock for AtEpoch {
    ///     fn now(&self) -> SystemTime {
    ///         SystemTime::UNIX_EPOCH
    ///     }
    /// }
    ///
    /// let config = Config::from_str("[logging.console]\nformat = \"json\"")?;
    /// let (dispatch, _logging) = scoped_with_clock(&config, std::io::sink, Arc::new(AtEpoch))?;
    /// tracing::dispatcher::with_default(&dispatch, || tracing::info!("stamped at the epoch"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn scoped_with_clock<W>(
        config: &Config,
        writer: W,
        clock: Arc<dyn Clock>,
    ) -> Result<(Dispatch, LoggingSystem), LoggingError>
    where
        W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    {
        LoggingSystem::build_with_clock(config, writer, clock)
    }
}
