use std::borrow::Cow;
use std::future::Future;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use oxbloom_config::{Config, ConfigBuilder, ConfigError};
use oxbloom_di::{Container, ContainerBuilder};

use crate::arguments::ApplicationArguments;
use crate::autoconfig::{self, AutoConfiguration, AutoConfigurations, ConfigRef};
use crate::banner::Banner;
use crate::error::{ExitStatus, StartupError, StartupPhase};
use crate::exit::{self, ExitCodeGenerator};
use crate::failure::{self, FailureAnalyzer};
use crate::listener::{Listeners, RunListener};
use crate::metadata::{PropertyMetadata, boot_properties};
use crate::output::Output;
use crate::runner::{ApplicationRunner, CommandLineRunner};
use crate::service::{self, Service};
use crate::shutdown::ShutdownHandle;
use crate::signal::{self, Cause};

type ConfigFn = Box<dyn FnOnce(ConfigBuilder) -> ConfigBuilder + Send>;
type ConfigureFn = Box<dyn FnOnce(ContainerBuilder) -> ContainerBuilder + Send>;
/// Makes the writer of the logging console output; see
/// [`OxbloomApplicationBuilder::logging_writer`].
#[cfg(feature = "logging")]
type LogWriterFn = Arc<dyn Fn() -> Box<dyn Write> + Send + Sync>;

/// The default for [`OxbloomApplicationBuilder::shutdown_timeout`].
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// The entry point of an Oxbloom application.
///
/// It loads the configuration, builds the container, fires lifecycle events
/// on [`RunListener`]s, runs the [`ApplicationRunner`] and
/// [`CommandLineRunner`] beans and starts and stops the [`Service`] beans. Start with [`OxbloomApplication::builder`].
///
/// ```
/// use oxbloom_boot::OxbloomApplication;
/// use oxbloom_config::Config;
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let status = OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .output(std::io::sink())
///     .run()
///     .await?;
/// assert_eq!(status.code(), 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct OxbloomApplication;

impl OxbloomApplication {
    /// Starts configuring an application run.
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// let builder = OxbloomApplication::builder().args(["--port=8080"]);
    /// # let _ = builder;
    /// ```
    pub fn builder() -> OxbloomApplicationBuilder {
        OxbloomApplicationBuilder::new()
    }
}

enum ConfigSource {
    Build(ConfigFn),
    /// Boxed: the builder is moved by value many times, and an inline `Config` would make
    /// every copy (and the run future) about 100 bytes larger.
    Ready(Box<Config>),
}

/// Configures and runs an Oxbloom application; created by
/// [`OxbloomApplication::builder`].
///
/// ```
/// use std::time::Duration;
/// use oxbloom_boot::OxbloomApplication;
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// # let dir = tempfile::tempdir()?;
/// # let dir = dir.path().to_owned();
/// let status = OxbloomApplication::builder()
///     .args(["--server.port=9000"])
///     .config(move |b| b.dir(dir).env_from([]))
///     .configure(|b| b.instance(42u32))
///     .shutdown_timeout(Duration::from_secs(10))
///     .output(std::io::sink())
///     .run()
///     .await?;
/// assert_eq!(status.code(), 0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[must_use = "an application does nothing until `run` is awaited"]
pub struct OxbloomApplicationBuilder {
    args: Vec<String>,
    config: ConfigSource,
    configure: Vec<ConfigureFn>,
    listeners: Vec<Arc<dyn RunListener>>,
    shutdown_timeout: Duration,
    shutdown: ShutdownHandle,
    output: Output,
    banner: Banner,
    analyzers: Vec<Arc<dyn FailureAnalyzer>>,
    failure_report: Option<bool>,
    auto: AutoConfigurations,
    excluded: Vec<ConfigRef>,
    /// `None` until [`logging`](Self::logging) or [`logging_writer`](Self::logging_writer) is
    /// called, so the builder grows by one pointer only: it is moved by value many times.
    #[cfg(feature = "logging")]
    logging: Option<Box<Logging>>,
}

impl std::fmt::Debug for OxbloomApplicationBuilder {
    /// Shows counts only: arguments may contain secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("OxbloomApplicationBuilder");
        d.field("args", &self.args.len())
            .field("configure", &self.configure.len())
            .field("listeners", &self.listeners.len())
            .field("shutdown_timeout", &self.shutdown_timeout)
            .field("analyzers", &self.analyzers.len())
            .field("failure_report", &self.failure_report)
            .field("auto_configurations", &self.auto.len())
            .field("excluded", &self.excluded.len());
        #[cfg(feature = "logging")]
        d.field("logging", &self.logging.as_ref().is_none_or(|l| l.enabled));
        d.finish_non_exhaustive()
    }
}

impl OxbloomApplicationBuilder {
    fn new() -> Self {
        Self {
            args: Vec::new(),
            config: ConfigSource::Build(Box::new(|b| b)),
            configure: Vec::new(),
            listeners: Vec::new(),
            shutdown_timeout: DEFAULT_SHUTDOWN_TIMEOUT,
            shutdown: ShutdownHandle::new(),
            output: Output::stderr(),
            banner: Banner::default(),
            analyzers: Vec::new(),
            failure_report: None,
            auto: AutoConfigurations::new(),
            excluded: Vec::new(),
            #[cfg(feature = "logging")]
            logging: None,
        }
    }

    /// The logging settings, created with the defaults on first use.
    #[cfg(feature = "logging")]
    fn logging_mut(&mut self) -> &mut Logging {
        self.logging.get_or_insert_with(|| {
            Box::new(Logging {
                enabled: true,
                writer: None,
                clock: None,
                began: None,
                system: None,
            })
        })
    }

