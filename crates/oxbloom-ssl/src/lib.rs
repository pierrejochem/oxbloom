//! Named SSL bundles for Oxbloom, loaded from the `ssl.*` configuration keys.
//!
//! A bundle has an identity (a certificate chain and its private key), a trust store (CA
//! certificates), or both. It comes from PEM values or files under `ssl.bundle.pem.<name>`, or
//! from a PKCS#12 file under `ssl.bundle.pkcs12.<name>`. [`SslBundle::server_config`] and
//! [`SslBundle::client_config`] turn a bundle into rustls configurations that use the
//! `aws-lc-rs` provider. Errors name the bundle and the key to fix, never a private key, a
//! password or a file path. [`PROPERTIES`] lists every key this crate reads.
//!
//! [`SslBundles::start_reload`] watches the bundle files every `ssl.reload-interval` and swaps
//! a changed bundle in for the next handshake; [`SslBundles::reload`] does the same on demand.
//! With the feature `di`, `SslBundles` implements `oxbloom_di::Lifecycle`, so a container
//! starts the task when it creates the bean and stops it at shutdown.
//!
//! # Examples
//!
//! ```
//! use oxbloom_config::Config;
//! use oxbloom_ssl::SslBundles;
//!
//! # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
//! # let (cert_pem, key_pem) = (generated.cert.pem(), generated.signing_key.serialize_pem());
//! // `cert_pem` and `key_pem` hold PEM text; a file path works the same way.
//! let config = Config::from_str(&format!(
//!     "[ssl.bundle.pem.web]\ncertificate = '''\n{cert_pem}'''\nprivate-key = '''\n{key_pem}'''"
//! ))?;
//! let bundles = SslBundles::from_config(&config)?;
//! let server = bundles.get("web")?.server_config()?;
//! assert_eq!(server.alpn_protocols, [b"http/1.1".to_vec()]);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod bundle;
mod error;
mod reload;
mod settings;

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arc_swap::ArcSwap;
use oxbloom_config::Config;
use rustls::client::ResolvesClientCert;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, ServerConfig, SignatureScheme};
use rustls_platform_verifier::BuilderVerifierExt;

use crate::bundle::{Material, provider};
use crate::reload::{HookSlot, Poller, ReloadState, lock};
use crate::settings::{BundleSource, Settings, Source, canonical};

pub use error::SslError;
pub use reload::ReloadTask;
pub use settings::PROPERTIES;

/// Internal hooks for this crate's tests. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    use crate::SslBundles;

    /// Runs `hook` with the bundle name between the read of a bundle's files and the second
    /// metadata check, in every reload of `bundles` and its clones.
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// let bundles = SslBundles::from_config(&Config::from_str("")?)?;
    /// oxbloom_ssl::__private::set_read_hook(&bundles, |_bundle| {});
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn set_read_hook(bundles: &SslBundles, hook: impl Fn(&str) + Send + Sync + 'static) {
        *crate::reload::lock(&bundles.read_hook) = Some(std::sync::Arc::new(hook));
    }
}

/// The SSL bundles loaded from the `ssl.*` keys, by name.
///
/// Cloning is cheap and every clone shares the same bundles.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_ssl::SslBundles;
///
/// let bundles = SslBundles::from_config(&Config::from_str("")?)?;
/// assert!(bundles.names().is_empty());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct SslBundles {
    bundles: Arc<Vec<SslBundle>>,
    reload_interval: Duration,
    expiry_warning: Duration,
    read_hook: HookSlot,
    /// The task the container started through `Lifecycle`.
    #[cfg(feature = "di")]
    task: Arc<Mutex<Option<ReloadTask>>>,
}

