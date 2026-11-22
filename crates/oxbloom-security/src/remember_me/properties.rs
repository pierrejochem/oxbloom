//! The reader of the `security.remember-me` keys.

use std::time::Duration;

use oxbloom_config::{Config, ConfigError, Secret};
use serde::Deserialize;

use super::settings::{DEFAULT_LIFETIME, MAX_LIFETIME, MIN_LIFETIME, MIN_SECRET_BYTES, RememberMe};
use crate::form_login::is_valid_field_name;
use crate::session::{is_valid_cookie_name, refuse_command_line};

const SECTION: &str = "security.remember-me";
const SECRET: &str = "security.remember-me.secret";
const OLD_SECRETS: &str = "security.remember-me.old-secrets";
const LIFETIME: &str = "security.remember-me.lifetime";
const FIELD_NAME: &str = "security.remember-me.field-name";
const COOKIE_NAME: &str = "security.remember-me.cookie-name";

const SECRET_RULE: &str = "must be at least 32 bytes";
const LIFETIME_RULE: &str = "must be between 1 minute and 365 days";
const FIELD_NAME_RULE: &str = "must be non-empty and made of ASCII letters, digits, `_` and `-`";
const COOKIE_NAME_RULE: &str =
    "must be a non-empty HTTP token (letters, digits and !#$%&'*+-.^_`|~)";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRememberMe {
    enabled: Option<bool>,
    secret: Option<Secret<String>>,
    #[serde(rename = "old-secrets")]
    old_secrets: Option<Vec<Secret<String>>>,
    lifetime: Option<Duration>,
    #[serde(rename = "field-name")]
    field_name: Option<String>,
    #[serde(rename = "cookie-name")]
    cookie_name: Option<String>,
}

/// The `security.remember-me` settings of an enabled remember-me, as
/// [`RememberMeProperties::from_config`] reads them.
///
/// The secret may be missing here: the caller decides what a missing secret means (boot fails
/// the startup naming `security.remember-me.secret`). `Debug` prints the secrets as
/// `[REDACTED]`.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_config::Config;
/// use oxbloom_security::RememberMeProperties;
///
/// let config = Config::from_str(
///     "[security.remember-me]\nenabled = true\nsecret = \"a-remember-me-secret-of-32-bytes\"",
/// )?;
/// let settings = RememberMeProperties::from_config(&config)?.expect("enabled");
/// assert_eq!(settings.lifetime, Duration::from_secs(14 * 24 * 3600));
/// assert_eq!(settings.cookie_name, "OXBLOOM_REMEMBER_ME");
/// assert!(!format!("{settings:?}").contains("a-remember-me-secret"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone)]
pub struct RememberMeSettings {
    /// `security.remember-me.secret`, the key new cookies are signed with; at least 32 bytes.
    pub secret: Option<Secret<String>>,
    /// `security.remember-me.old-secrets`, earlier keys whose cookies stay valid; each at least
    /// 32 bytes.
    pub old_secrets: Vec<Secret<String>>,
    /// `security.remember-me.lifetime`, how long a cookie stays valid from the time it is
    /// issued. Default 14 days, between 1 minute and 365 days.
    pub lifetime: Duration,
    /// `security.remember-me.field-name`, the login form field that asks for a cookie. Default
    /// `remember-me`.
    pub field_name: String,
    /// `security.remember-me.cookie-name`, the cookie name. Default `OXBLOOM_REMEMBER_ME`.
    pub cookie_name: String,
}