    /// Sets the command-line arguments, program name excluded. Default: none.
    ///
    /// They are parsed into [`ApplicationArguments`] and also handed to
    /// `ConfigBuilder::args` after the [`config`](Self::config) closure runs,
    /// so `--oxbloom.profiles.active=dev` and `--key=value` overrides work and
    /// replace any `args` the closure sets. With [`with_config`](Self::with_config)
    /// they are not applied to the configuration, but still populate
    /// [`ApplicationArguments`].
    ///
    /// Arguments are `String`: `std::env::args()` panics on an argument that is
    /// not valid UTF-8, so a program that must survive one passes
    /// `std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned())`.
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// let builder = OxbloomApplication::builder().args(std::env::args().skip(1));
    /// # let _ = builder;
    /// ```
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Adjusts the `ConfigBuilder` that boot creates with `Config::builder()`.
    ///
    /// Replaces an earlier [`with_config`](Self::with_config) or `config` call:
    /// the last one wins. Boot applies the [`args`](Self::args) with
    /// `ConfigBuilder::args` after the closure runs, replacing any `args` the
    /// closure sets, so a closure cannot drop the command line.
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// let builder = OxbloomApplication::builder().config(|b| b.dir("config").env_prefix("MYAPP"));
    /// # let _ = builder;
    /// ```
    pub fn config(
        mut self,
        f: impl FnOnce(ConfigBuilder) -> ConfigBuilder + Send + 'static,
    ) -> Self {
        self.config = ConfigSource::Build(Box::new(f));
        self
    }

