use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::panic::resume_unwind;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxbloom_boot::{
    AutoConfiguration, AutoConfigurations, ExitStatus, RequestLogAutoConfiguration, RunListener,
    ShutdownHandle, StartupError, WebServer, WebServerAutoConfiguration,
};
#[cfg(feature = "security")]
use oxbloom_boot::{
    JwtDecoderAutoConfiguration, PasswordEncoderAutoConfiguration, SecurityAutoConfiguration,
};
use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_di::{Container, ContainerBuilder};
use tokio::task::{JoinError, JoinHandle};

#[cfg(doc)]
use crate::TempConfigDir;
use crate::lifecycle::{
    RUN_TIMEOUT, Setup, assert_clean_shutdown, expect_startup_failure, lock, never_ready,
    startup_failed,
};
use crate::{Captured, TestClient};

/// The panic text when the application has no web server to wait for.
const NO_WEB_SERVER: &str = "no web server: register a dyn Controller or use TestApp::web()";

/// How often `start` looks for the bound address.
const POLL: Duration = Duration::from_millis(5);

/// Starts a whole boot application with a real web server for a test.
///
/// `TestApp` has the configuration methods and the hermetic defaults of
/// [`ContextRunner`](crate::ContextRunner): the process environment is not read, no
/// `application.toml` from the working directory is loaded, the banner is off, the output is
/// captured, `server.port` is `0` and `oxbloom.logging.enabled` is `false` unless a
/// [`property`](Self::property) sets them. The server binds `127.0.0.1` on a free port by
/// default (boot's default `server.address`); a property or `config_str` can override the
/// address. Boot's stop deadline is 5 seconds.
///
/// [`start`](Self::start) runs the application in a task and waits until its `WebServer` is
/// bound, then hands a [`RunningApp`] with a [`TestClient`] connected to it.
/// [`RunningApp::stop`] shuts it down and fails on a shutdown that boot only reports in its
/// output. `TestApp` is real-server only; for in-memory calls, use [`TestClient::in_memory`].
///
/// The presets choose the auto-configurations that run: [`new`](Self::new) none,
/// [`web`](Self::web) the request log and the web server, `secured` (feature `security`) adds
/// the JWT decoder and the security chain, and [`full`](Self::full) runs every auto-configuration
/// of the enabled features. [`exclude`](Self::exclude) still works after a preset.
///
/// The limits of `ContextRunner` apply: boot installs its Ctrl-C and SIGTERM handlers while the
/// server runs; the 10 second bounds are Tokio timers, so they cannot interrupt a `stop` that
/// blocks the thread of a current-thread runtime; and under
/// `#[tokio::test(start_paused = true)]` Tokio can advance paused time while the server waits on
/// I/O and fire the bounds early, so run `TestApp` without paused time.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_test::TestApp;
/// use oxbloom_web::{Controller, HttpMethod, Route};
///
/// struct Hello;
/// impl Controller for Hello {
///     fn routes(self: Arc<Self>) -> Vec<Route> {
///         vec![Route::new(HttpMethod::Get, "/hello", |_| async { "hello" })]
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let app = TestApp::web()
///     .configure(|b| {
///         b.instance(Hello)
///             .provides(|c: Arc<Hello>| -> Arc<dyn Controller> { c })
///     })
///     .start()
///     .await;
/// assert_eq!(app.client().get("/hello").send().await.text(), "hello");
/// assert_eq!(app.stop().await.status().code(), 0);
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
#[must_use = "a test app does nothing until `start` or `start_failing` is awaited"]
pub struct TestApp {
    setup: Setup,
}

impl std::fmt::Debug for TestApp {
    /// Shows counts only: properties may contain secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestApp")
            .field("properties", &self.setup.properties.len())
            .field("config_str", &self.setup.config_str.is_some())
            .field("config_dir", &self.setup.config_dir)
            .field("profiles", &self.setup.profiles)
            .finish_non_exhaustive()
    }
}

impl Default for TestApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TestApp {
    /// Creates an application with no auto-configuration, no properties, no configuration files
    /// and no wiring.
    ///
    /// Without [`WebServerAutoConfiguration`] there is no web server, unless the wiring
    /// registers one, so [`start`](Self::start) fails at once.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::{RequestLogAutoConfiguration, WebServerAutoConfiguration};
    /// use oxbloom_test::TestApp;
    ///
    /// // The same as `TestApp::web()`.
    /// let app = TestApp::new()
    ///     .auto_configuration::<RequestLogAutoConfiguration>()
    ///     .auto_configuration::<WebServerAutoConfiguration>();
    /// assert!(format!("{app:?}").starts_with("TestApp"));
    /// ```
    pub fn new() -> TestApp {
        TestApp {
            setup: Setup::new(),
        }
    }

