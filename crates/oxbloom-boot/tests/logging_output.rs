//! `logging.file.*` at startup. A binary of its own: the subscriber is global, so the failing
//! scenarios run first, in order, and the one install is made by the file scenario.
#![cfg(feature = "logging")]

use oxbloom_boot::{Banner, OxbloomApplication, OxbloomApplicationBuilder, StartupPhase};
use oxbloom_config::Config;
use oxbloom_test::Captured;

/// An application with `toml` as its configuration and a console capture that is never read.
fn app(toml: &str, out: &Captured) -> OxbloomApplicationBuilder {
    let sink = Captured::new();
    OxbloomApplication::builder()
        .with_config(Config::from_str(toml).unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .logging_writer(move || sink.clone())
}

#[tokio::test]
async fn file_output_scenarios() {
    unwritable_path_fails_startup_with_analysis().await;
    bad_file_key_is_analyzed().await;
    file_logging_from_config().await;
}

/// A regular file where the log directory should be.
async fn unwritable_path_fails_startup_with_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("hunter2-blocker");
    std::fs::write(&blocker, "x").unwrap();
    let toml = format!(
        "[logging.console]\ncolor = \"never\"\n[logging.file]\nenabled = true\npath = {:?}",
        blocker.to_str().unwrap()
    );
    let out = Captured::new();
    let err = app(&toml, &out)
        .failure_report(true)
        .run()
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    let analysis = err.analysis().expect("an analysis");
    let text = analysis.to_string();
    assert!(text.contains("hunter2-blocker"), "{text}");
    assert!(text.contains("check that the directory exists"), "{text}");
    let report = out.contents();
    assert!(report.contains("hunter2-blocker"), "{report}");
    assert!(report.contains("check that the directory"), "{report}");
}

async fn bad_file_key_is_analyzed() {
    let out = Captured::new();
    let err = app("[logging.file]\nrotation = \"hunter2\"", &out)
        .failure_report(true)
        .run()
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    let analysis = err.analysis().expect("an analysis");
    assert!(
        analysis.description().contains("`logging.file.rotation`"),
        "{analysis}"
    );
    assert!(
        analysis.action().unwrap().contains("logging.file.*"),
        "{analysis}"
    );
    assert!(!out.contents().contains("hunter2"), "{}", out.contents());
}

async fn file_logging_from_config() {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(
        "application.name = \"demo\"\n[logging.console]\ncolor = \"never\"\n\
         [logging.file]\nenabled = true\nrotation = \"never\"\nname = \"out.log\"\npath = {:?}",
        dir.path().to_str().unwrap()
    );
    let out = Captured::new();
    app(&toml, &out).run().await.unwrap();
    let mut text = String::new();
    for entry in std::fs::read_dir(dir.path()).unwrap() {
        text.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    assert!(text.contains("oxbloom::boot"), "{text:?}");
    assert!(text.contains("Starting demo"), "{text:?}");
    assert!(!text.contains('\u{1b}'), "no ANSI in files: {text:?}");
}
