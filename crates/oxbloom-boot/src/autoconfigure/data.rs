//! `DataAutoConfiguration`: the `DataSources` registry of the `data.*` keys, the default
//! source's pool bean, and `DataService`, which connects the pools at start and closes them at
//! shutdown.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use oxbloom_config::{Config, ConfigError};
#[cfg(feature = "data-migrate")]
use oxbloom_data::migrate::{EmbeddedMigrations, MigrateError};
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
///   running, so a run-to-completion application ends after its runners. With the feature
///   `data-migrate`, it also applies the migrations of every source with
///   `migrations.enabled = true` at start, from `migrations.path` or from the application's
///   optional `EmbeddedMigrations` bean (see [`DataService`]). Without the feature, a source
///   with `migrations.enabled = true` fails the build with a report naming the feature and
///   the key.
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
/// source. Without the feature `data-migrate`, a source that enables migrations fails here, so
/// it is never left unmigrated without a word.
fn data_settings(config: Arc<Config>) -> Result<DataSettings, DiError> {
    let settings =
        DataSettings::from_config(&config).map_err(DiError::creation_from::<DataSettings>)?;
    #[cfg(not(feature = "data-migrate"))]
    if let Some(source) = settings.sources.iter().find(|s| s.migrations.is_some()) {
        return Err(DiError::creation_from::<DataSettings>(
            MigrationsNeedFeature {
                source_name: source.name.clone(),
                key: format!("{}.migrations.enabled", source.config_prefix()),
            },
        ));
    }
    Ok(settings)
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

#[cfg(not(feature = "data-migrate"))]
fn data_service(sources: Arc<DataSources>, settings: Arc<DataSettings>) -> DataService {
    DataService::new(sources, settings.connect_on_startup)
}

/// The service with the migrations of the settings; the application's `EmbeddedMigrations`
/// bean is optional.
#[cfg(feature = "data-migrate")]
fn data_service(
    sources: Arc<DataSources>,
    settings: Arc<DataSettings>,
    embedded: Option<Arc<EmbeddedMigrations>>,
) -> DataService {
    let connect_on_startup = settings.connect_on_startup;
    DataService::with_migrations(sources, settings, embedded, connect_on_startup)
}

/// Whether the `data` section configures a source: a named one under `data.sources`, or any
/// key of the default source. Without a source there is nothing to connect or close, so no
/// `DataService` is registered. The reason of a skip never contains a value.
///
/// The `data.migrations.*` keys count as keys of the default source: a `migrations` table
/// without `data.url` is a default source without a URL, which the settings refuse with an
/// error on `data.url` whether the table enables the migrations or not. A source that enables
/// migrations always has a URL, so it registers the service, also with
/// `data.connect-on-startup = false`.
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

/// The migrations of a source failed while the `DataService` started, with the driver of the
/// source for the report. The `Display` is the fixed ``data source `<name>`: the migrations
/// failed``; the [`MigrateError`] is the source.
#[cfg(feature = "data-migrate")]
#[derive(Debug)]
pub(crate) struct DataMigrationFailed {
    pub(crate) error: MigrateError,
    pub(crate) driver: Option<Driver>,
}

#[cfg(feature = "data-migrate")]
impl fmt::Display for DataMigrationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "data source `{}`: the migrations failed",
            self.error.source_name()
        )
    }
}

#[cfg(feature = "data-migrate")]
impl std::error::Error for DataMigrationFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// A source enables migrations in a build without the feature `data-migrate`. The `Display`
/// names the source, the key and the feature, never a value.
#[cfg(not(feature = "data-migrate"))]
#[derive(Debug)]
pub(crate) struct MigrationsNeedFeature {
    pub(crate) source_name: String,
    pub(crate) key: String,
}

#[cfg(not(feature = "data-migrate"))]
impl fmt::Display for MigrationsNeedFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "data source `{}`: `{}` is true, but the `data-migrate` feature of `oxbloom-boot` is off",
            self.source_name, self.key
        )
    }
}

