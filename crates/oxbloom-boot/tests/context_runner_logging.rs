//! `ContextRunner` keeps the logging installation off even when `oxbloom-test` is built
//! without its own `logging` feature, as it is in this crate's dev-dependency.
//!
//! This binary holds this one test only, so no earlier run can have installed the global
//! subscriber: boot registers the `LoggingSystem` bean exactly when it installs logging.

#![cfg(feature = "logging")]

use oxbloom_logging::LoggingSystem;
use oxbloom_test::ContextRunner;

#[tokio::test(flavor = "current_thread")]
async fn the_context_runner_installs_no_logging_subscriber() {
    ContextRunner::new()
        .run(|view| assert!(!view.contains::<LoggingSystem>()))
        .await;
    assert!(!tracing::dispatcher::has_been_set());
}
