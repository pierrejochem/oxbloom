//! The reader of the `security.form-login` keys.

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;

use super::redirect::is_safe_redirect_target;
use super::settings::{FormLogin, is_valid_field_name};

const SECTION: &str = "security.form-login";
const LOGIN_PAGE: &str = "security.form-login.login-page";
const LOGIN_PROCESSING_URL: &str = "security.form-login.login-processing-url";
const LOGOUT_URL: &str = "security.form-login.logout-url";
const DEFAULT_SUCCESS_URL: &str = "security.form-login.default-success-url";
const FAILURE_URL: &str = "security.form-login.failure-url";
const LOGOUT_SUCCESS_URL: &str = "security.form-login.logout-success-url";
const USERNAME_FIELD: &str = "security.form-login.username-field";
const PASSWORD_FIELD: &str = "security.form-login.password-field";

const URL_RULE: &str = "must be a local path that starts with `/`, with no scheme, no `//` start, no backslash and no control character";
const FIELD_RULE: &str = "must be non-empty and use only letters, digits, `_` and `-`";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFormLogin {
    enabled: Option<bool>,
    #[serde(rename = "login-page")]
    login_page: Option<String>,
    #[serde(rename = "login-processing-url")]
    login_processing_url: Option<String>,
    #[serde(rename = "logout-url")]
    logout_url: Option<String>,
    #[serde(rename = "default-success-url")]
    default_success_url: Option<String>,
    #[serde(rename = "failure-url")]
    failure_url: Option<String>,
    #[serde(rename = "logout-success-url")]
    logout_success_url: Option<String>,
    #[serde(rename = "username-field")]
    username_field: Option<String>,
    #[serde(rename = "password-field")]
    password_field: Option<String>,
    #[serde(rename = "login-page-enabled")]
    login_page_enabled: Option<bool>,
}

/// The reader of the `security.form-login` settings.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::FormLoginConfig;
///
/// let config = Config::from_str("[security.form-login]\nenabled = true")?;
/// let form = FormLoginConfig::from_config(&config)?.expect("form login is enabled");
/// assert_eq!(form.login_page, "/login");
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct FormLoginConfig;

impl FormLoginConfig {
    /// Reads the `security.form-login` keys of `config`.
    ///
    /// Returns `None` when `enabled` is `false` or the section is absent. Otherwise the keys
    /// are `login-page` (default `/login`), `login-processing-url` (`/login`), `logout-url`
    /// (`/logout`), `default-success-url` (`/`), `failure-url` (`/login?error`),
    /// `logout-success-url` (`/login?logout`), `username-field` (`username`), `password-field`
    /// (`password`) and `login-page-enabled` (`true`). A URL must be a local path (see
    /// [`is_safe_redirect_target`](crate::is_safe_redirect_target)); a field name must be
    /// non-empty and use letters, digits, `_` and `-`.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key under `security.form-login`, a value
    /// of the wrong type, a URL that is not a local path, an invalid field name, two equal
    /// field names, and a logout path equal to the login page or the login processing path.
    /// The error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_security::FormLoginConfig;
    ///
    /// let config = Config::from_str("[security.form-login]\nenabled = false")?;
    /// assert!(FormLoginConfig::from_config(&config)?.is_none());
    ///
    /// let config = Config::from_str(
    ///     "[security.form-login]\nenabled = true\nfailure-url = \"https://evil.example\"",
    /// )?;
    /// let err = FormLoginConfig::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.form-login.failure-url"));
    /// assert!(!err.to_string().contains("evil.example"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Option<FormLogin>, ConfigError> {
        let raw = match config.get::<RawFormLogin>(SECTION) {
            Ok(raw) => raw,
            Err(ConfigError::Missing { key }) if key == SECTION => return Ok(None),
            Err(e) => return Err(e),
        };
        if !raw.enabled.unwrap_or(false) {
            return Ok(None);
        }
        let mut form = FormLogin::default();
        let urls = [
            (LOGIN_PAGE, raw.login_page, &mut form.login_page),
            (
                LOGIN_PROCESSING_URL,
                raw.login_processing_url,
                &mut form.login_processing_url,
            ),
            (LOGOUT_URL, raw.logout_url, &mut form.logout_url),
            (
                DEFAULT_SUCCESS_URL,
                raw.default_success_url,
                &mut form.default_success_url,
            ),
            (FAILURE_URL, raw.failure_url, &mut form.failure_url),
            (
                LOGOUT_SUCCESS_URL,
                raw.logout_success_url,
                &mut form.logout_success_url,
            ),
        ];
        for (key, value, slot) in urls {
            if let Some(value) = value {
                if !is_safe_redirect_target(&value) {
                    return Err(ConfigError::invalid(key, URL_RULE));
                }
                *slot = value;
            }
        }
        let fields = [
            (USERNAME_FIELD, raw.username_field, &mut form.username_field),
            (PASSWORD_FIELD, raw.password_field, &mut form.password_field),
        ];
        for (key, value, slot) in fields {
            if let Some(value) = value {
                if !is_valid_field_name(&value) {
                    return Err(ConfigError::invalid(key, FIELD_RULE));
                }
                *slot = value;
            }
        }
        if let Some(enabled) = raw.login_page_enabled {
            form.login_page_enabled = enabled;
        }
        form.validate().map_err(|_| {
            ConfigError::invalid(
                SECTION,
                "the field names must differ and the logout url must differ from the login page \
                 and the login processing url",
            )
        })?;
        Ok(Some(form))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(body: &str) -> Result<Option<FormLogin>, ConfigError> {
        let toml = format!("[security.form-login]\n{body}\n");
        FormLoginConfig::from_config(&Config::from_str(&toml).unwrap())
    }

