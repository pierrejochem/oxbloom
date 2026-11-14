//! Log levels as configured in `logging.level.*`.

use std::fmt;
use std::str::FromStr;

/// The level of a logging target, from most quiet (`Off`) to most verbose (`Trace`).
///
/// Parsed case-insensitively from `off`, `error`, `warn`, `info`, `debug` and `trace`; displayed
/// in lowercase.
///
/// # Examples
///
/// ```
/// use oxbloom_logging::Level;
///
/// let level: Level = "DEBUG".parse()?;
/// assert_eq!(level, Level::Debug);
/// assert_eq!(level.to_string(), "debug");
/// assert!(Level::Debug > Level::Info); // more verbose compares greater
/// # Ok::<(), oxbloom_logging::ParseLevelError>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Nothing is logged.
    Off,
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Informational events and above; the default root level.
    Info,
    /// Debug events and above.
    Debug,
    /// Everything.
    Trace,
}

const NAMES: [(&str, Level); 6] = [
    ("off", Level::Off),
    ("error", Level::Error),
    ("warn", Level::Warn),
    ("info", Level::Info),
    ("debug", Level::Debug),
    ("trace", Level::Trace),
];

/// The lowercase name, as accepted in `logging.level.*` and in filter directives.
///
/// ```
/// use oxbloom_logging::Level;
///
/// assert_eq!(Level::Trace.to_string(), "trace");
/// assert_eq!(format!("app={}", Level::Off), "app=off");
/// ```
impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = NAMES.iter().find(|(_, l)| l == self).map_or("", |(n, _)| n);
        f.write_str(name)
    }
}

/// Case-insensitive: `Info`, `INFO` and `info` are all [`Level::Info`].
///
/// ```
/// use oxbloom_logging::Level;
///
/// assert_eq!("Warn".parse::<Level>()?, Level::Warn);
/// assert!("verbose".parse::<Level>().is_err());
/// # Ok::<(), oxbloom_logging::ParseLevelError>(())
/// ```
impl FromStr for Level {
    type Err = ParseLevelError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NAMES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(s))
            .map(|&(_, level)| level)
            .ok_or(ParseLevelError)
    }
}

/// Reads a string through [`FromStr`], so any serde format matches case-insensitively. A
/// wrong name is an invalid value whose error lists the level names and never contains the
/// value, in every serde format.
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::Level;
///
/// let config = Config::from_str("level = \"TRACE\"")?;
/// assert_eq!(config.get::<Level>("level")?, Level::Trace);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
impl<'de> serde::Deserialize<'de> for Level {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        // Not `unknown_variant`: that echoes the value, which could be a secret.
        text.parse().map_err(|_| {
            serde::de::Error::invalid_value(
                serde::de::Unexpected::Other("an unknown level name"),
                &"one of off, error, warn, info, debug, trace",
            )
        })
    }
}

/// The text is not one of the level names. The error does not carry the text.
///
/// # Examples
///
/// ```
/// use oxbloom_logging::{Level, ParseLevelError};
///
/// let err = "loud".parse::<Level>().unwrap_err();
/// assert_eq!(err, ParseLevelError);
/// assert_eq!(err.to_string(), "expected one of off, error, warn, info, debug, trace");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseLevelError;

/// A fixed text listing the level names.
///
/// ```
/// use oxbloom_logging::ParseLevelError;
///
/// assert!(ParseLevelError.to_string().starts_with("expected one of"));
/// ```
impl fmt::Display for ParseLevelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected one of off, error, warn, info, debug, trace")
    }
}

/// A plain error without a source.
///
/// ```
/// use std::error::Error;
/// use oxbloom_logging::ParseLevelError;
///
/// assert!(ParseLevelError.source().is_none());
/// ```
impl std::error::Error for ParseLevelError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_round_trips_through_from_str() {
        for (name, level) in NAMES {
            assert_eq!(level.to_string(), name);
            assert_eq!(name.parse::<Level>(), Ok(level));
        }
    }

    #[test]
    fn unknown_name_from_any_format_lists_levels_without_the_value() {
        let err = serde_json::from_str::<Level>("\"hunter2\"").unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(
            text.contains("one of off, error, warn, info, debug, trace"),
            "{text}"
        );
        assert_eq!(
            serde_json::from_str::<Level>("\"WARN\"").unwrap(),
            Level::Warn
        );
    }

    #[test]
    fn order_follows_verbosity() {
        assert!(Level::Off < Level::Error && Level::Error < Level::Trace);
    }
}
