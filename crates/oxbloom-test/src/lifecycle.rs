//! The setup and the lifecycle rules that `ContextRunner` and `TestApp` share.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use oxbloom_boot::{
    AutoConfiguration, AutoConfigurations, Banner, OxbloomApplication, OxbloomApplicationBuilder,
    RunListener, ShutdownHandle, StartupError,
};
use oxbloom_core::Clock;
use oxbloom_di::{Container, ContainerBuilder};

use crate::{Captured, TempConfigDir};

/// The command-line defaults of every run, ahead of the user's properties.
pub(crate) const DEFAULT_ARGS: [&str; 2] = ["--server.port=0", "--oxbloom.logging.enabled=false"];

/// How long a run may take, from start to the end of shutdown.
pub(crate) const RUN_TIMEOUT: Duration = Duration::from_secs(10);

/// The stop deadline handed to boot, below `RUN_TIMEOUT`, so boot reports a hung `stop`
/// itself before the outer bound fires.
pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// The panic text when the whole run exceeds `RUN_TIMEOUT`.
pub(crate) const RUN_TIMEOUT_MESSAGE: &str =
    "the run did not finish within 10 seconds (startup and shutdown)";

/// Boot reports a failed shutdown only as a line in its output, and `run` still returns `Ok`.
/// These are the start and end of those lines:
///
/// - `service {name} did not stop in time` and `service {name} panicked while stopping`, from
///   `stop_all` in `crates/oxbloom-boot/src/service.rs`;
/// - `pre_destroy of {type} panicked during shutdown`, from `cleanup` in
///   `crates/oxbloom-boot/src/app.rs`.
///
/// The tests `a_service_whose_stop_panics_fails_the_run`,
/// `a_service_whose_stop_never_returns_fails_the_run_within_the_bound` and
/// `a_panicking_pre_destroy_fails_the_run` fail if that wording drifts.
pub(crate) const SHUTDOWN_FAILURES: [(&str, &str); 3] = [
    ("service ", " did not stop in time"),
    ("service ", " panicked while stopping"),
    ("pre_destroy of ", " panicked during shutdown"),
];

/// Returns whether `output` holds one of boot's shutdown-failure lines.
fn has_shutdown_failure(output: &str) -> bool {
    output.lines().any(|line| {
        SHUTDOWN_FAILURES
            .iter()
            .any(|(start, end)| line.starts_with(start) && line.ends_with(end))
    })
}

/// The configuration of a run: the boot builder plus the properties, files and profiles that
/// `prepare` turns into the hermetic defaults.
pub(crate) struct Setup {
    pub(crate) builder: OxbloomApplicationBuilder,
    pub(crate) properties: Vec<String>,
    pub(crate) config_str: Option<String>,
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) profiles: Vec<String>,
}

impl Setup {
    /// Creates a setup with no properties, no configuration files and no wiring.
    pub(crate) fn new() -> Setup {
        Setup {
            builder: OxbloomApplication::builder(),
            properties: Vec::new(),
            config_str: None,
            config_dir: None,
            profiles: Vec::new(),
        }
    }