impl SslBundles {
    /// Reads `ssl.*` from `config` and loads every bundle, reading the files they name.
    ///
    /// A value that starts with `-----BEGIN` is inline PEM, any other value is a file path.
    /// Without any `ssl` key the set is empty.
    ///
    /// # Errors
    ///
    /// - [`SslError::Config`] when a key has an invalid value, when a key under a bundle is
    ///   unknown (the message lists the valid keys), or with [`ConfigError::Missing`] when
    ///   `certificate` is set without `private-key` or the reverse.
    /// - [`SslError::DuplicateName`] when a name is used under both `pem` and `pkcs12`.
    /// - [`SslError::Io`], [`SslError::Pem`], [`SslError::Pkcs12`], [`SslError::KeyMismatch`]
    ///   and [`SslError::EmptyBundle`] when a bundle's material cannot be loaded.
    ///
    /// [`ConfigError::Missing`]: oxbloom_config::ConfigError::Missing
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    /// use oxbloom_ssl::{SslBundles, SslError};
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca_pem = generated.cert.pem();
    /// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
    /// let bundles = SslBundles::from_config(&config)?;
    /// assert_eq!(bundles.names(), ["client"]);
    ///
    /// let half = Config::from_str("[ssl.bundle.pem.web]\ncertificate = \"web.crt\"")?;
    /// assert!(matches!(
    ///     SslBundles::from_config(&half),
    ///     Err(SslError::Config(ConfigError::Missing { key })) if key == "ssl.bundle.pem.web.private-key"
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<SslBundles, SslError> {
        let settings = Settings::from_config(config)?;
        let mut bundles = Vec::with_capacity(settings.bundles.len());
        for source in settings.bundles {
            bundles.push(SslBundle::load(source)?);
        }
        bundles.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(SslBundles {
            bundles: Arc::new(bundles),
            reload_interval: settings.reload_interval,
            expiry_warning: settings.expiry_warning,
            read_hook: HookSlot::default(),
            #[cfg(feature = "di")]
            task: Arc::default(),
        })
    }

