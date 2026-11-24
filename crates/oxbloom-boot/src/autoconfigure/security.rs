//! `SecurityAutoConfiguration` and its contributor trait, `SecurityCustomizer`.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_di::{Configuration, ContainerBuilder, DiError, component};
use oxbloom_security::{
    AuthenticationProvider, CsrfProperties, FormLoginConfig, HeadersProperties, HttpBasicSettings,
    HttpSecurity, JwtDecoder, RememberMeProperties, RememberMeSettings, SecurityFilterChain,
    SessionConfig, SessionFilter, SessionManagementProperties, SessionRegistry, UserDetailsService,
};

use super::jwt::JwtDecoderAutoConfiguration;
use super::password::PasswordEncoderAutoConfiguration;
use crate::autoconfig::{AutoConfiguration, ConfigRef};
use crate::metadata::PropertyMetadata;

/// Contributes rules to the `SecurityFilterChain` that [`SecurityAutoConfiguration`] builds.
///
/// Register an implementation as `dyn SecurityCustomizer`. The customizers are applied in the
/// order the container gives them (ascending `order`, then registration order), each on the
/// result of the one before. `HttpSecurity::authorize` may be called only once in total, so
/// exactly one customizer should own the rules. With no customizer at all, every request must
/// be authenticated.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     Banner, OxbloomApplication, RunListener, SecurityCustomizer, auto_configurations,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_security::{HttpSecurity, SecurityFilterChain};
///
/// /// Opens `/health`; everything else needs authentication.
/// struct PublicHealth;
/// impl SecurityCustomizer for PublicHealth {
///     fn customize(&self, http: HttpSecurity) -> HttpSecurity {
///         http.authorize(|a| a.get("/health").permit_all().any_request().authenticated())
///     }
/// }
///
/// #[derive(Clone, Default)]
/// struct Chains(Arc<Mutex<usize>>);
/// impl RunListener for Chains {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = c.get_all::<SecurityFilterChain>().unwrap().len();
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let chains = Chains::default();
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(chains.clone())
///     .auto_configurations(auto_configurations())
///     .configure(|b| {
///         b.instance(PublicHealth)
///             .provides(|c: Arc<PublicHealth>| -> Arc<dyn SecurityCustomizer> { c })
///     })
///     .run()
///     .await?;
/// assert_eq!(*chains.0.lock().unwrap(), 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait SecurityCustomizer: Send + Sync {
    /// Returns `http` with this customizer's settings added.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::SecurityCustomizer;
    /// use oxbloom_security::HttpSecurity;
    ///
    /// /// Opens `/health`; everything else needs authentication.
    /// struct PublicHealth;
    /// impl SecurityCustomizer for PublicHealth {
    ///     fn customize(&self, http: HttpSecurity) -> HttpSecurity {
    ///         http.authorize(|a| a.get("/health").permit_all().any_request().authenticated())
    ///     }
    /// }
    ///
    /// let chain = PublicHealth.customize(HttpSecurity::new()).build();
    /// assert!(chain.is_ok());
    /// ```
    fn customize(&self, http: HttpSecurity) -> HttpSecurity;
}

