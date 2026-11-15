//! `PasswordEncoderAutoConfiguration`: the password encoder, the configured users and the DAO
//! authentication provider.

use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_di::{Conditions, Configuration, ContainerBuilder, DiError, component};
use oxbloom_security::{
    AuthenticationProvider, DaoAuthenticationProvider, DelegatingPasswordEncoder,
    InMemoryUserDetailsService, PasswordEncoder, PasswordSettings, UserDetailsService,
    users_from_config,
};

use crate::autoconfig::AutoConfiguration;
use crate::metadata::PropertyMetadata;

/// Registers the password beans of the `security.password` and `security.users` keys.
///
/// Available with the cargo feature `security`. It registers, in this order:
///
/// 1. A `DelegatingPasswordEncoder` from `security.password.*`, provided as
///    `dyn PasswordEncoder`, unless the application registered its own `dyn PasswordEncoder`.
/// 2. An `InMemoryUserDetailsService` with the users of `security.users.<name>`, provided as
///    `dyn UserDetailsService`, when the `security.users` section exists and the application
///    registered no `dyn UserDetailsService` of its own.
/// 3. A `DaoAuthenticationProvider`, provided as `dyn AuthenticationProvider`, when both a
///    `dyn UserDetailsService` and a `dyn PasswordEncoder` exist.
///
/// The beans alone do not open HTTP Basic: `security.http-basic.enabled` defaults to `false`.
/// With it set to `true`, [`SecurityAutoConfiguration`](crate::SecurityAutoConfiguration)
/// turns on Basic and adds every `dyn AuthenticationProvider` bean as a password provider; with
/// no such bean the build fails with `SecurityBuildError::HttpBasicWithoutProvider`.
///
/// A bad value, an unknown key or a plain-text password fails the build with
/// `DiError::Creation` whose source is the `ConfigError` naming the key; no message carries a
/// password, a hash or the configured value. The keys of `security.password` are not read when
/// the application registered its own encoder, and those of `security.users` are not read
/// when it registered its own user store.
///
/// # Examples
///
/// ```
/// use std::sync::{Arc, Mutex};
/// use oxbloom_boot::{Banner, OxbloomApplication, PasswordEncoderAutoConfiguration, RunListener};
/// use oxbloom_config::Config;
/// use oxbloom_di::Container;
/// use oxbloom_security::{
///     Argon2PasswordEncoder, AuthenticationProvider, DelegatingPasswordEncoder, PasswordEncoder,
///     UserDetailsService,
/// };
///
/// #[derive(Clone, Default)]
/// struct Seen(Arc<Mutex<(usize, usize, usize)>>);
/// impl RunListener for Seen {
///     fn context_ready(&self, c: &Container) {
///         *self.0.lock().unwrap() = (
///             c.get_all::<dyn PasswordEncoder>().unwrap().len(),
///             c.get_all::<dyn UserDetailsService>().unwrap().len(),
///             c.get_all::<dyn AuthenticationProvider>().unwrap().len(),
///         );
///     }
/// }
///
/// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
/// let run = |toml: String, seen: Seen| {
///     OxbloomApplication::builder()
///         .with_config(Config::from_str(&toml).unwrap())
///         .banner(Banner::off())
///         .output(std::io::sink())
///         .listener(seen)
///         .auto_configuration::<PasswordEncoderAutoConfiguration>()
///         .run()
/// };
///
/// // Without users there is only the encoder.
/// let seen = Seen::default();
/// run(String::new(), seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), (1, 0, 0));
///
/// // With a user, the store and the DAO provider join it.
/// let hash = DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1)?).encode("pw");
/// let seen = Seen::default();
/// run(format!("[security.users.alice]\npassword = \"{hash}\""), seen.clone()).await?;
/// assert_eq!(*seen.0.lock().unwrap(), (1, 1, 1));
///
/// // A plain-text password fails the build, naming the key and not the value.
/// let err = run("[security.users.bob]\npassword = \"hunter2\"".into(), Seen::default())
///     .await
///     .unwrap_err();
/// assert!(err.to_string().contains("security.users.bob.password"));
/// assert!(!format!("{err:?}").contains("hunter2"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[component]
pub struct PasswordEncoderAutoConfiguration;

