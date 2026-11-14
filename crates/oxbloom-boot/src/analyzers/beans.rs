//! The bean-wiring analyzer: cycle, missing, ambiguous and duplicate beans.

use oxbloom_di::DiError;

use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises `DiError` wiring variants anywhere in the cause chain.
pub(crate) struct BeanWiringAnalyzer;

impl FailureAnalyzer for BeanWiringAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        match failure.find_cause::<DiError>()? {
            DiError::Cycle { path } => Some(FailureAnalysis::new(
                format!(
                    "The beans form a dependency cycle: `{}`.",
                    path.join(" -> ")
                ),
                "Remove one of the dependencies, or move the shared logic into a third bean.",
            )),
            DiError::Missing {
                requested,
                required_by,
            } => {
                let description = match required_by.last() {
                    Some(last) => format!(
                        "`{last}` needs a bean of type `{requested}`, but none is registered."
                    ),
                    None => format!(
                        "A bean of type `{requested}` was requested, but none is registered."
                    ),
                };
                Some(FailureAnalysis::new(
                    description,
                    "Register one with `component`, `instance`, `bean` or `factory`, and check \
                     its profile conditions.",
                ))
            }
            DiError::Ambiguous {
                requested,
                candidates,
            } => Some(FailureAnalysis::new(
                format!(
                    "More than one bean can provide `{requested}`: `{}`.",
                    candidates.join(", ")
                ),
                "Mark one of them `primary`, or inject `Vec<Arc<..>>`.",
            )),
            DiError::Duplicate { type_name, sources } => Some(FailureAnalysis::new(
                format!(
                    "`{type_name}` is registered more than once: `{}`.",
                    sources.join(", ")
                ),
                "Register the type only once.",
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StartupPhase;
    use oxbloom_config::ConfigError;

    fn analyze(source: DiError) -> Option<FailureAnalysis> {
        BeanWiringAnalyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn cycle_exact_text() {
        let a = analyze(DiError::Cycle {
            path: strings(&["A", "B", "A"]),
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "The beans form a dependency cycle: `A -> B -> A`."
        );
        assert_eq!(
            a.action(),
            Some("Remove one of the dependencies, or move the shared logic into a third bean.")
        );
    }

    const REGISTER: &str = "Register one with `component`, `instance`, `bean` or `factory`, and check its profile conditions.";

    #[test]
    fn missing_with_required_by_uses_the_last_entry() {
        let a = analyze(DiError::Missing {
            requested: "Db".into(),
            required_by: strings(&["App", "Repo"]),
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "`Repo` needs a bean of type `Db`, but none is registered."
        );
        assert_eq!(a.action(), Some(REGISTER));
    }

    #[test]
    fn missing_without_required_by_uses_the_alternate_sentence() {
        let a = analyze(DiError::Missing {
            requested: "Db".into(),
            required_by: vec![],
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "A bean of type `Db` was requested, but none is registered."
        );
        assert_eq!(a.action(), Some(REGISTER));
    }

    #[test]
    fn ambiguous_exact_text() {
        let a = analyze(DiError::Ambiguous {
            requested: "dyn Store".into(),
            candidates: strings(&["Disk", "Remote"]),
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "More than one bean can provide `dyn Store`: `Disk, Remote`."
        );
        assert_eq!(
            a.action(),
            Some("Mark one of them `primary`, or inject `Vec<Arc<..>>`.")
        );
    }

    #[test]
    fn ambiguous_with_no_candidates_does_not_panic() {
        let a = analyze(DiError::Ambiguous {
            requested: "T".into(),
            candidates: vec![],
        })
        .unwrap();
        assert_eq!(a.description(), "More than one bean can provide `T`: ``.");
    }

    #[test]
    fn duplicate_exact_text() {
        let a = analyze(DiError::Duplicate {
            type_name: "Twins".into(),
            sources: strings(&["instance", "configuration"]),
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "`Twins` is registered more than once: `instance, configuration`."
        );
        assert_eq!(a.action(), Some("Register the type only once."));
    }

    #[test]
    fn duplicate_with_no_sources_does_not_panic() {
        let a = analyze(DiError::Duplicate {
            type_name: "T".into(),
            sources: vec![],
        })
        .unwrap();
        assert_eq!(a.description(), "`T` is registered more than once: ``.");
    }

    #[test]
    fn cycle_with_empty_path_does_not_panic() {
        let a = analyze(DiError::Cycle { path: vec![] }).unwrap();
        assert_eq!(a.description(), "The beans form a dependency cycle: ``.");
    }

    #[test]
    fn other_errors_are_none() {
        assert!(analyze(DiError::creation::<u8>("boom")).is_none());
        let config = StartupError::new(StartupPhase::Config, ConfigError::invalid("k", "bad"));
        assert!(BeanWiringAnalyzer.analyze(&config).is_none());
    }
}
