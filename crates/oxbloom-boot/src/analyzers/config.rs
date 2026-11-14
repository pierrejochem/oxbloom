//! The missing-configuration-key analyzer.

use oxbloom_config::ConfigError;

use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises `ConfigError::Missing` anywhere in the cause chain.
pub(crate) struct MissingConfigKeyAnalyzer;

impl FailureAnalyzer for MissingConfigKeyAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let ConfigError::Missing { key } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        Some(FailureAnalysis::new(
            format!("The required configuration key `{key}` is not set."),
            format!(
                "Set it in `application.toml` or the active profile file, as an environment \
                 variable (default prefix `APP_`, with `__` between key segments), or as \
                 `--{key}=<value>`."
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StartupPhase;
    use oxbloom_di::DiError;

    fn analyze(source: impl std::error::Error + Send + Sync + 'static) -> Option<FailureAnalysis> {
        MissingConfigKeyAnalyzer.analyze(&StartupError::new(StartupPhase::Config, source))
    }

    #[test]
    fn missing_key_exact_text() {
        let a = analyze(ConfigError::Missing {
            key: "db.port".into(),
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "The required configuration key `db.port` is not set."
        );
        assert_eq!(
            a.action(),
            Some(
                "Set it in `application.toml` or the active profile file, as an environment \
                 variable (default prefix `APP_`, with `__` between key segments), or as \
                 `--db.port=<value>`."
            )
        );
    }

    #[test]
    fn missing_key_behind_a_di_creation_error() {
        let di: DiError = ConfigError::Missing { key: "k".into() }.into();
        assert!(analyze(di).is_some());
    }

    #[test]
    fn other_errors_are_none() {
        assert!(analyze(ConfigError::invalid("k", "bad")).is_none());
        assert!(analyze(DiError::creation::<u8>("boom")).is_none());
    }
}