impl Configuration for PasswordEncoderAutoConfiguration {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
            .try_bean(password_encoder)
            .provides(|e: Arc<DelegatingPasswordEncoder>| -> Arc<dyn PasswordEncoder> { e })
            .on_missing_bean::<dyn PasswordEncoder>()
            .try_bean(user_details_service)
            .provides(|s: Arc<InMemoryUserDetailsService>| -> Arc<dyn UserDetailsService> { s })
            .on_missing_bean::<dyn UserDetailsService>()
            .on_property("security.users")
            .bean(dao_authentication_provider)
            .provides(|p: Arc<DaoAuthenticationProvider>| -> Arc<dyn AuthenticationProvider> { p })
            .on_bean::<dyn UserDetailsService>()
            .on_bean::<dyn PasswordEncoder>()
    }
}

impl AutoConfiguration for PasswordEncoderAutoConfiguration {
    /// None: the encoder is useful on its own, and each bean has its own conditions.
    fn conditions() -> Conditions {
        Conditions::new()
    }

    fn properties() -> &'static [PropertyMetadata] {
        &PASSWORD
    }
}

/// The encoder of the `security.password` keys; the `ConfigError` stays the typed source.
fn password_encoder(config: Arc<Config>) -> Result<DelegatingPasswordEncoder, DiError> {
    let creation = DiError::creation_from::<DelegatingPasswordEncoder>;
    PasswordSettings::from_config(&config)
        .and_then(|settings| settings.encoder())
        .map_err(creation)
}

/// The store of the `security.users` users; the `ConfigError` stays the typed source.
fn user_details_service(config: Arc<Config>) -> Result<InMemoryUserDetailsService, DiError> {
    let users =
        users_from_config(&config).map_err(DiError::creation_from::<InMemoryUserDetailsService>)?;
    InMemoryUserDetailsService::new(users)
        .map_err(DiError::creation_from::<InMemoryUserDetailsService>)
}

/// The DAO provider over the user store and the encoder.
fn dao_authentication_provider(
    users: Arc<dyn UserDetailsService>,
    encoder: Arc<dyn PasswordEncoder>,
) -> DaoAuthenticationProvider {
    DaoAuthenticationProvider::new(users, encoder)
}

/// Exactly the keys `PasswordSettings::from_config`, `HttpBasicSettings::from_config` and
/// `users_from_config` read (pinned by a test).
static PASSWORD: [PropertyMetadata; 10] = [
    PropertyMetadata::new(
        "security.http-basic.enabled",
        "bool",
        Some("false"),
        "Whether the security chain accepts `Authorization: Basic` against the password \
         provider beans. Configured users do not open Basic by themselves; with `true` and no \
         `AuthenticationProvider` bean the startup fails.",
    ),
    PropertyMetadata::new(
        "security.http-basic.realm",
        "String",
        Some("Oxbloom"),
        "The realm of the `WWW-Authenticate: Basic` challenge; no double quote, backslash or \
         control character.",
    ),
    PropertyMetadata::new(
        "security.password.argon2.iterations",
        "u32",
        Some("2"),
        "The Argon2id iteration count for new hashes, 1 to 100.",
    ),
    PropertyMetadata::new(
        "security.password.argon2.memory-kib",
        "u32",
        Some("19456"),
        "The Argon2id memory cost for new hashes in KiB, 8 to 1048576 and at least 8 times \
         `parallelism`.",
    ),
    PropertyMetadata::new(
        "security.password.argon2.parallelism",
        "u32",
        Some("1"),
        "The Argon2id lane count for new hashes, 1 to 64.",
    ),
    PropertyMetadata::new(
        "security.password.default-encoder",
        "String",
        Some("argon2"),
        "The id new hashes are stored with. Only `argon2` exists.",
    ),
    PropertyMetadata::new(
        "security.users.<name>.enabled",
        "bool",
        Some("true"),
        "Whether the user may log in.",
    ),
    PropertyMetadata::new(
        "security.users.<name>.locked",
        "bool",
        Some("false"),
        "Whether the user is locked out.",
    ),
    PropertyMetadata::new(
        "security.users.<name>.password",
        "String",
        None,
        "The stored hash of the user `<name>`: `{argon2}` followed by an Argon2 PHC string, as \
         the password encoder produces it. Plain-text passwords are refused. Its value is \
         never printed. The name is compared exactly at login.",
    ),
    PropertyMetadata::new(
        "security.users.<name>.roles",
        "array of String",
        None,
        "The roles of the user, without the `ROLE_` prefix. Unset, the user has none.",
    ),
];
