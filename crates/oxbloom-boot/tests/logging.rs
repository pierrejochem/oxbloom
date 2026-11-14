//! Boot installs the logging subscriber. A binary of its own: the subscriber is global, so
//! the scenarios that need the one install run in order inside a single test.
#![cfg(feature = "logging")]

use std::sync::{Arc, Mutex};

use oxbloom_boot::{
    Banner, OxbloomApplication, OxbloomApplicationBuilder, RunListener, StartupPhase,
};
use oxbloom_config::Config;
use oxbloom_di::Container;
use oxbloom_logging::{Level, LoggingSystem};
use oxbloom_test::Captured;

/// Remembers the `LoggingSystem` bean at `context_ready`, or `None` when there is none.
#[derive(Clone, Default)]
struct Bean(Arc<Mutex<Option<LoggingSystem>>>);

impl RunListener for Bean {
    fn context_ready(&self, container: &Container) {
        *self.0.lock().unwrap() = container.get::<LoggingSystem>().ok().map(|s| (*s).clone());
    }
}

impl Bean {
    fn get(&self) -> Option<LoggingSystem> {
        self.0.lock().unwrap().clone()
    }
}

/// An application with `toml` as its configuration, no banner, `out` as the output target and
/// `bean` as a listener.
fn app(toml: &str, out: &Captured, bean: &Bean) -> OxbloomApplicationBuilder {
    OxbloomApplication::builder()
        .with_config(Config::from_str(toml).unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .listener(bean.clone())
}

/// Colors off: `auto` looks at stdout, not at the capture writer.
const NO_COLOR: &str = "[logging.console]\ncolor = \"never\"\n";

#[tokio::test]
async fn install_once_in_order() {
    // The failing cases come first: they must run before the install, because the failure
    // `error` event would otherwise land in the capture of the first application.
    bad_level_fails_startup_naming_key().await;
    bad_enabled_value_fails_startup_naming_key().await;
    bad_level_prints_no_banner().await;

    // disabled_by_builder: nothing is installed and there is no bean.
    let (out, bean) = (Captured::new(), Bean::default());
    app("", &out, &bean).logging(false).run().await.unwrap();
    assert!(bean.get().is_none());
    assert!(!tracing::dispatcher::has_been_set());

    // disabled_by_property, in the file and on the command line.
    let bean = Bean::default();
    app("oxbloom.logging.enabled = false", &out, &bean)
        .run()
        .await
        .unwrap();
    assert!(bean.get().is_none());
    let bean = Bean::default();
    OxbloomApplication::builder()
        .args(["--oxbloom.logging.enabled=false"])
        .config(|b| b.dir("does-not-exist").env_from([]))
        .banner(Banner::off())
        .output(out.clone())
        .listener(bean.clone())
        .run()
        .await
        .unwrap();
    assert!(bean.get().is_none());
    assert!(!tracing::dispatcher::has_been_set());
    assert_eq!(out.contents(), "");

    // logging_system_is_a_bean: the first enabled run installs, and the bean changes levels.
    let logs = Captured::new();
    let bean = Bean::default();
    let writer = logs.clone();
    app(
        &format!("{NO_COLOR}[logging.level]\nroot = \"warn\""),
        &out,
        &bean,
    )
    .logging_writer(move || writer.clone())
    .run()
    .await
    .unwrap();
    assert!(tracing::dispatcher::has_been_set());
    let logging = bean.get().expect("a LoggingSystem bean");
    tracing::info!(target: "app", "hidden-before");
    assert!(
        !logs.contents().contains("hidden-before"),
        "{}",
        logs.contents()
    );
    logging.set_level("app", Level::Info).unwrap();
    tracing::info!(target: "app", "shown-after");
    assert!(
        logs.contents().contains("shown-after"),
        "{}",
        logs.contents()
    );
    assert_eq!(
        out.contents(),
        "",
        "logging writes nothing to the output target"
    );

    // already_installed_is_a_warning: one `warn` through the installed subscriber, no line
    // on the output target, no bean, and the second writer is never used.
    let before = logs.contents();
    let (out2, logs2, bean) = (Captured::new(), Captured::new(), Bean::default());
    let writer = logs2.clone();
    app(NO_COLOR, &out2, &bean)
        .logging_writer(move || writer.clone())
        .run()
        .await
        .unwrap();
    assert!(bean.get().is_none());
    assert_eq!(out2.contents(), "");
    assert_eq!(logs2.contents(), "");
    let added = logs.contents()[before.len()..].to_owned();
    let lines: Vec<&str> = added.lines().collect();
    assert_eq!(lines.len(), 1, "{added:?}");
    assert!(lines[0].contains("WARN"), "{added:?}");
    assert!(lines[0].contains("oxbloom::boot"), "{added:?}");
    assert!(
        lines[0].contains(
            "logging not installed: a global tracing subscriber or `log` logger is already installed"
        ),
        "{added:?}"
    );
    // The first application's levels still apply.
    tracing::info!(target: "other", "still-hidden");
    assert!(!logs.contents().contains("still-hidden"));
}

/// An invalid level fails before anything is installed or printed.
async fn bad_level_fails_startup_naming_key() {
    // The filter is built and checked before anything is installed.
    let (out, bean) = (Captured::new(), Bean::default());
    let err = app("[logging.level]\nx = \"hunter2\"", &out, &bean)
        .failure_report(true)
        .run()
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    assert!(bean.get().is_none());
    let report = out.contents();
    assert!(report.contains("`logging.level.x`"), "{report}");
    assert!(!report.contains("hunter2"), "{report}");
    assert!(!err.to_string().contains("hunter2"), "{err}");
    let analysis = err.analysis().expect("an analysis").to_string();
    assert!(
        analysis.contains("off, error, warn, info, debug or trace"),
        "{analysis}"
    );
}

async fn bad_enabled_value_fails_startup_naming_key() {
    let (out, bean) = (Captured::new(), Bean::default());
    let err = app("oxbloom.logging.enabled = \"hunter2\"", &out, &bean)
        .run()
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    assert!(err.to_string().contains("oxbloom.logging.enabled"), "{err}");
    assert!(!err.to_string().contains("hunter2"), "{err}");
    assert_eq!(out.contents(), "");
}

/// The install happens before the banner, so a rejected `logging.*` value prints no banner.
async fn bad_level_prints_no_banner() {
    let (out, bean) = (Captured::new(), Bean::default());
    let err = OxbloomApplication::builder()
        .with_config(Config::from_str("[logging.level]\nx = \"hunter2\"").unwrap())
        .banner(Banner::text("BANNER-MARKER"))
        .output(out.clone())
        .listener(bean.clone())
        .failure_report(true)
        .run()
        .await
        .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Config);
    let printed = out.contents();
    assert!(printed.contains("`logging.level.x`"), "{printed}");
    assert!(!printed.contains("BANNER-MARKER"), "{printed}");
}
