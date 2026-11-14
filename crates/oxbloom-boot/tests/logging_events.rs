//! The events boot emits at target `oxbloom::boot`. A binary of its own: the subscriber is
//! global, so every scenario runs in order inside a single test and shares one install.
#![cfg(feature = "logging")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use oxbloom_boot::{
    Banner, OxbloomApplication, OxbloomApplicationBuilder, RunListener, Service, ServiceError,
    ShutdownHandle, StartupPhase,
};
use oxbloom_di::{Container, ContainerBuilder};
use oxbloom_logging::{Level, LoggingSystem};
use oxbloom_test::Captured;

/// Remembers the `LoggingSystem` bean at `context_ready`.
#[derive(Clone, Default)]
struct Bean(Arc<Mutex<Option<LoggingSystem>>>);

impl RunListener for Bean {
    fn context_ready(&self, container: &Container) {
        *self.0.lock().unwrap() = container.get::<LoggingSystem>().ok().map(|s| (*s).clone());
    }
}

/// Stops the run once it is ready: a run with a service otherwise waits for a signal.
struct StopWhenReady(ShutdownHandle);

impl RunListener for StopWhenReady {
    fn ready(&self, _: &Container) {
        self.0.trigger();
    }
}

/// A service with a name and a start phase; `N` makes the types distinct beans.
struct Named<const N: u8>(&'static str, i32, bool);

#[async_trait]
impl<const N: u8> Service for Named<N> {
    async fn start(&self) -> Result<(), ServiceError> {
        if self.2 {
            return Err(ServiceError::new("refused to start"));
        }
        Ok(())
    }

    async fn stop(&self) {}

    fn phase(&self) -> i32 {
        self.1
    }

    fn name(&self) -> &'static str {
        self.0
    }
}

/// Registers a `Named<N>` as a `Service` bean.
fn service<const N: u8>(
    b: ContainerBuilder,
    name: &'static str,
    phase: i32,
    fail: bool,
) -> ContainerBuilder {
    b.instance(Named::<N>(name, phase, fail))
        .provides(|s: Arc<Named<N>>| -> Arc<dyn Service> { s })
}

/// The configuration files of one run: `application.toml`, plus the `dev` profile file.
fn dir(toml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("application.toml"), toml).unwrap();
    std::fs::write(dir.path().join("application-dev.toml"), "").unwrap();
    dir
}

/// Colors off: `auto` looks at stdout, not at the capture writer.
const BASE: &str = "[logging.console]\ncolor = \"never\"\n";

fn app(
    toml: &str,
    dir: &tempfile::TempDir,
    out: &Captured,
    logs: &Captured,
    bean: &Bean,
) -> OxbloomApplicationBuilder {
    let path = dir.path().to_owned();
    let writer = logs.clone();
    std::fs::write(
        dir.path().join("application.toml"),
        format!("{toml}\n{BASE}"),
    )
    .unwrap();
    let builder = OxbloomApplication::builder();
    let stop = StopWhenReady(builder.shutdown_handle());
    builder
        .args(["--oxbloom.profiles.active=dev"])
        .listener(stop)
        .config(move |b| b.dir(path).env_from([]).renamed("old.key", "new.key"))
        .banner(Banner::off())
        .output(out.clone())
        .listener(bean.clone())
        .logging_writer(move || writer.clone())
}

/// The lines added to `logs` since the last call.
fn added(logs: &Captured, seen: &mut usize) -> Vec<String> {
    let all = logs.contents();
    let lines = all[*seen..].lines().map(str::to_owned).collect();
    *seen = all.len();
    lines
}

/// The position of the first line containing `needle`, which must exist.
fn at(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle:?} in {lines:#?}"))
}

