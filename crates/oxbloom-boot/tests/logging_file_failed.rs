//! A container failure is logged to the file: the log writer outlives the container. A binary
//! of its own: the subscriber is global.
#![cfg(feature = "logging")]

use oxbloom_boot::{Banner, OxbloomApplication, StartupPhase};
use oxbloom_config::Config;
use oxbloom_test::Captured;

#[tokio::test]
async fn startup_failure_reaches_the_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(
        "[logging.console]\ncolor = \"never\"\n\
         [logging.file]\nenabled = true\nrotation = \"never\"\nname = \"out.log\"\npath = {:?}",
        dir.path().to_str().unwrap()
    );
    let sink = Captured::new();
    let console = sink.clone();
    // Two beans of one type: the container build fails.
    let err = OxbloomApplication::builder()
        .with_config(Config::from_str(&toml).unwrap())
        .banner(Banner::off())
        .output(std::io::sink())
        .logging_writer(move || console.clone())
        .configure(|b| b.instance(1_u32).instance(2_u32))
        .run()
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Container);
    assert!(
        sink.contents().contains("startup failed"),
        "{}",
        sink.contents()
    );
    let text = std::fs::read_to_string(dir.path().join("out.log")).unwrap();
    assert!(text.contains("Starting"), "{text:?}");
    assert!(text.contains("startup failed"), "{text:?}");
}
