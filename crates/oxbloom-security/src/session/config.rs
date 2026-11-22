//! The reader of the `security.session` keys.

use std::time::Duration;

use oxbloom_config::{Config, ConfigError, Origin, Secret};
use serde::Deserialize;

use super::settings::{SameSite, SecureMode, SessionSettings};

const SECTION: &str = "security.session";
const STORE: &str = "security.session.store";
const COOKIE_NAME: &str = "security.session.cookie-name";
const SECURE: &str = "security.session.secure";
const SAME_SITE: &str = "security.session.same-site";
const IDLE_TIMEOUT: &str = "security.session.idle-timeout";
const ABSOLUTE_TIMEOUT: &str = "security.session.absolute-timeout";
const MAX_SESSIONS: &str = "security.session.max-sessions";
const MAX_COOKIE_BYTES: &str = "security.session.max-cookie-bytes";

/// The configuration key of the session encryption key, `security.session.secret`.
///
/// Like [`SECRET_KEY`](crate::SECRET_KEY), it is refused on the command line (arguments show up
/// in process listings): set it in a file or an environment variable
/// (`APP_SECURITY__SESSION__SECRET`).
///
/// # Examples
///
/// ```
/// use oxbloom_security::SESSION_SECRET_KEY;
///
/// assert_eq!(SESSION_SECRET_KEY, "security.session.secret");
/// ```
pub const SESSION_SECRET_KEY: &str = "security.session.secret";

/// The configuration key of the earlier session keys, `security.session.old-secrets`.
///
/// Refused on the command line like [`SESSION_SECRET_KEY`].
///
/// # Examples
///
/// ```
/// use oxbloom_security::SESSION_OLD_SECRETS_KEY;
///
/// assert_eq!(SESSION_OLD_SECRETS_KEY, "security.session.old-secrets");
/// ```
pub const SESSION_OLD_SECRETS_KEY: &str = "security.session.old-secrets";

/// The shortest secret, in bytes, the cookie backend accepts.
const MIN_SECRET_BYTES: usize = 32;
const MIN_COOKIE_BYTES: usize = 256;
const MIN_IDLE_TIMEOUT: Duration = Duration::from_secs(1);
const CLI_REFUSED: &str = "secret keys cannot be set on the command line; use an environment variable or a \
     configuration file";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawSession {
    store: Option<String>,
    #[serde(rename = "cookie-name")]
    cookie_name: Option<String>,
    secure: Option<String>,
    #[serde(rename = "same-site")]
    same_site: Option<String>,
    #[serde(rename = "idle-timeout")]
    idle_timeout: Option<Duration>,
    #[serde(rename = "absolute-timeout")]
    absolute_timeout: Option<Duration>,
    #[serde(rename = "max-sessions")]
    max_sessions: Option<usize>,
    #[serde(rename = "max-cookie-bytes")]
    max_cookie_bytes: Option<usize>,
    secret: Option<Secret<String>>,
    #[serde(rename = "old-secrets")]
    old_secrets: Option<Vec<Secret<String>>>,
    // Read by `SessionManagementProperties`; accepted here so they are not unknown keys.
    #[serde(rename = "max-sessions-per-user")]
    _max_sessions_per_user: Option<serde::de::IgnoredAny>,
    #[serde(rename = "revalidate-interval")]
    _revalidate_interval: Option<serde::de::IgnoredAny>,
}

/// Where the sessions of `security.session.store` live.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::{SessionConfig, StoreKind};
///
/// let config = Config::from_str("[security.session]\nstore = \"memory\"")?;
/// assert_eq!(SessionConfig::from_config(&config)?.store, StoreKind::Memory);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// A server-side [`SessionStore`](crate::SessionStore): the in-memory one, or the
    /// application's own store bean. The default.
    Memory,
    /// The stateless [`CookieSessionBackend`](crate::CookieSessionBackend); needs
    /// `security.session.secret`.
    Cookie,
}