    /// Uses a ready `Config` instead of building one, typically in tests.
    ///
    /// Replaces an earlier [`config`](Self::config) or `with_config` call: the
    /// last one wins. The [`args`](Self::args) are not applied to a ready
    /// config; they still populate [`ApplicationArguments`].
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    /// use oxbloom_config::Config;
    ///
    /// let builder = OxbloomApplication::builder().with_config(Config::from_str("name = \"demo\"")?);
    /// # let _ = builder;
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = ConfigSource::Ready(Box::new(config));
        self
    }

    /// Adds user wiring to the container. Repeatable; closures apply in call order,
    /// after boot has registered `Config`, [`ApplicationArguments`] and
    /// [`ShutdownHandle`] as instances. Registering one of those types again is a
    /// `StartupError` in the `Container` phase.
    ///
    /// `Config` is already the container's condition environment, so
    /// `.on_property(..)` and the other property conditions in a closure read the
    /// configuration; a closure that calls `ContainerBuilder::environment` replaces
    /// it for every condition, those of the auto-configurations included.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::{ApplicationArguments, OxbloomApplication};
    ///
    /// struct Greeting(String);
    ///
    /// let builder = OxbloomApplication::builder()
    ///     .configure(|b| b.instance(7u8))
    ///     .configure(|b| {
    ///         b.bean(|args: Arc<ApplicationArguments>| {
    ///             Greeting(format!("hello {}", args.option("name").unwrap_or("world")))
    ///         })
    ///     });
    /// # let _ = builder;
    /// ```
    pub fn configure(
        mut self,
        f: impl FnOnce(ContainerBuilder) -> ContainerBuilder + Send + 'static,
    ) -> Self {
        self.configure.push(Box::new(f));
        self
    }

    /// Adds the [`AutoConfiguration`] `A` to the application's list. Repeatable; adding a
    /// configuration that is already in the list keeps its first position.
    ///
    /// When the container is built, boot registers the auto-configurations after the
    /// [`configure`](Self::configure) closures, each as one group named by its type name with
    /// its class-level conditions, in the order their `after` and `before` entries give (ties
    /// keep list order). User registrations therefore come first, so a default with
    /// `on_missing_bean` backs off when the application registered its own bean. The
    /// conditions read `Config` as their environment, unless a `configure` closure sets
    /// another one with `ContainerBuilder::environment`. An ordering cycle fails the run in the
    /// `Container` phase before the container is built. Set `oxbloom.main.conditions-report` to
    /// `console` to see which groups and beans were kept or skipped, and why (see the
    /// [crate documentation](crate)).
    ///
    /// ```
    /// use oxbloom_boot::{OxbloomApplication, auto_configuration};
    /// use oxbloom_config::Config;
    /// use oxbloom_di::component;
    ///
    /// pub struct Greeter(&'static str);
    ///
    /// #[component]
    /// pub struct GreeterAutoConfig;
    ///
    /// #[auto_configuration]
    /// impl GreeterAutoConfig {
    ///     // Backs off when the application registered its own `Greeter`.
    ///     #[bean(on_missing_bean)]
    ///     fn greeter(&self) -> Greeter {
    ///         Greeter("default")
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let status = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("")?)
    ///     .auto_configuration::<GreeterAutoConfig>()
    ///     .configure(|b| b.instance(Greeter("mine")))
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await?;
    /// assert_eq!(status.code(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn auto_configuration<A: AutoConfiguration>(mut self) -> Self {
        self.auto = std::mem::take(&mut self.auto).add::<A>();
        self
    }

    /// Appends the auto-configurations of `list`, typically the `auto_configurations()` of a
    /// starter crate, to the application's list. Lists combine in call order, also with
    /// [`auto_configuration`](Self::auto_configuration); a configuration that is already in the
    /// list keeps its first position. See [`auto_configuration`](Self::auto_configuration) for
    /// how they are registered.
    ///
    /// ```
    /// use oxbloom_boot::{AutoConfigurations, OxbloomApplication, auto_configuration};
    /// use oxbloom_config::Config;
    /// use oxbloom_di::component;
    ///
    /// pub struct Pool;
    /// pub struct Repository;
    ///
    /// #[component]
    /// pub struct PoolAutoConfig;
    /// #[auto_configuration(on_property("db.url"))]
    /// impl PoolAutoConfig {
    ///     #[bean]
    ///     fn pool(&self) -> Pool { Pool }
    /// }
    ///
    /// #[component]
    /// pub struct RepositoryAutoConfig;
    /// #[auto_configuration(after(PoolAutoConfig), on_bean(Pool))]
    /// impl RepositoryAutoConfig {
    ///     #[bean]
    ///     fn repository(&self) -> Repository { Repository }
    /// }
    ///
    /// /// What a starter crate exposes; the order of the list does not matter here.
    /// pub fn auto_configurations() -> AutoConfigurations {
    ///     AutoConfigurations::new()
    ///         .add::<RepositoryAutoConfig>()
    ///         .add::<PoolAutoConfig>()
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let status = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("[db]\nurl = \"mem:test\"")?)
    ///     .auto_configurations(auto_configurations())
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await?;
    /// assert_eq!(status.code(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn auto_configurations(mut self, list: AutoConfigurations) -> Self {
        self.auto.extend(list);
        self
    }

    /// Drops the auto-configuration `A` from the application's list: it is not registered,
    /// and `after` or `before` entries that name it are ignored. Exclusions apply to the
    /// combined list when [`run`](Self::run) resolves it, before the container is built, so
    /// they work whether they are called before or after the configuration is added;
    /// excluding a configuration that is not in the list does nothing.
    ///
    /// Use it to replace a starter's default with your own wiring: exclude the configuration
    /// and register the bean yourself.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::{AutoConfigurations, OxbloomApplication, auto_configuration};
    /// use oxbloom_config::Config;
    /// use oxbloom_di::component;
    ///
    /// pub struct Metrics(&'static str);
    ///
    /// #[component]
    /// pub struct MetricsAutoConfig;
    /// #[auto_configuration]
    /// impl MetricsAutoConfig {
    ///     #[bean]
    ///     fn metrics(&self) -> Metrics { Metrics("default") }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let status = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("")?)
    ///     .exclude::<MetricsAutoConfig>()
    ///     .auto_configurations(AutoConfigurations::new().add::<MetricsAutoConfig>())
    ///     // The application's own `Metrics`; without the exclusion it would be a duplicate.
    ///     .configure(|b| b.instance(Metrics("mine")))
    ///     .configure(|b| {
    ///         b.bean(|m: Arc<Metrics>| {
    ///             assert_eq!(m.0, "mine");
    ///             1_u8
    ///         })
    ///     })
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await?;
    /// assert_eq!(status.code(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn exclude<A: AutoConfiguration>(mut self) -> Self {
        let config = ConfigRef::of::<A>();
        if !self.excluded.contains(&config) {
            self.excluded.push(config);
        }
        self
    }

    /// The configuration keys this application reads: the boot keys plus those of the
    /// registered auto-configurations that are not excluded, sorted by key. A key declared more
    /// than once appears once, with its first declaration.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{OxbloomApplication, PropertyMetadata};
    ///
    /// let list = OxbloomApplication::builder().property_metadata();
    /// assert!(list.iter().any(|p| p.key == "oxbloom.config.import"));
    /// assert!(list.windows(2).all(|w| w[0].key < w[1].key));
    /// assert!(PropertyMetadata::to_json(&list).starts_with("[{\"key\":"));
    /// ```
    pub fn property_metadata(&self) -> Vec<PropertyMetadata> {
        let mut list: Vec<PropertyMetadata> = boot_properties().to_vec();
        #[cfg(feature = "logging")]
        list.extend(
            oxbloom_logging::PROPERTIES
                .iter()
                .map(|&(key, ty, default, text)| PropertyMetadata::new(key, ty, default, text)),
        );
        for d in self.auto.descriptors() {
            if !self.excluded.contains(&d.config) {
                list.extend_from_slice((d.properties)());
            }
        }
        // Stable, so the first declaration of a key stays ahead of the later ones.
        list.sort_by(|a, b| a.key.cmp(b.key));
        list.dedup_by(|later, first| later.key == first.key);
        list
    }

    /// Adds a listener that receives every phase, including `starting` and
    /// `environment_prepared`, which happen before the container exists.
    /// `RunListener` beans in the container receive `context_ready` onward, after
    /// the builder listeners.
    ///
    /// ```
    /// use oxbloom_boot::{OxbloomApplication, RunListener};
    ///
    /// struct Hello;
    /// impl RunListener for Hello {
    ///     fn starting(&self) {
    ///         println!("starting");
    ///     }
    /// }
    ///
    /// let builder = OxbloomApplication::builder().listener(Hello);
    /// # let _ = builder;
    /// ```
    pub fn listener(mut self, listener: impl RunListener + 'static) -> Self {
        self.listeners.push(Arc::new(listener));
        self
    }

    /// Bounds the whole stop sequence. Default: 30 seconds.
    ///
    /// All [`Service`] stops share one deadline, `shutdown_timeout` after the
    /// stop sequence begins. A `stop` still running at the deadline is aborted
    /// and its name written to the output; a service reached at or after the
    /// deadline is not stopped at all and also named. `Duration::ZERO` gives no
    /// grace: no `stop` runs and every started service is named.
    ///
    /// The deadline is enforced by the Tokio timer, so it cannot interrupt a
    /// `stop` that blocks the thread: a blocking `std::thread::sleep` in `stop`
    /// on a current-thread runtime delays `run()` for as long as it blocks. The
    /// `pre_destroy` hooks of the container shutdown that follows are not
    /// bounded by the deadline, and `Service::start` has no timeout at all.
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// let builder = OxbloomApplication::builder().shutdown_timeout(Duration::from_secs(5));
    /// # let _ = builder;
    /// ```
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// The [`ShutdownHandle`] of this application. Triggering it ends the run
    /// without a signal; a trigger before the run waits is remembered.
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// let builder = OxbloomApplication::builder();
    /// let handle = builder.shutdown_handle();
    /// handle.trigger();
    /// assert!(builder.shutdown_handle().is_triggered());
    /// ```
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        self.shutdown.clone()
    }

    /// Sets where the banner and diagnostics are written. Default: stderr.
    /// Write errors are ignored.
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// let builder = OxbloomApplication::builder().output(std::io::sink());
    /// # let _ = builder;
    /// ```
    pub fn output(mut self, output: impl Write + Send + 'static) -> Self {
        self.output = Output::new(output);
        self
    }

    /// Sets the startup banner. Default: [`Banner::default`].
    ///
    /// It is written to the output target after `environment_prepared` and
    /// before the container is built. The config property
    /// `oxbloom.main.banner-mode` overrides it: `console` prints (the default
    /// banner if this one is off), `off` prints nothing, any other value fails
    /// the run in the `Config` phase.
    ///
    /// ```
    /// use oxbloom_boot::{Banner, OxbloomApplication};
    ///
    /// let builder = OxbloomApplication::builder().banner(Banner::text("My service"));
    /// # let _ = builder;
    /// ```
    pub fn banner(mut self, banner: Banner) -> Self {
        self.banner = banner;
        self
    }

    /// Adds a [`FailureAnalyzer`] that is asked when startup fails, in any
    /// phase. Repeatable; builder analyzers are asked in call order, before
    /// `Arc<dyn FailureAnalyzer>` beans and the built-in analyzers, and the
    /// first analysis wins. The analysis is attached to the [`StartupError`]
    /// (see [`StartupError::analysis`]) and printed as the failure report.
    ///
    /// ```
    /// use std::io::Write;
    /// use std::sync::{Arc, Mutex};
    /// use oxbloom_boot::{
    ///     FailureAnalysis, FailureAnalyzer, OxbloomApplication, StartupError,
    /// };
    /// use oxbloom_config::Config;
    /// use oxbloom_di::DiError;
    ///
    /// struct DuplicateBean;
    ///
    /// impl FailureAnalyzer for DuplicateBean {
    ///     fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
    ///         match failure.find_cause::<DiError>()? {
    ///             DiError::Duplicate { .. } => Some(FailureAnalysis::new(
    ///                 "A bean is registered twice.",
    ///                 "Remove one of the registrations.",
    ///             )),
    ///             _ => None,
    ///         }
    ///     }
    /// }
    ///
    /// # #[derive(Clone, Default)]
    /// # struct Buf(Arc<Mutex<Vec<u8>>>);
    /// # impl Write for Buf {
    /// #     fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
    /// #         self.0.lock().unwrap().extend_from_slice(b);
    /// #         Ok(b.len())
    /// #     }
    /// #     fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    /// # }
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let out = Buf::default();
    /// // `Config` is registered by boot; registering it again fails.
    /// let code = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("")?)
    ///     .configure(|b| b.instance(Config::from_str("").unwrap()))
    ///     .failure_analyzer(DuplicateBean)
    ///     .output(out.clone())
    ///     .run_and_exit()
    ///     .await;
    /// # let _ = code;
    /// let text = String::from_utf8(out.0.lock().unwrap().clone())?;
    /// assert!(text.contains("APPLICATION FAILED TO START"));
    /// assert!(text.contains("Description:\n\nA bean is registered twice."));
    /// assert!(text.ends_with("Action:\n\nRemove one of the registrations.\n"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn failure_analyzer(mut self, analyzer: impl FailureAnalyzer) -> Self {
        self.analyzers.push(Arc::new(analyzer));
        self
    }

    /// Sets whether the failure report is printed to the output target.
    ///
    /// [`run`](Self::run) prints it only when this is set to `true`.
    /// [`run_and_exit`](Self::run_and_exit) prints it unless this is set to
    /// `false`, in which case it writes the bare error line instead. The
    /// report is printed once, as the last step before the error is returned.
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// // A test that wants the report on stderr from `run()`.
    /// let builder = OxbloomApplication::builder().failure_report(true);
    /// # let _ = builder;
    /// ```
    pub fn failure_report(mut self, enabled: bool) -> Self {
        self.failure_report = Some(enabled);
        self
    }

    /// Sets whether [`run`](Self::run) installs the logging subscriber. Default: `true`.
    ///
    /// When enabled and the property `oxbloom.logging.enabled` is not `false`, boot calls
    /// `oxbloom_logging::LoggingSystem::init` right after the configuration is loaded and
    /// before the banner and the container, and registers the `LoggingSystem` as a bean.
    /// Boot keeps its own clone until `run` returns, so the log file also receives the
    /// `failed` and shutdown events; events logged after `run` returns are not written to it.
    /// An invalid `logging.*` value fails the run in the `Config` phase with a report that
    /// names the key, never the value. When the process already has a global `tracing`
    /// subscriber, for example from an earlier run, boot keeps it, registers no bean and
    /// emits one `warn` event with target `oxbloom::boot` through it; nothing is written
    /// to the [`output`](Self::output) target.
    ///
    /// ```
    /// use oxbloom_boot::OxbloomApplication;
    /// use oxbloom_config::Config;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// // A test that installs its own subscriber, or none.
    /// let status = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("")?)
    ///     .logging(false)
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await?;
    /// assert_eq!(status.code(), 0);
    /// assert!(!tracing::dispatcher::has_been_set());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg(feature = "logging")]
    pub fn logging(mut self, enabled: bool) -> Self {
        self.logging_mut().enabled = enabled;
        self
    }

    /// Sends the logging console output to the writers `make` returns instead of standard
    /// output, so tests can capture it. `make` is called for every event. Not a public API.
    ///
    /// ```
    /// use std::io::Write;
    /// use std::sync::{Arc, Mutex};
    /// use oxbloom_boot::OxbloomApplication;
    /// use oxbloom_config::Config;
    ///
    /// #[derive(Clone, Default)]
    /// struct Buf(Arc<Mutex<Vec<u8>>>);
    /// impl Write for Buf {
    ///     fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
    ///         self.0.lock().unwrap().extend_from_slice(b);
    ///         Ok(b.len())
    ///     }
    ///     fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let logs = Buf::default();
    /// let writer = logs.clone();
    /// OxbloomApplication::builder()
    ///     .with_config(Config::from_str("logging.console.color = \"never\"")?)
    ///     .logging_writer(move || writer.clone())
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await?;
    /// tracing::info!(target: "app", "captured");
    /// assert!(String::from_utf8(logs.0.lock().unwrap().clone())?.contains("captured"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg(feature = "logging")]
    #[doc(hidden)]
    pub fn logging_writer<W: Write + 'static>(
        mut self,
        make: impl Fn() -> W + Send + Sync + 'static,
    ) -> Self {
        self.logging_mut().writer = Some(Arc::new(move || Box::new(make()) as Box<dyn Write>));
        self
    }

    /// Reads the timestamps of the JSON log output from `clock` instead of the system time,
    /// so tests can pin the time in the log lines.
    ///
    /// Logging starts before the container exists, so a `dyn Clock` bean cannot reach it; this
    /// method passes the clock directly. It has no effect when logging is off or when a global
    /// subscriber was already installed.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::{Duration, SystemTime};
    /// use oxbloom_boot::OxbloomApplication;
    /// use oxbloom_config::Config;
    /// use oxbloom_core::Clock;
    ///
    /// struct OneDayAfterTheEpoch;
    ///
    /// impl Clock for OneDayAfterTheEpoch {
    ///     fn now(&self) -> SystemTime {
    ///         SystemTime::UNIX_EPOCH + Duration::from_secs(86_400)
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let status = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("logging.console.format = \"json\"")?)
    ///     .logging_clock(Arc::new(OneDayAfterTheEpoch))
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await?;
    /// // JSON log lines now carry "timestamp":"1970-01-02T00:00:00.000Z".
    /// assert_eq!(status.code(), 0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[cfg(feature = "logging")]
    pub fn logging_clock(mut self, clock: Arc<dyn oxbloom_core::Clock>) -> Self {
        self.logging_mut().clock = Some(clock);
        self
    }

    /// Runs the application: loads the configuration, builds the container, runs
    /// the runners, starts the [`Service`] beans, waits for shutdown, stops the
    /// services and shuts the container down.
    ///
    /// With no service, `run()` returns right after the runners and listens for
    /// no signal. With at least one, it waits for the first of Ctrl-C, SIGTERM
    /// (Unix only) and the [`ShutdownHandle`] (at once if it already was
    /// triggered), then stops the services in reverse start order within
    /// [`shutdown_timeout`](Self::shutdown_timeout) and returns the exit code:
    /// the highest [`ExitCodeGenerator::exit_code`] among the beans that provide
    /// `dyn ExitCodeGenerator`, each first mapped to 0..=255 (a negative code or
    /// one above 255 becomes 255), read after the services stopped and the
    /// container was shut down, or 0 with no generator. See
    /// [`run_and_exit`](Self::run_and_exit) for the process exit code.
    ///
    /// A Ctrl-C or SIGTERM while the services stop (a second signal, or the
    /// first one after a [`ShutdownHandle`] trigger) forces the exit: the
    /// rest of the stop sequence is abandoned, `Container::shutdown()` is
    /// skipped (no `pre_destroy` hook runs), `stopped` fires and `run()`
    /// returns `Ok` with code 130 for Ctrl-C or 143 for SIGTERM. The
    /// [`ShutdownHandle`] is the portable way to end a run, for tests and for
    /// platforms without SIGTERM.
    ///
    /// Signal handlers are installed from just before the services start until
    /// the stop sequence ends, and only when there is at least one service. A
    /// signal while a service is still starting is kept: the remaining services
    /// start, then the wait ends at once and they stop gracefully. Tokio's
    /// process-wide handler stays installed afterwards, so later Ctrl-C or
    /// SIGTERM no longer terminate the process by default. Whenever a service
    /// is registered, a Tokio runtime with the time and IO drivers is required,
    /// such as `#[tokio::main]` or a builder with `enable_all()`.
    ///
    /// Await `run()` to completion. Dropping the future, or losing it in a
    /// `select!`, leaves the started services running and the container is never
    /// shut down; trigger the [`ShutdownHandle`] instead. The stop deadline
    /// cannot interrupt a `stop` that blocks the thread, the `pre_destroy` hooks
    /// are not bounded by it, and `Service::start` has no timeout.
    ///
    /// On failure, the [`FailureAnalyzer`]s are asked and the first analysis is
    /// attached to the error, `failed` is called once on every listener known
    /// at that point, the services started so far are stopped in reverse
    /// order, the container is shut down if it was built, and the error names
    /// the phase. Its `source()` is the `ConfigError`, `DiError`,
    /// `RunnerError` or `ServiceError`. The failure report is printed only
    /// with [`failure_report(true)`](Self::failure_report).
    ///
    /// ```
    /// use oxbloom_boot::{OxbloomApplication, StartupPhase};
    /// use oxbloom_config::Config;
    /// use oxbloom_di::DiError;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// let status = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("")?)
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await?;
    /// assert_eq!(status.code(), 0);
    ///
    /// // `Config` is registered by boot; registering it again fails.
    /// let err = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("")?)
    ///     .configure(|b| b.instance(Config::from_str("").unwrap()))
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await
    ///     .unwrap_err();
    /// assert_eq!(err.phase(), StartupPhase::Container);
    /// let source = std::error::Error::source(&err).and_then(|s| s.downcast_ref::<DiError>());
    /// assert!(matches!(source, Some(DiError::Duplicate { .. })));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn run(self) -> Result<ExitStatus, StartupError> {
        self.run_with_output(false).await.0
    }

    /// Runs like [`run`](Self::run), then turns the result into a process exit
    /// code: the [`ExitStatus`] code, or 1 after a failure. After a failure
    /// the failure report is written to the output target, or, with
    /// [`failure_report(false)`](Self::failure_report), the error's message as
    /// one line. Never both.
    ///
    /// Use it as the tail of `main`, which then returns the code to the
    /// operating system. The code from `run` is already in 0..=255 (see
    /// [`ExitCodeGenerator`] for the mapping).
    ///
    /// ```no_run
    /// use std::process::ExitCode;
    /// use oxbloom_boot::OxbloomApplication;
    ///
    /// #[tokio::main(flavor = "current_thread")]
    /// async fn main() -> ExitCode {
    ///     OxbloomApplication::builder()
    ///         .args(std::env::args().skip(1))
    ///         .run_and_exit()
    ///         .await
    /// }
    /// ```
    pub async fn run_and_exit(self) -> std::process::ExitCode {
        // With the flag unset or `true`, `run_with_output` prints the report.
        let bare_line = self.failure_report == Some(false);
        match self.run_with_output(true).await {
            (Ok(status), _) => exit::to_exit_code(status.code()),
            (Err(error), output) => {
                if bare_line {
                    output.line(&error.to_string());
                }
                std::process::ExitCode::from(1)
            }
        }
    }

    /// Runs, and after a failure analyzes it, fires `failed`, cleans up and
    /// prints the report when the flag (or `print_default` without one) says so.
    async fn run_with_output(
        mut self,
        print_default: bool,
    ) -> (Result<ExitStatus, StartupError>, Output) {
        let analyzers = std::mem::take(&mut self.analyzers);
        let print_report = self.failure_report.unwrap_or(print_default);
        let (mut run, plan) = self.split();
        let mut result = run.execute(plan).await;
        if let Err(error) = &mut result {
            // Collected before `cleanup`, which shuts the container down. A
            // failing `get_all` is ignored: it may be the failure itself. A
            // panicking factory is treated the same way.
            let beans = match &run.container {
                None => Vec::new(),
                Some(c) => catch_unwind(AssertUnwindSafe(|| c.get_all::<dyn FailureAnalyzer>()))
                    .unwrap_or_else(|_| {
                        run.output.line("failure analyzer beans panicked");
                        Ok(Vec::new())
                    })
                    .unwrap_or_default(),
            };
            if let Some(analysis) = failure::analyze_failure(&analyzers, &beans, error, &run.output)
            {
                error.set_analysis(analysis);
            }
            drop(beans);
            #[cfg(feature = "logging")]
            crate::events::failed(error);
            run.listeners.fire(&run.output, |l| l.failed(error));
            run.cleanup(std::future::pending()).await;
            if print_report {
                run.output.line(&failure::report_text(error));
            }
        }
        (result, run.output)
    }

    fn split(self) -> (Run, Plan) {
        let run = Run {
            listeners: Listeners(self.listeners),
            output: self.output,
            container: None,
            started: Vec::new(),
            shutdown_timeout: self.shutdown_timeout,
            #[cfg(feature = "logging")]
            logging: {
                let mut logging = self.logging.unwrap_or_else(|| {
                    Box::new(Logging {
                        enabled: true,
                        writer: None,
                        clock: None,
                        began: None,
                        system: None,
                    })
                });
                logging.began = Some(std::time::Instant::now());
                logging
            },
        };
        let plan = Plan {
            args: self.args,
            config: self.config,
            configure: self.configure,
            shutdown: self.shutdown,
            banner: self.banner,
            auto: self.auto,
            excluded: self.excluded,
        };
        (run, plan)
    }
}

