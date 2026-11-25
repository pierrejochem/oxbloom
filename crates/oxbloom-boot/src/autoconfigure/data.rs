//! `DataAutoConfiguration`: the `DataSources` registry of the `data.*` keys, the default
//! source's pool bean, and `DataService`, which connects the pools at start and closes them at
//! shutdown.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use oxbloom_config::{Config, ConfigError};
use oxbloom_data::{DataError, DataSettings, DataSources, Driver, SourceError, TlsMaterial};
use oxbloom_di::{
    ConditionContext, ConditionOutcome, Conditions, Configuration, ContainerBuilder, DiError,
    component,
};
#[cfg(feature = "ssl")]
use oxbloom_ssl::SslBundles;

use crate::autoconfig::{AutoConfiguration, ConfigRef};
use crate::error::ServiceError;
use crate::metadata::PropertyMetadata;
use crate::service::Service;

/// How long the start-up check waits for each source.
const PING_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the shutdown waits for the pools to close.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
/// The log target of the data sources.
const TARGET: &str = "oxbloom::data";

/// Registers the data sources of the `data.*` keys: the [`DataSources`] registry, the default
/// source's sqlx pool as a bean of its own, and a [`DataService`] that connects the pools at
/// start and closes them at shutdown.
///
/// Available with the cargo features `data-postgres`, `data-mysql` and `data-sqlite`; each one
/// turns on the matching driver of `oxbloom-data`. The group runs when the configuration has a
/// `data` section (the condition `on_property("data")`); the conditions report never shows a
/// value. It registers:
///
/// - the [`DataSettings`] of the `data.*` keys. An unknown key or a bad value fails the build
///   with `DiError::Creation` whose source is the `ConfigError` naming the full key, never the
///   value (the URL is a secret);
/// - the [`DataSources`] registry with a lazy pool per source, unless the application
///   registered a `DataSources` bean itself; then the application's registry is the one the
///   default pool bean, the [`DataService`] and the health indicators use. An `ssl-bundle` names a bundle
///   of the `SslBundles` bean (feature `ssl`), whose trust certificates and client identity
///   the pool uses as PEM; an unknown name fails the build with a `ConfigError::Invalid` on
///   `data.ssl-bundle` or `data.sources.<name>.ssl-bundle` that lists the available bundle
///   names. Without the feature `ssl` a named bundle is an error on the same key. A source
///   whose driver feature is off, or whose URL the driver refuses, fails the build with a
///   report that names the source, its key and, for a missing driver, the feature to enable;
/// - the default source's pool as `Arc<PgPool>`, `Arc<MySqlPool>` or `Arc<SqlitePool>`
///   (whichever driver `data.url` names), unless the application registered a bean of that
///   type itself. Named sources are reachable through the registry only;
/// - the [`DataService`] as a `dyn Service`, when the section configures a source (a
///   `data` section with only `connect-on-startup` registers none). The service does not keep
///   running, so a run-to-completion application ends after its runners.
///
/// The pools are built while the container is built, which `OxbloomApplicationBuilder::run`
/// does on the application's Tokio runtime; sqlx ties the maintenance task of a pool to that
/// runtime. A pool keeps the bundle material it was built with and does not follow a reload.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "data-sqlite")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     AutoConfiguration, Banner, DataAutoConfiguration, OxbloomApplication, RunListener,
///     StartupPhase,
/// };
/// use oxbloom_config::{Config, ConfigError};
/// use oxbloom_data::DataSources;
/// use oxbloom_data::sqlx::SqlitePool;
/// use oxbloom_di::Container;
///
/// /// The source names and whether the default pool bean exists, when the context is ready.
/// #[derive(Clone, Default)]
/// struct Seen(Arc<Mutex<Option<(Vec<String>, bool)>>>);
/// impl RunListener for Seen {
///     fn context_ready(&self, c: &Container) {
///         let sources = c.get::<DataSources>().unwrap();
///         let names = sources.names().into_iter().map(String::from).collect();
///         *self.0.lock().unwrap() = Some((names, c.get::<SqlitePool>().is_ok()));
///     }
/// }
///
/// tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
///     let seen = Seen::default();
///     let builder = OxbloomApplication::builder()
///         .with_config(Config::from_str(
///             "[data]\nurl = \"sqlite::memory:\"\n[data.sources.cache]\nurl = \"sqlite::memory:\"",
///         )?)
///         .banner(Banner::off())
///         .output(std::io::sink())
///         .listener(seen.clone())
///         .auto_configuration::<DataAutoConfiguration>();
///     // The data service does not keep running, so the run ends by itself.
///     builder.run().await?;
///     let names = vec!["default".to_string(), "cache".to_string()];
///     assert_eq!(*seen.0.lock().unwrap(), Some((names, true)));
///
///     // An unknown scheme fails the build with the key; the URL is never shown.
///     let err = OxbloomApplication::builder()
///         .with_config(Config::from_str("[data]\nurl = \"redis://cache.internal\"")?)
///         .banner(Banner::off())
///         .output(std::io::sink())
///         .auto_configuration::<DataAutoConfiguration>()
///         .run()
///         .await
///         .unwrap_err();
///     assert_eq!(err.phase(), StartupPhase::Container);
///     assert!(matches!(
///         err.find_cause::<ConfigError>(),
///         Some(ConfigError::Invalid { key, .. }) if key == "data.url"
///     ));
///
///     let keys: Vec<_> = DataAutoConfiguration::properties().iter().map(|p| p.key).collect();
///     assert!(keys.contains(&"data.connect-on-startup"));
///     Ok::<(), Box<dyn std::error::Error>>(())
/// })?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "data-sqlite"))]
/// # fn main() {}
/// ```
#[component]
pub struct DataAutoConfiguration;

