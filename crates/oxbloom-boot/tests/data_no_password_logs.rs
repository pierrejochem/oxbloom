//! No password, user or host of a data source URL reaches the failure report, the error, the
//! output or the debug logs when a Postgres source is unreachable at start. A binary of its
//! own with a single test: the log subscriber is installed once per process by the first run
//! with logging, so only a test that owns the process is sure its sink receives the events.

#![cfg(feature = "data-postgres")]

use oxbloom_boot::{Banner, DataAutoConfiguration, OxbloomApplication, StartupPhase};
use oxbloom_config::Config;
use oxbloom_test::Captured;

/// A Postgres URL with a password to a closed port: the boot fails at the start-up ping, and
/// neither the report nor the debug logs contain the password, the user or the host.
#[tokio::test]
async fn no_password_in_the_report_or_logs() {
    // A port that was free a moment ago: connecting is refused at once.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let password = "pw-that-must-never-leak";
    let mut toml = format!(
        "[data]\nurl = \"postgres://leaky-user:{password}@127.0.0.1:{port}/app\"\n\
         acquire-timeout = \"1s\"\n"
    );
    if cfg!(feature = "logging") {
        toml.push_str("[logging.console]\ncolor = \"never\"\n[logging.level]\nroot = \"debug\"\n");
    }
    let output = Captured::new();
    let logs = Captured::new();
    let builder = OxbloomApplication::builder()
        .with_config(Config::from_str(&toml).unwrap())
        .banner(Banner::off())
        .output(output.clone())
        .auto_configuration::<DataAutoConfiguration>();
    #[cfg(feature = "logging")]
    let builder = {
        let sink = logs.clone();
        builder.logging_writer(move || sink.clone())
    };
    let error = tokio::time::timeout(std::time::Duration::from_secs(20), builder.run())
        .await
        .expect("the run ends")
        .unwrap_err();
    assert_eq!(error.phase(), StartupPhase::Service);
    let report = error.analysis().expect("a report").to_string();
    assert!(report.contains("`default`"), "{report}");
    let texts = [
        report,
        error.to_string(),
        format!("{error:?}"),
        output.contents(),
        logs.contents(),
    ];
    let host = format!("127.0.0.1:{port}");
    for text in &texts {
        assert!(!text.contains(password), "{text}");
        assert!(!text.contains("leaky-user"), "{text}");
        assert!(!text.contains(&host), "{text}");
        assert!(!text.contains("postgres://"), "{text}");
    }
    if cfg!(feature = "logging") {
        // This test is the only one in the binary, so its run installed the subscriber.
        assert!(
            logs.contents()
                .contains("startup failed in the service phase"),
            "{}",
            logs.contents()
        );
    }
}