    fn on(body: &str) -> Result<Option<FormLogin>, ConfigError> {
        read(&format!("enabled = true\n{body}"))
    }

    fn rejects(body: &str, key: &str, value: &str) {
        let err = on(body).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(key), "{text}");
        assert!(!text.contains(value), "{text} echoes {value}");
        assert!(
            !format!("{err:?}").contains(value),
            "{err:?} echoes {value}"
        );
    }

    #[test]
    fn absent_or_disabled_gives_none() {
        let config = Config::from_str("").unwrap();
        assert_eq!(FormLoginConfig::from_config(&config).unwrap(), None);
        assert_eq!(read("enabled = false").unwrap(), None);
        assert_eq!(read("login-page = \"/signin\"").unwrap(), None);
    }

    #[test]
    fn defaults_when_only_enabled() {
        assert_eq!(on("").unwrap(), Some(FormLogin::default()));
    }

    #[test]
    fn every_key_is_read() {
        let form = on("login-page = \"/signin\"\n\
             login-processing-url = \"/do-login\"\n\
             logout-url = \"/signout\"\n\
             default-success-url = \"/home\"\n\
             failure-url = \"/signin?bad\"\n\
             logout-success-url = \"/signin?bye\"\n\
             username-field = \"user_name\"\n\
             password-field = \"pass-word\"\n\
             login-page-enabled = false")
        .unwrap()
        .unwrap();
        assert_eq!(
            form,
            FormLogin::default()
                .login_page("/signin")
                .login_processing_url("/do-login")
                .logout_url("/signout")
                .default_success_url("/home")
                .failure_url("/signin?bad")
                .logout_success_url("/signin?bye")
                .username_field("user_name")
                .password_field("pass-word")
                .login_page_enabled(false)
        );
    }

    #[test]
    fn unknown_key_is_rejected_naming_it() {
        let err = on("bogus-key = 1").unwrap_err().to_string();
        assert!(err.contains("security.form-login"), "{err}");
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn bad_urls_are_rejected_naming_the_key_not_the_value() {
        rejects("login-page = \"https://evil.example\"", LOGIN_PAGE, "evil");
        rejects(
            "login-processing-url = \"//evil.example\"",
            LOGIN_PROCESSING_URL,
            "evil",
        );
        rejects("logout-url = \"logout\"", LOGOUT_URL, "logout\"");
        rejects(
            "default-success-url = \"/a\\\\evil\"",
            DEFAULT_SUCCESS_URL,
            "evil",
        );
        rejects("failure-url = \"http://evil.example\"", FAILURE_URL, "evil");
        rejects("logout-success-url = \"\"", LOGOUT_SUCCESS_URL, "\"\"");
    }

    #[test]
    fn bad_field_names_are_rejected_naming_the_key_not_the_value() {
        rejects("username-field = \"\"", USERNAME_FIELD, "\"\"");
        rejects("password-field = \"pa ss=word\"", PASSWORD_FIELD, "pa ss");
        rejects("username-field = \"a&b\"", USERNAME_FIELD, "a&b");
    }

    #[test]
    fn clashing_settings_are_rejected() {
        let err = on("username-field = \"x\"\npassword-field = \"x\"").unwrap_err();
        assert!(err.to_string().contains(SECTION), "{err}");
        let err = on("logout-url = \"/login\"").unwrap_err();
        assert!(err.to_string().contains(SECTION), "{err}");
    }

    #[test]
    fn wrong_types_are_rejected() {
        assert!(read("enabled = \"yes\"").is_err());
        assert!(on("login-page-enabled = \"no\"").is_err());
    }
}