impl Configuration for DataAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        let builder = builder
            .try_bean(data_settings)
            .try_bean(data_sources)
            // The application's own registry wins.
            .on_missing_bean::<DataSources>()
            .bean(data_service)
            .condition("data_sources_configured", sources_configured)
            // The lowest phase starts first; the order keeps it first within that phase, so
            // the pools are up before every other service and close after them.
            .order(i32::MIN)
            .provides(|s: Arc<DataService>| -> Arc<dyn Service> { s });
        #[cfg(feature = "data-postgres")]
        let builder = builder
            .try_bean(default_postgres)
            .condition("default_source_is_postgres", |cx| {
                default_driver_is(cx, Driver::Postgres)
            })
            .on_missing_bean::<oxbloom_data::sqlx::PgPool>();
        #[cfg(feature = "data-mysql")]
        let builder = builder
            .try_bean(default_mysql)
            .condition("default_source_is_mysql", |cx| {
                default_driver_is(cx, Driver::Mysql)
            })
            .on_missing_bean::<oxbloom_data::sqlx::MySqlPool>();
        #[cfg(feature = "data-sqlite")]
        let builder = builder
            .try_bean(default_sqlite)
            .condition("default_source_is_sqlite", |cx| {
                default_driver_is(cx, Driver::Sqlite)
            })
            .on_missing_bean::<oxbloom_data::sqlx::SqlitePool>();
        builder
    }
}

impl AutoConfiguration for DataAutoConfiguration {
    /// Runs when the configuration has a `data` section.
    fn conditions() -> Conditions {
        Conditions::new().on_property("data")
    }

    /// The `SslBundles` bean resolves the `ssl-bundle` keys.
    fn after() -> Vec<ConfigRef> {
        [
            #[cfg(feature = "ssl")]
            ConfigRef::of::<super::SslAutoConfiguration>(),
        ]
        .into()
    }

    fn properties() -> &'static [PropertyMetadata] {
        static DATA: OnceLock<Vec<PropertyMetadata>> = OnceLock::new();
        DATA.get_or_init(|| {
            oxbloom_data::PROPERTIES
                .iter()
                .map(|&(key, ty, default, text)| PropertyMetadata::new(key, ty, default, text))
                .collect()
        })
    }
}

/// The settings of the `data` keys; the `ConfigError` names the full key and stays the typed
/// source.
fn data_settings(config: Arc<Config>) -> Result<DataSettings, DiError> {
    DataSettings::from_config(&config).map_err(DiError::creation_from::<DataSettings>)
}