/// The inputs consumed by [`Run::execute`].
struct Plan {
    args: Vec<String>,
    config: ConfigSource,
    configure: Vec<ConfigureFn>,
    shutdown: ShutdownHandle,
    banner: Banner,
    auto: AutoConfigurations,
    excluded: Vec<ConfigRef>,
}

/// The logging settings of the builder; `None` in the builder means the defaults.
#[cfg(feature = "logging")]
struct Logging {
    enabled: bool,
    writer: Option<LogWriterFn>,
    /// The clock of the JSON log timestamps; `None` means the system time.
    clock: Option<Arc<dyn oxbloom_core::Clock>>,
    /// When the run began, for "Started {app} in {secs}s"; set by `split`.
    began: Option<std::time::Instant>,
    /// A clone of the installed system, so the log file writer outlives the container (which
    /// holds the bean): `failed`, the cleanup and `stopped` still reach the file.
    system: Option<oxbloom_logging::LoggingSystem>,
}

/// State shared by the run steps and the cleanup after a failure.
struct Run {
    listeners: Listeners,
    output: Output,
    container: Option<Container>,
    /// Services started so far, in start order.
    started: Vec<Arc<dyn Service>>,
    shutdown_timeout: Duration,
    /// Boxed, so the run's future grows by one pointer only. Dropped with the `Run`, after
    /// `failed`, the cleanup and `stopped`.
    #[cfg(feature = "logging")]
    logging: Box<Logging>,
}

