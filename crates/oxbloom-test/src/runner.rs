use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use oxbloom_boot::{
    AutoConfiguration, AutoConfigurations, RunListener, ShutdownHandle, StartupError,
};
use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_di::{ConditionReport, Container, ContainerBuilder, DiError};

use crate::Captured;
#[cfg(doc)]
use crate::TempConfigDir;
use crate::lifecycle::{
    RUN_TIMEOUT, RUN_TIMEOUT_MESSAGE, Setup, assert_clean_shutdown, expect_startup_failure, lock,
    never_ready, startup_failed,
};

/// The check handed to [`ContextRunner::run`], boxed so the probe listener can hold it.
type Check = Box<dyn FnOnce(&ContextView<'_>) + Send>;

/// Runs an application context through the real boot lifecycle for a test.
///
/// The runner starts the application to the ready event, hands the check a [`ContextView`],
/// then shuts the application down and waits at most 10 seconds for the whole run. A panic in
/// the check, such as a failed assertion, is re-thrown after shutdown, so it fails the test.
///
/// The run is hermetic and quiet: the process environment is not read (`env_from([])`), no
/// `application.toml` from the working directory is loaded, the banner is off, the output is
/// captured into a [`Captured`] buffer, `server.port` is `0` and `oxbloom.logging.enabled` is
/// `false` unless a [`property`](Self::property) sets them, and the failure report is printed into
/// the captured output. Boot's stop deadline is 5 seconds, and a shutdown that boot only
/// reports in its output, such as a `Service::stop` that panics or misses the deadline, or a
/// `pre_destroy` hook that panics, fails the run.
///
/// Boot installs its Ctrl-C and SIGTERM handlers whenever at least one `Service` runs, such
/// as a web server, and offers no switch to turn that off. While such a run lasts, a Ctrl-C
/// ends the application under test instead of the test process, and Tokio's handler stays
/// installed afterwards.
///
/// The 10 second bound has two limits. It is a Tokio timer, so it cannot interrupt a check or
/// a `Service::stop` that blocks the thread on a current-thread runtime. And under
/// `#[tokio::test(start_paused = true)]`, Tokio advances paused time on its own whenever the
/// runtime is idle, so boot's 5 second stop deadline (a spurious `shutdown failed`) or the
/// 10 second bound can fire while a server waits on I/O; run the runner without paused time.
///
/// # Examples
///
/// ```
/// use oxbloom_test::ContextRunner;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// ContextRunner::new()
///     .property("demo.name", "x")
///     .configure(|b| b.instance(7u32))
///     .run(|view| {
///         assert_eq!(*view.get::<u32>().unwrap(), 7);
///         assert_eq!(view.config().get::<String>("demo.name").unwrap(), "x");
///     })
///     .await;
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
#[must_use = "a context runner does nothing until `run` or `run_failing` is awaited"]
pub struct ContextRunner {
    setup: Setup,
}

impl std::fmt::Debug for ContextRunner {
    /// Shows counts only: properties may contain secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextRunner")
            .field("properties", &self.setup.properties.len())
            .field("config_str", &self.setup.config_str.is_some())
            .field("config_dir", &self.setup.config_dir)
            .field("profiles", &self.setup.profiles)
            .finish_non_exhaustive()
    }
}