#[cfg(not(feature = "data-migrate"))]
impl std::error::Error for MigrationsNeedFeature {}

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
/// With the feature `data-migrate` and a service made by `DataService::with_migrations` (as
/// [`DataAutoConfiguration`] does), the
/// start then applies the migrations of every source with `migrations.enabled = true`, one
/// source after the other in the order of [`DataSources::names`], the default source first,
/// before the service reports started; every service with a later phase therefore sees the
/// migrated schema. The migrations come from the source's `migrations.path` directory, or
/// else from the application's `EmbeddedMigrations` bean; a source with both uses the
/// directory and logs one `info` event at target `oxbloom::data` that names only the source.
/// The first failure stops the run and fails the start with an error whose `Display` is
/// ``data source `<name>`: the migrations failed`` and whose source is the
/// `oxbloom_data::migrate::MigrateError`; the failure report names the source, the version of
/// the migration at fault and the key to change, never SQL, a path or the URL. Migrations win
/// over `data.connect-on-startup = false`: a source that migrates is checked and connected at
/// start, and the other sources stay unconnected.
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
    /// The settings and embedded migrators of the migrations; `None` for a service made with
    /// [`DataService::new`]. Boxed, so the service stays small without the migrations.
    #[cfg(feature = "data-migrate")]
    migrations: Option<Box<Migrations>>,
}