    /// The bundle named `name`, matched like configuration keys (case-insensitive, ignoring
    /// `-` and `_`).
    ///
    /// # Errors
    ///
    /// [`SslError::UnknownBundle`] with the sorted names of the loaded bundles when no bundle
    /// has that name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::{SslBundles, SslError};
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca_pem = generated.cert.pem();
    /// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
    /// let bundles = SslBundles::from_config(&config)?;
    /// assert_eq!(bundles.get("client")?.name(), "client");
    ///
    /// let err = bundles.get("web").unwrap_err();
    /// assert!(matches!(err, SslError::UnknownBundle { .. }));
    /// assert_eq!(err.to_string(), "unknown SSL bundle `web`; available bundles: client");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn get(&self, name: &str) -> Result<SslBundle, SslError> {
        let wanted = canonical(name);
        self.bundles
            .iter()
            .find(|b| canonical(b.name()) == wanted)
            .cloned()
            .ok_or_else(|| SslError::UnknownBundle {
                name: name.to_string(),
                available: self.names(),
            })
    }

    /// The names of every bundle, sorted, as written in the configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca = generated.cert.pem();
    /// let config = Config::from_str(&format!(
    ///     "[ssl.bundle.pem.web]\ntrust = '''\n{ca}'''\n[ssl.bundle.pem.api]\ntrust = '''\n{ca}'''"
    /// ))?;
    /// assert_eq!(SslBundles::from_config(&config)?.names(), ["api", "web"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn names(&self) -> Vec<String> {
        self.bundles.iter().map(|b| b.name().to_string()).collect()
    }

    /// How often bundle files are checked for changes, from `ssl.reload-interval` (default
    /// 30 seconds). Zero means the check is off.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// let bundles = SslBundles::from_config(&Config::from_str("[ssl]\nreload-interval = \"5s\"")?)?;
    /// assert_eq!(bundles.reload_interval(), Duration::from_secs(5));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reload_interval(&self) -> Duration {
        self.reload_interval
    }

    /// How long before expiry a certificate produces a warning, from `ssl.expiry-warning`
    /// (default 14 days).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// let bundles = SslBundles::from_config(&Config::from_str("")?)?;
    /// assert_eq!(bundles.expiry_warning(), Duration::from_secs(14 * 24 * 60 * 60));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn expiry_warning(&self) -> Duration {
        self.expiry_warning
    }

    /// Rebuilds every bundle now from all its sources, files and inline PEM alike, and returns
    /// the result per bundle, in name order.
    ///
    /// A bundle that loads is swapped in, and the next handshake of every configuration built
    /// from it uses the new identity. A bundle that fails keeps its old material. Nothing calls
    /// this automatically; [`SslBundles::start_reload`] polls instead. A client configuration
    /// fixes its trust store when [`SslBundle::client_config`] builds it, so a reloaded trust
    /// store reaches only configurations built afterwards. The call reads the files on the
    /// calling thread with blocking I/O; inside an async task, run it on a blocking thread
    /// such as `tokio::task::spawn_blocking`.
    ///
    /// # Errors
    ///
    /// Each failed bundle has the error [`SslBundles::from_config`] would return for it, or
    /// [`SslError::Io`] with [`ErrorKind::Interrupted`](std::io::ErrorKind::Interrupted) and
    /// the key of the file that kept changing while it was read three times.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::{SslBundles, SslError};
    ///
    /// # let dir = tempfile::tempdir()?;
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca_file = dir.path().join("ca.pem");
    /// # std::fs::write(&ca_file, generated.cert.pem())?;
    /// # let ca_path = ca_file.display();
    /// // `ca_path` names a PEM file with the CA certificate.
    /// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = \"{ca_path}\""))?;
    /// let bundles = SslBundles::from_config(&config)?;
    /// let results = bundles.reload();
    /// assert_eq!(results[0].0, "client");
    /// assert!(results[0].1.is_ok());
    ///
    /// // A broken file is reported, and the bundle keeps its trust store.
    /// # std::fs::write(&ca_file, "not a certificate")?;
    /// let results = bundles.reload();
    /// assert!(matches!(results[0].1, Err(SslError::Pem { .. })));
    /// assert!(bundles.get("client")?.has_trust());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reload(&self) -> Vec<(String, Result<(), SslError>)> {
        let hook = lock(&self.read_hook).clone();
        self.bundles
            .iter()
            .map(|bundle| {
                let result = reload::reload_now(bundle, hook.as_deref());
                (bundle.name().to_string(), result)
            })
            .collect()
    }

    /// Starts the task that checks the files of every bundle every
    /// [`reload_interval`](SslBundles::reload_interval).
    ///
    /// It compares each file's size and modification time, following symlinks, so a
    /// Kubernetes secret mount that swaps a symlink is seen. Inline PEM and PKCS#12 passwords
    /// are not watched. When a file changed, the whole bundle is rebuilt and swapped in for
    /// the next handshake; when a file changes during the read, the task tries again on the
    /// next tick. A failed reload keeps the old material and logs one warning on target
    /// `oxbloom::ssl` with the bundle name and the error kind, and logs again only after the
    /// files change again. As with [`SslBundles::reload`], a client configuration built
    /// earlier keeps its trust store.
    ///
    /// A certificate file and a key file that are replaced one after the other can be seen
    /// between the two writes, which gives one `key-mismatch` warning before the second write
    /// is picked up. Replace both at once, with a directory or symlink swap as Kubernetes
    /// secret mounts do, to avoid it.
    ///
    /// With an interval of zero the returned task does nothing. Inside a Tokio runtime, which
    /// needs its time driver, the task runs on that runtime; outside one, on its own thread.
    /// Events go to the subscriber that is current when the task starts or, when there is
    /// none, to the default subscriber when each event is emitted, so a global subscriber
    /// installed later still receives them.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca_pem = generated.cert.pem();
    /// let config = Config::from_str(&format!(
    ///     "[ssl]\nreload-interval = \"5s\"\n[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"
    /// ))?;
    /// let bundles = SslBundles::from_config(&config)?;
    /// let task = bundles.start_reload();
    /// assert!(task.is_running());
    /// task.stop();
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn start_reload(&self) -> ReloadTask {
        let poller = Poller {
            bundles: Arc::clone(&self.bundles),
            hook: Arc::clone(&self.read_hook),
        };
        ReloadTask::start(poller, self.reload_interval)
    }
}

/// Starts the reload task when the container creates the bean and stops it at shutdown.
/// Register it with `ContainerBuilder::lifecycle::<SslBundles>()` after the bean.
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_di::ContainerBuilder;
/// use oxbloom_ssl::SslBundles;
///
/// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
/// # let ca_pem = generated.cert.pem();
/// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
/// let container = ContainerBuilder::new()
///     .instance(SslBundles::from_config(&config)?)
///     .lifecycle::<SslBundles>()
///     .build()?;
/// assert_eq!(container.get::<SslBundles>()?.names(), ["client"]);
/// container.shutdown(); // stops the reload task
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[cfg(feature = "di")]
impl oxbloom_di::Lifecycle for SslBundles {
    fn post_construct(&self) -> Result<(), oxbloom_di::DiError> {
        let mut task = lock(&self.task);
        if task.is_none() {
            *task = Some(self.start_reload());
        }
        Ok(())
    }