/// Registers the `SecurityFilterChain`, unless the application registered its own.
///
/// The chain starts from `HttpSecurity::new()`. The [`SecurityCustomizer`] beans are applied
/// in order; when there is none, the only rule is `any_request().authenticated()`, so the
/// application stays closed until it opens routes. A `dyn JwtDecoder` bean (from
/// [`JwtDecoderAutoConfiguration`], which runs first, or the application's) is added with
/// `oauth2_resource_server`.
///
/// HTTP Basic is off unless `security.http-basic.enabled` is `true` (read with
/// `oxbloom_security::HttpBasicSettings::from_config`). Then the chain calls `http_basic` with
/// `security.http-basic.realm` and adds every `dyn AuthenticationProvider` bean (such as the
/// `DaoAuthenticationProvider` of [`PasswordEncoderAutoConfiguration`], which runs first) with
/// `password_provider`. Configured users alone never open Basic. With Basic enabled and no
/// provider bean the build fails with `SecurityBuildError::HttpBasicWithoutProvider`.
///
/// Form login is off unless `security.form-login.enabled` is `true` (read with
/// `oxbloom_security::FormLoginConfig::from_config`). Then the chain calls `form_login` with
/// the configured settings and adds every `dyn AuthenticationProvider` bean as a password
/// provider, so the user chain needs the same provider beans as Basic does: with none the
/// build fails with `SecurityBuildError::FormLoginWithoutProvider`. Form login keeps the
/// `Authentication` in the session, so it needs the session filter of
/// [`SessionAutoConfiguration`](crate::SessionAutoConfiguration), which exists only when the
/// `security.session` table is present: without it the startup fails with a `DiError::Creation`
/// that names `security.session`.
///
/// A `SecurityBuildError` fails the build as `DiError::Creation` with that error as source; a
/// bad `security.http-basic`, `security.form-login`, `security.csrf`, `security.headers`,
/// `security.remember-me` or session management key fails it the same way with the
/// `ConfigError` naming the key. The metadata of this auto-configuration lists the
/// `security.form-login`, `security.csrf`, `security.headers` and `security.remember-me` keys,
/// so excluding it hides them.
///
/// CSRF protection follows form login (read with `oxbloom_security::CsrfProperties::from_config`).
/// With form login on, the chain calls `csrf` with the `security.csrf` settings: the check is on
/// unless `security.csrf.enabled` is `false`, and `field-name`, `header-name` and
/// `token-endpoint` are optional. With form login off the chain never calls `csrf`; then
/// `security.csrf.enabled = true` and every other `security.csrf` key fail the startup with a
/// `ConfigError` naming the key, and `enabled = false` alone is accepted and does nothing. A
/// token endpoint equal to a form login path fails the build with
/// `SecurityBuildError::InvalidCsrfConfig`. The chain is configured after every
/// `SecurityCustomizer`, as for form login, so the configuration wins over a customizer that
/// calls `.csrf(..)`.
///
/// The security response headers follow `security.headers` (read with
/// `oxbloom_security::HeadersProperties::from_config`): they are on by default, `off` removes
/// one header and `enabled = false` removes all of them. The session management keys follow
/// `security.session` (read with `oxbloom_security::SessionManagementProperties::from_config`):
///
/// - `max-sessions-per-user` calls `HttpSecurity::session_limit` with the per-user index of the
///   session filter (`SessionFilter::principal_sessions`). Without an index (the cookie store,
///   or a store bean that does not implement `PrincipalSessions`) the startup fails with an
///   error that names `security.session.max-sessions-per-user`. Without form login the build
///   fails with `SecurityBuildError::SessionLimitWithoutFormLogin`.
/// - `revalidate-interval` (default five minutes, `0s` off) calls `HttpSecurity::revalidate`
///   with the `dyn UserDetailsService` bean. The default applies only with form login and that
///   bean and stays off without them. A set interval without the bean fails the startup with an
///   error that names `security.session.revalidate-interval`; a set interval without form login
///   fails the build with `SecurityBuildError::RevalidateWithoutFormLogin`.
///
/// Remember-me follows `security.remember-me` (read with
/// `oxbloom_security::RememberMeProperties::from_config`). It is off unless `enabled` is `true`,
/// but its keys are checked either way: an unknown key or a bad value fails the startup even
/// while it is off. When on, the chain calls `remember_me` with the configured secret, old
/// secrets, lifetime, field name and cookie name, the `dyn UserDetailsService` bean (the same
/// bean revalidation uses) and the clock below. Enabled remember-me needs form login, a secret
/// and that bean: without form login the startup fails with an error that names
/// `security.form-login.enabled`, without a secret one that names
/// `security.remember-me.secret`, and without the bean one that names
/// `security.remember-me.enabled`. A cookie name equal to the session cookie name
/// fails the startup with an error that names `security.remember-me.cookie-name`, since the
/// two cookies would overwrite each other. No error or report shows a secret.
///
/// These settings and the optional `dyn Clock` bean (`HttpSecurity::clock`, default the system
/// clock; remember-me gets the same clock, so the login time, the revalidation and the cookie
/// expiry follow one clock) are applied before the customizers, so a `SecurityCustomizer` can
/// still change them; a customizer that calls `headers` receives the configured headers and
/// changes only what it touches.
///
/// With the feature `oauth2`, OAuth2 login follows `security.oauth2`. When
/// `security.oauth2.client.registration` has at least one entry, the chain calls
/// `oauth2_login` after the customizers with the registrations and providers of
/// `oxbloom_security::OAuth2Settings::from_config`, the `dyn HttpClient` bean (from
/// [`HttpClientAutoConfiguration`](crate::HttpClientAutoConfiguration) or the application),
/// `security.oauth2.base-url`, the optional `security.oauth2.failure-url` and the `dyn Clock`
/// bean when there is one. It steps aside when a `SecurityCustomizer` already called
/// `oauth2_login`: then the customizer wins and the base URL is not required. The keys are
/// checked either way: an unknown key under `security.oauth2` or a bad registration fails the
/// startup with a `ConfigError` that names the full key and never holds the value or the
/// client secret. The section is parsed once, into a lazy bean that the OAuth2 client of
/// `OAuth2ClientAutoConfiguration` reads too, so the login and the client see the same
/// registrations. With registrations:
///
/// - `security.oauth2.base-url` is required (a `ConfigError::Missing`), and must be an absolute
///   `https` URL (`http` only for a loopback host) with no path beyond an optional `/`, no
///   query and no fragment, because the callback path `/login/oauth2/code/{id}` is fixed.
/// - A `redirect-uri` that does not resolve to `{base-url}/login/oauth2/code/{id}` fails with
///   a `ConfigError` on `security.oauth2.client.registration.<id>.redirect-uri`.
/// - `security.oauth2.failure-url` must be a local path outside the two OAuth2 endpoints.
/// - Form login must be on (`security.form-login.enabled = true`); it needs no password
///   provider then. Without it, or without an `HttpClient` bean, the startup fails with an
///   error that says so.
///
/// A `SessionRegistry` bean is registered, unless the application has one, when there is a
/// `SessionFilter` bean and `security.session.store` is `memory` (or missing). It is lazy: it is
/// built from `SessionFilter::principal_sessions` when first asked for, and a store without the
/// per-user index fails only the request for the registry, never the startup. With the cookie
/// store there is no registry bean.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{
///     AutoConfiguration, Banner, ConfigRef, JwtDecoderAutoConfiguration, OxbloomApplication,
///     PasswordEncoderAutoConfiguration, RunListener, SecurityAutoConfiguration,
///     auto_configurations,
/// };
/// use oxbloom_config::Config;
/// use oxbloom_di::{Container, DiError};
/// use oxbloom_security::{HttpSecurity, SecurityFilterChain};
///
/// #[derive(Clone, Default)]
/// struct Skipped(Arc<Mutex<bool>>);
/// impl RunListener for Skipped {
///     fn context_ready(&self, c: &Container) {
///         assert_eq!(c.get_all::<SecurityFilterChain>().unwrap().len(), 1);
///         *self.0.lock().unwrap() = c.condition_report().records().iter().any(|r| {
///             !r.matched() && r.reason().contains("SecurityFilterChain")
///         });
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// // The application's own chain: the default one steps aside.
/// let skipped = Skipped::default();
/// OxbloomApplication::builder()
///     .with_config(Config::from_str("")?)
///     .banner(Banner::off())
///     .output(std::io::sink())
///     .listener(skipped.clone())
///     .auto_configurations(auto_configurations())
///     .configure(|b| {
///         b.try_bean(|| -> Result<SecurityFilterChain, DiError> {
///             Ok(HttpSecurity::new().authorize(|a| a.any_request().permit_all()).build()?)
///         })
///     })
///     .run()
///     .await?;
/// assert!(*skipped.0.lock().unwrap());
/// assert_eq!(
///     SecurityAutoConfiguration::after(),
///     [
///         ConfigRef::of::<JwtDecoderAutoConfiguration>(),
///         ConfigRef::of::<PasswordEncoderAutoConfiguration>(),
///     ]
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct SecurityAutoConfiguration;