/// The registry of every source. The bundles are resolved first, so an unknown bundle name is
/// a configuration error on its key that lists the available names.
fn data_sources(
    settings: Arc<DataSettings>,
    #[cfg(feature = "ssl")] bundles: Option<Arc<SslBundles>>,
) -> Result<DataSources, DiError> {
    let creation = DiError::creation_from::<DataSources>;
    let mut material: HashMap<String, TlsMaterial> = HashMap::new();
    for source in &settings.sources {
        let Some(name) = source.ssl_bundle.as_deref() else {
            continue;
        };
        let key = format!("{}.ssl-bundle", source.config_prefix());
        #[cfg(feature = "ssl")]
        let tls = bundle_material(key, name, bundles.as_deref()).map_err(creation)?;
        #[cfg(not(feature = "ssl"))]
        let tls = bundle_material(key).map_err(creation)?;
        material.insert(name.to_owned(), tls);
    }
    let tls = |name: &str| material.get(name).cloned().ok_or(DataError::Tls);
    DataSources::build(&settings, &tls).map_err(|error| {
        let driver = settings
            .sources
            .iter()
            .find(|s| s.name == error.source_name())
            .map(|s| s.driver);
        DiError::creation_from::<DataSources>(SourceBuildError { error, driver })
    })
}

/// The PEM material of the bundle `name`, or an error on `key` that lists the available
/// bundle names (never a path or file content).
#[cfg(feature = "ssl")]
fn bundle_material(
    key: String,
    name: &str,
    bundles: Option<&SslBundles>,
) -> Result<TlsMaterial, ConfigError> {
    let available = crate::web::available(bundles);
    let bundle = bundles.and_then(|b| b.get(name).ok()).ok_or_else(|| {
        ConfigError::invalid(
            key,
            format!("names an SSL bundle that does not exist; available bundles: {available}"),
        )
    })?;
    Ok(TlsMaterial {
        trust_pem: bundle.trust_pem(),
        identity: bundle.identity_pem(),
    })
}

/// Without the feature `ssl` there are no bundles: a named one is an error on `key`.
#[cfg(not(feature = "ssl"))]
fn bundle_material(key: String) -> Result<TlsMaterial, ConfigError> {
    Err(ConfigError::invalid(
        key,
        "needs the `ssl` feature of `oxbloom-boot`; available bundles: none",
    ))
}

fn data_service(sources: Arc<DataSources>, settings: Arc<DataSettings>) -> DataService {
    DataService::new(sources, settings.connect_on_startup)
}

/// Whether the `data` section configures a source: a named one under `data.sources`, or any
/// key of the default source. Without a source there is nothing to connect or close, so no
/// `DataService` is registered. The reason of a skip never contains a value.
fn sources_configured(cx: &ConditionContext) -> ConditionOutcome {
    let default_keys = oxbloom_data::PROPERTIES
        .iter()
        .map(|p| p.0)
        .filter(|key| !key.starts_with("data.sources.") && *key != "data.connect-on-startup");
    if cx.contains("data.sources") || default_keys.into_iter().any(|key| cx.contains(key)) {
        ConditionOutcome::matched()
    } else {
        ConditionOutcome::skipped("the data section configures no source")
    }
}

/// Whether the scheme of `data.url` names `driver`, by the rule of [`Driver::from_url`]. Only
/// the scheme is read; the reason of a skip never contains the URL.
fn default_driver_is(cx: &ConditionContext, driver: Driver) -> ConditionOutcome {
    match cx
        .property("data.url")
        .as_deref()
        .and_then(Driver::from_url)
    {
        Some(found) if found == driver => ConditionOutcome::matched(),
        Some(_) => ConditionOutcome::skipped(format!(
            "the default data source does not use the {driver} driver"
        )),
        None => ConditionOutcome::skipped("there is no default data source with a known scheme"),
    }
}

#[cfg(feature = "data-postgres")]
fn default_postgres(sources: Arc<DataSources>) -> Result<oxbloom_data::sqlx::PgPool, DiError> {
    sources
        .default_name()
        .and_then(|name| sources.postgres(name))
        .cloned()
        .ok_or_else(|| {
            DiError::creation::<oxbloom_data::sqlx::PgPool>(
                "the default data source is not a Postgres source",
            )
        })
}

