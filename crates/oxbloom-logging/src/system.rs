//! The installed subscriber and the handle that changes its levels at runtime.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, IsTerminal};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use oxbloom_config::Config;
use oxbloom_core::{Clock, SystemClock};
use tracing::Dispatch;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Layer, Layered, SubscriberExt};
use tracing_subscriber::{EnvFilter, Registry, reload};

use crate::error::LoggingError;
use crate::file::file_layer;
use crate::json::{JsonFields, JsonFormat};
use crate::level::Level;
use crate::settings::{ConsoleColor, ConsoleFormat, JsonOptions, Settings, check_target};

/// The registry with the reloadable level filter: what the console layer sits on.
pub(crate) type Filtered = Layered<reload::Layer<EnvFilter, Registry>, Registry>;

/// The logging system: a `tracing` subscriber built from the `logging.*` keys, and the handle
/// that changes its levels while the application runs.
///
/// [`init`](Self::init) installs it as the global subscriber, once per process. Clones share
/// the same levels. Only boot or the application calls `init`, never a library.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::{Level, LoggingSystem};
///
/// let config = Config::from_str("[logging.level]\nroot = \"warn\"")?;
/// let logging = LoggingSystem::init(&config)?;
/// tracing::info!("filtered out");
///
/// logging.set_level("my_app", Level::Debug)?;
/// tracing::debug!(target: "my_app", "now printed");
/// assert_eq!(logging.level("my_app::db"), Level::Debug);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct LoggingSystem {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    filter: reload::Handle<EnvFilter, Registry>,
    /// Dropped with the last clone, which flushes the log file.
    #[expect(dead_code, reason = "held only so its drop flushes the file")]
    guard: Option<WorkerGuard>,
}

struct State {
    /// The levels now in force.
    current: Settings,
    /// The levels from the last `init` or `reload`, for `reset`.
    baseline: Settings,
    /// Counts changes to `current`, so a filter built from an older one is replaced.
    version: u64,
}