impl Configuration for SecurityAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        // The `security.oauth2` section, parsed once for the OAuth2 login of the chain and the
        // `OAuth2Client` of `OAuth2ClientAutoConfiguration`; lazy, so it is read only when one
        // of them is created.
        #[cfg(feature = "oauth2")]
        let builder = builder.try_bean(super::oauth2::oauth2_properties).lazy();
        builder
            .try_bean(security_filter_chain)
            .on_missing_bean::<SecurityFilterChain>()
            .try_bean(session_registry)
            .on_bean::<SessionFilter>()
            .on_property_value_or_missing("security.session.store", "memory")
            .on_missing_bean::<SessionRegistry>()
            .lazy()
    }
}

impl AutoConfiguration for SecurityAutoConfiguration {
    fn after() -> Vec<ConfigRef> {
        vec![
            ConfigRef::of::<JwtDecoderAutoConfiguration>(),
            ConfigRef::of::<PasswordEncoderAutoConfiguration>(),
        ]
    }

    fn properties() -> &'static [PropertyMetadata] {
        #[cfg(feature = "oauth2")]
        {
            &MERGED
        }
        #[cfg(not(feature = "oauth2"))]
        {
            &SECURITY
        }
    }
}

/// The startup warning when `security.oauth2` is configured but the feature `oauth2` is off,
/// so the section would otherwise be ignored without a word. Fixed text, no value.
#[cfg_attr(feature = "oauth2", allow(dead_code))]
const OAUTH2_WITHOUT_FEATURE: &str = "the configuration has a security.oauth2 section, but the \
     feature `oauth2` of oxbloom-boot is off, so OAuth2 login is not configured: enable the \
     feature or remove the section";

