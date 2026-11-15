//! Oxbloom application bootstrap: the counterpart of Spring Boot's
//! `SpringApplication`.
//!
//! [`OxbloomApplication::builder`] runs an application in a fixed order:
//!
//! 1. parse the arguments into [`ApplicationArguments`] (`starting`);
//! 2. build the `Config` (`environment_prepared`), install the logging
//!    subscriber (Cargo feature `logging`) and print the [`Banner`];
//! 3. build the container with `Config`, [`ApplicationArguments`] and
//!    [`ShutdownHandle`] registered, then the `configure` closures and the
//!    auto-configurations, print the conditions evaluation report when it is
//!    on (`context_ready`);
//! 4. run the [`ApplicationRunner`] and [`CommandLineRunner`] beans;
//! 5. start the [`Service`] beans in ascending phase;
//! 6. fire `started` and `ready`;
//! 7. with at least one service, wait for Ctrl-C, SIGTERM (Unix) or a
//!    [`ShutdownHandle`] trigger;
//! 8. stop the services in reverse order under one deadline (`stopping`);
//! 9. shut the container down;
//! 10. compute the [`ExitStatus`] from the [`ExitCodeGenerator`] beans
//!     (`stopped`).
//!
//! Each step fires the named [`RunListener`] event. A failure fires `failed`,
//! stops what was started and returns a [`StartupError`] naming the
//! [`StartupPhase`]. [`OxbloomApplicationBuilder::run_and_exit`] turns the
//! outcome into a process exit code for `main`.
//!
//! Before `failed` fires, the [`FailureAnalyzer`]s registered with
//! [`OxbloomApplicationBuilder::failure_analyzer`] or as beans are asked to
//! explain the failure. The first [`FailureAnalysis`] is attached to the
//! [`StartupError`] and printed as a report with a description and an action:
//! by `run_and_exit` by default, and by `run` with
//! [`OxbloomApplicationBuilder::failure_report`] set to `true`.
//!
//! The configuration key `oxbloom.main.conditions-report` shows which
//! conditional registrations were kept and which were skipped, and why. It is
//! `off` by default; `console` writes the conditions evaluation report to the
//! output target right after the container is built, before `context_ready`,
//! and also when the build fails, before `failed` and the failure report. A
//! build that fails with `Invalid` (builder misuse) evaluates no condition, so
//! its report shows `(none)` in both sections. Any
//! other value fails the run in the `Config` phase with an error that names the
//! key but not the value. Set it in `application.toml` or on the command line
//! with `--oxbloom.main.conditions-report=console`.
//!
//! ```
//! use std::io::Write;
//! use std::sync::{Arc, Mutex};
//! use oxbloom_boot::{Banner, OxbloomApplication};
//! use oxbloom_config::Config;
//!
//! struct Cache;
//!
//! # #[derive(Clone, Default)]
//! # struct Buf(Arc<Mutex<Vec<u8>>>);
//! # impl Write for Buf {
//! #     fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
//! #         self.0.lock().unwrap().extend_from_slice(b);
//! #         Ok(b.len())
//! #     }
//! #     fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
//! # }
//! # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
//! let out = Buf::default();
//! OxbloomApplication::builder()
//!     .with_config(Config::from_str("oxbloom.main.conditions-report = \"console\"")?)
//!     .banner(Banner::off())
//!     .configure(|b| b.instance(Cache).on_property("cache.enabled"))
//!     .output(out.clone())
//!     .run()
//!     .await?;
//! let text = String::from_utf8(out.0.lock().unwrap().clone())?;
//! let cache = std::any::type_name::<Cache>();
//! assert_eq!(
//!     text,
//!     format!(
//!         "CONDITIONS EVALUATION REPORT\n\nMatched:\n  (none)\n\nSkipped:\n  \
//!          instance {cache}: on_property - property \"cache.enabled\" is not set\n"
//!     )
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Cargo feature `web` adds `WebServer`, a [`Service`] that serves an
//! `oxbloom_web::App`.
//!
//! Cargo feature `ssl` (implies `web`) adds `SslAutoConfiguration`, which registers the
//! `oxbloom_ssl::SslBundles` of the `ssl.bundle.*` keys, `WebServer::with_ssl`, which serves
//! HTTPS with the bundle that `server.ssl.bundle` names, and a failure report for
//! `oxbloom_ssl::SslError` that names the key to fix and never a secret.
//!
//! Cargo feature `actuator` (implies `web`) adds `ActuatorAutoConfiguration`, which serves the
//! health, info and (with `logging`) loggers endpoints of `oxbloom-actuator` and keeps the
//! readiness state in step with the run: accepting at `ready`, refusing at `stopping`.
//!
//! Cargo feature `tasks` adds `TasksAutoConfiguration`, which runs the `#[scheduled]` methods
//! of every `dyn Scheduling` bean with the `oxbloom-tasks` scheduler, configured by the
//! `tasks.scheduling.*` keys, `SchedulerService`, which starts it after the web server and
//! stops it first, and `RetryAutoConfiguration`, which makes the `tasks.retry.*` keys the
//! process-wide defaults of every `#[retryable]` method.
//!
//! Cargo feature `logging` installs the `oxbloom_logging` subscriber, configured by the
//! `logging.*` keys, and registers its `LoggingSystem` as a bean so levels can be changed at
//! runtime. Turn it off with `OxbloomApplicationBuilder::logging(false)` or
//! `oxbloom.logging.enabled = false`. A process that already has a global `tracing`
//! subscriber keeps it: boot then registers no bean and emits one `warn` event with target
//! `oxbloom::boot` through that subscriber.
//!
//! ```
//! use std::sync::Arc;
//! use oxbloom_boot::{
//!     async_trait, ApplicationArguments, ApplicationRunner, OxbloomApplication, RunnerError,
//! };
//! use oxbloom_config::Config;
//!
//! struct Hello;
//!
//! #[async_trait]
//! impl ApplicationRunner for Hello {
//!     async fn run(&self, args: &ApplicationArguments) -> Result<(), RunnerError> {
//!         println!("hello {}", args.option("name").unwrap_or("world"));
//!         Ok(())
//!     }
//! }
//!
//! # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
//! let status = OxbloomApplication::builder()
//!     .args(["--name=oxbloom"])
//!     .with_config(Config::from_str("")?)
//!     .output(std::io::sink())
//!     .configure(|b| {
//!         b.instance(Hello)
//!             .provides(|h: Arc<Hello>| -> Arc<dyn ApplicationRunner> { h })
//!     })
//!     .run()
//!     .await?;
//! // No service: `run()` returns right after the runners.
//! assert_eq!(status.code(), 0);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod analyzers;
mod app;
mod arguments;
mod autoconfig;
#[cfg(any(feature = "security", feature = "web", feature = "tasks"))]
mod autoconfigure;
mod banner;
mod error;
#[cfg(feature = "logging")]
mod events;
mod exit;
mod failure;
mod listener;
mod metadata;
mod output;
mod runner;
mod service;
mod shutdown;
mod signal;
#[cfg(feature = "web")]
mod web;

