//! The logging analyzer: invalid `logging` keys and unwritable log files.

use oxbloom_config::ConfigError;
use oxbloom_logging::LoggingError;

use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises a `ConfigError::Invalid` for a key in the `logging` section anywhere in the
/// cause chain, also inside a `LoggingError::Config`, and a `LoggingError::Io` for a log
/// directory or file. The report names the key or the path with a fixed text and never the
/// configuration error's message, so no value can reach it; for an I/O failure it adds the
/// error kind, which carries no data.
pub(crate) struct LoggingConfigAnalyzer;

impl FailureAnalyzer for LoggingConfigAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        // `LoggingError::Config` is transparent, so its `source()` skips the `ConfigError`.
        let config = match failure.find_cause::<LoggingError>() {
            Some(LoggingError::Io { path, source }) => {
                return Some(FailureAnalysis::new(
                    format!(
                        "Cannot write the log file at `{}` ({}).",
                        path.display(),
                        source.kind()
                    ),
                    "Fix `logging.file.path`: check that the directory exists or can be \
                     created and that the process may write to it.",
                ));
            }
            Some(LoggingError::Config(e)) => Some(e),
            _ => None,
        };
        let ConfigError::Invalid { key, .. } = config.or_else(|| failure.find_cause())? else {
            return None;
        };
        if key != "logging" && !key.starts_with("logging.") {
            return None;
        }
        Some(FailureAnalysis::new(
            format!("The logging configuration key `{key}` is not valid."),
            "Fix the `logging` section: `logging.level.root` and `logging.level.<target>` take \
             off, error, warn, info, debug or trace; `logging.group.<name>` is an array of \
             target names, which use letters, digits, `_`, `-` and `::`; \
             `logging.console.format` is full, compact or pretty; `logging.console.color` is \
             auto, always or never; `logging.file.*` keys take a non-empty `path` and `name`, a \
             `rotation` of hourly, daily or never and a `max-history` above 0; `logging.json.*` \
             keys are boolean `fields.<field>` toggles and non-empty, distinct `names.<field>`; \
             `logging.request.level` is error, warn, info, debug or trace and \
             `logging.request.headers` is an array of header names.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StartupPhase;
    use oxbloom_di::DiError;

    fn analyze(source: impl std::error::Error + Send + Sync + 'static) -> Option<FailureAnalysis> {
        LoggingConfigAnalyzer.analyze(&StartupError::new(StartupPhase::Config, source))
    }

    #[test]
    fn invalid_logging_key_text_never_shows_the_message() {
        let e = LoggingError::Config(ConfigError::invalid("logging.level.x", "hunter2"));
        let a = analyze(e).unwrap();
        assert_eq!(
            a.description(),
            "The logging configuration key `logging.level.x` is not valid."
        );
        assert!(a.action().unwrap().contains("off, error, warn"));
        assert!(!a.to_string().contains("hunter2"));
    }

    #[test]
    fn plain_and_wrapped_config_errors_are_recognised() {
        assert!(analyze(ConfigError::invalid("logging", "bad")).is_some());
        let di: DiError = ConfigError::invalid("logging.console.color", "bad").into();
        assert!(analyze(di).is_some());
    }

    #[test]
    fn io_error_names_the_path_and_the_action() {
        let e = LoggingError::Io {
            path: "/var/log/app".into(),
            source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let a = analyze(e).unwrap();
        assert_eq!(
            a.description(),
            "Cannot write the log file at `/var/log/app` (permission denied)."
        );
        assert!(a.action().unwrap().contains(
            "check that the directory exists or can be created and that the process may write to it"
        ));
    }

    #[test]
    fn io_error_text_never_shows_the_os_message() {
        let e = LoggingError::Io {
            path: "logs".into(),
            source: std::io::Error::other("hunter2"),
        };
        assert!(!analyze(e).unwrap().to_string().contains("hunter2"));
    }

    #[test]
    fn invalid_key_action_mentions_the_new_sections() {
        let e = LoggingError::Config(ConfigError::invalid("logging.file.rotation", "x"));
        let action = analyze(e).unwrap().action().unwrap().to_owned();
        for needle in ["logging.file.*", "logging.json.*", "logging.request."] {
            assert!(action.contains(needle), "{needle}: {action}");
        }
    }

    #[test]
    fn other_keys_and_errors_are_none() {
        assert!(analyze(ConfigError::invalid("loggingx", "bad")).is_none());
        assert!(analyze(ConfigError::invalid("oxbloom.logging.enabled", "bad")).is_none());
        assert!(analyze(LoggingError::AlreadyInstalled).is_none());
        assert!(analyze(LoggingError::Filter("x".into())).is_none());
        assert!(
            analyze(ConfigError::Missing {
                key: "logging.level.root".into()
            })
            .is_none()
        );
    }
}
