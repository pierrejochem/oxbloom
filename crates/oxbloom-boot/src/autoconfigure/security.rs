//! `SecurityAutoConfiguration` and its contributor trait, `SecurityCustomizer`.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_core::{Clock, SystemClock};
use oxbloom_di::{Configuration, ContainerBuilder, DiError, component};
use oxbloom_security::{
    AuthenticationProvider, CsrfProperties, FormLoginConfig, HeadersProperties, HttpBasicSettings,
    HttpSecurity, JwtDecoder, SecurityFilterChain, SessionFilter, SessionManagementProperties,
    SessionRegistry, UserDetailsService,
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
/// bad `security.http-basic`, `security.form-login`, `security.csrf`, `security.headers` or
/// session management key fails it the same way with the `ConfigError` naming the key. The
/// metadata of this auto-configuration lists the `security.form-login`, `security.csrf` and
/// `security.headers` keys, so excluding it hides them.
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
/// These settings and the optional `dyn Clock` bean (`HttpSecurity::clock`, default the system
/// clock) are applied before the customizers, so a `SecurityCustomizer` can still change them;
/// a customizer that calls `headers` receives the configured headers and changes only what it
/// touches.
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
        &SECURITY
    }
}

/// The chain from the customizers, or the closed default, plus the decoder when there is one
/// and HTTP Basic over the provider beans when `security.http-basic.enabled` is `true`. The
/// headers, the session limit, the revalidation and the clock are set before the customizers,
/// so a customizer can still change them.
fn security_filter_chain(
    config: Arc<Config>,
    customizers: Vec<Arc<dyn SecurityCustomizer>>,
    decoder: Option<Arc<dyn JwtDecoder>>,
    providers: Vec<Arc<dyn AuthenticationProvider>>,
    session: Option<Arc<SessionFilter>>,
    users: Option<Arc<dyn UserDetailsService>>,
    clock: Option<Arc<dyn Clock>>,
) -> Result<SecurityFilterChain, DiError> {
    let creation = DiError::creation_from::<SecurityFilterChain>;
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
    let mut http = HttpSecurity::new()
        .clock(clock.unwrap_or_else(|| Arc::new(SystemClock)))
        .headers(move |_| headers);
    if let Some((index, max)) = limit {
        http = http.session_limit(index, max);
    }
    if let Some((interval, users)) = revalidate {
        http = http.revalidate(interval, users);
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
    http.build()
        .map_err(DiError::creation_from::<SecurityFilterChain>)
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

/// Exactly the keys `FormLoginConfig::from_config`, `CsrfProperties::from_config` and
/// `HeadersProperties::from_config` read (pinned by tests), sorted by key.
static SECURITY: [PropertyMetadata; 23] = [
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
];