impl Default for ContextRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextRunner {
    /// Creates a runner with no properties, no configuration files and no wiring.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .run(|view| assert!(view.contains::<Config>()))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn new() -> ContextRunner {
        ContextRunner {
            setup: Setup::new(),
        }
    }

    /// Sets the property `key` to `value` as a command-line override (`--key=value`).
    ///
    /// Properties win over every configuration file. Because they are command-line overrides,
    /// a secret such as `security.jwt.secret` is refused here as in production; put secrets in
    /// [`config_str`](Self::config_str) or a [`TempConfigDir`] instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .config_str("[demo]\nname = \"file\"\n")
    ///     .property("demo.name", "override")
    ///     .run(|view| assert_eq!(view.config().get::<String>("demo.name").unwrap(), "override"))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.setup = self.setup.property(key, value);
        self
    }

    /// Uses `toml` as the `application.toml` of the run.
    ///
    /// The text is written into a temporary directory that the runner owns and removes after
    /// the run. Together with [`config_dir`](Self::config_dir), the files of that directory
    /// are copied into the temporary directory first, and `toml` replaces its
    /// `application.toml`. A later call replaces an earlier one.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .config_str("[demo]\nname = \"a\"\n")
    ///     .run(|view| assert_eq!(view.config().get::<String>("demo.name").unwrap(), "a"))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn config_str(mut self, toml: impl Into<String>) -> Self {
        self.setup = self.setup.config_str(toml);
        self
    }

    /// Loads the `application*.toml` files from `path`. A later call replaces an earlier one.
    ///
    /// Without this call, the runner loads no files at all, not even from the working
    /// directory.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::{ContextRunner, TempConfigDir};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let dir = TempConfigDir::new().file("application.toml", "[demo]\nname = \"dir\"\n");
    /// ContextRunner::new()
    ///     .config_dir(dir.path())
    ///     .run(|view| assert_eq!(view.config().get::<String>("demo.name").unwrap(), "dir"))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn config_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.setup = self.setup.config_dir(path);
        self
    }

    /// Activates the profile `name`. Repeatable: later profiles win.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::{ContextRunner, TempConfigDir};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let dir = TempConfigDir::new()
    ///     .file("application.toml", "mode = \"base\"")
    ///     .file("application-dev.toml", "mode = \"dev\"");
    /// ContextRunner::new()
    ///     .config_dir(dir.path())
    ///     .profile("dev")
    ///     .run(|view| assert_eq!(view.config().get::<String>("mode").unwrap(), "dev"))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn profile(mut self, name: impl Into<String>) -> Self {
        self.setup = self.setup.profile(name);
        self
    }

    /// Adds wiring to the container, like `OxbloomApplicationBuilder::configure`. Repeatable;
    /// closures apply in call order.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .configure(|b| b.instance("hello"))
    ///     .run(|view| assert_eq!(*view.get::<&str>().unwrap(), "hello"))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn configure(
        mut self,
        f: impl FnOnce(ContainerBuilder) -> ContainerBuilder + Send + 'static,
    ) -> Self {
        self.setup = self.setup.configure(f);
        self
    }

    /// Adds the auto-configuration `A`, like `OxbloomApplicationBuilder::auto_configuration`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::auto_configuration;
    /// use oxbloom_di::component;
    /// use oxbloom_test::ContextRunner;
    ///
    /// pub struct Greeter(&'static str);
    ///
    /// #[component]
    /// pub struct GreeterAutoConfig;
    ///
    /// #[auto_configuration]
    /// impl GreeterAutoConfig {
    ///     #[bean(on_missing_bean)]
    ///     fn greeter(&self) -> Greeter {
    ///         Greeter("default")
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .auto_configuration::<GreeterAutoConfig>()
    ///     .run(|view| assert_eq!(view.get::<Greeter>().unwrap().0, "default"))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn auto_configuration<A: AutoConfiguration>(mut self) -> Self {
        self.setup = self.setup.auto_configuration::<A>();
        self
    }

    /// Appends the auto-configurations of `list`, like
    /// `OxbloomApplicationBuilder::auto_configurations`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfigurations, auto_configuration};
    /// use oxbloom_di::component;
    /// use oxbloom_test::ContextRunner;
    ///
    /// pub struct Pool;
    ///
    /// #[component]
    /// pub struct PoolAutoConfig;
    ///
    /// #[auto_configuration(on_property("db.url"))]
    /// impl PoolAutoConfig {
    ///     #[bean]
    ///     fn pool(&self) -> Pool {
    ///         Pool
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let list = || AutoConfigurations::new().add::<PoolAutoConfig>();
    /// ContextRunner::new()
    ///     .auto_configurations(list())
    ///     .property("db.url", "mem:test")
    ///     .run(|view| assert!(view.contains::<Pool>()))
    ///     .await;
    /// // Without the property, the condition does not match.
    /// ContextRunner::new()
    ///     .auto_configurations(list())
    ///     .run(|view| assert!(!view.contains::<Pool>()))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn auto_configurations(mut self, list: AutoConfigurations) -> Self {
        self.setup = self.setup.auto_configurations(list);
        self
    }

    /// Drops the auto-configuration `A` from the list, like `OxbloomApplicationBuilder::exclude`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::auto_configuration;
    /// use oxbloom_di::component;
    /// use oxbloom_test::ContextRunner;
    ///
    /// pub struct Metrics;
    ///
    /// #[component]
    /// pub struct MetricsAutoConfig;
    ///
    /// #[auto_configuration]
    /// impl MetricsAutoConfig {
    ///     #[bean]
    ///     fn metrics(&self) -> Metrics {
    ///         Metrics
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .auto_configuration::<MetricsAutoConfig>()
    ///     .exclude::<MetricsAutoConfig>()
    ///     .run(|view| assert!(!view.contains::<Metrics>()))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn exclude<A: AutoConfiguration>(mut self) -> Self {
        self.setup = self.setup.exclude::<A>();
        self
    }

    /// Registers `clock` as the `Arc<dyn Clock>` bean of the run, which the JWT
    /// auto-configuration reads. With the feature `logging`, it is also handed to
    /// `OxbloomApplicationBuilder::logging_clock` for the JSON log timestamps, although the
    /// runner keeps the logging installation off.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::{Duration, SystemTime};
    /// use oxbloom_core::Clock;
    /// use oxbloom_test::{ContextRunner, FixedClock};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let at = SystemTime::UNIX_EPOCH + Duration::from_secs(60);
    /// ContextRunner::new()
    ///     .clock(Arc::new(FixedClock::at(at)))
    ///     .run(move |view| assert_eq!(view.get::<dyn Clock>().unwrap().now(), at))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.setup = self.setup.clock(clock);
        self
    }

    /// Runs the application to the ready event, calls `check` with a [`ContextView`], then
    /// shuts the application down.
    ///
    /// The check runs inside boot's `ready` listener callback. A panic in it, such as a
    /// failed assertion, is caught there, shutdown still happens, and the panic is re-thrown
    /// with its original payload once the run has ended.
    ///
    /// # Panics
    ///
    /// Panics with the check's payload when the check panics; with `startup failed: ` followed
    /// by the error and the captured output, which holds the failure report, when startup
    /// fails; with `shutdown failed` followed by the captured output when boot reports a
    /// failed shutdown, such as a `Service::stop` that panics or misses the 5 second deadline;
    /// and with `the run did not finish within 10 seconds (startup and shutdown)` when the
    /// whole run takes longer. A panic of the check wins over a failed shutdown. The bounds
    /// cannot interrupt a check or `stop` that blocks a current-thread runtime, and paused
    /// Tokio time can fire them early (see [`ContextRunner`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .configure(|b| b.instance(7u32))
    ///     .run(|view| {
    ///         assert_eq!(*view.get::<u32>().unwrap(), 7);
    ///         assert!(!view.contains::<String>());
    ///     })
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub async fn run(self, check: impl FnOnce(&ContextView<'_>) + Send + 'static) {
        let output = Captured::new();
        let (builder, _dir) = self.setup.prepare(&output);
        let probe = Arc::new(Probe {
            check: Mutex::new(Some(Box::new(check))),
            payload: Mutex::new(None),
            shutdown: builder.shutdown_handle(),
            output: output.clone(),
        });
        let builder = builder.listener(ProbeListener(probe.clone()));
        match tokio::time::timeout(RUN_TIMEOUT, builder.run()).await {
            Err(_) => panic!("{RUN_TIMEOUT_MESSAGE}"),
            Ok(Err(error)) => startup_failed(&error, &output),
            Ok(Ok(_)) => {
                if let Some(payload) = lock(&probe.payload).take() {
                    resume_unwind(payload);
                }
                assert_clean_shutdown(&output);
                if lock(&probe.check).is_some() {
                    never_ready(&output);
                }
            }
        }
    }

    /// Runs the application and expects startup to fail, then calls `check` with a
    /// [`FailureView`] of the error and the captured output.
    ///
    /// # Panics
    ///
    /// Panics with `startup succeeded but a failure was expected` when startup succeeds, and
    /// with `the run did not finish within 10 seconds (startup and shutdown)` when the whole
    /// run takes longer.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::StartupPhase;
    /// use oxbloom_config::Config;
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     // `Config` is registered by boot; registering it again fails the container.
    ///     .configure(|b| b.instance(Config::from_str("").unwrap()))
    ///     .run_failing(|failure| {
    ///         assert_eq!(failure.error().phase(), StartupPhase::Container);
    ///         assert!(failure.output().contains("APPLICATION FAILED TO START"));
    ///     })
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub async fn run_failing(self, check: impl FnOnce(&FailureView<'_>)) {
        let output = Captured::new();
        let (builder, _dir) = self.setup.prepare(&output);
        let error = expect_startup_failure(builder, &output).await;
        check(&FailureView {
            error: &error,
            output: &output,
        });
    }
}

/// The state the runner shares with its listener.
struct Probe {
    check: Mutex<Option<Check>>,
    payload: Mutex<Option<Box<dyn Any + Send>>>,
    shutdown: ShutdownHandle,
    output: Captured,
}

/// Runs the check at the ready event, keeps its panic, and always triggers shutdown.
struct ProbeListener(Arc<Probe>);

impl RunListener for ProbeListener {
    fn ready(&self, container: &Container) {
        let check = lock(&self.0.check).take();
        if let Some(check) = check {
            let view = ContextView {
                container,
                output: &self.0.output,
            };
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| check(&view))) {
                *lock(&self.0.payload) = Some(payload);
            }
        }
        self.0.shutdown.trigger();
    }
}

