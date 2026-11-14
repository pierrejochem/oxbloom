use std::fmt;
use std::path::PathBuf;

/// Where a configuration value came from. Never contains the value itself.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Origin;
///
/// let file = Origin::File { path: "application-dev.toml".into(), line: 4 };
/// assert_eq!(file.to_string(), "application-dev.toml:4");
/// assert_eq!(Origin::Inline { line: 3 }.to_string(), "line 3");
/// assert_eq!(
///     Origin::Env { name: "APP_SERVER__PORT".into() }.to_string(),
///     "env APP_SERVER__PORT"
/// );
/// assert_eq!(Origin::Cli { arg: "--server.port".into() }.to_string(), "--server.port");
/// ```
///
/// The enum is `#[non_exhaustive]`: a `match` outside this crate needs a wildcard arm.
///
/// ```compile_fail
/// use oxbloom_config::Origin;
///
/// fn file_line(origin: &Origin) -> Option<usize> {
///     match origin {
///         Origin::File { line, .. } | Origin::Inline { line } => Some(*line),
///         Origin::Env { .. } | Origin::Cli { .. } => None,
///     }
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Origin {
    /// A line in a configuration file.
    File {
        /// The file.
        path: PathBuf,
        /// 1-based line.
        line: usize,
    },
    /// A line in TOML text given directly (for example `Config::from_str`).
    Inline {
        /// 1-based line.
        line: usize,
    },
    /// An environment variable.
    Env {
        /// Variable name.
        name: String,
    },
    /// A command-line argument, by key (`--server.port`), without its value.
    Cli {
        /// The argument key part.
        arg: String,
    },
}

/// Short form: `application-dev.toml:4`, `line 3`, `env APP_SERVER__PORT`, `--server.port`.
///
/// ```
/// use oxbloom_config::Origin;
///
/// assert_eq!(Origin::Inline { line: 1 }.to_string(), "line 1");
/// ```
impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File { path, line } => write!(f, "{}:{line}", path.display()),
            Self::Inline { line } => write!(f, "line {line}"),
            Self::Env { name } => write!(f, "env {name}"),
            Self::Cli { arg } => write!(f, "{arg}"),
        }
    }
}