impl Run {
    fn fire(&self, f: impl Fn(&dyn RunListener)) {
        self.listeners.fire(&self.output, f);
    }

    /// The run steps. A failure returns early; [`OxbloomApplicationBuilder::run`]
    /// then fires `failed` and calls [`Run::cleanup`].
    async fn execute(
        &mut self,
        #[cfg_attr(not(feature = "logging"), allow(unused_mut))] mut plan: Plan,
    ) -> Result<ExitStatus, StartupError> {
        // 1. Arguments.
        let args = ApplicationArguments::new(plan.args.iter().cloned());
        self.fire(|l| l.starting());

        // 2. Configuration.
        let config = match plan.config {
            ConfigSource::Ready(config) => *config,
            // Applied after the closure, so the closure cannot drop them.
            ConfigSource::Build(f) => f(Config::builder())
                .args(plan.args)
                .build()
                .map_err(|e| StartupError::new(StartupPhase::Config, e))?,
        };
        self.fire(|l| l.environment_prepared(&config));
        let banner = banner_line(&config, &plan.banner, self.output.color())?;
        // Read before anything is printed, so a rejected mode prints nothing. With the
        // feature `logging`, this also installs the subscriber (one `?` site, a small frame).
        #[cfg(not(feature = "logging"))]
        let report_mode = conditions_report_mode(&config)?;
        #[cfg(feature = "logging")]
        let report_mode = report_mode_and_logging(&config, &mut self.logging, &mut plan.configure)?;
        if let Some(line) = banner {
            self.output.line(&line);
        }

        // 3. Container.
        let profiles: Vec<String> = config.active_profiles().to_vec();
        // `Config` is the environment of the conditions; a user `environment(..)` replaces it.
        let mut builder = ContainerBuilder::new()
            .environment(config.clone())
            .instance(config)
            .instance(args.clone())
            .instance(plan.shutdown.clone())
            .active_profiles(profiles);
        for f in plan.configure {
            builder = f(builder);
        }
        // Auto-configurations come after the user's wiring, so their `on_missing_bean`
        // conditions back off. A cycle fails before any of them is registered.
        let auto = autoconfig::resolve(&plan.auto, &plan.excluded)
            .map_err(|e| StartupError::new(StartupPhase::Container, e))?;
        for d in auto {
            builder = builder.group(d.config.name(), (d.conditions)(), d.register);
        }
        let container: &Container = self
            .container
            .insert(build_container(builder, report_mode, &self.output).map_err(container_error)?);
        // Builder listeners first, then `RunListener` beans, from here on.
        let beans = container
            .get_all::<dyn RunListener>()
            .map_err(container_error)?;
        self.listeners.0.extend(beans);
        self.listeners
            .fire(&self.output, |l| l.context_ready(container));

        // 4. Runners.
        let app_runners = container
            .get_all::<dyn ApplicationRunner>()
            .map_err(container_error)?;
        for runner in app_runners {
            runner.run(&args).await.map_err(runner_error)?;
        }
        let cmd_runners = container
            .get_all::<dyn CommandLineRunner>()
            .map_err(container_error)?;
        for runner in cmd_runners {
            runner.run(args.raw()).await.map_err(runner_error)?;
        }

        // Exit code generators, collected now: a shut-down container refuses
        // `get_all`. They are evaluated after the services stopped.
        let generators = container
            .get_all::<dyn ExitCodeGenerator>()
            .map_err(container_error)?;

        // 5. Services.
        let services = container
            .get_all::<dyn Service>()
            .map_err(container_error)?;
        // Signal listeners exist from just before the services start until the
        // end of step 8, and only when there is a service: a signal during a
        // slow `start` is kept and ends the step-7 wait at once.
        let signals = (!services.is_empty()).then(signal::Signals::install);
        let started = service::start_all(&services, &self.output).await;
        // From here on, `cleanup` stops what was started.
        self.started = match started {
            Ok(started) => started,
            Err((started, error)) => {
                self.started = started;
                return Err(StartupError::new(StartupPhase::Service, error));
            }
        };

        // 6. Started and ready.
        self.listeners.fire(&self.output, |l| l.started(container));
        self.listeners.fire(&self.output, |l| l.ready(container));
        #[cfg(feature = "logging")]
        announce_started(container, &self.logging);

        // 7. Wait, only when a service was started (and then `signals` exists).
        let forced = match signals {
            None => {
                self.fire(|l| l.stopping());
                self.cleanup(std::future::pending()).await
            }
            Some(mut signals) => {
                signal::wait_for_shutdown(&plan.shutdown, &mut signals).await;
                // 8. The same listeners carry over, so a signal after the step-7
                // wake, during the listeners or the stop sequence is seen.
                let second = signal::second_signal(signals);
                self.fire(|l| l.stopping());
                // 9. inside `cleanup`, skipped after a second signal.
                self.cleanup(second).await
            }
        };

        // 10. Exit status.
        let status = match forced {
            Some(cause) => ExitStatus::new(cause.forced_exit_code()),
            // Each code is mapped before the maximum, so -1 beats 0.
            None => ExitStatus::new(
                generators
                    .iter()
                    .map(|g| exit::process_code(g.exit_code()))
                    .max()
                    .unwrap_or(0),
            ),
        };
        self.fire(|l| l.stopped(&status));
        Ok(status)
    }