/// Whether the configuration holds any key under `security.oauth2`.
#[cfg_attr(feature = "oauth2", allow(dead_code))]
fn has_oauth2_section(config: &Config) -> bool {
    !config.keys("security.oauth2").is_empty()
}

/// The chain from the customizers, or the closed default, plus the decoder when there is one
/// and HTTP Basic over the provider beans when `security.http-basic.enabled` is `true`. The
/// headers, the session limit, the revalidation, remember-me and the clock are set before the
/// customizers, so a customizer can still change them; form login and CSRF are set after them,
/// so there the configuration wins. With `oauth2`, OAuth2 login follows the customizers too,
/// and steps aside when one of them configured it.
// Each parameter is an injected bean; the container needs them as separate arguments.
#[cfg_attr(feature = "oauth2", allow(clippy::too_many_arguments))]
fn security_filter_chain(
    config: Arc<Config>,
    customizers: Vec<Arc<dyn SecurityCustomizer>>,
    decoder: Option<Arc<dyn JwtDecoder>>,
    providers: Vec<Arc<dyn AuthenticationProvider>>,
    session: Option<Arc<SessionFilter>>,
    users: Option<Arc<dyn UserDetailsService>>,
    clock: Option<Arc<dyn Clock>>,
    #[cfg(feature = "oauth2")] client: Option<Arc<dyn oxbloom_http_client::HttpClient>>,
    #[cfg(feature = "oauth2")] oauth2: Arc<super::oauth2::OAuth2Properties>,
) -> Result<SecurityFilterChain, DiError> {
    let creation = DiError::creation_from::<SecurityFilterChain>;
    #[cfg(not(feature = "oauth2"))]
    if has_oauth2_section(&config) {
        tracing::warn!(target: "oxbloom::boot", "{OAUTH2_WITHOUT_FEATURE}");
    }
    #[cfg(feature = "oauth2")]
    let oauth2_clock = clock.clone();
    let basic = HttpBasicSettings::from_config(&config).map_err(creation)?;
    let form = FormLoginConfig::from_config(&config).map_err(creation)?;
    if form.is_some() && session.is_none() {
        // The session filter is registered only when the `security.session` table exists.
        return Err(DiError::creation_from::<SecurityFilterChain>(
            FormLoginWithoutSession,
        ));
    }
    let form_enabled = form.is_some();
    let csrf = CsrfProperties::from_config(&config, form_enabled).map_err(creation)?;
    let headers = HeadersProperties::from_config(&config).map_err(creation)?;
    let management = SessionManagementProperties::from_config(&config).map_err(creation)?;
    let limit = match management.max_sessions_per_user {
        None => None,
        Some(max) => match session.as_ref().and_then(|s| s.principal_sessions()) {
            Some(index) => Some((index, max)),
            None => {
                return Err(DiError::creation_from::<SecurityFilterChain>(
                    SessionManagementError::LimitWithoutIndex,
                ));
            }
        },
    };
    let remember_me = RememberMeProperties::from_config(&config).map_err(creation)?;
    let remember_me = match remember_me {
        Some(settings) => Some(remember_me_parts(
            &config,
            settings,
            form_enabled,
            users.clone(),
        )?),
        None => None,
    };
    // The default interval applies only with form login and a user service; an explicit one
    // is passed on, so the build reports it when form login is off.
    let revalidate = match (management.revalidate_interval, users) {
        (Some(interval), Some(users)) if form_enabled || management.revalidate_explicit => {
            Some((interval, users))
        }
        (Some(_), None) if management.revalidate_explicit => {
            return Err(DiError::creation_from::<SecurityFilterChain>(
                SessionManagementError::RevalidateWithoutUserService,
            ));
        }
        _ => None,
    };
    // One clock for the form login handlers, the revalidation and remember-me.
    let clock = clock.unwrap_or_else(|| Arc::new(SystemClock));
    let mut http = HttpSecurity::new()
        .clock(clock.clone())
        .headers(move |_| headers);
    if let Some((index, max)) = limit {
        http = http.session_limit(index, max);
    }
    if let Some((interval, users)) = revalidate {
        http = http.revalidate(interval, users);
    }
    if let Some((settings, users)) = remember_me {
        http = http.remember_me(move |r| settings.apply(r).user_details(users).clock(clock));
    }
    if customizers.is_empty() {
        // Closed by default: nothing is reachable without authentication.
        http = http.authorize(|a| a.any_request().authenticated());
    }
    for customizer in &customizers {
        http = customizer.customize(http);
    }
    if let Some(decoder) = decoder {
        http = http.oauth2_resource_server(|r| r.jwt(decoder));
    }
    // OAuth2 login from the configuration, unless a customizer configured it: then the
    // customizer wins and boot steps aside.
    #[cfg(feature = "oauth2")]
    let boot_oauth2 = oauth2.has_registrations() && !http.has_oauth2_login();
    #[cfg(feature = "oauth2")]
    if boot_oauth2 {
        http = oauth2
            .apply(http, client, oauth2_clock)
            .map_err(oauth2_creation)?;
    }
    if basic.enabled {
        let realm = basic.realm;
        http = http.http_basic(|b| b.realm(realm));
    }
    if let Some(form) = form {
        http = http.form_login(move |_| form);
    }
    if let Some(csrf) = csrf {
        // `HttpSecurity::csrf` needs form login, and the reader gives `Some` only with it.
        http = http.csrf(move |_| csrf);
    }
    if basic.enabled || form_enabled {
        for provider in providers {
            http = http.password_provider(provider);
        }
    }
    http.build().map_err(|error| {
        // With boot's OAuth2 login, the errors that only the configuration can cause name
        // their key.
        #[cfg(feature = "oauth2")]
        if boot_oauth2 && let Some(failure) = super::oauth2::config_failure(&error) {
            return oauth2_creation(failure);
        }
        DiError::creation_from::<SecurityFilterChain>(error)
    })
}