/// The `security.session` settings: the backend, the cookie and lifetime settings and the keys
/// of the cookie backend.
///
/// `Debug` prints the secrets as `[REDACTED]`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::{SessionConfig, StoreKind};
///
/// let config = Config::from_str(
///     "[security.session]\nstore = \"cookie\"\nsecret = \"an-example-secret-of-32-bytes!!!\"",
/// )?;
/// let session = SessionConfig::from_config(&config)?;
/// assert_eq!(session.store, StoreKind::Cookie);
/// assert!(!format!("{session:?}").contains("an-example"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// `security.session.store`. Default [`StoreKind::Memory`].
    pub store: StoreKind,
    /// The cookie and lifetime settings, validated with [`SessionSettings::validate`].
    pub settings: SessionSettings,
    /// `security.session.secret`, the key new cookies are sealed with. Required with
    /// [`StoreKind::Cookie`]; at least 32 bytes.
    pub secret: Option<Secret<String>>,
    /// `security.session.old-secrets`, the earlier keys that may still open a cookie; each at
    /// least 32 bytes.
    pub old_secrets: Vec<Secret<String>>,
}

impl SessionConfig {
    /// Reads the `security.session` keys of `config`.
    ///
    /// The keys are `store` (`memory` or `cookie`, default `memory`), `cookie-name` (an HTTP
    /// token, default `OXBLOOM_SESSION`), `secure` (`auto`, `always` or `never`, default
    /// `auto`), `same-site` (`lax`, `strict` or `none`, default `lax`; `none` needs `secure =
    /// "always"`), `idle-timeout` (at least one second, at most `absolute-timeout`, default
    /// 30 minutes), `absolute-timeout` (positive, default 8 hours), `max-sessions` (at least 1,
    /// default 100 000), `max-cookie-bytes` (at least 256, default 3072), `secret` and
    /// `old-secrets` (each at least 32 bytes; `secret` is required with `store = "cookie"`).
    /// The keys `max-sessions-per-user` and `revalidate-interval` are accepted and left to
    /// [`SessionManagementProperties`](crate::SessionManagementProperties).
    /// Durations take the configuration's duration form, such as `90s` or `30m`. A missing
    /// section gives the defaults.
    ///
    /// `secret` and `old-secrets` are refused when they come from the command line, whether
    /// or not the keys were registered with `ConfigBuilder::secret_key`.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key under `security.session`, a value of
    /// the wrong type, a bad `store`, `secure` or `same-site`, a cookie name that is not an HTTP
    /// token, `same-site = "none"` without `secure = "always"`, a sub-second or reversed
    /// timeout, a zero absolute timeout, `max-sessions = 0`, `max-cookie-bytes` below 256, a
    /// secret shorter than 32 bytes, `store = "cookie"` without a secret, and a secret from the
    /// command line. The error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_security::SessionConfig;
    ///
    /// let config = Config::from_str("[security.session]\nidle-timeout = \"10m\"")?;
    /// let session = SessionConfig::from_config(&config)?;
    /// assert_eq!(session.settings.idle_timeout, Duration::from_secs(600));
    ///
    /// let config = Config::from_str("[security.session]\nstore = \"cookie\"")?;
    /// let err = SessionConfig::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.session.secret"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<SessionConfig, ConfigError> {
        refuse_command_line(config, SESSION_SECRET_KEY)?;
        refuse_command_line(config, SESSION_OLD_SECRETS_KEY)?;
        let raw = match config.get::<RawSession>(SECTION) {
            Ok(raw) => raw,
            Err(ConfigError::Missing { key }) if key == SECTION => RawSession::default(),
            Err(e) => return Err(e),
        };
        let old_secrets = raw.old_secrets.unwrap_or_default();
        for index in 0..old_secrets.len() {
            refuse_command_line(config, &format!("{SESSION_OLD_SECRETS_KEY}[{index}]"))?;
        }