pub use app::{OxbloomApplication, OxbloomApplicationBuilder};
pub use arguments::ApplicationArguments;
pub use autoconfig::{AutoConfiguration, AutoConfigurations, ConfigRef};
#[cfg(feature = "actuator")]
pub use autoconfigure::ActuatorAutoConfiguration;
#[cfg(feature = "ssl")]
pub use autoconfigure::SslAutoConfiguration;
#[cfg(any(feature = "security", feature = "web", feature = "tasks"))]
pub use autoconfigure::auto_configurations;
#[cfg(feature = "security")]
pub use autoconfigure::{
    JwtDecoderAutoConfiguration, PasswordEncoderAutoConfiguration, SecurityAutoConfiguration,
    SecurityCustomizer, SessionAutoConfiguration,
};
#[cfg(feature = "web")]
pub use autoconfigure::{RequestLogAutoConfiguration, WebServerAutoConfiguration};
#[cfg(feature = "tasks")]
pub use autoconfigure::{RetryAutoConfiguration, SchedulerService, TasksAutoConfiguration};
pub use banner::Banner;
pub use error::{ExitStatus, RunnerError, ServiceError, StartupError, StartupPhase};
pub use exit::ExitCodeGenerator;
pub use failure::{FailureAnalysis, FailureAnalyzer};
pub use listener::RunListener;
pub use metadata::{PropertyMetadata, boot_properties};
pub use runner::{ApplicationRunner, CommandLineRunner};
pub use service::Service;
pub use shutdown::ShutdownHandle;
#[cfg(feature = "web")]
pub use web::{BindError, WebServer};

/// The class-level conditions of an [`AutoConfiguration`], re-exported from `oxbloom_di` so
/// that `#[auto_configuration]` and hand-written impls can name it through `oxbloom_boot`.
///
/// ```
/// use oxbloom_boot::Conditions;
///
/// let _only_with_a_url = Conditions::new().on_property("db.url");
/// ```
pub use oxbloom_di::Conditions;

/// The supertrait of [`AutoConfiguration`], re-exported from `oxbloom_di`.
///
/// ```
/// use oxbloom_boot::Configuration;
/// use oxbloom_di::ContainerBuilder;
///
/// struct Limits { max: u32 }
/// struct Defaults;
/// impl Configuration for Defaults {
///     fn register(builder: ContainerBuilder) -> ContainerBuilder {
///         builder.bean(|| Limits { max: 10 })
///     }
/// }
///
/// let c = ContainerBuilder::new().configuration::<Defaults>().build()?;
/// assert_eq!(c.get::<Limits>()?.max, 10);
/// # Ok::<(), oxbloom_di::DiError>(())
/// ```
pub use oxbloom_di::Configuration;

pub use oxbloom_macros::auto_configuration;

/// The attribute to put on `impl` blocks of [`ApplicationRunner`],
/// [`CommandLineRunner`] and [`Service`], re-exported from the `async-trait`
/// crate so you need no `async-trait` dependency of your own.
///
/// ```
/// use oxbloom_boot::{async_trait, CommandLineRunner, RunnerError};
///
/// struct Noop;
///
/// #[async_trait]
/// impl CommandLineRunner for Noop {
///     async fn run(&self, _args: &[String]) -> Result<(), RunnerError> {
///         Ok(())
///     }
/// }
/// ```
pub use async_trait::async_trait;