impl RememberMeSettings {
    /// Copies these settings into `remember_me`: the secret (when there is one), the old
    /// secrets, the lifetime, the field name and the cookie name. The user service and the
    /// clock stay as they are.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Config;
    /// use oxbloom_security::{
    ///     Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity,
    ///     InMemoryUserDetailsService, RememberMeProperties,
    /// };
    ///
    /// let config = Config::from_str(
    ///     "[security.remember-me]\nenabled = true\nlifetime = \"7d\"\n\
    ///      secret = \"a-remember-me-secret-of-32-bytes\"",
    /// )?;
    /// let settings = RememberMeProperties::from_config(&config)?.expect("enabled");
    /// let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
    /// let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
    /// let chain = HttpSecurity::new()
    ///     .password_provider(Arc::new(DaoAuthenticationProvider::new(users.clone(), encoder)))
    ///     .form_login(|f| f)
    ///     .remember_me(|r| settings.apply(r).user_details(users))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build();
    /// assert!(chain.is_ok());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn apply(self, remember_me: RememberMe) -> RememberMe {
        let remember_me = match self.secret {
            Some(secret) => remember_me.secret(secret),
            None => remember_me,
        };
        remember_me
            .old_secrets(self.old_secrets)
            .lifetime(self.lifetime)
            .field_name(self.field_name)
            .cookie_name(self.cookie_name)
    }
}

/// The reader of the `security.remember-me` settings.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::RememberMeProperties;
///
/// // Off unless `enabled = true`.
/// let config = Config::from_str("")?;
/// assert!(RememberMeProperties::from_config(&config)?.is_none());
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RememberMeProperties;