    /// Stops the started services in reverse order under one deadline, then
    /// shuts the container down if it was built. Runs at most once per run and
    /// is the only place that calls `Container::shutdown`.
    ///
    /// The stop sequence races `interrupt`. If `interrupt` finishes first, the
    /// rest of the stop sequence is abandoned (the `stop` in progress is
    /// aborted), the container is dropped without `Container::shutdown`, and
    /// the cause is returned.
    async fn cleanup(&mut self, interrupt: impl Future<Output = Cause>) -> Option<Cause> {
        let started = std::mem::take(&mut self.started);
        if !started.is_empty() {
            let stop = service::stop_all(started, deadline(self.shutdown_timeout), &self.output);
            tokio::select! {
                // A finished stop sequence wins over a signal at the same moment.
                biased;
                () = stop => {}
                cause = interrupt => {
                    self.container = None;
                    return Some(cause);
                }
            }
        }
        if let Some(container) = self.container.take() {
            let report = container.shutdown();
            for failure in &report.failures {
                self.output.line(&format!(
                    "pre_destroy of {} panicked during shutdown",
                    failure.type_name
                ));
            }
        }
        None
    }
}

const BANNER_MODE_KEY: &str = "oxbloom.main.banner-mode";

/// The banner line to print, after applying `oxbloom.main.banner-mode`.
fn banner_line(
    config: &Config,
    banner: &Banner,
    color: bool,
) -> Result<Option<String>, StartupError> {
    let line = |b: &Banner| b.line(color).map(Cow::into_owned);
    match config.get::<String>(BANNER_MODE_KEY) {
        Ok(mode) if mode == "off" => Ok(None),
        Ok(mode) if mode == "console" => Ok(line(banner).or_else(|| line(&Banner::default()))),
        // The offending value is never echoed: it could be a secret.
        Ok(_) => Err(StartupError::new(
            StartupPhase::Config,
            format!("unsupported value for {BANNER_MODE_KEY}; expected console or off"),
        )),
        Err(ConfigError::Missing { .. }) => Ok(line(banner)),
        Err(e) => Err(StartupError::new(StartupPhase::Config, e)),
    }
}