    /// Creates an application with the web slice: `RequestLogAutoConfiguration` and
    /// `WebServerAutoConfiguration`. Controllers and filters come from
    /// [`configure`](Self::configure); there is no security chain and no JWT decoder.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// // No security chain: a request without credentials is answered.
    /// assert_eq!(app.client().get("/ping").send().await.status(), 200);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn web() -> TestApp {
        TestApp::new()
            .auto_configuration::<RequestLogAutoConfiguration>()
            .auto_configuration::<WebServerAutoConfiguration>()
    }

    /// Creates an application with the secured slice: the [`web`](Self::web) slice plus
    /// `JwtDecoderAutoConfiguration`, `PasswordEncoderAutoConfiguration` and
    /// `SecurityAutoConfiguration`. Needs the feature `security`.
    ///
    /// Without a `SecurityCustomizer`, every request must be authenticated. Put the
    /// `security.jwt` section in [`config_str`](Self::config_str), because secret keys are
    /// refused as properties, and move the time of the `exp` check with [`clock`](Self::clock).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::secured()
    ///     .config_str(
    ///         "[security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"an-example-secret-of-32-bytes!!!\"\n",
    ///     )
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.client().get("/ping").send().await.status(), 401);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[cfg(feature = "security")]
    pub fn secured() -> TestApp {
        TestApp::web()
            .auto_configuration::<JwtDecoderAutoConfiguration>()
            .auto_configuration::<PasswordEncoderAutoConfiguration>()
            .auto_configuration::<SecurityAutoConfiguration>()
    }

    /// Creates an application with `oxbloom_boot::auto_configurations()`: every
    /// auto-configuration of the features `oxbloom-test` was built with.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::full()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert!(app.addr().ip().is_loopback());
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn full() -> TestApp {
        TestApp::new().auto_configurations(oxbloom_boot::auto_configurations())
    }

    /// Sets the property `key` to `value` as a command-line override (`--key=value`).
    ///
    /// Properties win over every configuration file. A secret such as `security.jwt.secret` is
    /// refused here as in production; put secrets in [`config_str`](Self::config_str) or a
    /// [`TempConfigDir`] instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .property("demo.name", "override")
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.config().get::<String>("demo.name").unwrap(), "override");
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.setup = self.setup.property(key, value);
        self
    }

    /// Uses `toml` as the `application.toml` of the run. A later call replaces an earlier one.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .config_str("[demo]\nname = \"a\"\n")
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.config().get::<String>("demo.name").unwrap(), "a");
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn config_str(mut self, toml: impl Into<String>) -> Self {
        self.setup = self.setup.config_str(toml);
        self
    }

    /// Loads the `application*.toml` files from `path`. A later call replaces an earlier one.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::{TempConfigDir, TestApp};
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let dir = TempConfigDir::new().file("application.toml", "[demo]\nname = \"dir\"\n");
    /// let app = TestApp::web()
    ///     .config_dir(dir.path())
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.config().get::<String>("demo.name").unwrap(), "dir");
    /// app.stop().await;
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
    /// use std::sync::Arc;
    /// use oxbloom_test::{TempConfigDir, TestApp};
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let dir = TempConfigDir::new()
    ///     .file("application.toml", "mode = \"base\"")
    ///     .file("application-dev.toml", "mode = \"dev\"");
    /// let app = TestApp::web()
    ///     .config_dir(dir.path())
    ///     .profile("dev")
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.config().get::<String>("mode").unwrap(), "dev");
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn profile(mut self, name: impl Into<String>) -> Self {
        self.setup = self.setup.profile(name);
        self
    }

    /// Adds wiring to the container, like `OxbloomApplicationBuilder::configure`. Repeatable;
    /// closures apply in call order. Register controllers here as `dyn Controller`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.client().get("/ping").send().await.text(), "pong");
    /// app.stop().await;
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
    /// use oxbloom_boot::WebServerAutoConfiguration;
    /// use oxbloom_test::TestApp;
    ///
    /// let app = TestApp::new().auto_configuration::<WebServerAutoConfiguration>();
    /// # let _ = app;
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
    /// use oxbloom_boot::{AutoConfigurations, WebServerAutoConfiguration};
    /// use oxbloom_test::TestApp;
    ///
    /// let app = TestApp::new()
    ///     .auto_configurations(AutoConfigurations::new().add::<WebServerAutoConfiguration>());
    /// # let _ = app;
    /// ```
    pub fn auto_configurations(mut self, list: AutoConfigurations) -> Self {
        self.setup = self.setup.auto_configurations(list);
        self
    }

    /// Drops the auto-configuration `A` from the list, also one that a preset added.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::RequestLogAutoConfiguration;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .exclude::<RequestLogAutoConfiguration>()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.client().get("/ping").send().await.status(), 200);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn exclude<A: AutoConfiguration>(mut self) -> Self {
        self.setup = self.setup.exclude::<A>();
        self
    }

    /// Registers `clock` as the `Arc<dyn Clock>` bean of the run, which the JWT
    /// auto-configuration reads for the `exp` and `nbf` checks. With the feature `logging`, it is
    /// also handed to `OxbloomApplicationBuilder::logging_clock`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use std::time::SystemTime;
    /// use oxbloom_test::{ManualClock, TestApp};
    ///
    /// let clock = Arc::new(ManualClock::at(SystemTime::UNIX_EPOCH));
    /// let app = TestApp::web().clock(clock.clone());
    /// # let _ = app;
    /// ```
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.setup = self.setup.clock(clock);
        self
    }

    /// Adds `listener` to the run, so a test can observe the lifecycle events. Repeatable.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::RunListener;
    /// use oxbloom_di::Container;
    /// use oxbloom_test::{Log, TestApp};
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// #[derive(Clone)]
    /// struct Ready(Log);
    /// impl RunListener for Ready {
    ///     fn ready(&self, _: &Container) {
    ///         self.0.push("ready");
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let log = Log::new();
    /// let app = TestApp::web()
    ///     .listener(Ready(log.clone()))
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(log.entries(), ["ready"]);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn listener(mut self, listener: impl RunListener + 'static) -> Self {
        self.setup.builder = self.setup.builder.listener(listener);
        self
    }

    /// Starts the application in a task, waits until its web server is bound, and returns the
    /// [`RunningApp`].
    ///
    /// The wait is bounded at 10 seconds. An application without a web server, such as one with
    /// no `dyn Controller` bean, fails at once instead of waiting. With several `WebServer`
    /// beans, the first bound one wins.
    ///
    /// When `start` returns, every listener added with [`listener`](Self::listener) has seen
    /// `ready`. A `RunListener` registered as a bean in the container can still be inside its
    /// `ready` callback at that point.
    ///
    /// # Panics
    ///
    /// Panics with `startup failed: ` followed by the error and the captured output, which holds
    /// the failure report, when startup fails; with
    /// `no web server: register a dyn Controller or use TestApp::web()` followed by the captured
    /// output when the application became ready without a web server; and with
    /// `the application did not start within 10 seconds` when the server is not bound in time.
    /// Each panic triggers the shutdown of the application first.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_ne!(app.addr().port(), 0);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub async fn start(self) -> RunningApp {
        let output = Captured::new();
        let (builder, dir) = self.setup.prepare(&output);
        let state = Arc::new(BootState::default());
        let builder = builder.listener(BootProbe(state.clone()));
        // Dropped on every panic below, so the application always shuts down.
        let guard = ShutdownOnDrop(builder.shutdown_handle());
        let mut task = tokio::spawn(async move {
            let result = builder.run().await;
            // The configuration directory lives until the run has ended.
            drop(dir);
            result
        });
        let waited = tokio::time::timeout(RUN_TIMEOUT, async {
            loop {
                if let Some(server) = *lock(&state.server) {
                    return Some(server);
                }
                if task.is_finished() {
                    // The marker may have been set right before the run ended.
                    return *lock(&state.server);
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await;
        let addr = match waited {
            Err(_) => panic!(
                "the application did not start within 10 seconds\n{}",
                output.contents()
            ),
            Ok(Some(Some(addr))) => addr,
            Ok(Some(None)) => panic!("{NO_WEB_SERVER}\n{}", output.contents()),
            Ok(None) => match (&mut task).await {
                Ok(Err(error)) => startup_failed(&error, &output),
                Ok(Ok(_)) => never_ready(&output),
                Err(join) => rethrow(join),
            },
        };
        let config = lock(&state.config)
            .clone()
            .expect("the configuration is captured before the server starts");
        RunningApp {
            client: TestClient::connect(dialable(addr)),
            addr,
            config,
            output,
            task,
            guard,
        }
    }

    /// Runs the application and expects startup to fail, then returns the error, for a bad
    /// `server.*` value or a port that is in use. The captured output is part of the panic
    /// message when startup succeeds instead.
    ///
    /// # Panics
    ///
    /// Panics with `startup succeeded but a failure was expected` followed by the captured
    /// output when startup succeeds (the application is shut down first), and with
    /// `the run did not finish within 10 seconds (startup and shutdown)` when the run takes
    /// longer.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::StartupPhase;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let error = TestApp::web()
    ///     .property("server.port", "70000")
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start_failing()
    ///     .await;
    /// assert_eq!(error.phase(), StartupPhase::Container);
    /// assert!(error.to_string().contains("server.port"));
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub async fn start_failing(self) -> StartupError {
        let output = Captured::new();
        let (builder, _dir) = self.setup.prepare(&output);
        expect_startup_failure(builder, &output).await
    }
}

/// What the probe saw: the configuration, and the bound address once the application is ready
/// (`Some(None)` when it has no web server).
#[derive(Default)]
struct BootState {
    config: Mutex<Option<Arc<Config>>>,
    server: Mutex<Option<Option<SocketAddr>>>,
}

/// Captures the configuration at `context_ready` and the address of the web server at `ready`.
///
/// The address is read at `ready` rather than at `started`, which fires just before it on the
/// same call: by then every listener has seen `ready`, so `start` returns after the whole
/// startup sequence. The services, and with them the server, are started before `started`.
struct BootProbe(Arc<BootState>);

impl RunListener for BootProbe {
    fn context_ready(&self, container: &Container) {
        *lock(&self.0.config) = container.get::<Config>().ok();
    }

    fn ready(&self, container: &Container) {
        let addr = container
            .get_all::<WebServer>()
            .unwrap_or_default()
            .iter()
            .find_map(|server| server.local_addr());
        *lock(&self.0.server) = Some(addr);
    }
}

/// Re-throws the panic that ended the run task with its payload, or panics when the task was
/// cancelled.
fn rethrow(join: JoinError) -> ! {
    match join.try_into_panic() {
        Ok(payload) => resume_unwind(payload),
        Err(join) => panic!("the run task ended: {join}"),
    }
}

/// Returns `addr`, with an unspecified IP (`0.0.0.0` or `::`) replaced by the loopback address
/// of the same family, so a client can dial a server that listens on every interface.
fn dialable(addr: SocketAddr) -> SocketAddr {
    let ip = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, addr.port())
}

/// Triggers shutdown when dropped, without waiting for it.
struct ShutdownOnDrop(ShutdownHandle);

impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        self.0.trigger();
    }
}

