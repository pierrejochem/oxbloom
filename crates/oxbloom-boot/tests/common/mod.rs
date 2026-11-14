//! Shared helpers for integration tests.
#![allow(dead_code)]

/// Turns the logging installation off, where the feature exists: a test that is not about
/// logging then installs no global subscriber and prints no startup events.
pub fn quiet(
    builder: oxbloom_boot::OxbloomApplicationBuilder,
) -> oxbloom_boot::OxbloomApplicationBuilder {
    #[cfg(feature = "logging")]
    let builder = builder.logging(false);
    builder
}
