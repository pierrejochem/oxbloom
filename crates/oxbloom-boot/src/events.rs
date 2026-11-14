//! The `tracing` events boot emits, all at target `oxbloom::boot`. Only compiled with the
//! feature `logging`; the callers are `cfg`-gated too, so other builds are unchanged.

use std::time::Duration;

use oxbloom_config::{Config, ConfigWarning};
use oxbloom_di::{ConditionRecord, ConditionReport};

use crate::error::StartupError;

const TARGET: &str = "oxbloom::boot";

/// The text used when `application.name` is not configured.
pub(crate) const DEFAULT_NAME: &str = "application";

/// The application name: `application.name`, or `application` when it is not configured.
pub(crate) fn app_name(config: &Config) -> String {
    config
        .get::<String>("application.name")
        .unwrap_or_else(|_| DEFAULT_NAME.to_owned())
}

/// `text` on one line: line breaks become spaces, as in [`failed`], so a configured name
/// cannot forge a log line.
fn one_line(text: &str) -> String {
    text.replace(['\r', '\n'], " ")
}

pub(crate) fn starting(app: &str, profiles: &[String]) {
    let profiles = if profiles.is_empty() {
        "default".to_owned()
    } else {
        one_line(&profiles.join(","))
    };
    let app = one_line(app);
    tracing::info!(target: TARGET, profiles = %profiles, "Starting {app}");
}

pub(crate) fn started(app: &str, elapsed: Duration) {
    let app = one_line(app);
    tracing::info!(target: TARGET, "Started {app} in {:.3}s", elapsed.as_secs_f64());
}

pub(crate) fn config_warning(warning: &ConfigWarning) {
    tracing::warn!(target: TARGET, "{warning}");
}

/// One `debug` event per condition record of `report`.
///
/// Not inlined, so its locals stay out of the frame of the caller, which sits below the
/// container build on the stack.
#[inline(never)]
pub(crate) fn conditions(report: &ConditionReport) {
    report.records().iter().for_each(condition);
}

fn condition(record: &ConditionRecord) {
    tracing::debug!(
        target: TARGET,
        "{}: {} {} - {}",
        record.registration(),
        record.condition(),
        if record.matched() { "matched" } else { "skipped" },
        record.reason()
    );
}

pub(crate) fn service_started(name: &str) {
    tracing::info!(target: TARGET, "{name} started");
}

pub(crate) fn service_stopped(name: &str) {
    tracing::info!(target: TARGET, "{name} stopped");
}

#[cfg(feature = "web")]
pub(crate) fn listening(addr: std::net::SocketAddr) {
    tracing::info!(target: TARGET, "Web server listening on {addr}");
}

/// One line: line breaks in the error text become spaces.
pub(crate) fn failed(error: &StartupError) {
    let text = one_line(&error.to_string());
    tracing::error!(target: TARGET, "{text}");
}