const CONDITIONS_REPORT_KEY: &str = "oxbloom.main.conditions-report";

/// Where `oxbloom.main.conditions-report` sends the conditions evaluation report.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReportMode {
    Off,
    Console,
}

/// The value of `oxbloom.main.conditions-report`: `off` (also when the key is absent) or
/// `console`.
fn conditions_report_mode(config: &Config) -> Result<ReportMode, StartupError> {
    match config.get::<String>(CONDITIONS_REPORT_KEY) {
        Ok(mode) if mode == "off" => Ok(ReportMode::Off),
        Ok(mode) if mode == "console" => Ok(ReportMode::Console),
        // The offending value is never echoed: it could be a secret.
        Ok(_) => Err(StartupError::new(
            StartupPhase::Config,
            format!("unsupported value for {CONDITIONS_REPORT_KEY}; expected console or off"),
        )),
        Err(ConfigError::Missing { .. }) => Ok(ReportMode::Off),
        Err(e) => Err(StartupError::new(StartupPhase::Config, e)),
    }
}

#[cfg(feature = "logging")]
const LOGGING_ENABLED_KEY: &str = "oxbloom.logging.enabled";

/// Installs the logging subscriber unless the builder or `oxbloom.logging.enabled` turned it
/// off, then emits "Starting {app}" and one `warn` per configuration warning, so these are the
/// first events a new subscriber sees. On a successful install, the `LoggingSystem` is
/// registered as a bean by a configure step put first in `configure`, and a clone is kept in
/// `logging` until the run ends. When another global
/// tracing subscriber or `log` logger was installed first, the result is one `warn` event
/// through the existing subscriber, nothing on the output target and no bean.
///
/// A synchronous function of its own, so its locals are not part of `execute`'s future.
#[cfg(feature = "logging")]
#[inline(never)]
fn start_logging(
    config: &Config,
    logging: &mut Logging,
    configure: &mut Vec<ConfigureFn>,
) -> Result<(), StartupError> {
    use oxbloom_logging::{LoggingError, LoggingSystem};

    let enabled = match config.get::<bool>(LOGGING_ENABLED_KEY) {
        Ok(enabled) => enabled,
        Err(ConfigError::Missing { .. }) => true,
        Err(e) => return Err(StartupError::new(StartupPhase::Config, e)),
    };
    if logging.enabled && enabled {
        let installed = match (logging.writer.take(), logging.clock.take()) {
            (Some(make), None) => LoggingSystem::init_with_writer(config, move || make()),
            (None, None) => LoggingSystem::init(config),
            (Some(make), Some(clock)) => {
                LoggingSystem::init_with_clock(config, move || make(), clock)
            }
            (None, Some(clock)) => LoggingSystem::init_with_clock(config, std::io::stdout, clock),
        };
        match installed {
            Ok(system) => {
                logging.system = Some(system.clone());
                configure.insert(0, Box::new(move |b| b.instance(system)));
            }
            Err(LoggingError::AlreadyInstalled) => tracing::warn!(
                target: "oxbloom::boot",
                "logging not installed: a global tracing subscriber or `log` logger is already installed"
            ),
            Err(e) => return Err(StartupError::new(StartupPhase::Config, e)),
        }
    }
    let name = crate::events::app_name(config);
    crate::events::starting(&name, config.active_profiles());
    config
        .warnings()
        .iter()
        .for_each(crate::events::config_warning);
    Ok(())
}