/// A started application with a bound web server, returned by [`TestApp::start`].
///
/// Dropping it triggers shutdown without waiting; [`stop`](Self::stop) waits and checks the
/// shutdown. Only the listeners added with [`TestApp::listener`] are guaranteed to have seen
/// `ready` when it is handed out; a `RunListener` bean can still be inside `ready`. With
/// several `WebServer` beans, it holds the first bound one.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_test::TestApp;
/// use oxbloom_web::{Controller, HttpMethod, Route};
///
/// struct Ping;
/// impl Controller for Ping {
///     fn routes(self: Arc<Self>) -> Vec<Route> {
///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let app = TestApp::web()
///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
///     .start()
///     .await;
/// assert_eq!(app.client().addr(), Some(app.addr()));
/// app.stop().await;
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
#[must_use = "dropping a running app shuts it down at once"]
pub struct RunningApp {
    client: TestClient,
    addr: SocketAddr,
    config: Arc<Config>,
    output: Captured,
    task: JoinHandle<Result<ExitStatus, StartupError>>,
    guard: ShutdownOnDrop,
}

impl std::fmt::Debug for RunningApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningApp")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl RunningApp {
    /// Returns a client connected to the web server of the application.
    ///
    /// When the server listens on every interface (`server.address` is `0.0.0.0` or `::`), the
    /// client dials the loopback address of the same family on the bound port.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.client().get("/ping").send().await.text(), "pong");
    /// assert_eq!(app.client().get("/missing").send().await.status(), 404);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn client(&self) -> &TestClient {
        &self.client
    }

    /// Returns a new HTTPS client for an application that serves `server.ssl.bundle`, trusting
    /// only the CA certificates in `ca_pem`.
    ///
    /// It is [`TestClient::connect_tls`] to the address [`client`](Self::client) dials, so it
    /// verifies the server name `localhost` and keeps one connection alive. The client is
    /// owned by the caller; [`stop`](Self::stop) still ends the application while it holds an
    /// idle connection, because the server closes idle connections when it shuts down.
    ///
    /// # Panics
    ///
    /// Panics with `connect_tls: the CA PEM holds no valid certificate` when `ca_pem` holds no
    /// usable certificate. The message never contains the PEM text.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_boot::SslAutoConfiguration;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// // A self-signed `localhost` certificate is its own CA.
    /// let key = rcgen::KeyPair::generate().unwrap();
    /// let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
    ///     .unwrap()
    ///     .self_signed(&key)
    ///     .unwrap();
    /// let toml = format!(
    ///     "[ssl.bundle.pem.web]\ncertificate = '''\n{}'''\nprivate-key = '''\n{}'''\n\
    ///      [server.ssl]\nbundle = \"web\"\n",
    ///     cert.pem(),
    ///     key.serialize_pem()
    /// );
    /// let app = TestApp::web()
    ///     .auto_configuration::<SslAutoConfiguration>()
    ///     .config_str(toml)
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// let client = app.client_tls(&cert.pem());
    /// assert_eq!(client.get("/ping").send().await.text(), "pong");
    /// drop(client);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    #[cfg(feature = "tls")]
    pub fn client_tls(&self, ca_pem: &str) -> TestClient {
        TestClient::connect_tls(dialable(self.addr), ca_pem)
    }

    /// Returns the address the web server is bound to, on `127.0.0.1` by default (boot's default
    /// `server.address`). A wildcard address such as `0.0.0.0` is returned as bound.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert!(app.addr().ip().is_loopback());
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns the loaded configuration, captured when the container was built.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .property("demo.size", "3")
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.config().get::<u32>("demo.size").unwrap(), 3);
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn config(&self) -> Arc<Config> {
        self.config.clone()
    }

    /// Returns the output the application wrote so far.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// // The banner is off and logging is off, so a plain run writes nothing.
    /// assert_eq!(app.output().contents(), "");
    /// app.stop().await;
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn output(&self) -> &Captured {
        &self.output
    }

    /// Shuts the application down, waits at most 10 seconds for the run to end, and returns
    /// the exit status with the final output.
    ///
    /// # Panics
    ///
    /// Panics with `shutdown failed` followed by the captured output when boot reports a failed
    /// shutdown, such as a `Service::stop` that panics or misses the 5 second deadline; with
    /// `the application did not stop within 10 seconds` when the run does not end in time; and
    /// with the payload of a panic that ended the run task. The bound cannot interrupt a `stop`
    /// that blocks a current-thread runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// let stopped = app.stop().await;
    /// assert_eq!(stopped.status().code(), 0);
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub async fn stop(self) -> Stopped {
        let RunningApp {
            client,
            output,
            task,
            guard,
            ..
        } = self;
        // The client's idle connections are closed first, so the server has nothing to drain.
        drop(client);
        guard.0.trigger();
        match tokio::time::timeout(RUN_TIMEOUT, task).await {
            Err(_) => panic!(
                "the application did not stop within 10 seconds\n{}",
                output.contents()
            ),
            Ok(Err(join)) => rethrow(join),
            Ok(Ok(Err(error))) => panic!("the run failed: {error}\n{}", output.contents()),
            Ok(Ok(Ok(status))) => {
                assert_clean_shutdown(&output);
                Stopped { status, output }
            }
        }
    }
}