        let store = match raw.store.as_deref() {
            None | Some("memory") => StoreKind::Memory,
            Some("cookie") => StoreKind::Cookie,
            Some(_) => return Err(ConfigError::invalid(STORE, "must be `memory` or `cookie`")),
        };
        let defaults = SessionSettings::default();
        let cookie_name = raw
            .cookie_name
            .unwrap_or_else(|| defaults.cookie_name.clone());
        if SessionSettings::default()
            .cookie_name(cookie_name.as_str())
            .validate()
            .is_err()
        {
            return Err(ConfigError::invalid(
                COOKIE_NAME,
                "must be a non-empty HTTP token (letters, digits and !#$%&'*+-.^_`|~)",
            ));
        }
        let secure = match raw.secure.as_deref() {
            None => defaults.secure,
            Some(text) => text
                .parse::<SecureMode>()
                .map_err(|_| ConfigError::invalid(SECURE, "must be `auto`, `always` or `never`"))?,
        };
        let same_site = match raw.same_site.as_deref() {
            None => defaults.same_site,
            Some(text) => text.parse::<SameSite>().map_err(|_| {
                ConfigError::invalid(SAME_SITE, "must be `lax`, `strict` or `none`")
            })?,
        };
        if same_site == SameSite::None && secure != SecureMode::Always {
            return Err(ConfigError::invalid(
                SAME_SITE,
                "`none` needs `security.session.secure = \"always\"`",
            ));
        }
        let absolute_timeout = raw.absolute_timeout.unwrap_or(defaults.absolute_timeout);
        if absolute_timeout.is_zero() {
            return Err(ConfigError::invalid(ABSOLUTE_TIMEOUT, "must be positive"));
        }
        let idle_timeout = raw.idle_timeout.unwrap_or(defaults.idle_timeout);
        if idle_timeout < MIN_IDLE_TIMEOUT {
            return Err(ConfigError::invalid(
                IDLE_TIMEOUT,
                "must be at least one second",
            ));
        }
        if idle_timeout > absolute_timeout {
            return Err(ConfigError::invalid(
                IDLE_TIMEOUT,
                "must not be longer than security.session.absolute-timeout",
            ));
        }
        let max_sessions = raw.max_sessions.unwrap_or(defaults.max_sessions);
        if max_sessions == 0 {
            return Err(ConfigError::invalid(MAX_SESSIONS, "must be at least 1"));
        }
        let max_cookie_bytes = raw.max_cookie_bytes.unwrap_or(defaults.max_cookie_bytes);
        if max_cookie_bytes < MIN_COOKIE_BYTES {
            return Err(ConfigError::invalid(
                MAX_COOKIE_BYTES,
                "must be at least 256",
            ));
        }
        if raw
            .secret
            .as_ref()
            .is_some_and(|s| s.expose().len() < MIN_SECRET_BYTES)
        {
            return Err(ConfigError::invalid(
                SESSION_SECRET_KEY,
                "must be at least 32 bytes",
            ));
        }
        if let Some(index) = old_secrets
            .iter()
            .position(|s| s.expose().len() < MIN_SECRET_BYTES)
        {
            return Err(ConfigError::invalid(
                format!("{SESSION_OLD_SECRETS_KEY}[{index}]"),
                "must be at least 32 bytes",
            ));
        }
        if store == StoreKind::Cookie && raw.secret.is_none() {
            return Err(ConfigError::invalid(
                SESSION_SECRET_KEY,
                "is required with `security.session.store = \"cookie\"`",
            ));
        }

        let settings = SessionSettings {
            cookie_name,
            secure,
            same_site,
            idle_timeout,
            absolute_timeout,
            max_sessions,
            max_cookie_bytes,
            ..defaults
        };
        // Every rule above has its own message; this keeps the reader and `validate` in step.
        settings
            .validate()
            .map_err(|_| ConfigError::invalid(SECTION, "the session settings are not valid"))?;
        Ok(SessionConfig {
            store,
            settings,
            secret: raw.secret,
            old_secrets,
        })
    }
}

