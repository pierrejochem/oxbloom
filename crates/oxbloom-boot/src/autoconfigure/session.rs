//! `SessionAutoConfiguration`: the session filter of the `security.session` keys.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_security::{
    CookieSessionBackend, InMemorySessionStore, SessionBackend, SessionConfig, SessionFilter,
    SessionStore, StoreKind,
};
use oxbloom_web::Filter;

use super::password::PasswordEncoderAutoConfiguration;
use super::security::SecurityAutoConfiguration;
use crate::autoconfig::{AutoConfiguration, ConfigRef};
use crate::metadata::PropertyMetadata;

/// Registers a [`SessionFilter`] as a `dyn Filter` bean from the `security.session` keys.
///
/// Available with the cargo feature `security`. The group runs only when the
/// `security.session` table exists in the configuration (the condition `on_property`), for
/// example with `[security.session]` in a file, any key under it, or an environment variable
/// such as `APP_SECURITY__SESSION__STORE`. An application that never configures sessions gets
/// no filter and pays nothing. The configured filter steps aside when the application registered
/// its own `SessionFilter` bean (`on_missing_bean::<SessionFilter>()`); a bare `SessionFilter`
/// bean is not a `dyn Filter`, so the application must also provide it as `dyn Filter` for it
/// to run.
///
/// The backend follows `security.session.store`:
///
/// - `memory` (the default): a server-side store. The application's `dyn SessionStore` bean,
///   when there is one, is used (this is how other stores plug in); otherwise an
///   `InMemorySessionStore` with the configured limits.
/// - `cookie`: the stateless `CookieSessionBackend`, keyed by `security.session.secret` and
///   `security.session.old-secrets`. A `dyn SessionStore` bean is ignored.
///
/// An optional `dyn Clock` bean (`oxbloom_core::Clock`) replaces the system time in the
/// filter, the in-memory store and the cookie backend alike, because each of them decides
/// expiry with its own clock.
///
/// The filter takes its slot through `Filter::order()`, one step before the security chain, so
/// authentication can read the session. The auto-configuration runs after
/// [`PasswordEncoderAutoConfiguration`] and before [`SecurityAutoConfiguration`].
///
/// A bad value, an unknown key, `store = "cookie"` without a secret, a secret shorter than 32
/// bytes, or a secret set on the command line fails the build with `DiError::Creation` whose
/// source is the `ConfigError` naming the key; no message carries a secret or the configured
/// value.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{Banner, OxbloomApplication, RunListener, SessionAutoConfiguration};
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_security::SessionFilter;
///
/// #[derive(Clone, Default)]
/// struct Seen(Arc<Mutex<bool>>);
/// impl RunListener for Seen {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get::<SessionFilter>().is_ok();
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let run = |toml: &str, seen: Seen| {
///     OxbloomApplication::builder()
///         .with_config(Config::from_str(toml).unwrap())
///         .banner(Banner::off())
///         .output(std::io::sink())
///         .listener(seen)
///         .auto_configuration::<SessionAutoConfiguration>()
///         .run()
/// };
///
/// // Without the section there is no filter.
/// let seen = Seen::default();
/// run("", seen.clone()).await?;
/// assert!(!*seen.0.lock().unwrap());
///
/// // With it, the in-memory store backs the filter.
/// let seen = Seen::default();
/// run("[security.session]\nidle-timeout = \"10m\"", seen.clone()).await?;
/// assert!(*seen.0.lock().unwrap());
///
/// // The cookie store without a secret fails the build, naming the key.
/// let err = run("[security.session]\nstore = \"cookie\"", Seen::default()).await.unwrap_err();
/// assert!(err.to_string().contains("security.session.secret"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct SessionAutoConfiguration;

impl Configuration for SessionAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(session_filter)
            .provides(|f: Arc<SessionFilter>| -> Arc<dyn Filter> { f })
            .on_missing_bean::<SessionFilter>()
    }
}

impl AutoConfiguration for SessionAutoConfiguration {
    /// Runs only when the `security.session` table exists.
    fn conditions() -> Conditions {
        Conditions::new().on_property("security.session")
    }

    fn after() -> Vec<ConfigRef> {
        vec![ConfigRef::of::<PasswordEncoderAutoConfiguration>()]
    }

