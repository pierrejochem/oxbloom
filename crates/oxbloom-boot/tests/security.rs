#![cfg(feature = "security")]

mod common;

use std::future::Future;
use std::time::Duration;

use oxbloom_boot::{
    Banner, FailureAnalysis, FailureAnalyzer, OxbloomApplication, OxbloomApplicationBuilder,
    StartupError, StartupPhase,
};
use oxbloom_config::Config;
use oxbloom_di::DiError;
use oxbloom_security::{HttpSecurity, SecurityBuildError, SecurityFilterChain};
use oxbloom_test::Captured;

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

fn app(out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str("").unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .failure_report(true)
}

#[tokio::test]
async fn bad_pattern_through_try_bean() {
    let out = Captured::new();
    let err = within(
        app(&out)
            .configure(|b| {
                b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
                    Ok(HttpSecurity::new()
                        .authorize(|a| a.path("/users/{id}").authenticated())
                        .build()?)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    let analysis = err.analysis().unwrap();
    assert!(analysis.description().contains("Ant syntax"));
    assert_eq!(
        analysis.action(),
        Some("Use `/users/*` or `/users/**` and match the rest in a handler.")
    );
    let report = out.contents();
    assert!(report.contains("A security matcher pattern is not valid."));
    assert!(report.contains("Ant syntax"));
    assert!(!report.contains("/users/{id}"));
}

#[tokio::test]
async fn missing_authorize_section_report() {
    let out = Captured::new();
    let err = within(
        app(&out)
            .configure(|b| {
                b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
                    Ok(HttpSecurity::new().build()?)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.analysis().unwrap().description(),
        "The security configuration has no authorize section."
    );
    let report = out.contents();
    assert!(report.contains("Call `authorize` and add at least one rule."));
}

#[tokio::test]
async fn duplicate_authorize_report() {
    let out = Captured::new();
    let err = within(
        app(&out)
            .configure(|b| {
                b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
                    Ok(HttpSecurity::new()
                        .authorize(|a| a.path("/a").authenticated())
                        .authorize(|a| a.path("/b").authenticated())
                        .build()?)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.analysis().unwrap().description(),
        "`authorize` was called more than once."
    );
    assert!(
        out.contents()
            .contains("Put all rules in one `authorize` call.")
    );
}

struct Override;

impl FailureAnalyzer for Override {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        failure
            .find_cause::<SecurityBuildError>()
            .map(|_| FailureAnalysis::with_no_action("custom security diagnosis"))
    }
}

#[tokio::test]
async fn builder_analyzer_overrides_security_builtin() {
    let out = Captured::new();
    let err = within(
        app(&out)
            .failure_analyzer(Override)
            .configure(|b| {
                b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
                    Ok(HttpSecurity::new().build()?)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.analysis().unwrap().description(),
        "custom security diagnosis"
    );
    assert!(!out.contents().contains("authorize section"));
}