/// The startup failure of an OAuth2 configuration problem, with the typed error as source.
#[cfg(feature = "oauth2")]
fn oauth2_creation(failure: super::oauth2::OAuth2Failure) -> DiError {
    match failure {
        super::oauth2::OAuth2Failure::Config(error) => {
            DiError::creation_from::<SecurityFilterChain>(error)
        }
        super::oauth2::OAuth2Failure::Login(error) => {
            DiError::creation_from::<SecurityFilterChain>(error)
        }
    }
}

/// The checked remember-me settings and the user service, or the startup failure that names
/// the missing piece (form login, the secret, the user service) or a cookie name equal to the
/// session cookie name.
fn remember_me_parts(
    config: &Config,
    settings: RememberMeSettings,
    form_enabled: bool,
    users: Option<Arc<dyn UserDetailsService>>,
) -> Result<(RememberMeSettings, Arc<dyn UserDetailsService>), DiError> {
    let fail = |e: RememberMeError| Err(DiError::creation_from::<SecurityFilterChain>(e));
    if !form_enabled {
        return fail(RememberMeError::WithoutFormLogin);
    }
    if settings.secret.is_none() {
        return fail(RememberMeError::WithoutSecret);
    }
    let Some(users) = users else {
        return fail(RememberMeError::WithoutUserService);
    };
    let session = SessionConfig::from_config(config)
        .map_err(DiError::creation_from::<SecurityFilterChain>)?;
    if session.settings.cookie_name == settings.cookie_name {
        return fail(RememberMeError::CookieNameClash);
    }
    Ok((settings, users))
}

/// The registry over the per-user index of the session filter. The bean is lazy: a server
/// store without the index fails only an application that asks for the registry.
fn session_registry(session: Arc<SessionFilter>) -> Result<SessionRegistry, DiError> {
    session
        .principal_sessions()
        .map(SessionRegistry::new)
        .ok_or_else(|| {
            DiError::creation_from::<SessionRegistry>(SessionManagementError::RegistryWithoutIndex)
        })
}

/// The startup failures of the session management keys. The texts are fixed and name the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionManagementError {
    /// `security.session.max-sessions-per-user` with a backend that has no per-user index.
    LimitWithoutIndex,
    /// An explicit `security.session.revalidate-interval` without a `UserDetailsService` bean.
    RevalidateWithoutUserService,
    /// The `SessionRegistry` was asked for, but the session store has no per-user index.
    RegistryWithoutIndex,
}