impl LoggingSystem {
    /// Builds the subscriber from `config` and installs it as the global default, with
    /// console output on standard output. Events from crates that use the `log` facade reach
    /// the same subscriber and honor the same levels.
    ///
    /// # Errors
    ///
    /// [`LoggingError::Config`] when a `logging.*` value is invalid (the error names the key),
    /// and [`LoggingError::AlreadyInstalled`] when this process already has a global `tracing`
    /// subscriber or a `log` logger, for example after an earlier `init`. When a foreign
    /// `tracing` subscriber is already installed, the `log` bridge stays installed and forwards
    /// `log` records to that subscriber.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::{LoggingError, LoggingSystem};
    ///
    /// let config = Config::from_str("[logging.console]\nformat = \"compact\"")?;
    /// let logging = LoggingSystem::init(&config)?;
    /// tracing::info!("started");
    ///
    /// // The subscriber is per process.
    /// assert!(matches!(LoggingSystem::init(&config), Err(LoggingError::AlreadyInstalled)));
    /// # drop(logging);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn init(config: &Config) -> Result<LoggingSystem, LoggingError> {
        Self::init_with_writer(config, io::stdout)
    }

    /// [`init`](Self::init) with console output to `writer` instead of standard output, so
    /// tests can capture it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::LoggingSystem;
    ///
    /// let config = Config::from_str("")?;
    /// let logging = LoggingSystem::init_with_writer(&config, std::io::stderr)?;
    /// tracing::info!("to standard error");
    /// # drop(logging);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[doc(hidden)]
    pub fn init_with_writer<W>(config: &Config, writer: W) -> Result<LoggingSystem, LoggingError>
    where
        W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    {
        let (dispatch, system) = Self::build(config, writer)?;
        Self::install(dispatch)?;
        Ok(system)
    }

    /// [`init_with_writer`](Self::init_with_writer) with the JSON timestamps read from `clock`
    /// instead of the system time, so tests can pin the time in the log output.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Config;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_logging::LoggingSystem;
    ///
    /// let config = Config::from_str("[logging.console]\nformat = \"json\"")?;
    /// let logging = LoggingSystem::init_with_clock(&config, std::io::stderr, Arc::new(SystemClock))?;
    /// tracing::info!("stamped by the clock");
    /// # drop(logging);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[doc(hidden)]
    pub fn init_with_clock<W>(
        config: &Config,
        writer: W,
        clock: Arc<dyn Clock>,
    ) -> Result<LoggingSystem, LoggingError>
    where
        W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    {
        let (dispatch, system) = Self::build_with_clock(config, writer, clock)?;
        Self::install(dispatch)?;
        Ok(system)
    }

    /// Installs `dispatch` as the global subscriber, with the `log` bridge.
    fn install(dispatch: Dispatch) -> Result<(), LoggingError> {
        // The `log` bridge first: if it fails nothing is installed. If a subscriber exists
        // already, the bridge forwards to that one, which is what its owner wants anyway.
        tracing_log::LogTracer::init().map_err(|_| LoggingError::AlreadyInstalled)?;
        tracing::dispatcher::set_global_default(dispatch)
            .map_err(|_| LoggingError::AlreadyInstalled)?;
        // Let `log` skip records no filter wants; reloads keep this up to date.
        tracing_log::log::set_max_level(tracing_log::AsLog::as_log(
            &tracing::level_filters::LevelFilter::current(),
        ));
        Ok(())
    }

    /// The subscriber and its handle, not installed anywhere.
    pub(crate) fn build<W>(config: &Config, writer: W) -> Result<(Dispatch, Self), LoggingError>
    where
        W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    {
        Self::build_with_clock(config, writer, Arc::new(SystemClock))
    }

    /// [`build`](Self::build) with the JSON timestamps read from `clock`.
    pub(crate) fn build_with_clock<W>(
        config: &Config,
        writer: W,
        clock: Arc<dyn Clock>,
    ) -> Result<(Dispatch, Self), LoggingError>
    where
        W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
    {
        let settings = Settings::from_config(config)?;
        let (filter, handle) = reload::Layer::new(env_filter(&settings.directives())?);
        let mut layers = vec![console_layer(
            settings.format(),
            settings.color(),
            settings.json(),
            writer,
            clock.clone(),
        )];
        let mut guard = None;
        if let Some(opts) = settings.file() {
            let (layer, worker) = file_layer(opts, settings.json(), clock)?;
            layers.push(layer);
            guard = Some(worker);
        }
        let dispatch = Dispatch::new(Registry::default().with(filter).with(layers));
        let system = LoggingSystem {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    current: settings.clone(),
                    baseline: settings,
                    version: 0,
                }),
                filter: handle,
                guard,
            }),
        };
        Ok((dispatch, system))
    }

    /// Checks that `target` is a name that [`set_level`](Self::set_level) accepts: not
    /// empty, and only letters, digits, `_`, `-` and `:`. A dotted name such as `a.b` is
    /// rejected. The check changes nothing and needs no installed subscriber.
    ///
    /// # Errors
    ///
    /// [`LoggingError::Config`] naming `logging.level.<target>` when the name breaks the rule.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_logging::LoggingSystem;
    ///
    /// assert!(LoggingSystem::validate_target("oxbloom_web::router").is_ok());
    /// assert!(LoggingSystem::validate_target("root").is_ok());
    /// assert!(LoggingSystem::validate_target("a.b").is_err());
    /// assert!(LoggingSystem::validate_target("").is_err());
    /// ```
    pub fn validate_target(target: &str) -> Result<(), LoggingError> {
        check_target(target)
    }

    /// Sets the level of `target` at runtime. `root` is the default level; a group name
    /// sets the level of the group, which applies to each member without a level of its own.
    /// Group and `root` names match like configuration keys (case and `-`/`_` ignored).
    ///
    /// # Errors
    ///
    /// [`LoggingError::Config`] naming `logging.level.<target>` when the target contains
    /// characters other than letters, digits, `_`, `-` and `:`, and [`LoggingError::Filter`]
    /// when the subscriber no longer exists.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::{Level, LoggingSystem};
    ///
    /// let logging = LoggingSystem::init(&Config::from_str("")?)?;
    /// logging.set_level("root", "warn".parse()?)?;
    /// logging.set_level("oxbloom_web::router", Level::Trace)?;
    /// assert_eq!(logging.level("root"), Level::Warn);
    /// assert_eq!(logging.level("oxbloom_web::router"), Level::Trace);
    /// assert!(logging.set_level("not,a,target", Level::Trace).is_err());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_level(&self, target: &str, level: Level) -> Result<(), LoggingError> {
        self.change(|state| state.current.set(target, level))
    }

    /// Returns `target` to its level from the last [`init`](Self::init) or
    /// [`reload`](Self::reload), undoing [`set_level`](Self::set_level). For a group this
    /// restores the group and each of its members.
    ///
    /// # Errors
    ///
    /// As for [`set_level`](Self::set_level).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::{Level, LoggingSystem};
    ///
    /// let logging = LoggingSystem::init(&Config::from_str("[logging.level]\napp = \"warn\"")?)?;
    /// logging.set_level("app", Level::Trace)?;
    /// logging.reset("app")?;
    /// assert_eq!(logging.level("app"), Level::Warn);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reset(&self, target: &str) -> Result<(), LoggingError> {
        self.change(|state| {
            let State {
                current, baseline, ..
            } = state;
            current.reset(target, baseline)
        })
    }

    /// The level in force for `target`. `root` gives the default level and a group name its
    /// group's level (root when the group has none). Any other target gets the level of the
    /// longest configured target it starts with, as the filter matches, falling back to root.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::{Level, LoggingSystem};
    ///
    /// let config = Config::from_str("[logging.level]\nroot = \"error\"\noxbloom_web = \"debug\"")?;
    /// let logging = LoggingSystem::init(&config)?;
    /// assert_eq!(logging.level("root"), Level::Error);
    /// assert_eq!(logging.level("oxbloom_web::router"), Level::Debug);
    /// assert_eq!(logging.level("oxbloom_security"), Level::Error);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn level(&self, target: &str) -> Level {
        self.lock().current.level(target)
    }

    /// Every level now configured: `root` and each target with a level of its own, including
    /// the members of a group that has a level, in name order. A target that is not listed
    /// takes the level of its longest listed prefix, as [`level`](Self::level) tells.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::{Level, LoggingSystem};
    ///
    /// let config = Config::from_str("[logging.level]\nroot = \"warn\"\noxbloom_web = \"debug\"")?;
    /// let logging = LoggingSystem::init(&config)?;
    /// logging.set_level("app", Level::Trace)?;
    /// let levels: Vec<_> = logging.levels().into_iter().collect();
    /// assert_eq!(
    ///     levels,
    ///     [
    ///         ("app".to_string(), Level::Trace),
    ///         ("oxbloom_web".to_string(), Level::Debug),
    ///         ("root".to_string(), Level::Warn),
    ///     ]
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn levels(&self) -> BTreeMap<String, Level> {
        self.lock().current.configured_levels()
    }

    /// Re-reads `logging.level.*` and `logging.group.*` from `config` and applies them: keys
    /// no longer present are removed and earlier [`set_level`](Self::set_level) changes are
    /// dropped. The result is the new baseline for [`reset`](Self::reset). The
    /// `logging.console.*`, `logging.file.*` and `logging.json.*` keys are validated, not
    /// applied: an invalid one fails `reload`, but the outputs are fixed at install time. Only
    /// levels change.
    ///
    /// # Errors
    ///
    /// [`LoggingError::Config`] when a value is invalid; the current levels then stay.
    /// [`LoggingError::Filter`] when the subscriber no longer exists.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::{Level, LoggingSystem};
    ///
    /// let logging = LoggingSystem::init(&Config::from_str("[logging.level]\napp = \"debug\"")?)?;
    /// logging.reload(&Config::from_str("[logging.level]\nroot = \"warn\"")?)?;
    /// assert_eq!(logging.level("app"), Level::Warn);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reload(&self, config: &Config) -> Result<(), LoggingError> {
        let settings = Settings::from_config(config)?;
        self.change(|state| {
            state.current = settings.clone();
            state.baseline = settings;
            Ok(())
        })
    }

    /// Applies `edit` under the lock, then installs a filter for the result outside it.
    fn change(
        &self,
        edit: impl FnOnce(&mut State) -> Result<(), LoggingError>,
    ) -> Result<(), LoggingError> {
        {
            let mut state = self.lock();
            edit(&mut state)?;
            state.version += 1;
        }
        // The lock is never held while calling into `tracing`: the reload rebuilds interest
        // caches and may run while an event is being handled on this thread. Two concurrent
        // changes may install their filters out of order, so repeat until the installed
        // filter is built from the current levels.
        loop {
            let (directives, version) = {
                let state = self.lock();
                (state.current.directives(), state.version)
            };
            let filter = env_filter(&directives)?;
            self.inner
                .filter
                .reload(filter)
                .map_err(|e| LoggingError::Filter(e.to_string()))?;
            if self.lock().version == version {
                return Ok(());
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // Every edit leaves the state whole, so a panic elsewhere does not spoil it.
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Opaque: shows the type only.
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::LoggingSystem;
///
/// let logging = LoggingSystem::init(&Config::from_str("")?)?;
/// assert_eq!(format!("{logging:?}"), "LoggingSystem { .. }");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl fmt::Debug for LoggingSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoggingSystem").finish_non_exhaustive()
    }
}

/// The filter for `directives`. Targets are checked when they are read, so parsing fails only
/// on a bug; the error carries no configuration text.
fn env_filter(directives: &str) -> Result<EnvFilter, LoggingError> {
    EnvFilter::builder()
        .parse(directives)
        .map_err(|_| LoggingError::Filter("the level directives could not be parsed".into()))
}

/// The console layer for `format` and `color`, writing to `writer`. The JSON format reads its
/// timestamp from `clock`.
fn console_layer<W>(
    format: ConsoleFormat,
    color: ConsoleColor,
    json: &JsonOptions,
    writer: W,
    clock: Arc<dyn Clock>,
) -> Box<dyn Layer<Filtered> + Send + Sync>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let ansi = match color {
        ConsoleColor::Always => true,
        ConsoleColor::Never => false,
        // The convention at no-color.org: a non-empty `NO_COLOR` turns colors off.
        ConsoleColor::Auto => {
            io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        }
    };
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(ansi);
    match format {
        ConsoleFormat::Full => layer.boxed(),
        ConsoleFormat::Compact => layer.compact().boxed(),
        ConsoleFormat::Pretty => layer.pretty().boxed(),
        ConsoleFormat::Json => layer
            .event_format(JsonFormat::new(json.clone(), clock))
            .fmt_fields(JsonFields)
            .boxed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_target_accepts_the_names_set_level_accepts() {
        for name in ["root", "app", "my-app", "oxbloom_web::router"] {
            assert!(LoggingSystem::validate_target(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn validate_target_rejects_dotted_empty_and_directive_characters() {
        for name in ["", "a.b", "a,b", "a=b", "a[b]", "a b"] {
            let error = LoggingSystem::validate_target(name).unwrap_err();
            assert!(matches!(error, LoggingError::Config(_)), "{name}");
        }
    }
}