#[cfg(feature = "data-mysql")]
fn default_mysql(sources: Arc<DataSources>) -> Result<oxbloom_data::sqlx::MySqlPool, DiError> {
    sources
        .default_name()
        .and_then(|name| sources.mysql(name))
        .cloned()
        .ok_or_else(|| {
            DiError::creation::<oxbloom_data::sqlx::MySqlPool>(
                "the default data source is not a MySQL source",
            )
        })
}

#[cfg(feature = "data-sqlite")]
fn default_sqlite(sources: Arc<DataSources>) -> Result<oxbloom_data::sqlx::SqlitePool, DiError> {
    sources
        .default_name()
        .and_then(|name| sources.sqlite(name))
        .cloned()
        .ok_or_else(|| {
            DiError::creation::<oxbloom_data::sqlx::SqlitePool>(
                "the default data source is not a SQLite source",
            )
        })
}

/// A source the registry could not build, with its driver for the report. The `Display` is
/// the fixed text of the [`SourceError`], which stays the source.
#[derive(Debug)]
pub(crate) struct SourceBuildError {
    pub(crate) error: SourceError,
    pub(crate) driver: Option<Driver>,
}

impl fmt::Display for SourceBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, f)
    }
}

impl std::error::Error for SourceBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// A source that failed the start-up check. The `Display` is the fixed text of the
/// [`SourceError`], which names the source; the [`DataError`] of the check is the source.
#[derive(Debug)]
pub(crate) struct DataUnreachable(pub(crate) SourceError);

impl fmt::Display for DataUnreachable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for DataUnreachable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0.error)
    }
}

/// The boot [`Service`] of the [`DataSources`] registry, registered by
/// [`DataAutoConfiguration`].
///
/// With `data.connect-on-startup` (the default), [`start`](Service::start) checks the sources
/// one at a time in the order of [`DataSources::names`], each with a 10 second timeout, and
/// stops at the first source that fails, so an unreachable database fails the boot and
/// several unreachable ones cost one wait, not one each.
/// The `ServiceError` then wraps an error whose `Display` names the source with a fixed text,
/// such as ``data source `orders`: could not connect to the database``, and whose source is
/// the [`DataError`] of the check, such as [`DataError::Connect`];
/// the failure report names the source and never the URL. A refused connection can also end
/// in [`DataError::Pool`], because sqlx retries it until the source's `acquire-timeout`; the
/// report treats both as an unreachable source. With the default `acquire-timeout` of 30
/// seconds, an unreachable Postgres or MySQL server therefore makes the start wait the full 10
/// seconds of its check; set a shorter `acquire-timeout` to fail sooner. Without
/// `data.connect-on-startup` the pools connect on first use.
///
/// [`stop`](Service::stop) marks every pool closed at once, so none hands out a new
/// connection, then waits up to 10 seconds in all for the pools to close, which needs the
/// connections in use to be returned. When that time runs out it logs one `warn` event at
/// target `oxbloom::data` whose `sources` field lists only the sources that did not finish
/// closing (see [`DataSources::close_all_within`]); their connections in use are closed when
/// they are returned. Boot's own stop deadline bounds the stop as well.
///
/// Its [`phase`](Service::phase) is `i32::MIN` and its registration has the lowest DI order,
/// so it starts before every other service and stops after them. It does not
/// [keep running](Service::keeps_running): the pools only need to be open while other code
/// uses them, so an application whose only services are data services ends after its runners
/// instead of waiting for a shutdown signal.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "data-sqlite")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::Arc;
/// use oxbloom_boot::{DataService, Service};
/// use oxbloom_config::Config;
/// use oxbloom_data::{DataError, DataSettings, DataSources};
///
/// tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
///     let config = Config::from_str("[data]\nurl = \"sqlite::memory:\"")?;
///     let settings = DataSettings::from_config(&config)?;
///     let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls))?);
///     let service = DataService::new(sources.clone(), true);
///     assert_eq!(service.phase(), i32::MIN);
///     assert_eq!(service.name(), "DataSources");
///     assert!(!service.keeps_running());
///
///     service.start().await?;
///     assert_eq!(sources.pool_counts("default").map(|(size, _)| size), Some(1));
///     service.stop().await;
///     assert!(sources.sqlite("default").ok_or("no default")?.is_closed());
///     Ok::<(), Box<dyn std::error::Error>>(())
/// })?;
/// # Ok(())
/// # }
/// # #[cfg(not(feature = "data-sqlite"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct DataService {
    sources: Arc<DataSources>,
    connect_on_startup: bool,
}