impl std::fmt::Display for SessionManagementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::LimitWithoutIndex => {
                "security.session.max-sessions-per-user needs a session store that can list the \
                 sessions of a user, such as the in-memory store; the cookie store and stores \
                 without that index cannot"
            }
            Self::RevalidateWithoutUserService => {
                "security.session.revalidate-interval needs a `UserDetailsService` bean to load \
                 the user again, such as the one of the `security.users` section"
            }
            Self::RegistryWithoutIndex => {
                "the `SessionRegistry` needs a session store that can list the sessions of a \
                 user; the configured session store cannot"
            }
        })
    }
}

impl std::error::Error for SessionManagementError {}

/// The startup failures of `security.remember-me.enabled = true`. The texts are fixed and name
/// the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RememberMeError {
    /// Remember-me without form login.
    WithoutFormLogin,
    /// Remember-me without `security.remember-me.secret`.
    WithoutSecret,
    /// Remember-me without a `UserDetailsService` bean.
    WithoutUserService,
    /// `security.remember-me.cookie-name` equal to the session cookie name.
    CookieNameClash,
}

impl std::fmt::Display for RememberMeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WithoutFormLogin => {
                "security.remember-me.enabled needs form login: set security.form-login.enabled \
                 to true"
            }
            Self::WithoutSecret => {
                "security.remember-me.enabled needs security.remember-me.secret, a secret of at \
                 least 32 bytes set in a file or an environment variable"
            }
            Self::WithoutUserService => {
                "security.remember-me.enabled needs a `UserDetailsService` bean to load the user \
                 named in the cookie, such as the one of the `security.users` section"
            }
            Self::CookieNameClash => {
                "security.remember-me.cookie-name must differ from the session cookie name \
                 (security.session.cookie-name); the two cookies would overwrite each other"
            }
        })
    }
}

impl std::error::Error for RememberMeError {}

/// The startup failure of form login without the `security.session` table.
#[derive(Debug)]
struct FormLoginWithoutSession;

impl std::fmt::Display for FormLoginWithoutSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "security.form-login.enabled needs the session filter, which exists only when the \
             `security.session` table is present (for example `[security.session]` with \
             `store = \"memory\"`)",
        )
    }
}

impl std::error::Error for FormLoginWithoutSession {}

/// [`SECURITY`] and [`OAUTH2`] together, sorted by key.
#[cfg(feature = "oauth2")]
static MERGED: std::sync::LazyLock<Vec<PropertyMetadata>> = std::sync::LazyLock::new(|| {
    let mut all: Vec<PropertyMetadata> = SECURITY.iter().chain(OAUTH2.iter()).cloned().collect();
    all.sort_by_key(|p| p.key);
    all
});

/// Exactly the keys `OAuth2Settings::from_config` reads (pinned by tests) and the two boot
/// keys `security.oauth2.base-url` and `security.oauth2.failure-url`, sorted by key.
#[cfg(feature = "oauth2")]
static OAUTH2: [PropertyMetadata; 14] = [
    PropertyMetadata::new(
        "security.oauth2.base-url",
        "String",
        None,
        "The external base URL of the application, the `{base-url}` of every redirect URI: an \
         absolute `https` URL (`http` only for a loopback host) with no path, query or \
         fragment, such as `https://app.example`. Required when a registration is configured.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.provider.<id>.authorization-uri",
        "String",
        None,
        "The authorization endpoint (absolute `https` URL; `http` only for a loopback host). \
         Required together with `token-uri` unless `issuer-uri` is set.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.provider.<id>.issuer-uri",
        "String",
        None,
        "The issuer of an OpenID Connect provider (absolute `https` URL; `http` only for a \
         loopback host). Replaces `authorization-uri` and `token-uri`.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.provider.<id>.jwk-set-uri",
        "String",
        None,
        "The JWK set endpoint (absolute `https` URL; `http` only for a loopback host).",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.provider.<id>.token-uri",
        "String",
        None,
        "The token endpoint (absolute `https` URL; `http` only for a loopback host). Required \
         together with `authorization-uri` unless `issuer-uri` is set.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.provider.<id>.user-info-uri",
        "String",
        None,
        "The user info endpoint (absolute `https` URL; `http` only for a loopback host). A \
         plain OAuth2 provider (no `issuer-uri`, no `jwk-set-uri`) needs this and \
         `user-name-attribute`.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.provider.<id>.user-name-attribute",
        "String",
        None,
        "The attribute that names the user. A plain OAuth2 provider (no `issuer-uri`, no \
         `jwk-set-uri`) needs this and `user-info-uri`.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.registration.<id>.client-authentication",
        "String",
        Some("basic"),
        "How the client authenticates at the token endpoint: `basic`, `post` or `none`.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.registration.<id>.client-id",
        "String",
        None,
        "The client id. Required.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.registration.<id>.client-secret",
        "Secret<String>",
        None,
        "The client secret; never printed. Required unless `client-authentication` is `none`.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.registration.<id>.provider",
        "String",
        None,
        "The provider id under `security.oauth2.client.provider`. Defaults to the \
         registration id.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.registration.<id>.redirect-uri",
        "String",
        Some("{base-url}/login/oauth2/code/{id}"),
        "The redirect URI template; `{base-url}` and `{id}` are resolved. The callback path is \
         fixed, so it must resolve to `{base-url}/login/oauth2/code/{id}`.",
    ),
    PropertyMetadata::new(
        "security.oauth2.client.registration.<id>.scope",
        "Vec<String>",
        None,
        "The scopes to request.",
    ),
    PropertyMetadata::new(
        "security.oauth2.failure-url",
        "String",
        None,
        "Where a failed OAuth2 login goes: a local path outside `/oauth2/authorization/` and \
         `/login/oauth2/code/`. Unset means the failure URL of form login.",
    ),
];

