use std::fmt;
use std::path::PathBuf;

use crate::origin::Origin;

/// Error returned while loading or binding configuration.
///
/// Messages name keys and file paths only; configuration values never appear in them. The one
/// exception is [`UnresolvedPlaceholder`](Self::UnresolvedPlaceholder): its `name` is text from a
/// file value (at most 32 characters). Environment and command-line values are taken literally;
/// placeholders are resolved only in values from files, so those values never reach that error.
///
/// # Examples
///
/// ```
/// use oxbloom_config::{Config, ConfigError};
///
/// let config = Config::from_str("[server]\nport = \"abc\"")?;
///
/// // Wrong type: the message names the key, never the value.
/// match config.get::<u16>("server.port") {
///     Err(ConfigError::Invalid { key, .. }) => assert_eq!(key, "server.port"),
///     other => panic!("{other:?}"),
/// }
/// // Absent key.
/// assert!(matches!(config.get::<u16>("nope"), Err(ConfigError::Missing { .. })));
/// // Broken TOML.
/// assert!(matches!(Config::from_str("a = = 1"), Err(ConfigError::Parse { .. })));
/// # Ok::<(), ConfigError>(())
/// ```
///
/// The enum is `#[non_exhaustive]`: later versions may add variants, so a `match` outside this
/// crate needs a wildcard arm.
///
/// ```
/// use oxbloom_config::ConfigError;
///
/// fn kind(err: &ConfigError) -> &'static str {
///     match err {
///         ConfigError::Missing { .. } => "missing",
///         ConfigError::Invalid { .. } => "invalid",
///         _ => "other",
///     }
/// }
/// assert_eq!(kind(&ConfigError::invalid("a", "bad")), "invalid");
/// ```
///
/// Without the wildcard arm the match does not compile:
///
/// ```compile_fail
/// use oxbloom_config::ConfigError;
///
/// fn kind(err: &ConfigError) -> &'static str {
///     match err {
///         ConfigError::Io { .. } => "io",
///         ConfigError::Parse { .. } => "parse",
///         ConfigError::Missing { .. } => "missing",
///         ConfigError::Invalid { .. } => "invalid",
///         ConfigError::DuplicateKey { .. } => "duplicate",
///         ConfigError::ImportCycle { .. } => "cycle",
///         ConfigError::ImportMissing { .. } => "import missing",
///         ConfigError::ImportInvalid { .. } => "import invalid",
///         ConfigError::ImportTooDeep => "too deep",
///         ConfigError::Deprecated { .. } => "deprecated",
///         ConfigError::ImportLimit => "limit",
///         ConfigError::FileTooLarge { .. } => "too large",
///         ConfigError::UnresolvedPlaceholder { .. } => "unresolved",
///         ConfigError::PlaceholderCycle { .. } => "placeholder cycle",
///         ConfigError::PlaceholderTooLarge { .. } => "placeholder too large",
///     }
/// }
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigError {
    /// A configuration file could not be read.
    Io {
        /// File that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A configuration file is not valid TOML.
    Parse {
        /// File that failed to parse.
        path: PathBuf,
        /// 1-based line of the error, when known.
        line: Option<usize>,
        /// Fixed description without file contents.
        message: String,
    },
    /// A required key is absent.
    Missing {
        /// Dotted key path.
        key: String,
    },
    /// A key is present but has the wrong type or an unusable value.
    Invalid {
        /// Dotted key path.
        key: String,
        /// Description of the problem, never containing the value.
        message: String,
        /// Where the failing value came from, when it has an origin.
        origin: Option<Origin>,
    },
    /// Two spellings of one canonical key (`max-conn`, `max_conn`) in the same file (or `Config::from_str` text). Two spellings in the environment or on the command line are not reported.
    DuplicateKey {
        /// Dotted key path, as spelled by the first occurrence.
        key: String,
        /// Where the key was first defined.
        first: Origin,
        /// Where the conflicting spelling was defined.
        second: Origin,
    },
    /// A `oxbloom.config.import` chain reaches a file that is already being imported.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application.toml"), "oxbloom.config.import = [\"a.toml\"]")?;
    /// std::fs::write(dir.path().join("a.toml"), "oxbloom.config.import = [\"a.toml\"]")?;
    ///
    /// let err = Config::builder().dir(dir.path()).env_from([]).build().unwrap_err();
    /// assert!(matches!(&err, ConfigError::ImportCycle { files } if files.len() == 2));
    /// assert_eq!(err.to_string(), "configuration imports form a cycle");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ImportCycle {
        /// The import chain, relative to the config directory, ending with the repeated file.
        files: Vec<PathBuf>,
    },
    /// A `oxbloom.config.import` entry without the `optional:` prefix names a file that does not exist.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application.toml"), "oxbloom.config.import = [\"gone.toml\"]")?;
    ///
    /// let err = Config::builder().dir(dir.path()).env_from([]).build().unwrap_err();
    /// assert!(matches!(&err, ConfigError::ImportMissing { path } if path.ends_with("gone.toml")));
    /// assert_eq!(err.to_string(), "configuration import file not found");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ImportMissing {
        /// The entry's path, relative to the config directory.
        path: PathBuf,
    },
    /// A `oxbloom.config.import` entry is absolute, contains `..` or resolves (through symlinks) to a
    /// file outside the config directory.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application.toml"), "oxbloom.config.import = [\"../x.toml\"]")?;
    ///
    /// let err = Config::builder().dir(dir.path()).env_from([]).build().unwrap_err();
    /// assert!(matches!(err, ConfigError::ImportInvalid { .. }));
    /// assert_eq!(
    ///     err.to_string(),
    ///     "configuration import path must be relative and stay inside the config directory"
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ImportInvalid {
        /// The entry's path as written, without the `optional:` prefix.
        path: PathBuf,
    },
    /// `oxbloom.config.import` files are nested more than 8 levels deep.
    ///
    /// ```
    /// use oxbloom_config::ConfigError;
    ///
    /// assert_eq!(
    ///     ConfigError::ImportTooDeep.to_string(),
    ///     "configuration imports are nested more than 8 levels deep"
    /// );
    /// ```
    ImportTooDeep,
    /// `deny_deprecated(true)` was set and a renamed or removed key is still configured.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application.toml"), "[old]\nkey = 1")?;
    ///
    /// let err = Config::builder()
    ///     .dir(dir.path())
    ///     .env_from([])
    ///     .renamed("old.key", "new.key")
    ///     .deny_deprecated(true)
    ///     .build()
    ///     .unwrap_err();
    /// assert!(matches!(&err, ConfigError::Deprecated { key, .. } if key == "old.key"));
    /// assert_eq!(
    ///     err.to_string(),
    ///     "deprecated configuration key `old.key` (from application.toml:2)"
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    Deprecated {
        /// The deprecated key, as registered.
        key: String,
        /// Where the deprecated key is set.
        origin: Origin,
    },
    /// One load followed more than 256 `oxbloom.config.import` entries in total (all files, nested
    /// imports and repeats included), which guards against import fan-out.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("empty.toml"), "")?;
    /// let list = vec!["\"empty.toml\""; 257].join(", ");
    /// std::fs::write(dir.path().join("application.toml"), format!("oxbloom.config.import = [{list}]"))?;
    ///
    /// let err = Config::builder().dir(dir.path()).env_from([]).build().unwrap_err();
    /// assert!(matches!(err, ConfigError::ImportLimit));
    /// assert_eq!(err.to_string(), "too many configuration imports (more than 256)");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ImportLimit,
    /// A `${name}` placeholder in a file value names neither a configuration key nor an
    /// environment variable and has no default, or it is malformed (unterminated, a nested name,
    /// or it names a table or an array). `name` is empty for a malformed placeholder and holds
    /// at most 32 characters, plus a trailing `…` when it is cut; for an unterminated `${` it is
    /// the text after the `${` (never anything after a `:`). Write `$${` for a literal `${`.
    /// Environment and command-line values are taken literally and never cause this error.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let err = Config::from_str("url = \"${DB_HOST}\"").unwrap_err();
    /// assert!(matches!(
    ///     &err,
    ///     ConfigError::UnresolvedPlaceholder { key, name } if key == "url" && name == "DB_HOST"
    /// ));
    /// assert_eq!(
    ///     err.to_string(),
    ///     "unresolved placeholder `DB_HOST` in configuration key `url`"
    /// );
    ///
    /// // A literal `${` is written `$${`.
    /// let config = Config::from_str("tpl = \"$${DB_HOST}\"")?;
    /// assert_eq!(config.get::<String>("tpl")?, "${DB_HOST}");
    /// # Ok::<(), ConfigError>(())
    /// ```
    UnresolvedPlaceholder {
        /// Dotted key of the string that contains the placeholder.
        key: String,
        /// The placeholder name (a key or environment variable name); empty when malformed.
        name: String,
    },
    /// A configuration file (`application.toml`, a profile file or an import) is larger than
    /// the size limit: 1 MiB unless set with
    /// [`ConfigBuilder::max_file_size`](crate::ConfigBuilder::max_file_size). The text names
    /// neither the file nor its content; `path` holds the file.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application.toml"), "name = \"demo\"")?;
    ///
    /// let err = Config::builder().dir(dir.path()).max_file_size(8).env_from([]).build().unwrap_err();
    /// assert!(matches!(&err, ConfigError::FileTooLarge { path } if path.ends_with("application.toml")));
    /// assert_eq!(err.to_string(), "configuration file is larger than the size limit");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    FileTooLarge {
        /// The file, inside the config directory.
        path: PathBuf,
    },
    /// Placeholders refer to each other in a cycle, or are nested more than 32 levels deep.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let err = Config::from_str("a = \"${b}\"\nb = \"${a}\"").unwrap_err();
    /// assert!(matches!(&err, ConfigError::PlaceholderCycle { keys } if keys == &["a", "b"]));
    /// assert_eq!(err.to_string(), "placeholders form a cycle: a -> b");
    /// ```
    PlaceholderCycle {
        /// The dotted keys involved, in resolution order.
        keys: Vec<String>,
    },
    /// Placeholder expansion would exceed the size budget: 1 MiB for one resolved string or
    /// 16 MiB for all strings resolved by one load. Never contains a value.
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let big = "x".repeat(600_000);
    /// let err = Config::from_str(&format!("big = \"{big}\"\nv = \"${{big}}${{big}}\"")).unwrap_err();
    /// assert!(matches!(&err, ConfigError::PlaceholderTooLarge { key } if key == "v"));
    /// assert_eq!(
    ///     err.to_string(),
    ///     "placeholder expansion in configuration key `v` is too large"
    /// );
    /// ```
    PlaceholderTooLarge {
        /// Dotted key of the string being resolved when the budget ran out.
        key: String,
    },
}

