mod common;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use oxbloom_boot::{
    Banner, FailureAnalysis, FailureAnalyzer, OxbloomApplication, OxbloomApplicationBuilder,
    StartupError,
};
use oxbloom_config::Config;
use oxbloom_di::{DiError, Resolver};
use oxbloom_test::Captured;

async fn within<F: Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

const STARS: &str = "***************************";

fn head() -> String {
    format!("{STARS}\nAPPLICATION FAILED TO START\n{STARS}\n\nDescription:\n\n")
}

fn app(config: &str, out: &Captured) -> OxbloomApplicationBuilder {
    common::quiet(OxbloomApplication::builder())
        .with_config(Config::from_str(config).unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .failure_report(true)
}

struct Pool;
struct Left;
struct Right;
struct Needs;
struct Absent;

trait Store: Send + Sync {}
struct Disk;
struct Remote;
impl Store for Disk {}
impl Store for Remote {}

#[tokio::test]
async fn missing_key_through_try_bean() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .configure(|b| {
                b.try_bean(|config: Arc<Config>| -> Result<Pool, DiError> {
                    let _port: u16 = config.get("db.port")?;
                    Ok(Pool)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    let report = out.contents();
    assert!(report.starts_with(&head()));
    assert!(report.contains("The required configuration key `db.port` is not set."));
    assert!(report.contains("`application.toml`"));
    assert!(report.contains("environment variable"));
    assert!(report.contains("`--db.port=<value>`"));
    assert_eq!(
        err.analysis().unwrap().description(),
        "The required configuration key `db.port` is not set."
    );
}

#[tokio::test]
async fn cycle_report() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .configure(|b| {
                b.factory(|r: &Resolver| -> Result<Left, DiError> {
                    r.get::<Right>()?;
                    Ok(Left)
                })
                .factory(|r: &Resolver| -> Result<Right, DiError> {
                    r.get::<Left>()?;
                    Ok(Right)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    let desc = err.analysis().unwrap().description().to_owned();
    assert!(
        desc.starts_with("The beans form a dependency cycle: `"),
        "{desc}"
    );
    assert!(desc.contains(" -> "));
    assert!(out.contents().contains("Remove one of the dependencies"));
}

#[tokio::test]
async fn missing_bean_report() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .configure(|b| {
                b.factory(|r: &Resolver| -> Result<Needs, DiError> {
                    r.get::<Absent>()?;
                    Ok(Needs)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    let analysis = err.analysis().unwrap();
    assert!(analysis.description().contains("needs a bean of type"));
    assert!(analysis.description().contains("Absent"));
    assert!(out.contents().contains("Register one with `component`"));
}

#[tokio::test]
async fn ambiguous_report() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .configure(|b| {
                b.instance(Disk)
                    .provides(|d: Arc<Disk>| -> Arc<dyn Store> { d })
                    .instance(Remote)
                    .provides(|r: Arc<Remote>| -> Arc<dyn Store> { r })
                    .factory(|r: &Resolver| -> Result<Needs, DiError> {
                        r.get::<dyn Store>()?;
                        Ok(Needs)
                    })
            })
            .run(),
    )
    .await
    .unwrap_err();
    let desc = err.analysis().unwrap().description().to_owned();
    assert!(
        desc.starts_with("More than one bean can provide `"),
        "{desc}"
    );
    assert!(desc.contains("Disk") && desc.contains("Remote"));
    assert!(out.contents().contains("Mark one of them `primary`"));
}

#[tokio::test]
async fn duplicate_report() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .configure(|b| b.instance(Disk).instance(Disk))
            .run(),
    )
    .await
    .unwrap_err();
    let desc = err.analysis().unwrap().description().to_owned();
    assert!(desc.contains("Disk"), "{desc}");
    assert!(desc.contains("is registered more than once: `"));
    assert!(out.contents().contains("Register the type only once."));
}

struct Override;

impl FailureAnalyzer for Override {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        failure
            .find_cause::<oxbloom_config::ConfigError>()
            .map(|_| FailureAnalysis::with_no_action("custom diagnosis"))
    }
}

#[tokio::test]
async fn user_analyzer_overrides_builtin() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .failure_analyzer(Override)
            .configure(|b| {
                b.try_bean(|config: Arc<Config>| -> Result<Pool, DiError> {
                    let _port: u16 = config.get("db.port")?;
                    Ok(Pool)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.analysis().unwrap().description(), "custom diagnosis");
    let report = out.contents();
    assert!(report.contains("custom diagnosis"));
    assert!(!report.contains("db.port"));
}

#[tokio::test]
async fn marker_value_never_in_report() {
    let marker = "ZZ-MARKER-VALUE-ZZ";
    let out = Captured::new();
    let err = within(
        app(&format!("db.port = \"{marker}\""), &out)
            .configure(|b| {
                b.try_bean(|config: Arc<Config>| -> Result<Pool, DiError> {
                    let _port: u16 = config.get("db.port")?;
                    Ok(Pool)
                })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert!(!out.contents().contains(marker));
    assert!(!err.to_string().contains(marker));
    assert!(!format!("{err:?}").contains(marker));
    assert!(err.analysis().is_none());
}

#[tokio::test]
async fn unparsable_config_file_gets_the_generic_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application.toml"), "a = = 1").unwrap();
    let path = dir.path().to_owned();
    let out = Captured::new();
    let err = within(
        OxbloomApplication::builder()
            .config(move |b| b.dir(path).env_from([]))
            .banner(Banner::off())
            .output(out.clone())
            .failure_report(true)
            .run(),
    )
    .await
    .unwrap_err();
    assert!(err.analysis().is_none());
    let report = out.contents();
    assert_eq!(report, format!("{}{err}\n", head()));
}

#[tokio::test]
async fn failure_without_typed_cause_prints_the_generic_block() {
    let out = Captured::new();
    let err = within(
        app("", &out)
            .configure(|b| {
                b.try_bean(|| -> Result<Pool, DiError> { Err(DiError::creation::<Pool>("boom")) })
            })
            .run(),
    )
    .await
    .unwrap_err();
    assert!(err.analysis().is_none());
    assert_eq!(out.contents(), format!("{}{err}\n", head()));
}
