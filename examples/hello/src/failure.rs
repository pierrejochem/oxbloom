//! Feature: a custom `FailureAnalyzer`.
//!
//! When `hello.greeting` is empty, startup fails and boot prints this
//! analyzer's report instead of the generic one.

use oxbloom_boot::{FailureAnalysis, FailureAnalyzer, StartupError};

use crate::greeting::EmptyGreeting;

/// Explains an empty `hello.greeting`.
pub struct EmptyGreetingAnalyzer;

impl FailureAnalyzer for EmptyGreetingAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        // `find_cause` walks the cause chain: StartupError -> DiError -> EmptyGreeting.
        failure.find_cause::<EmptyGreeting>()?;
        Some(FailureAnalysis::new(
            "The configuration key `hello.greeting` is empty, so there is nothing to greet with.",
            "Set `hello.greeting` in `application.toml` or the active profile file to a non-empty text.",
        ))
    }
}