impl ConfigError {
    /// An [`Invalid`](Self::Invalid) error for `key` without an origin. `message` must not
    /// contain the configuration value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::ConfigError;
    ///
    /// let err = ConfigError::invalid("security.jwt.secret", "secret is too short");
    /// assert!(matches!(&err, ConfigError::Invalid { origin: None, .. }));
    /// assert_eq!(
    ///     err.to_string(),
    ///     "invalid configuration key `security.jwt.secret`: secret is too short"
    /// );
    /// ```
    pub fn invalid(key: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            key: key.into(),
            message: message.into(),
            origin: None,
        }
    }
}

/// One-line message with the key or file path and, for parse errors, the line number.
/// `Invalid` also names the origin of the failing value when it has one.
///
/// ```
/// use oxbloom_config::{Config, ConfigError};
/// # let dir = tempfile::tempdir()?;
///
/// let err = ConfigError::Missing { key: "server.port".into() };
/// assert_eq!(err.to_string(), "missing configuration key `server.port`");
///
/// let config = Config::builder()
///     .dir(dir.path())
///     .env_from([("APP_SERVER__PORT".to_string(), "abc".to_string())])
///     .build()?;
/// assert_eq!(
///     config.get::<u16>("server.port").unwrap_err().to_string(),
///     "invalid configuration key `server.port` (from env APP_SERVER__PORT): expected unsigned integer"
/// );
/// assert_eq!(Config::from_str("a = = 1").unwrap_err().to_string(), "line 1: invalid TOML");
///
/// use oxbloom_config::Origin;
/// let dup = ConfigError::DuplicateKey {
///     key: "max-conn".into(),
///     first: Origin::Inline { line: 1 },
///     second: Origin::Inline { line: 2 },
/// };
/// assert_eq!(
///     dup.to_string(),
///     "duplicate configuration key `max-conn` (defined at line 1 and line 2)"
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            // Text given directly (`Config::from_str`) has no path: no leading `:`.
            Self::Parse {
                path,
                line,
                message,
            } => match (path.as_os_str().is_empty(), line) {
                (false, Some(line)) => write!(f, "{}:{line}: {message}", path.display()),
                (false, None) => write!(f, "{}: {message}", path.display()),
                (true, Some(line)) => write!(f, "line {line}: {message}"),
                (true, None) => f.write_str(message),
            },
            Self::Missing { key } => write!(f, "missing configuration key `{key}`"),
            Self::Invalid {
                key,
                message,
                origin: Some(origin),
            } => write!(
                f,
                "invalid configuration key `{key}` (from {origin}): {message}"
            ),
            Self::Invalid {
                key,
                message,
                origin: None,
            } => write!(f, "invalid configuration key `{key}`: {message}"),
            Self::DuplicateKey { key, first, second } => write!(
                f,
                "duplicate configuration key `{key}` (defined at {first} and {second})"
            ),
            // Fixed texts: the paths are configuration values, kept in the fields only.
            Self::ImportCycle { .. } => f.write_str("configuration imports form a cycle"),
            Self::ImportMissing { .. } => f.write_str("configuration import file not found"),
            Self::ImportInvalid { .. } => f.write_str(
                "configuration import path must be relative and stay inside the config directory",
            ),
            Self::ImportLimit => f.write_str("too many configuration imports (more than 256)"),
            Self::Deprecated { key, origin } => {
                write!(f, "deprecated configuration key `{key}` (from {origin})")
            }
            Self::ImportTooDeep => {
                f.write_str("configuration imports are nested more than 8 levels deep")
            }
            Self::UnresolvedPlaceholder { key, name } if name.is_empty() => write!(
                f,
                "malformed or unterminated placeholder in configuration key `{key}`"
            ),
            Self::UnresolvedPlaceholder { key, name } => {
                write!(
                    f,
                    "unresolved placeholder `{name}` in configuration key `{key}`"
                )
            }
            Self::PlaceholderTooLarge { key } => write!(
                f,
                "placeholder expansion in configuration key `{key}` is too large"
            ),
            Self::FileTooLarge { .. } => {
                f.write_str("configuration file is larger than the size limit")
            }
            Self::PlaceholderCycle { keys } => {
                write!(f, "placeholders form a cycle: {}", keys.join(" -> "))
            }
        }
    }
}