    fn pre_destroy(&self) {
        let task = lock(&self.task).take();
        if let Some(task) = task {
            task.stop();
        }
    }
}

/// Lists the bundles by name with their contents; never a key, a password or a path.
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_ssl::SslBundles;
///
/// let bundles = SslBundles::from_config(&Config::from_str("")?)?;
/// assert_eq!(format!("{bundles:?}"), "SslBundles { bundles: [], reload_interval: 30s, expiry_warning: 1209600s }");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl fmt::Debug for SslBundles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SslBundles")
            .field("bundles", &*self.bundles)
            .field("reload_interval", &self.reload_interval)
            .field("expiry_warning", &self.expiry_warning)
            .finish()
    }
}

/// One named SSL bundle: an identity, a trust store, or both.
///
/// Cloning is cheap and every clone shares the same material. The material sits behind an
/// atomically swapped pointer, and the configurations built from the bundle read it at each
/// handshake.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_ssl::SslBundles;
///
/// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
/// # let ca_pem = generated.cert.pem();
/// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
/// let client = SslBundles::from_config(&config)?.get("client")?;
/// assert!(client.has_trust() && !client.has_identity());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct SslBundle {
    inner: Arc<Inner>,
}

struct Inner {
    source: BundleSource,
    material: Arc<ArcSwap<Material>>,
    reload: Mutex<ReloadState>,
}

impl SslBundle {
    fn load(source: BundleSource) -> Result<SslBundle, SslError> {
        let reload = ReloadState::new(&source);
        let material = bundle::load(&source)?;
        Ok(SslBundle {
            inner: Arc::new(Inner {
                source,
                material: Arc::new(ArcSwap::from_pointee(material)),
                reload: Mutex::new(reload),
            }),
        })
    }