/// A view of a ready application context, handed to the check of [`ContextRunner::run`].
///
/// It borrows the container, so it lives only as long as the check.
///
/// # Examples
///
/// ```
/// use oxbloom_test::ContextRunner;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// ContextRunner::new()
///     .configure(|b| b.instance(1u8))
///     .run(|view| {
///         assert!(view.contains::<u8>());
///         assert!(view.conditions().records().is_empty());
///     })
///     .await;
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct ContextView<'a> {
    container: &'a Container,
    output: &'a Captured,
}

impl std::fmt::Debug for ContextView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextView").finish_non_exhaustive()
    }
}

impl ContextView<'_> {
    /// Returns the bean `T`, or the container's error that says why there is none.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::DiError;
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .configure(|b| b.instance(7u32))
    ///     .run(|view| {
    ///         assert_eq!(*view.get::<u32>().unwrap(), 7);
    ///         assert!(matches!(view.get::<String>(), Err(DiError::Missing { .. })));
    ///     })
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Arc<T>, DiError> {
        self.container.get::<T>()
    }

    /// Returns whether the container holds a bean `T` that resolves.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .configure(|b| b.instance(7u32))
    ///     .run(|view| {
    ///         assert!(view.contains::<u32>());
    ///         assert!(!view.contains::<String>());
    ///     })
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn contains<T: ?Sized + Send + Sync + 'static>(&self) -> bool {
        self.container.get::<T>().is_ok()
    }

    /// Returns the loaded configuration.
    ///
    /// # Panics
    ///
    /// Panics if the container holds no `Config`, which boot always registers.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .property("demo.size", "3")
    ///     .run(|view| assert_eq!(view.config().get::<u32>("demo.size").unwrap(), 3))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn config(&self) -> Arc<Config> {
        self.container
            .get::<Config>()
            .expect("boot registers the configuration as a bean")
    }

    /// Returns the report of the conditions the container evaluated, with the outcome of each.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .configure(|b| b.instance(1u8).on_property("feature.on"))
    ///     .run(|view| {
    ///         let record = &view.conditions().records()[0];
    ///         assert_eq!(record.condition(), "on_property");
    ///         assert!(!record.matched());
    ///     })
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn conditions(&self) -> &ConditionReport {
        self.container.condition_report()
    }

    /// Returns the output the application wrote so far.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     // The banner is off, so a plain run writes nothing.
    ///     .run(|view| assert_eq!(view.output().contents(), ""))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn output(&self) -> &Captured {
        self.output
    }
}

/// A view of a failed startup, handed to the check of [`ContextRunner::run_failing`].
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_test::ContextRunner;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// ContextRunner::new()
///     .configure(|b| b.instance(Config::from_str("").unwrap()))
///     .run_failing(|failure| assert!(failure.error().analysis().is_some()))
///     .await;
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct FailureView<'a> {
    error: &'a StartupError,
    output: &'a Captured,
}

impl std::fmt::Debug for FailureView<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FailureView")
            .field("error", self.error)
            .finish_non_exhaustive()
    }
}

impl FailureView<'_> {
    /// Returns the startup error.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::StartupPhase;
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .config_str("not toml")
    ///     .run_failing(|failure| assert_eq!(failure.error().phase(), StartupPhase::Config))
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn error(&self) -> &StartupError {
        self.error
    }

    /// Returns the captured output, which holds the printed failure report.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_test::ContextRunner;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// ContextRunner::new()
    ///     .configure(|b| b.instance(Config::from_str("").unwrap()))
    ///     .run_failing(|failure| {
    ///         assert!(failure.output().contains("APPLICATION FAILED TO START"));
    ///         assert!(failure.output().contains("is registered more than once"));
    ///     })
    ///     .await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn output(&self) -> &Captured {
        self.output
    }
}