/// `Io` errors expose the underlying `std::io::Error` as their source.
///
/// ```
/// use std::error::Error;
/// use oxbloom_config::{Config, ConfigError};
///
/// let dir = tempfile::tempdir()?;
/// let err = Config::builder()
///     .dir(dir.path())
///     .profile("no-such-profile")
///     .env_from([])
///     .build()
///     .unwrap_err();
/// assert!(matches!(err, ConfigError::Io { .. }));
/// assert!(err.source().is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync_static<T: Send + Sync + 'static>() {}

    #[test]
    fn error_is_send_sync_static() {
        assert_send_sync_static::<ConfigError>();
    }

    #[test]
    fn invalid_error_names_key_not_value() {
        let e = ConfigError::invalid("server.port", "expected u16");
        let text = format!("{e} {e:?}");
        assert!(text.contains("server.port"));
    }

    #[test]
    fn invalid_constructor_has_no_origin() {
        let e = ConfigError::invalid("server.port", "expected u16");
        assert!(matches!(
            &e,
            ConfigError::Invalid { key, message, origin: None }
                if key == "server.port" && message == "expected u16"
        ));
        assert_eq!(
            e.to_string(),
            "invalid configuration key `server.port`: expected u16"
        );
    }

    #[test]
    fn invalid_display_names_origin_when_present() {
        let e = ConfigError::Invalid {
            key: "server.port".into(),
            message: "expected unsigned integer".into(),
            origin: Some(Origin::File {
                path: "application.toml".into(),
                line: 4,
            }),
        };
        assert_eq!(
            e.to_string(),
            "invalid configuration key `server.port` (from application.toml:4): expected unsigned integer"
        );
    }

    #[test]
    fn parse_display_without_path() {
        let e = ConfigError::Parse {
            path: PathBuf::new(),
            line: Some(3),
            message: "invalid TOML".into(),
        };
        assert_eq!(e.to_string(), "line 3: invalid TOML");
        let e = ConfigError::Parse {
            path: PathBuf::new(),
            line: None,
            message: "invalid TOML".into(),
        };
        assert_eq!(e.to_string(), "invalid TOML");
    }
}