/// The result of [`RunningApp::stop`]: the exit status and the final output.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_test::TestApp;
/// use oxbloom_web::{Controller, HttpMethod, Route};
///
/// struct Ping;
/// impl Controller for Ping {
///     fn routes(self: Arc<Self>) -> Vec<Route> {
///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let stopped = TestApp::web()
///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
///     .start()
///     .await
///     .stop()
///     .await;
/// assert_eq!(stopped.status().code(), 0);
/// assert_eq!(stopped.output().contents(), "");
/// # });
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Debug)]
pub struct Stopped {
    status: ExitStatus,
    output: Captured,
}

impl Stopped {
    /// Returns the exit status of the run.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert_eq!(app.stop().await.status().code(), 0);
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn status(&self) -> &ExitStatus {
        &self.status
    }

    /// Returns the output of the whole run, including shutdown.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_test::TestApp;
    /// use oxbloom_web::{Controller, HttpMethod, Route};
    ///
    /// struct Ping;
    /// impl Controller for Ping {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/ping", |_| async { "pong" })]
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let app = TestApp::web()
    ///     .configure(|b| b.instance(Ping).provides(|c: Arc<Ping>| -> Arc<dyn Controller> { c }))
    ///     .start()
    ///     .await;
    /// assert!(!app.stop().await.output().contains("shutdown failed"));
    /// # });
    /// # Ok::<(), std::io::Error>(())
    /// ```
    pub fn output(&self) -> &Captured {
        &self.output
    }
}