    /// The bundle name, as written in the configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca_pem = generated.cert.pem();
    /// let config = Config::from_str(&format!("[ssl.bundle.pem.Client]\ntrust = '''\n{ca_pem}'''"))?;
    /// assert_eq!(SslBundles::from_config(&config)?.get("client")?.name(), "Client");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn name(&self) -> &str {
        &self.inner.source.name
    }

    /// Whether the bundle has an identity: a certificate chain and its private key.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let (cert, key) = (generated.cert.pem(), generated.signing_key.serialize_pem());
    /// let config = Config::from_str(&format!(
    ///     "[ssl.bundle.pem.web]\ncertificate = '''\n{cert}'''\nprivate-key = '''\n{key}'''"
    /// ))?;
    /// assert!(SslBundles::from_config(&config)?.get("web")?.has_identity());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn has_identity(&self) -> bool {
        self.inner.material.load().identity.is_some()
    }

    /// Whether the bundle has a trust store of CA certificates.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let (cert, key) = (generated.cert.pem(), generated.signing_key.serialize_pem());
    /// let config = Config::from_str(&format!(
    ///     "[ssl.bundle.pem.web]\ncertificate = '''\n{cert}'''\nprivate-key = '''\n{key}'''"
    /// ))?;
    /// assert!(!SslBundles::from_config(&config)?.get("web")?.has_trust());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn has_trust(&self) -> bool {
        self.inner.material.load().trust.is_some()
    }

    /// A rustls server configuration that presents the bundle's identity, without client
    /// authentication, with ALPN `http/1.1` only.
    ///
    /// The certificate is resolved at each handshake from the bundle's current material, so
    /// the configuration stays valid for the life of the bundle.
    ///
    /// # Errors
    ///
    /// [`SslError::NoIdentity`] when the bundle has only a trust store.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::{SslBundles, SslError};
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let (cert, key) = (generated.cert.pem(), generated.signing_key.serialize_pem());
    /// let config = Config::from_str(&format!(
    ///     "[ssl.bundle.pem.web]\ncertificate = '''\n{cert}'''\nprivate-key = '''\n{key}'''\n\
    ///      [ssl.bundle.pem.client]\ntrust = '''\n{cert}'''"
    /// ))?;
    /// let bundles = SslBundles::from_config(&config)?;
    /// let server = bundles.get("web")?.server_config()?;
    /// assert_eq!(server.alpn_protocols, [b"http/1.1".to_vec()]);
    ///
    /// let err = bundles.get("client")?.server_config().unwrap_err();
    /// assert!(matches!(err, SslError::NoIdentity { .. }));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn server_config(&self) -> Result<Arc<ServerConfig>, SslError> {
        if !self.has_identity() {
            return Err(SslError::NoIdentity {
                bundle: self.name().to_string(),
            });
        }
        let mut config = ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(rustls::DEFAULT_VERSIONS)
            .expect("the aws-lc-rs provider supports the default protocol versions")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(CurrentIdentity {
                material: Arc::clone(&self.inner.material),
            }));
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }

    /// A rustls client configuration that verifies servers against the bundle's trust store,
    /// or with the platform verifier when the bundle has none, and presents the bundle's
    /// identity as a client certificate when it has one.
    ///
    /// The client certificate is resolved at each handshake, so a reloaded identity reaches
    /// the next handshake. The trust store is fixed when this method is called: after a
    /// reload, only configurations built afterwards verify against the new trust store.
    ///
    /// # Errors
    ///
    /// [`SslError::Io`] naming the bundle's `trust` key (`path` for PKCS#12) when the bundle
    /// has no trust store and the platform verifier cannot be set up.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_ssl::SslBundles;
    ///
    /// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    /// # let ca_pem = generated.cert.pem();
    /// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
    /// let client = SslBundles::from_config(&config)?.get("client")?.client_config()?;
    /// assert!(!client.client_auth_cert_resolver.has_certs());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn client_config(&self) -> Result<Arc<ClientConfig>, SslError> {
        let material = self.inner.material.load();
        let builder = ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(rustls::DEFAULT_VERSIONS)
            .expect("the aws-lc-rs provider supports the default protocol versions");
        let builder = match &material.trust {
            Some(roots) => builder.with_root_certificates(Arc::clone(roots)),
            None => builder.with_platform_verifier().map_err(|_| {
                let leaf = match self.inner.source.source {
                    Source::Pem { .. } => "trust",
                    Source::Pkcs12 { .. } => "path",
                };
                SslError::Io {
                    bundle: self.name().to_string(),
                    key: self.inner.source.key(leaf),
                    kind: std::io::ErrorKind::Other,
                }
            })?,
        };
        let config = if material.identity.is_some() {
            builder.with_client_cert_resolver(Arc::new(CurrentIdentity {
                material: Arc::clone(&self.inner.material),
            }))
        } else {
            builder.with_no_client_auth()
        };
        Ok(Arc::new(config))
    }
}

/// Shows the name and which parts the bundle has; never a key, a password or a path.
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_ssl::SslBundles;
///
/// # let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
/// # let ca_pem = generated.cert.pem();
/// let config = Config::from_str(&format!("[ssl.bundle.pem.client]\ntrust = '''\n{ca_pem}'''"))?;
/// let client = SslBundles::from_config(&config)?.get("client")?;
/// assert_eq!(format!("{client:?}"), "SslBundle { name: \"client\", identity: false, trust: true }");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
impl fmt::Debug for SslBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SslBundle")
            .field("name", &self.name())
            .field("identity", &self.has_identity())
            .field("trust", &self.has_trust())
            .finish()
    }
}

/// Resolves the bundle's current identity at each handshake, on the server and the client
/// side, so a swapped material reaches the next handshake.
struct CurrentIdentity {
    material: Arc<ArcSwap<Material>>,
}

impl CurrentIdentity {
    fn current(&self) -> Option<Arc<CertifiedKey>> {
        self.material.load().identity.clone()
    }
}

impl fmt::Debug for CurrentIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CurrentIdentity")
            .field("identity", &self.material.load().identity.is_some())
            .finish()
    }
}

impl ResolvesServerCert for CurrentIdentity {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.current()
    }
}

impl ResolvesClientCert for CurrentIdentity {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        self.current()
    }

    fn has_certs(&self) -> bool {
        self.material.load().identity.is_some()
    }
}