impl RememberMeProperties {
    /// Reads the `security.remember-me` keys of `config`.
    ///
    /// The keys are `enabled` (default `false`), `secret` and `old-secrets` (each at least 32
    /// bytes), `lifetime` (a duration such as `14d` or `12h`, between 1 minute and 365 days,
    /// default 14 days), `field-name` (ASCII letters, digits, `_` and `-`, default
    /// `remember-me`) and `cookie-name` (an HTTP token, default `OXBLOOM_REMEMBER_ME`).
    ///
    /// The result is `None` unless `enabled` is `true`. A disabled section is still checked in
    /// full: unknown keys, values of the wrong type and every value rule above are rejected
    /// whether or not remember-me is enabled; only a missing secret is accepted, since the
    /// secret is needed only when remember-me is on. `secret` and `old-secrets` are refused
    /// when they come from the command line, enabled or not.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key under `security.remember-me`, a
    /// value of the wrong type, a secret shorter than 32 bytes, a lifetime outside 1 minute to
    /// 365 days, a bad field or cookie name, and a secret from the command line. The error
    /// never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_security::RememberMeProperties;
    ///
    /// let config = Config::from_str("[security.remember-me]\nenabled = true\nlifetime = \"2h\"")?;
    /// let settings = RememberMeProperties::from_config(&config)?.expect("enabled");
    /// assert_eq!(settings.lifetime, Duration::from_secs(7200));
    /// assert!(settings.secret.is_none());
    ///
    /// // A short secret is an error that names the key and not the value.
    /// let config = Config::from_str("[security.remember-me]\nenabled = true\nsecret = \"short\"")?;
    /// let err = RememberMeProperties::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.remember-me.secret"));
    /// assert!(!format!("{err} {err:?}").contains("short\""));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Option<RememberMeSettings>, ConfigError> {
        refuse_command_line(config, SECRET)?;
        refuse_command_line(config, OLD_SECRETS)?;
        let raw = match config.get::<RawRememberMe>(SECTION) {
            Ok(raw) => raw,
            Err(ConfigError::Missing { key }) if key == SECTION => return Ok(None),
            Err(e) => return Err(e),
        };
        let old_secrets = raw.old_secrets.unwrap_or_default();
        for index in 0..old_secrets.len() {
            refuse_command_line(config, &format!("{OLD_SECRETS}[{index}]"))?;
        }
        // Every value is checked, enabled or not; only a missing secret is left to the caller.
        let long_enough = |s: &Secret<String>| s.expose().len() >= MIN_SECRET_BYTES;
        if raw.secret.as_ref().is_some_and(|s| !long_enough(s)) {
            return Err(ConfigError::invalid(SECRET, SECRET_RULE));
        }
        if let Some(index) = old_secrets.iter().position(|s| !long_enough(s)) {
            return Err(ConfigError::invalid(
                format!("{OLD_SECRETS}[{index}]"),
                SECRET_RULE,
            ));
        }
        let lifetime = raw.lifetime.unwrap_or(DEFAULT_LIFETIME);
        if !(MIN_LIFETIME..=MAX_LIFETIME).contains(&lifetime) {
            return Err(ConfigError::invalid(LIFETIME, LIFETIME_RULE));
        }
        let defaults = RememberMe::new();
        let field_name = raw.field_name.unwrap_or(defaults.field_name);
        if !is_valid_field_name(&field_name) {
            return Err(ConfigError::invalid(FIELD_NAME, FIELD_NAME_RULE));
        }
        let cookie_name = raw.cookie_name.unwrap_or(defaults.cookie_name);
        if !is_valid_cookie_name(&cookie_name) {
            return Err(ConfigError::invalid(COOKIE_NAME, COOKIE_NAME_RULE));
        }
        if !raw.enabled.unwrap_or(false) {
            return Ok(None);
        }
        Ok(Some(RememberMeSettings {
            secret: raw.secret,
            old_secrets,
            lifetime,
            field_name,
            cookie_name,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "a-remember-me-secret-of-32-bytes";

    fn read(body: &str) -> Result<Option<RememberMeSettings>, ConfigError> {
        let toml = format!("[security.remember-me]\n{body}\n");
        RememberMeProperties::from_config(&Config::from_str(&toml).unwrap())
    }

    fn enabled(body: &str) -> Result<Option<RememberMeSettings>, ConfigError> {
        read(&format!("enabled = true\n{body}"))
    }

    fn rejects(result: Result<Option<RememberMeSettings>, ConfigError>, key: &str, value: &str) {
        let err = result.expect_err("the value is refused");
        let text = format!("{err} {err:?}");
        assert!(text.contains(key), "{text}");
        assert!(!text.contains(value), "{text} echoes {value}");
    }

    #[test]
    fn off_without_the_section_or_enabled() {
        let empty = Config::from_str("").unwrap();
        assert!(RememberMeProperties::from_config(&empty).unwrap().is_none());
        assert!(read("").unwrap().is_none());
        assert!(
            read("enabled = false\nlifetime = \"1h\"")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_defaults_when_enabled() {
        let settings = enabled("").unwrap().unwrap();
        assert!(settings.secret.is_none());
        assert!(settings.old_secrets.is_empty());
        assert_eq!(settings.lifetime, DEFAULT_LIFETIME);
        assert_eq!(settings.field_name, "remember-me");
        assert_eq!(settings.cookie_name, "OXBLOOM_REMEMBER_ME");
    }

    #[test]
    fn every_key_is_read() {
        let settings = enabled(&format!(
            "secret = \"{KEY}\"\nold-secrets = [\"{KEY}-old\"]\nlifetime = \"2h\"\n\
             field-name = \"stay\"\ncookie-name = \"REMEMBER\""
        ))
        .unwrap()
        .unwrap();
        assert_eq!(settings.secret.unwrap().expose(), KEY);
        assert_eq!(settings.old_secrets.len(), 1);
        assert_eq!(settings.old_secrets[0].expose(), &format!("{KEY}-old"));
        assert_eq!(settings.lifetime, Duration::from_secs(7200));
        assert_eq!(settings.field_name, "stay");
        assert_eq!(settings.cookie_name, "REMEMBER");
    }

    #[test]
    fn bound_edges() {
        assert!(enabled("lifetime = \"1m\"").is_ok());
        assert!(enabled("lifetime = \"365d\"").is_ok());
        rejects(enabled("lifetime = \"59s\""), LIFETIME, "59");
        rejects(enabled("lifetime = \"366d\""), LIFETIME, "366");
        rejects(enabled("lifetime = \"0s\""), LIFETIME, "0s");
        assert!(enabled(&format!("secret = \"{}\"", "s".repeat(32))).is_ok());
        rejects(
            enabled(&format!("secret = \"{}\"", "s".repeat(31))),
            SECRET,
            &"s".repeat(31),
        );
        rejects(
            enabled(&format!("old-secrets = [\"{KEY}\", \"short-old-secret\"]")),
            "security.remember-me.old-secrets[1]",
            "short-old-secret",
        );
    }

    #[test]
    fn bad_names_fail_naming_the_key() {
        rejects(enabled("field-name = \"bad name\""), FIELD_NAME, "bad name");
        rejects(enabled("field-name = \"\""), FIELD_NAME, "\"\"");
        rejects(
            enabled("cookie-name = \"bad;name\""),
            COOKIE_NAME,
            "bad;name",
        );
        rejects(enabled("cookie-name = \"\""), COOKIE_NAME, "\"\"");
        rejects(enabled("lifetime = \"forever\""), LIFETIME, "forever");
        rejects(
            read("enabled = \"sometimes\""),
            "security.remember-me.enabled",
            "sometimes",
        );
    }

    #[test]
    fn a_disabled_section_is_validated_too() {
        let err = read("bogus-key = 1").unwrap_err();
        assert!(err.to_string().contains(SECTION), "{err}");
        assert!(err.to_string().contains("bogus"), "{err}");
        assert!(read("enabled = false\nlifetime = true").is_err());
        assert!(read("enabled = false\ncookie-name = { a = 1 }").is_err());
        // Every value is checked, enabled or not, without echoing it.
        for prefix in ["enabled = false\n", ""] {
            let disabled = |body: &str| read(&format!("{prefix}{body}"));
            rejects(
                disabled("secret = \"short-secret-value\""),
                SECRET,
                "short-secret-value",
            );
            rejects(
                disabled(&format!("old-secrets = [\"{KEY}\", \"short-old-value\"]")),
                "security.remember-me.old-secrets[1]",
                "short-old-value",
            );
            rejects(disabled("lifetime = \"366d\""), LIFETIME, "366d");
            rejects(disabled("lifetime = \"0s\""), LIFETIME, "0s");
            rejects(
                disabled("field-name = \"bad name\""),
                FIELD_NAME,
                "bad name",
            );
            rejects(
                disabled("cookie-name = \"bad;name\""),
                COOKIE_NAME,
                "bad;name",
            );
        }
        // A fully valid disabled section is `None`, and a missing secret is fine while
        // disabled.
        assert!(
            read(&format!(
                "enabled = false\nsecret = \"{KEY}\"\nold-secrets = [\"{KEY}\"]\n\
                 lifetime = \"1h\"\nfield-name = \"stay\"\ncookie-name = \"KEEP\""
            ))
            .unwrap()
            .is_none()
        );
        assert!(
            read("enabled = false\nlifetime = \"1h\"")
                .unwrap()
                .is_none()
        );
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
                format!("--security.remember-me.secret={KEY}"),
                SECRET.to_owned(),
            ),
            (
                format!("--security.remember-me.old-secrets[0]={KEY}"),
                format!("{OLD_SECRETS}[0]"),
            ),
        ] {
            let err = RememberMeProperties::from_config(&build(&arg)).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { key: k, .. } if *k == key),
                "{err:?}"
            );
            assert!(err.to_string().contains("command line"), "{err}");
            assert!(!format!("{err} {err:?}").contains(KEY));
        }
    }

    #[test]
    fn debug_never_shows_a_secret() {
        let settings = enabled(&format!(
            "secret = \"{KEY}\"\nold-secrets = [\"{KEY}-old\"]"
        ))
        .unwrap()
        .unwrap();
        let debug = format!("{settings:?}");
        assert!(!debug.contains(KEY), "{debug}");
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn apply_copies_every_setting() {
        let settings = enabled(&format!(
            "secret = \"{KEY}\"\nold-secrets = [\"{KEY}-old\"]\nlifetime = \"2h\"\n\
             field-name = \"stay\"\ncookie-name = \"REMEMBER\""
        ))
        .unwrap()
        .unwrap();
        let r = settings.apply(RememberMe::new());
        assert_eq!(r.secret.as_ref().unwrap().expose(), KEY);
        assert_eq!(r.old_secrets.len(), 1);
        assert_eq!(r.lifetime, Duration::from_secs(7200));
        assert_eq!(r.field_name, "stay");
        assert_eq!(r.cookie_name, "REMEMBER");
        // Without a secret the builder keeps none.
        let r = enabled("").unwrap().unwrap().apply(RememberMe::new());
        assert!(r.secret.is_none());
    }
}
