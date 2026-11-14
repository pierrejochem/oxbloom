//! The guest greeter of `src/auto.rs`: the auto-configured default, the
//! application's override behind `hello.custom-greeter`, and the conditions
//! evaluation report that explains both.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hello::auto::GuestGreeter;
use oxbloom_boot::{RunListener, ShutdownHandle};
use oxbloom_di::Container;
use oxbloom_test::Captured;

/// Records the guest greeter's welcome once the application is ready, then
/// stops it.
struct Probe {
    welcome: Arc<Mutex<Option<String>>>,
    handle: ShutdownHandle,
}

impl RunListener for Probe {
    fn ready(&self, container: &Container) {
        let guest = container.get::<dyn GuestGreeter>().unwrap();
        *self.welcome.lock().unwrap() = Some(guest.welcome());
        self.handle.trigger();
    }
}

/// Runs the example with `args` until it is ready; returns the guest welcome
/// and everything written to the output target.
async fn run(args: &[&str]) -> (String, String) {
    let welcome = Arc::new(Mutex::new(None));
    let out = Captured::new();
    let builder = hello::application(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(args.iter().copied())
        .output(out.clone());
    let probe = Probe {
        welcome: Arc::clone(&welcome),
        handle: builder.shutdown_handle(),
    };
    let status = tokio::time::timeout(Duration::from_secs(5), builder.listener(probe).run())
        .await
        .expect("test timed out")
        .expect("application ran");
    assert_eq!(status.code(), 0);
    let welcome = welcome.lock().unwrap().take().expect("ready fired");
    let text = out.contents();
    (welcome, text)
}

/// The part of the report between `from` and `to`.
fn section<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let start = text.find(from).expect("section start") + from.len();
    let end = text[start..].find(to).map_or(text.len(), |i| start + i);
    &text[start..end]
}

#[tokio::test]
async fn the_auto_configured_default_is_used_without_the_flag() {
    let (welcome, out) = run(&["--server.port=0"]).await;
    assert_eq!(welcome, "Welcome, guest!");
    // The report is off by default.
    assert!(!out.contains("CONDITIONS EVALUATION REPORT"), "{out}");
}

#[tokio::test]
async fn the_user_bean_wins_with_the_flag() {
    let (welcome, _) = run(&["--server.port=0", "--hello.custom-greeter=true"]).await;
    assert_eq!(welcome, "Make yourself at home, guest!");
}

#[tokio::test]
async fn a_flag_that_is_not_exactly_true_keeps_the_default() {
    let (welcome, _) = run(&["--server.port=0", "--hello.custom-greeter=yes"]).await;
    assert_eq!(welcome, "Welcome, guest!");
}

#[tokio::test]
async fn the_report_shows_the_default_matched_and_the_override_skipped() {
    let (_, out) = run(&[
        "--server.port=0",
        "--oxbloom.main.conditions-report=console",
    ])
    .await;
    let matched = section(&out, "Matched:\n", "\n\nSkipped:");
    let skipped = section(&out, "Skipped:\n", "\n");
    assert!(
        matched.contains(
            "hello::auto::GuestAutoConfig: on_property - property \"hello.greeting\" is set"
        ),
        "{out}"
    );
    assert!(
        matched.contains(
            "hello::auto::GuestAutoConfig::standard_guest_greeter: on_missing_bean - \
             no earlier registration provides `dyn hello::auto::GuestGreeter`"
        ),
        "{out}"
    );
    assert!(
        skipped.ends_with("on_property_value - property \"hello.custom-greeter\" is not set"),
        "{out}"
    );
}

#[tokio::test]
async fn the_report_shows_the_override_matched_and_the_default_skipped() {
    let args = [
        "--server.port=0",
        "--hello.custom-greeter=true",
        "--oxbloom.main.conditions-report=console",
    ];
    let (_, out) = run(&args).await;
    let matched = section(&out, "Matched:\n", "\n\nSkipped:");
    let skipped = section(&out, "Skipped:\n", "\n");
    assert!(
        matched.contains(
            "on_property_value - property \"hello.custom-greeter\" has the expected value"
        ),
        "{out}"
    );
    assert_eq!(
        skipped,
        "  hello::auto::GuestAutoConfig::standard_guest_greeter: on_missing_bean - \
         an earlier registration already provides `dyn hello::auto::GuestGreeter`",
        "{out}"
    );
    // The report never shows a configuration value.
    assert!(
        !out.contains("= true") && !out.contains("\"true\""),
        "{out}"
    );
}

#[tokio::test]
async fn console_report_lists_web_auto_configurations() {
    let (_, out) = run(&[
        "--server.port=0",
        "--oxbloom.main.conditions-report=console",
    ])
    .await;
    let matched = section(&out, "Matched:\n", "\n\nSkipped:");
    assert!(
        matched.contains(
            "oxbloom_boot::autoconfigure::server::WebServerAutoConfiguration: on_bean - \
             an earlier registration provides `dyn oxbloom_web::controller::Controller`"
        ),
        "{out}"
    );
    assert!(
        matched.contains(
            "on_missing_bean - no earlier registration provides `oxbloom_boot::web::WebServer`"
        ),
        "{out}"
    );
}