#[tokio::test]
async fn startup_events_in_order() {
    let (out, logs, bean) = (Captured::new(), Captured::new(), Bean::default());
    let mut seen = 0;
    let cfg = dir("");

    // starting_and_started_events, service_events_in_order, config_warning_is_warn_level and
    // condition_decisions_at_debug_only (absent at info): the first run installs, at `info`.
    app(
        "application.name = \"demo\"\n[old]\nkey = 1",
        &cfg,
        &out,
        &logs,
        &bean,
    )
    .configure(|b| service::<1>(service::<2>(b, "Second", 1, false), "First", 0, false))
    .configure(|b| b.instance(7u8).on_missing_bean::<u8>())
    .run()
    .await
    .unwrap();
    let lines = added(&logs, &mut seen);
    assert!(
        lines.iter().all(|l| l.contains("oxbloom::boot")),
        "{lines:#?}"
    );
    let starting = at(&lines, "Starting demo");
    assert!(lines[starting].contains("INFO"), "{lines:#?}");
    assert!(lines[starting].contains("dev"), "profiles: {lines:#?}");
    let warning = at(&lines, "old.key");
    assert!(lines[warning].contains("WARN"), "{lines:#?}");
    let first = at(&lines, "First started");
    let second = at(&lines, "Second started");
    let stop_second = at(&lines, "Second stopped");
    let stop_first = at(&lines, "First stopped");
    let started = at(&lines, "Started demo in ");
    assert!(lines[started].contains("INFO"), "{lines:#?}");
    assert!(starting < first && first < second, "{lines:#?}");
    assert!(second < started, "{lines:#?}");
    assert!(
        started < stop_second && stop_second < stop_first,
        "{lines:#?}"
    );
    assert!(lines[first].contains("INFO") && lines[stop_first].contains("INFO"));
    assert!(
        !lines.iter().any(|l| l.contains("DEBUG")),
        "no condition decisions at info: {lines:#?}"
    );
    assert_eq!(out.contents(), "", "events are not report lines");
    // The subscriber is installed by now; later applications log through it.
    let logging = bean
        .0
        .lock()
        .unwrap()
        .clone()
        .expect("a LoggingSystem bean");

    // condition_decisions_at_debug_only, the debug half: one event per decision.
    logging.set_level("oxbloom::boot", Level::Debug).unwrap();
    app("", &cfg, &out, &logs, &Bean::default())
        .configure(|b| b.instance(7u8).on_missing_bean::<u8>())
        .run()
        .await
        .unwrap();
    let lines = added(&logs, &mut seen);
    let decisions: Vec<&String> = lines.iter().filter(|l| l.contains("DEBUG")).collect();
    assert_eq!(decisions.len(), 1, "{lines:#?}");
    assert!(decisions[0].contains("oxbloom::boot"), "{lines:#?}");
    assert!(decisions[0].contains("on_missing_bean"), "{lines:#?}");
    assert!(decisions[0].contains("matched"), "{lines:#?}");

    // The default name: no `application.name` is configured.
    assert!(
        lines.iter().any(|l| l.contains("Starting application")),
        "{lines:#?}"
    );
    logging.set_level("oxbloom::boot", Level::Info).unwrap();
    added(&logs, &mut seen);

    // name_line_breaks_become_spaces: a name cannot forge a second log line.
    app(
        "application.name = \"demo\\nINFO forged\\r\"",
        &cfg,
        &out,
        &logs,
        &Bean::default(),
    )
    .run()
    .await
    .unwrap();
    let lines = added(&logs, &mut seen);
    assert!(
        !lines.iter().any(|l| l.starts_with("INFO forged")),
        "{lines:#?}"
    );
    assert!(!logs.contents().contains('\r'), "{lines:#?}");
    at(&lines, "Starting demo INFO forged ");
    at(&lines, "Started demo INFO forged  in ");

    // web_server_address_event: the bound address is logged once the server listens.
    #[cfg(feature = "web")]
    {
        let server = oxbloom_boot::WebServer::new(
            oxbloom_web::App::new(),
            &oxbloom_config::Config::from_str("[server]\nport = 0").unwrap(),
        )
        .unwrap();
        let bean = server.clone();
        let builder = app("", &cfg, &out, &logs, &Bean::default()).configure(move |b| {
            b.instance(bean)
                .provides(|s: Arc<oxbloom_boot::WebServer>| -> Arc<dyn Service> { s })
        });
        builder.run().await.unwrap();
        let addr = server.local_addr().expect("the server was bound");
        let lines = added(&logs, &mut seen);
        let line = at(&lines, "listening");
        assert!(lines[line].contains("INFO"), "{lines:#?}");
        assert!(lines[line].contains("oxbloom::boot"), "{lines:#?}");
        assert!(lines[line].contains(&addr.to_string()), "{lines:#?}");
    }

    // failure_logs_one_error_line_and_still_prints_report.
    let err = app(
        "application.name = \"demo\"",
        &cfg,
        &out,
        &logs,
        &Bean::default(),
    )
    .configure(|b| service::<1>(b, "Broken", 0, true))
    .failure_report(true)
    .run()
    .await
    .unwrap_err();
    assert_eq!(err.phase(), StartupPhase::Service);
    let lines = added(&logs, &mut seen);
    let errors: Vec<&String> = lines.iter().filter(|l| l.contains("ERROR")).collect();
    assert_eq!(errors.len(), 1, "{lines:#?}");
    assert!(errors[0].contains("oxbloom::boot"), "{lines:#?}");
    assert!(errors[0].contains("service phase"), "{lines:#?}");
    assert!(
        !lines.iter().any(|l| l.contains("Started demo")),
        "no Started event after a failure: {lines:#?}"
    );
    let report = out.contents();
    assert!(report.contains("refused to start"), "{report}");
}