    /// Sets the property `key` to `value` as a command-line override (`--key=value`).
    pub(crate) fn property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties
            .push(format!("--{}={}", key.into(), value.into()));
        self
    }

    /// Uses `toml` as the `application.toml` of the run.
    pub(crate) fn config_str(mut self, toml: impl Into<String>) -> Self {
        self.config_str = Some(toml.into());
        self
    }

    /// Loads the `application*.toml` files from `path`.
    pub(crate) fn config_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_dir = Some(path.into());
        self
    }

    /// Activates the profile `name`.
    pub(crate) fn profile(mut self, name: impl Into<String>) -> Self {
        self.profiles.push(name.into());
        self
    }

    /// Adds wiring to the container.
    pub(crate) fn configure(
        mut self,
        f: impl FnOnce(ContainerBuilder) -> ContainerBuilder + Send + 'static,
    ) -> Self {
        self.builder = self.builder.configure(f);
        self
    }

    /// Adds the auto-configuration `A`.
    pub(crate) fn auto_configuration<A: AutoConfiguration>(mut self) -> Self {
        self.builder = self.builder.auto_configuration::<A>();
        self
    }

    /// Appends the auto-configurations of `list`.
    pub(crate) fn auto_configurations(mut self, list: AutoConfigurations) -> Self {
        self.builder = self.builder.auto_configurations(list);
        self
    }

    /// Drops the auto-configuration `A` from the list.
    pub(crate) fn exclude<A: AutoConfiguration>(mut self) -> Self {
        self.builder = self.builder.exclude::<A>();
        self
    }

    /// Registers `clock` as the `Arc<dyn Clock>` bean of the run. With the feature `logging`,
    /// it is also handed to `OxbloomApplicationBuilder::logging_clock`.
    pub(crate) fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        let bean = clock.clone();
        self.builder = self.builder.configure(move |b| {
            b.instance(RunnerClock(bean))
                .provides(|c: Arc<RunnerClock>| -> Arc<dyn Clock> { c })
        });
        #[cfg(feature = "logging")]
        {
            self.builder = self.builder.logging_clock(clock);
        }
        #[cfg(not(feature = "logging"))]
        drop(clock);
        self
    }

    /// Applies the hermetic defaults and the configuration to the boot builder. The returned
    /// directory must live until the run has ended.
    pub(crate) fn prepare(
        self,
        output: &Captured,
    ) -> (OxbloomApplicationBuilder, Option<TempConfigDir>) {
        let (dir, owned) = match (self.config_str, self.config_dir) {
            (None, Some(path)) => (path, None),
            (text, path) => {
                let owned = TempConfigDir::new();
                if let Some(path) = path {
                    copy_dir(&path, owned.path());
                }
                if let Some(text) = text {
                    std::fs::write(owned.path().join("application.toml"), text)
                        .expect("the application.toml of the runner can be written");
                }
                (owned.path().to_path_buf(), Some(owned))
            }
        };
        let profiles = self.profiles;
        // The defaults come first, so a property for the same key wins: a repeated key keeps the
        // last value. The logging switch is a property as well, because `.logging(false)` below
        // exists only when this crate's own `logging` feature is on, while boot's feature may be
        // on through another crate.
        let args = DEFAULT_ARGS
            .iter()
            .map(|&a| a.to_owned())
            .chain(self.properties);
        let builder = self
            .builder
            .config(move |b| {
                profiles
                    .into_iter()
                    .fold(b.dir(dir).env_from([]), |b, p| b.profile(p))
            })
            .args(args)
            .banner(Banner::off())
            .output(output.clone())
            .failure_report(true)
            .shutdown_timeout(SHUTDOWN_TIMEOUT);
        #[cfg(feature = "logging")]
        let builder = builder.logging(false);
        (builder, owned)
    }
}

/// Copies the regular files and directories of `from` into `to`, recursively. Symbolic links
/// are skipped, so a link that points back up the tree cannot make the copy loop.
fn copy_dir(from: &Path, to: &Path) {
    let entries = std::fs::read_dir(from).unwrap_or_else(|e| {
        panic!(
            "the configuration directory {} cannot be read: {e}",
            from.display()
        )
    });
    for entry in entries {
        let entry = entry.expect("a directory entry can be read");
        let target = to.join(entry.file_name());
        // `DirEntry::file_type` does not follow symbolic links.
        let kind = entry.file_type().expect("a file type can be read");
        if kind.is_dir() {
            std::fs::create_dir_all(&target).expect("a directory can be created");
            copy_dir(&entry.path(), &target);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target).expect("a configuration file can be copied");
        }
    }
}

/// Panics with `shutdown failed` and the captured output when boot reported a failed shutdown.
pub(crate) fn assert_clean_shutdown(output: &Captured) {
    let text = output.contents();
    if has_shutdown_failure(&text) {
        panic!("shutdown failed\n{text}");
    }
}

/// Panics with the startup `error` and the captured output, which holds the failure report.
pub(crate) fn startup_failed(error: &StartupError, output: &Captured) -> ! {
    panic!("startup failed: {error}\n{}", output.contents())
}

/// Panics because the run ended without reaching the ready event.
pub(crate) fn never_ready(output: &Captured) -> ! {
    panic!(
        "the application stopped without becoming ready\n{}",
        output.contents()
    )
}

/// Runs `builder` and returns its startup error, within `RUN_TIMEOUT`.
///
/// A run that unexpectedly succeeds is shut down at the ready event, so it still ends, and then
/// panics with `startup succeeded but a failure was expected`.
pub(crate) async fn expect_startup_failure(
    builder: OxbloomApplicationBuilder,
    output: &Captured,
) -> StartupError {
    let shutdown = builder.shutdown_handle();
    let builder = builder.listener(TriggerOnReady(shutdown));
    match tokio::time::timeout(RUN_TIMEOUT, builder.run()).await {
        Err(_) => panic!("{RUN_TIMEOUT_MESSAGE}"),
        Ok(Ok(_)) => panic!(
            "startup succeeded but a failure was expected\n{}",
            output.contents()
        ),
        Ok(Err(error)) => error,
    }
}

/// Triggers shutdown at the ready event, so a run that was expected to fail still ends when it
/// starts a service.
struct TriggerOnReady(ShutdownHandle);

impl RunListener for TriggerOnReady {
    fn ready(&self, _container: &Container) {
        self.0.trigger();
    }
}

/// Locks `mutex`, also when a panic poisoned it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// The clock handed to `ContextRunner::clock`, as a concrete bean type.
struct RunnerClock(Arc<dyn Clock>);

impl Clock for RunnerClock {
    fn now(&self) -> SystemTime {
        self.0.now()
    }
}
