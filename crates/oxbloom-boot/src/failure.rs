use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use crate::analyzers;
use crate::error::StartupError;
use crate::output::Output;

/// The line of asterisks above and below the report title.
const STARS: &str = "***************************";

/// A diagnosis of a startup failure: what went wrong and, optionally, what to
/// do about it.
///
/// `Display` renders the report block that boot prints, without a trailing
/// line break. The description and action are printed as given, so an
/// analyzer must not put the block markers (the line of asterisks or the
/// `APPLICATION FAILED TO START` title) into them. They must not contain
/// configuration values, argument values or secrets either.
///
/// ```
/// use oxbloom_boot::FailureAnalysis;
///
/// let analysis = FailureAnalysis::new(
///     "The required configuration key `db.url` is not set.",
///     "Set `db.url` in `application.toml`.",
/// );
/// assert_eq!(
///     analysis.to_string(),
///     "***************************\n\
///      APPLICATION FAILED TO START\n\
///      ***************************\n\
///      \n\
///      Description:\n\
///      \n\
///      The required configuration key `db.url` is not set.\n\
///      \n\
///      Action:\n\
///      \n\
///      Set `db.url` in `application.toml`."
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FailureAnalysis {
    description: String,
    action: Option<String>,
}

impl FailureAnalysis {
    /// Creates an analysis with a description and an action.
    ///
    /// ```
    /// use oxbloom_boot::FailureAnalysis;
    ///
    /// let analysis = FailureAnalysis::new("The port is in use.", "Pick another port.");
    /// assert_eq!(analysis.action(), Some("Pick another port."));
    /// ```
    pub fn new(description: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            action: Some(action.into()),
        }
    }

    /// Creates an analysis without an action; the report then has no Action
    /// section.
    ///
    /// ```
    /// use oxbloom_boot::FailureAnalysis;
    ///
    /// let analysis = FailureAnalysis::with_no_action("The database is down.");
    /// assert_eq!(analysis.action(), None);
    /// assert!(analysis.to_string().ends_with("Description:\n\nThe database is down."));
    /// ```
    pub fn with_no_action(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            action: None,
        }
    }

    /// What went wrong.
    ///
    /// ```
    /// use oxbloom_boot::FailureAnalysis;
    ///
    /// let analysis = FailureAnalysis::with_no_action("The database is down.");
    /// assert_eq!(analysis.description(), "The database is down.");
    /// ```
    pub fn description(&self) -> &str {
        &self.description
    }

    /// What to do about it, if the analyzer knows.
    ///
    /// ```
    /// use oxbloom_boot::FailureAnalysis;
    ///
    /// let analysis = FailureAnalysis::new("The port is in use.", "Pick another port.");
    /// assert_eq!(analysis.action(), Some("Pick another port."));
    /// ```
    pub fn action(&self) -> Option<&str> {
        self.action.as_deref()
    }
}

impl fmt::Display for FailureAnalysis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{STARS}\nAPPLICATION FAILED TO START\n{STARS}\n\nDescription:\n\n{}",
            self.description
        )?;
        if let Some(action) = &self.action {
            write!(f, "\n\nAction:\n\n{action}")?;
        }
        Ok(())
    }
}

/// Turns a [`StartupError`] into a [`FailureAnalysis`] when it recognises the
/// cause.
///
/// Register analyzers with
/// [`OxbloomApplicationBuilder::failure_analyzer`](crate::OxbloomApplicationBuilder::failure_analyzer)
/// or as `Arc<dyn FailureAnalyzer>` beans. Boot asks the builder analyzers in
/// call order, then the beans in DI order (only when the container was
/// built), then the built-in analyzers, and the first `Some` wins. Use
/// [`StartupError::find_cause`] to reach the typed error. A panicking analyzer
/// is skipped and reported as `analyzer {name} panicked`. Oxbloom never prints
/// the panic message, but the default panic hook still prints the panic
/// payload to stderr.
///
/// The text must not contain configuration values, argument values or
/// secrets; type names and configuration key names are fine.
///
/// ```
/// use oxbloom_boot::{FailureAnalysis, FailureAnalyzer, StartupError, StartupPhase};
///
/// #[derive(Debug)]
/// struct QuotaExceeded;
/// impl std::fmt::Display for QuotaExceeded {
///     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
///         f.write_str("quota exceeded")
///     }
/// }
/// impl std::error::Error for QuotaExceeded {}
///
/// struct QuotaAnalyzer;
///
/// impl FailureAnalyzer for QuotaAnalyzer {
///     fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
///         failure.find_cause::<QuotaExceeded>().map(|_| {
///             FailureAnalysis::new("The API quota is used up.", "Raise the quota.")
///         })
///     }
/// }
///
/// let error = StartupError::new(StartupPhase::Service, QuotaExceeded);
/// let analysis = QuotaAnalyzer.analyze(&error);
/// assert_eq!(analysis.unwrap().description(), "The API quota is used up.");
/// assert!(QuotaAnalyzer.name().ends_with("QuotaAnalyzer"));
/// ```
pub trait FailureAnalyzer: Send + Sync + 'static {
    /// Returns an analysis when this analyzer recognises `failure`.
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis>;

    /// The analyzer's name, by default its type name.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Asks the builder analyzers, then the bean analyzers, then the built-ins,
/// and returns the first analysis. Each call is panic-isolated.
pub(crate) fn analyze_failure(
    builder: &[Arc<dyn FailureAnalyzer>],
    beans: &[Arc<dyn FailureAnalyzer>],
    failure: &StartupError,
    out: &Output,
) -> Option<FailureAnalysis> {
    let builtin = analyzers::builtin();
    builder.iter().chain(beans).chain(&builtin).find_map(|a| {
        catch_unwind(AssertUnwindSafe(|| a.analyze(failure))).unwrap_or_else(|_| {
            let name = catch_unwind(AssertUnwindSafe(|| a.name())).unwrap_or("<unknown>");
            out.line(&format!("analyzer {name} panicked"));
            None
        })
    })
}

/// The report block for `failure`: its analysis, or the generic block with
/// the error's `Display` as description.
pub(crate) fn report_text(failure: &StartupError) -> String {
    match failure.analysis() {
        Some(analysis) => analysis.to_string(),
        None => FailureAnalysis::with_no_action(failure.to_string()).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StartupPhase;

    const HEAD: &str = "***************************\nAPPLICATION FAILED TO START\n***************************\n\nDescription:\n\n";

    #[test]
    fn display_with_action() {
        let a = FailureAnalysis::new("d", "a");
        assert_eq!(a.to_string(), format!("{HEAD}d\n\nAction:\n\na"));
        assert_eq!(STARS.len(), 27);
    }

    #[test]
    fn display_without_action() {
        let a = FailureAnalysis::with_no_action("d");
        assert_eq!(a.to_string(), format!("{HEAD}d"));
    }

    #[test]
    fn report_text_falls_back_to_the_error() {
        let mut e = StartupError::new(StartupPhase::Config, "x");
        assert_eq!(
            report_text(&e),
            format!("{HEAD}startup failed in the config phase: x")
        );
        e.set_analysis(FailureAnalysis::with_no_action("y"));
        assert_eq!(report_text(&e), format!("{HEAD}y"));
    }

    #[test]
    fn analyze_failure_with_no_analyzer_is_none() {
        let e = StartupError::new(StartupPhase::Config, "x");
        let out = Output::new(std::io::sink());
        assert_eq!(analyze_failure(&[], &[], &e, &out), None);
    }
}
