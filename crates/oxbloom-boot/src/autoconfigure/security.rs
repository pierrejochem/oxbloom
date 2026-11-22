//! `SecurityAutoConfiguration` and its contributor trait, `SecurityCustomizer`.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_di::{Configuration, ContainerBuilder, DiError, component};
use oxbloom_security::{
    AuthenticationProvider, CsrfProperties, FormLoginConfig, HttpBasicSettings, HttpSecurity,
    JwtDecoder, SecurityFilterChain, SessionFilter,
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
/// bad `security.http-basic`, `security.form-login` or `security.csrf` key fails it the same
/// way with the `ConfigError` naming the key. The metadata of this auto-configuration lists the
/// `security.form-login` and `security.csrf` keys, so excluding it hides them.
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
/// and HTTP Basic over the provider beans when `security.http-basic.enabled` is `true`.
fn security_filter_chain(
    config: Arc<Config>,
    customizers: Vec<Arc<dyn SecurityCustomizer>>,
    decoder: Option<Arc<dyn JwtDecoder>>,
    providers: Vec<Arc<dyn AuthenticationProvider>>,
    session: Option<Arc<SessionFilter>>,
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
    let mut http = HttpSecurity::new();
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

/// Exactly the keys `FormLoginConfig::from_config` and `CsrfProperties::from_config` read
/// (pinned by tests), sorted by key.
static SECURITY: [PropertyMetadata; 14] = [
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
];