/// [`conditions_report_mode`], then [`start_logging`]: one call, so `execute` has one `?` site
/// for both.
#[cfg(feature = "logging")]
#[inline(never)]
fn report_mode_and_logging(
    config: &Config,
    logging: &mut Logging,
    configure: &mut Vec<ConfigureFn>,
) -> Result<ReportMode, StartupError> {
    let mode = conditions_report_mode(config)?;
    start_logging(config, logging, configure)?;
    Ok(mode)
}

/// Emits "Started {app} in {secs}s", with the name read from the container's `Config` bean and
/// the time measured from `logging.began`.
///
/// A synchronous function of its own, like [`start_logging`].
#[cfg(feature = "logging")]
#[inline(never)]
fn announce_started(container: &Container, logging: &Logging) {
    let name = match container.get::<Config>() {
        Ok(config) => crate::events::app_name(&config),
        Err(_) => crate::events::DEFAULT_NAME.to_owned(),
    };
    let elapsed = logging.began.map(|b| b.elapsed()).unwrap_or_default();
    crate::events::started(&name, elapsed);
}

/// Builds the container and, in `console` mode, writes the conditions evaluation report to
/// `output` before returning, whether or not the build succeeded: before `context_ready` on
/// success, and before `failed`, the cleanup and the failure report on failure.
///
/// A synchronous function of its own, so the build's frame is not part of `execute`'s future
/// and the report is dropped before the run goes on.
#[inline(never)]
fn build_container(
    builder: ContainerBuilder,
    mode: ReportMode,
    output: &Output,
) -> Result<Container, oxbloom_di::DiError> {
    let (built, report) = builder.build_with_report();
    #[cfg(feature = "logging")]
    crate::events::conditions(&report);
    if mode == ReportMode::Console {
        output.line(&report.to_string());
    }
    built
}

/// The stop deadline `timeout` from now; a timeout too large to represent
/// waits practically forever.
fn deadline(timeout: Duration) -> tokio::time::Instant {
    let now = tokio::time::Instant::now();
    now.checked_add(timeout)
        .unwrap_or_else(|| now + Duration::from_secs(60 * 60 * 24 * 365 * 30))
}

fn container_error(e: oxbloom_di::DiError) -> StartupError {
    StartupError::new(StartupPhase::Container, e)
}

fn runner_error(e: crate::error::RunnerError) -> StartupError {
    StartupError::new(StartupPhase::Runner, e)
}
