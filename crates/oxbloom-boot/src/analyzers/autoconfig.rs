//! The auto-configuration ordering-cycle analyzer.

use crate::autoconfig::AutoConfigurationCycle;
use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises an [`AutoConfigurationCycle`] anywhere in the cause chain.
pub(crate) struct AutoConfigurationCycleAnalyzer;

impl FailureAnalyzer for AutoConfigurationCycleAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let cycle = failure.find_cause::<AutoConfigurationCycle>()?;
        Some(FailureAnalysis::new(
            format!(
                "The auto-configurations form an ordering cycle: `{}`.",
                cycle.path.join(" -> ")
            ),
            "Remove one `after` or `before` entry so that the order is no longer circular.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StartupPhase;
    use oxbloom_di::DiError;

    fn analyze(source: impl std::error::Error + Send + Sync + 'static) -> Option<FailureAnalysis> {
        AutoConfigurationCycleAnalyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    #[test]
    fn cycle_exact_text() {
        let a = analyze(AutoConfigurationCycle {
            path: vec!["app::A", "app::B", "app::A"],
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "The auto-configurations form an ordering cycle: `app::A -> app::B -> app::A`."
        );
        assert_eq!(
            a.action(),
            Some("Remove one `after` or `before` entry so that the order is no longer circular.")
        );
    }

    #[test]
    fn other_errors_are_none() {
        assert!(analyze(DiError::creation::<u8>("boom")).is_none());
    }
}