impl DataService {
    /// A service for `sources` that checks every source at start when `connect_on_startup`
    /// is `true`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "data-sqlite")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use oxbloom_boot::{DataService, Service};
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    ///     // The file's directory does not exist: the check fails and names the source.
    ///     let config = Config::from_str("[data]\nurl = \"sqlite:/no-such-oxbloom-dir/app.db\"")?;
    ///     let settings = DataSettings::from_config(&config)?;
    ///     let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls))?);
    ///     let err = DataService::new(sources.clone(), true).start().await.unwrap_err();
    ///     assert_eq!(err.to_string(), "data source `default`: could not connect to the database");
    ///
    ///     // Without the check the service starts and the pool stays unconnected.
    ///     DataService::new(sources.clone(), false).start().await?;
    ///     assert_eq!(sources.pool_counts("default"), Some((0, 0)));
    ///     Ok::<(), Box<dyn std::error::Error>>(())
    /// })?;
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "data-sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn new(sources: Arc<DataSources>, connect_on_startup: bool) -> DataService {
        DataService {
            sources,
            connect_on_startup,
        }
    }

    /// The registry this service connects and closes.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "data-sqlite")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use oxbloom_boot::DataService;
    /// use oxbloom_config::Config;
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    /// let _guard = runtime.enter();
    /// let config = Config::from_str("[data.sources.cache]\nurl = \"sqlite::memory:\"")?;
    /// let settings = DataSettings::from_config(&config)?;
    /// let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls))?);
    /// let service = DataService::new(sources.clone(), true);
    /// assert!(Arc::ptr_eq(service.sources(), &sources));
    /// assert_eq!(service.sources().names(), ["cache"]);
    /// # Ok(())
    /// # }
    /// # #[cfg(not(feature = "data-sqlite"))]
    /// # fn main() {}
    /// ```
    pub fn sources(&self) -> &Arc<DataSources> {
        &self.sources
    }
}

#[async_trait]
impl Service for DataService {
    /// Checks every source when `data.connect-on-startup` is on; the first failing source
    /// fails the start.
    async fn start(&self) -> Result<(), ServiceError> {
        if !self.connect_on_startup {
            return Ok(());
        }
        // One source at a time, stopping at the first failure, so several unreachable sources
        // do not each add their wait to the start.
        for name in self.sources.names() {
            if let Err(error) = self.sources.ping(name, PING_TIMEOUT).await {
                return Err(ServiceError::new(DataUnreachable(SourceError::new(
                    name, error,
                ))));
            }
        }
        Ok(())
    }

    /// Marks every pool closed, then waits up to 10 seconds in all for the closes; the sources
    /// whose close did not finish are named in one warning.
    async fn stop(&self) {
        let unfinished = self.sources.close_all_within(CLOSE_TIMEOUT).await;
        if !unfinished.is_empty() {
            tracing::warn!(
                target: TARGET,
                sources = %unfinished.join(", "),
                "these data sources did not close within 10 seconds"
            );
        }
    }

    /// `false`: the pools only need to be open between start and stop, so an application
    /// whose runners do the work ends after them.
    fn keeps_running(&self) -> bool {
        false
    }

    /// `i32::MIN`: the pools are up before every other service and close after them.
    fn phase(&self) -> i32 {
        i32::MIN
    }

    fn name(&self) -> &'static str {
        "DataSources"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_are_exactly_the_data_crate_keys() {
        let keys: Vec<_> = DataAutoConfiguration::properties()
            .iter()
            .map(|p| p.key)
            .collect();
        let expected: Vec<_> = oxbloom_data::PROPERTIES.iter().map(|p| p.0).collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn the_errors_show_fixed_text_and_keep_their_source() {
        use std::error::Error;

        let unreachable = DataUnreachable(SourceError::new("orders", DataError::Connect));
        assert_eq!(
            unreachable.to_string(),
            "data source `orders`: could not connect to the database"
        );
        assert_eq!(
            unreachable
                .source()
                .and_then(|s| s.downcast_ref::<DataError>()),
            Some(&DataError::Connect)
        );
    }
}