/// An error when the leaf `key` was set on the command line.
fn refuse_command_line(config: &Config, key: &str) -> Result<(), ConfigError> {
    match config.origin(key) {
        Some(origin @ Origin::Cli { .. }) => Err(ConfigError::Invalid {
            key: key.to_owned(),
            message: CLI_REFUSED.to_owned(),
            origin: Some(origin.clone()),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "an-example-secret-of-32-bytes!!!";

    fn read(toml: &str) -> Result<SessionConfig, ConfigError> {
        SessionConfig::from_config(&Config::from_str(toml).unwrap())
    }

    fn section(body: &str) -> Result<SessionConfig, ConfigError> {
        read(&format!("[security.session]\n{body}\n"))
    }

    /// The key of an `Invalid` error.
    fn key_of(result: Result<SessionConfig, ConfigError>) -> String {
        match result {
            Err(ConfigError::Invalid { key, .. }) => key,
            other => panic!("expected an invalid key, got {other:?}"),
        }
    }

    /// The error text of `body`, checked to name `key` and not to contain `value`.
    fn rejects(body: &str, key: &str, value: &str) {
        let err = section(body).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(key), "{text}");
        assert!(!text.contains(value), "{text} echoes {value}");
        assert!(
            !format!("{err:?}").contains(value),
            "{err:?} echoes {value}"
        );
    }

    #[test]
    fn defaults_when_only_the_store_is_set() {
        let session = section("store = \"memory\"").unwrap();
        assert_eq!(session.store, StoreKind::Memory);
        assert_eq!(session.settings, SessionSettings::default());
        assert!(session.secret.is_none());
        assert!(session.old_secrets.is_empty());
        // A missing section gives the same.
        let session = read("").unwrap();
        assert_eq!(session.store, StoreKind::Memory);
        assert_eq!(session.settings, SessionSettings::default());
    }

    #[test]
    fn every_key_is_read() {
        let session = section(&format!(
            "store = \"cookie\"\ncookie-name = \"SID\"\nsecure = \"always\"\n\
             same-site = \"none\"\nidle-timeout = \"10m\"\nabsolute-timeout = \"2h\"\n\
             max-sessions = 10\nmax-cookie-bytes = 4000\nsecret = \"{SECRET}\"\n\
             old-secrets = [\"{SECRET}-old\"]"
        ))
        .unwrap();
        assert_eq!(session.store, StoreKind::Cookie);
        let expected = SessionSettings::default()
            .cookie_name("SID")
            .secure(SecureMode::Always)
            .same_site(SameSite::None)
            .idle_timeout(Duration::from_secs(600))
            .absolute_timeout(Duration::from_secs(7200))
            .max_sessions(10)
            .max_cookie_bytes(4000);
        assert_eq!(session.settings, expected);
        assert_eq!(session.secret.unwrap().expose(), SECRET);
        assert_eq!(session.old_secrets.len(), 1);
        assert_eq!(session.old_secrets[0].expose(), &format!("{SECRET}-old"));
    }

    #[test]
    fn bound_edges() {
        assert!(section("max-sessions = 1").is_ok());
        rejects("max-sessions = 0", MAX_SESSIONS, "= 0");
        assert!(section("max-cookie-bytes = 256").is_ok());
        rejects("max-cookie-bytes = 255", MAX_COOKIE_BYTES, "255");
        assert!(section("idle-timeout = \"1s\"").is_ok());
        rejects("idle-timeout = \"999ms\"", IDLE_TIMEOUT, "999");
        rejects("idle-timeout = \"0s\"", IDLE_TIMEOUT, "0s");
        rejects("absolute-timeout = \"0s\"", ABSOLUTE_TIMEOUT, "0s");
        // Equal timeouts pass, a reversed pair names the idle timeout.
        assert!(section("idle-timeout = \"1h\"\nabsolute-timeout = \"1h\"").is_ok());
        rejects(
            "idle-timeout = \"2h\"\nabsolute-timeout = \"1h\"",
            IDLE_TIMEOUT,
            "2h",
        );
        // The secret is at least 32 bytes.
        assert!(section(&format!("secret = \"{}\"", "s".repeat(32))).is_ok());
        rejects(
            &format!("secret = \"{}\"", "s".repeat(31)),
            SESSION_SECRET_KEY,
            &"s".repeat(31),
        );
        rejects(
            &format!("old-secrets = [\"{SECRET}\", \"short-old-secret\"]"),
            "security.session.old-secrets[1]",
            "short-old-secret",
        );
    }

    #[test]
    fn bad_names_fail_naming_the_key() {
        rejects("store = \"redis-secret-host\"", STORE, "redis-secret-host");
        rejects("secure = \"sometimes\"", SECURE, "sometimes");
        rejects("same-site = \"Lax\"", SAME_SITE, "\"Lax\"");
        rejects("cookie-name = \"bad;name\"", COOKIE_NAME, "bad;name");
        rejects("cookie-name = \"\"", COOKIE_NAME, "\"\"");
        rejects("max-sessions = \"many\"", MAX_SESSIONS, "many");
        rejects("idle-timeout = \"forever\"", IDLE_TIMEOUT, "forever");
    }

    #[test]
    fn session_management_keys_are_accepted_and_left_alone() {
        let session = section("max-sessions-per-user = 2\nrevalidate-interval = \"1m\"").unwrap();
        assert_eq!(session.settings, SessionSettings::default());
    }

    #[test]
    fn unknown_key_is_rejected_naming_it() {
        let err = section("store = \"memory\"\nbogus-key = 1").unwrap_err();
        assert!(err.to_string().contains("security.session"), "{err}");
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn same_site_none_needs_secure_always() {
        assert_eq!(key_of(section("same-site = \"none\"")), SAME_SITE);
        assert_eq!(
            key_of(section("same-site = \"none\"\nsecure = \"auto\"")),
            SAME_SITE
        );
        assert!(section("same-site = \"none\"\nsecure = \"always\"").is_ok());
    }

    #[test]
    fn cookie_store_needs_a_secret() {
        assert_eq!(
            key_of(section("store = \"cookie\"")),
            SESSION_SECRET_KEY.to_owned()
        );
        assert!(section(&format!("store = \"cookie\"\nsecret = \"{SECRET}\"")).is_ok());
        // The memory store does not need one.
        assert!(section("store = \"memory\"").is_ok());
    }

    #[test]
    fn secrets_from_the_command_line_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let build = |arg: &str| {
            Config::builder()
                .dir(dir.path())
                .env_from([])
                .args([arg.to_owned()])
                .build()
                .unwrap()
        };
        for (arg, key) in [
            (
                format!("--security.session.secret={SECRET}"),
                SESSION_SECRET_KEY.to_owned(),
            ),
            (
                format!("--security.session.old-secrets[0]={SECRET}"),
                format!("{SESSION_OLD_SECRETS_KEY}[0]"),
            ),
        ] {
            let err = SessionConfig::from_config(&build(&arg)).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { key: k, .. } if *k == key),
                "{err:?}"
            );
            assert!(err.to_string().contains("command line"), "{err}");
            assert!(!format!("{err:?}").contains(SECRET));
        }
        // The same secret from a file is accepted.
        assert!(section(&format!("secret = \"{SECRET}\"")).is_ok());
    }

    #[test]
    fn debug_never_shows_a_secret() {
        let session = section(&format!(
            "secret = \"{SECRET}\"\nold-secrets = [\"{SECRET}-old\"]"
        ))
        .unwrap();
        let debug = format!("{session:?}");
        assert!(!debug.contains(SECRET), "{debug}");
        assert!(debug.contains("[REDACTED]"));
    }
}