/// Exactly the keys `FormLoginConfig::from_config`, `CsrfProperties::from_config`,
/// `HeadersProperties::from_config` and `RememberMeProperties::from_config` read (pinned by
/// tests), sorted by key.
static SECURITY: [PropertyMetadata; 29] = [
    PropertyMetadata::new(
        "security.csrf.enabled",
        "bool",
        Some("true"),
        "Whether the CSRF check runs. Follows form login: on when form login is on. Setting it \
         to `true` without form login fails the startup.",
    ),
    PropertyMetadata::new(
        "security.csrf.field-name",
        "String",
        Some("_csrf"),
        "The name of the hidden form field that carries the CSRF token; letters, digits, `_` \
         and `-`. Needs form login.",
    ),
    PropertyMetadata::new(
        "security.csrf.header-name",
        "String",
        Some("X-CSRF-TOKEN"),
        "The name of the request header that carries the CSRF token; letters, digits, `_` and \
         `-`. Needs form login.",
    ),
    PropertyMetadata::new(
        "security.csrf.token-endpoint",
        "String",
        None,
        "The path of an endpoint that answers GET with the CSRF token as JSON; a local path \
         that differs from the form login paths. Needs form login. Unset means no endpoint.",
    ),
    PropertyMetadata::new(
        "security.form-login.default-success-url",
        "String",
        Some("/"),
        "Where a login goes when no request was saved; a local path.",
    ),
    PropertyMetadata::new(
        "security.form-login.enabled",
        "bool",
        Some("false"),
        "Whether the security chain offers form login, logout, the saved request and the \
         default login page. Needs the `security.session` table and a password provider bean; \
         without them the startup fails.",
    ),
    PropertyMetadata::new(
        "security.form-login.failure-url",
        "String",
        Some("/login?error"),
        "Where a failed login goes; a local path.",
    ),
    PropertyMetadata::new(
        "security.form-login.login-page",
        "String",
        Some("/login"),
        "The path of the login page; a local path.",
    ),
    PropertyMetadata::new(
        "security.form-login.login-page-enabled",
        "bool",
        Some("true"),
        "Whether the chain serves its default login page at `login-page`.",
    ),
    PropertyMetadata::new(
        "security.form-login.login-processing-url",
        "String",
        Some("/login"),
        "The path the login form posts to; a local path.",
    ),
    PropertyMetadata::new(
        "security.form-login.logout-success-url",
        "String",
        Some("/login?logout"),
        "Where a logout goes; a local path.",
    ),
    PropertyMetadata::new(
        "security.form-login.logout-url",
        "String",
        Some("/logout"),
        "The path that logs out on POST; a local path that differs from the login page and the \
         login processing url.",
    ),
    PropertyMetadata::new(
        "security.form-login.password-field",
        "String",
        Some("password"),
        "The name of the password field of the login form; letters, digits, `_` and `-`, and \
         different from the username field.",
    ),
    PropertyMetadata::new(
        "security.form-login.username-field",
        "String",
        Some("username"),
        "The name of the username field of the login form; letters, digits, `_` and `-`.",
    ),
    PropertyMetadata::new(
        "security.headers.cache-control",
        "String",
        Some("no-store"),
        "The `Cache-Control` value added to a response of an authenticated request (session, \
         Basic or bearer) that has none; `off` removes it.",
    ),
    PropertyMetadata::new(
        "security.headers.content-type-options",
        "String",
        Some("nosniff"),
        "The `X-Content-Type-Options` value added to every response that has none; `off` \
         removes it.",
    ),
    PropertyMetadata::new(
        "security.headers.enabled",
        "bool",
        Some("true"),
        "Whether the security chain adds its response headers at all. A header the \
         application set is never changed.",
    ),
    PropertyMetadata::new(
        "security.headers.frame-options",
        "String",
        Some("DENY"),
        "The `X-Frame-Options` value added to every response that has none; `off` removes it.",
    ),
    PropertyMetadata::new(
        "security.headers.hsts.enabled",
        "bool",
        Some("true"),
        "Whether `Strict-Transport-Security` is added to a response of an HTTPS request.",
    ),
    PropertyMetadata::new(
        "security.headers.hsts.include-subdomains",
        "bool",
        Some("true"),
        "Whether the HSTS header carries `includeSubDomains`.",
    ),
    PropertyMetadata::new(
        "security.headers.hsts.max-age",
        "Duration",
        Some("365d"),
        "The `max-age` of the HSTS header, in whole seconds; at least one second.",
    ),
    PropertyMetadata::new(
        "security.headers.hsts.preload",
        "bool",
        Some("false"),
        "Whether the HSTS header carries `preload`.",
    ),
    PropertyMetadata::new(
        "security.headers.referrer-policy",
        "String",
        Some("no-referrer"),
        "The `Referrer-Policy` value added to every response that has none; `off` removes it.",
    ),
    PropertyMetadata::new(
        "security.remember-me.cookie-name",
        "String",
        Some("OXBLOOM_REMEMBER_ME"),
        "The name of the remember-me cookie; an HTTP token that differs from the session \
         cookie name.",
    ),
    PropertyMetadata::new(
        "security.remember-me.enabled",
        "bool",
        Some("false"),
        "Whether a login with the remember-me checkbox gets a signed cookie that logs the user \
         in again after the session is gone. Needs form login, `secret` and a \
         `UserDetailsService` bean; without them the startup fails.",
    ),
    PropertyMetadata::new(
        "security.remember-me.field-name",
        "String",
        Some("remember-me"),
        "The name of the login form field that asks for the cookie; letters, digits, `_` and \
         `-`. The values `on`, `true`, `yes` and `1` ask for it.",
    ),
    PropertyMetadata::new(
        "security.remember-me.lifetime",
        "Duration",
        Some("14d"),
        "How long a cookie stays valid from the time it is issued; using it does not extend \
         it. Between 1 minute and 365 days.",
    ),
    PropertyMetadata::new(
        "security.remember-me.old-secrets",
        "array of String",
        None,
        "Earlier remember-me secrets whose cookies stay valid after a rotation; each at least \
         32 bytes. Secrets: their values are never printed, and they are refused on the \
         command line; set them in a file or an environment variable.",
    ),
    PropertyMetadata::new(
        "security.remember-me.secret",
        "String",
        None,
        "The key that signs new remember-me cookies, at least 32 bytes; required when \
         remember-me is enabled. A secret: its value is never printed, and it is refused on \
         the command line; set it in a file or an environment variable.",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_security_oauth2_section_is_found_whatever_it_holds() {
        let has = |toml: &str| has_oauth2_section(&Config::from_str(toml).unwrap());
        assert!(!has(""));
        assert!(!has("[security]\nhttp-basic.enabled = true\n"));
        assert!(has(
            "[security.oauth2]\nbase-url = \"https://app.example\"\n"
        ));
        assert!(has(
            "[security.oauth2.client.registration.idp]\nclient-id = \"app\"\n"
        ));
        // A key that only starts with the same letters is another section.
        assert!(!has("[security]\noauth2x = 1\n"));
    }

    #[test]
    fn the_warning_names_the_key_and_the_feature_and_no_value() {
        assert!(OAUTH2_WITHOUT_FEATURE.contains("security.oauth2"));
        assert!(OAUTH2_WITHOUT_FEATURE.contains("feature `oauth2`"));
    }
}