/// What [`DataService::with_migrations`] keeps for the migrations.
#[cfg(feature = "data-migrate")]
#[derive(Debug)]
struct Migrations {
    settings: Arc<DataSettings>,
    embedded: Option<Arc<EmbeddedMigrations>>,
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
            #[cfg(feature = "data-migrate")]
            migrations: None,
        }
    }

    /// A service for `sources` that also applies the migrations of every source of `settings`
    /// with `migrations.enabled = true` at start, after the connectivity check. `embedded` is
    /// the application's [`EmbeddedMigrations`], used for a source without a `migrations.path`.
    /// A source that migrates is connected even when `connect_on_startup` is `false`; the
    /// other sources then stay unconnected.
    ///
    /// Available with the cargo feature `data-migrate`.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(all(feature = "data-sqlite", feature = "data-migrate"))]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use std::sync::Arc;
    /// use oxbloom_boot::{DataService, Service};
    /// use oxbloom_config::Config;
    /// use oxbloom_data::migrate::EmbeddedMigrations;
    /// use oxbloom_data::sqlx::SqlSafeStr;
    /// use oxbloom_data::sqlx::migrate::{Migration, MigrationType, Migrator};
    /// use oxbloom_data::{DataError, DataSettings, DataSources};
    ///
    /// tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    ///     let config = Config::from_str(
    ///         "[data]\nurl = \"sqlite::memory:\"\nconnect-on-startup = false\n\
    ///          [data.migrations]\nenabled = true",
    ///     )?;
    ///     let settings = Arc::new(DataSettings::from_config(&config)?);
    ///     let sources = Arc::new(DataSources::build(&settings, &|_| Err(DataError::Tls))?);
    ///     // In an application: `static MIGRATOR: Migrator = sqlx::migrate!("./migrations");`
    ///     let migrator: &'static Migrator = Box::leak(Box::new(Migrator::with_migrations(vec![
    ///         Migration::new(
    ///             1,
    ///             "create accounts".into(),
    ///             MigrationType::Simple,
    ///             "CREATE TABLE accounts (id INTEGER PRIMARY KEY)".into_sql_str(),
    ///             false,
    ///         ),
    ///     ])));
    ///     let embedded = Arc::new(EmbeddedMigrations::new().default_source(migrator));
    ///     let connect_on_startup = settings.connect_on_startup;
    ///     let service = DataService::with_migrations(
    ///         sources.clone(),
    ///         settings,
    ///         Some(embedded),
    ///         connect_on_startup,
    ///     );
    ///
    ///     // The source migrates, so it connects although `connect-on-startup` is false.
    ///     service.start().await?;
    ///     let pool = sources.sqlite("default").ok_or("no default")?;
    ///     let count: i64 = oxbloom_data::sqlx::query_scalar("SELECT count(*) FROM accounts")
    ///         .fetch_one(pool)
    ///         .await?;
    ///     assert_eq!(count, 0);
    ///     service.stop().await;
    ///     Ok::<(), Box<dyn std::error::Error>>(())
    /// })?;
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "data-sqlite", feature = "data-migrate")))]
    /// # fn main() {}
    /// ```
    #[cfg(feature = "data-migrate")]
    pub fn with_migrations(
        sources: Arc<DataSources>,
        settings: Arc<DataSettings>,
        embedded: Option<Arc<EmbeddedMigrations>>,
        connect_on_startup: bool,
    ) -> DataService {
        DataService {
            sources,
            connect_on_startup,
            migrations: Some(Box::new(Migrations { settings, embedded })),
        }
    }

    /// The sources to migrate with their settings: the sources of the registry in the order of
    /// [`DataSources::names`], then the sources the settings migrate but the registry lacks
    /// (an application's own registry), whose run fails with an unknown source.
    #[cfg(feature = "data-migrate")]
    fn migration_plan(&self) -> Vec<(&str, &oxbloom_data::MigrationSettings)> {
        let Some(migrations) = self.migrations.as_deref() else {
            return Vec::new();
        };
        let enabled = |name: &str| {
            migrations
                .settings
                .sources
                .iter()
                .find(|s| s.name == name)
                .and_then(|s| s.migrations.as_ref())
        };
        let names = self.sources.names();
        let mut plan: Vec<_> = names
            .iter()
            .filter_map(|&name| enabled(name).map(|m| (name, m)))
            .collect();
        plan.extend(migrations.settings.sources.iter().filter_map(|s| {
            let m = s.migrations.as_ref()?;
            (!names.contains(&s.name.as_str())).then_some((s.name.as_str(), m))
        }));
        plan
    }

    /// Applies the migrations of the plan one source at a time; the first failure stops the
    /// run.
    #[cfg(feature = "data-migrate")]
    async fn migrate(
        &self,
        plan: &[(&str, &oxbloom_data::MigrationSettings)],
    ) -> Result<(), ServiceError> {
        let embedded = self
            .migrations
            .as_deref()
            .and_then(|m| m.embedded.as_deref());
        for &(name, settings) in plan {
            let migrator = embedded.and_then(|e| e.get(name));
            if settings.path.is_some() && migrator.is_some() {
                tracing::info!(
                    target: TARGET,
                    source = name,
                    "the migrations of this data source come from its `migrations.path`, not from its embedded migrations"
                );
            }
            match oxbloom_data::migrate::run(&self.sources, name, settings, migrator).await {
                Ok(report) if !report.is_empty() => tracing::info!(
                    target: TARGET,
                    source = name,
                    applied = report.applied().len(),
                    latest_version = report.latest_version(),
                    "applied the pending migrations of this data source"
                ),
                Ok(_) => {}
                Err(error) => {
                    let driver = self.sources.driver_of(name).or_else(|| {
                        let settings = &self.migrations.as_deref()?.settings;
                        settings
                            .sources
                            .iter()
                            .find(|s| s.name == name)
                            .map(|s| s.driver)
                    });
                    return Err(ServiceError::new(DataMigrationFailed { error, driver }));
                }
            }
        }
        Ok(())
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
    /// Checks every source when `data.connect-on-startup` is on, and every source that
    /// migrates in any case; the first failing source fails the start. Then, with the feature
    /// `data-migrate`, applies the migrations source by source; the first failure fails the
    /// start.
    async fn start(&self) -> Result<(), ServiceError> {
        #[cfg(feature = "data-migrate")]
        let plan = self.migration_plan();
        #[cfg(feature = "data-migrate")]
        let check = |name: &str| self.connect_on_startup || plan.iter().any(|&(n, _)| n == name);
        #[cfg(not(feature = "data-migrate"))]
        let check = |_: &str| self.connect_on_startup;
        // One source at a time, stopping at the first failure, so several unreachable sources
        // do not each add their wait to the start.
        for name in self.sources.names() {
            if !check(name) {
                continue;
            }
            if let Err(error) = self.sources.ping(name, PING_TIMEOUT).await {
                return Err(ServiceError::new(DataUnreachable(SourceError::new(
                    name, error,
                ))));
            }
        }
        #[cfg(feature = "data-migrate")]
        self.migrate(&plan).await?;
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
