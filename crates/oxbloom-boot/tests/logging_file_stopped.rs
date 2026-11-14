//! The last events of a run, logged by a `stopped` listener after the container shut down,
//! reach the log file. A binary of its own: the subscriber is global.
#![cfg(feature = "logging")]

use oxbloom_boot::{Banner, ExitStatus, OxbloomApplication, RunListener};
use oxbloom_config::Config;

struct LogsStopped;

impl RunListener for LogsStopped {
    fn stopped(&self, status: &ExitStatus) {
        tracing::info!(target: "app", "listener saw stopped with {}", status.code());
    }
}

#[tokio::test]
async fn stopped_listener_event_reaches_the_log_file() {
    let dir = tempfile::tempdir().unwrap();
    let toml = format!(
        "[logging.console]\ncolor = \"never\"\n\
         [logging.file]\nenabled = true\nrotation = \"never\"\nname = \"out.log\"\npath = {:?}",
        dir.path().to_str().unwrap()
    );
    let status = OxbloomApplication::builder()
        .with_config(Config::from_str(&toml).unwrap())
        .banner(Banner::off())
        .output(std::io::sink())
        .logging_writer(std::io::sink)
        .listener(LogsStopped)
        .run()
        .await
        .unwrap();
    assert_eq!(status.code(), 0);
    let text = std::fs::read_to_string(dir.path().join("out.log")).unwrap();
    assert!(text.contains("Started"), "{text:?}");
    assert!(text.contains("listener saw stopped with 0"), "{text:?}");
}