    fn before() -> Vec<ConfigRef> {
        vec![ConfigRef::of::<SecurityAutoConfiguration>()]
    }

    fn properties() -> &'static [PropertyMetadata] {
        &SESSION
    }
}

/// The filter of the `security.session` keys; the `ConfigError` stays the typed source. The
/// application's store is used with `store = "memory"`, and the clock bean everywhere.
fn session_filter(
    config: Arc<Config>,
    store: Option<Arc<dyn SessionStore>>,
    clock: Option<Arc<dyn Clock>>,
) -> Result<SessionFilter, DiError> {
    let creation = DiError::creation_from::<SessionFilter>;
    let session = SessionConfig::from_config(&config).map_err(creation)?;
    let clock = clock.unwrap_or_else(|| Arc::new(SystemClock));
    let backend = match (session.store, session.secret.as_ref()) {
        (StoreKind::Cookie, Some(secret)) => SessionBackend::Cookie(Arc::new(
            CookieSessionBackend::new(
                secret,
                &session.old_secrets,
                &session.settings,
                clock.clone(),
            )
            .map_err(DiError::creation_from::<SessionFilter>)?,
        )),
        // The reader refuses a cookie store without a secret, so this arm is the memory store.
        _ => SessionBackend::Server(store.unwrap_or_else(|| {
            Arc::new(InMemorySessionStore::new(&session.settings, clock.clone()))
        })),
    };
    Ok(SessionFilter::new(backend, session.settings)
        .map_err(DiError::creation_from::<SessionFilter>)?
        .with_clock(clock))
}

/// Exactly the keys `SessionConfig::from_config` reads (pinned by a test).
static SESSION: [PropertyMetadata; 10] = [
    PropertyMetadata::new(
        "security.session.absolute-timeout",
        "Duration",
        Some("8h"),
        "How long a session lives in total, however active it is; positive.",
    ),
    PropertyMetadata::new(
        "security.session.cookie-name",
        "String",
        Some("OXBLOOM_SESSION"),
        "The name of the session cookie; an HTTP token.",
    ),
    PropertyMetadata::new(
        "security.session.idle-timeout",
        "Duration",
        Some("30m"),
        "How long a session may sit unused before it expires; at least one second and at most \
         `absolute-timeout`.",
    ),
    PropertyMetadata::new(
        "security.session.max-cookie-bytes",
        "usize",
        Some("3072"),
        "The largest session cookie the filter issues, in bytes; at least 256. A cookie store \
         session over it answers 503.",
    ),
    PropertyMetadata::new(
        "security.session.max-sessions",
        "usize",
        Some("100000"),
        "The most sessions the in-memory store holds; at least 1. At the cap a new session \
         evicts the one with the oldest last touch.",
    ),
    PropertyMetadata::new(
        "security.session.old-secrets",
        "array of String",
        None,
        "Earlier keys of the cookie store that may still open a cookie, tried in order; each \
         at least 32 bytes. Secrets: their values are never printed, and they are refused on \
         the command line; set them in a file or an environment variable.",
    ),
    PropertyMetadata::new(
        "security.session.same-site",
        "String",
        Some("lax"),
        "The `SameSite` attribute of the cookie: `lax`, `strict` or `none`; `none` needs \
         `secure = \"always\"`.",
    ),
    PropertyMetadata::new(
        "security.session.secret",
        "String",
        None,
        "The key the cookie store seals new cookies with, at least 32 bytes; required with \
         `store = \"cookie\"`. A secret: its value is never printed, and it is refused on the \
         command line; set it in a file or an environment variable.",
    ),
    PropertyMetadata::new(
        "security.session.secure",
        "String",
        Some("auto"),
        "When the cookie is `Secure`: `auto` (when the request is HTTPS, directly or by \
         trusted forwarded headers), `always` or `never`.",
    ),
    PropertyMetadata::new(
        "security.session.store",
        "String",
        Some("memory"),
        "Where sessions live: `memory` (a server-side store: the application's `SessionStore` \
         bean, or the in-memory one) or `cookie` (encrypted in the cookie, needs `secret`). The \
         session filter is registered only when the `security.session` table exists.",
    ),
];
