//! The reader of the `security.csrf` keys.

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;

use super::CsrfConfig;
use crate::form_login::{is_safe_redirect_target, is_valid_field_name};

const SECTION: &str = "security.csrf";
const ENABLED: &str = "security.csrf.enabled";
const FIELD_NAME: &str = "security.csrf.field-name";
const HEADER_NAME: &str = "security.csrf.header-name";
const TOKEN_ENDPOINT: &str = "security.csrf.token-endpoint";

const NAME_RULE: &str = "must be non-empty and use only letters, digits, `_` and `-`";
const ENDPOINT_RULE: &str = "must be a local path that starts with `/`, with no scheme, no `//` start, no backslash and no control character";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawCsrf {
    enabled: Option<bool>,
    #[serde(rename = "field-name")]
    field_name: Option<String>,
    #[serde(rename = "header-name")]
    header_name: Option<String>,
    #[serde(rename = "token-endpoint")]
    token_endpoint: Option<String>,
}

/// The reader of the `security.csrf` settings.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::CsrfProperties;
///
/// let config = Config::from_str("[security.csrf]\ntoken-endpoint = \"/csrf\"")?;
/// let csrf = CsrfProperties::from_config(&config, true)?.expect("form login is on");
/// assert_eq!(csrf.token_endpoint.as_deref(), Some("/csrf"));
/// assert_eq!(csrf.field_name, "_csrf");
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct CsrfProperties;

impl CsrfProperties {
    /// Reads the `security.csrf` keys of `config`.
    ///
    /// The keys are `enabled`, `field-name` (default `_csrf`), `header-name` (`X-CSRF-TOKEN`)
    /// and `token-endpoint` (none). With form login on (`form_login_enabled`), the check
    /// follows form login: `enabled` defaults to `true`, and the result is always `Some`. With
    /// form login off the result is `None`, because CSRF protection belongs to form login and
    /// there is nothing to apply. A field or header name must be non-empty and use letters,
    /// digits, `_` and `-`; the token endpoint must be a local path (see
    /// [`is_safe_redirect_target`](crate::is_safe_redirect_target)). That the endpoint differs
    /// from the form login paths is checked later, by `HttpSecurity::build`.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key under `security.csrf`, a value of
    /// the wrong type, an invalid name or endpoint, `enabled = true` without form login, and
    /// any other `security.csrf` key without form login (the table would be ignored, so it is
    /// refused). The error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_security::CsrfProperties;
    ///
    /// // Nothing configured and no form login: nothing to apply.
    /// let config = Config::from_str("")?;
    /// assert!(CsrfProperties::from_config(&config, false)?.is_none());
    ///
    /// // `enabled = true` without form login is an error that names the key.
    /// let config = Config::from_str("[security.csrf]\nenabled = true")?;
    /// let err = CsrfProperties::from_config(&config, false).unwrap_err();
    /// assert!(err.to_string().contains("security.csrf.enabled"));
    ///
    /// // A bad name is an error that names the key and not the value.
    /// let config = Config::from_str("[security.csrf]\nfield-name = \"a b\"")?;
    /// let err = CsrfProperties::from_config(&config, true).unwrap_err();
    /// assert!(err.to_string().contains("security.csrf.field-name"));
    /// assert!(!err.to_string().contains("a b"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(
        config: &Config,
        form_login_enabled: bool,
    ) -> Result<Option<CsrfConfig>, ConfigError> {
        let raw = match config.get::<RawCsrf>(SECTION) {
            Ok(raw) => Some(raw),
            Err(ConfigError::Missing { key }) if key == SECTION => None,
            Err(e) => return Err(e),
        };
        if !form_login_enabled {
            let Some(raw) = raw else { return Ok(None) };
            if raw.enabled == Some(true) {
                return Err(ConfigError::invalid(
                    ENABLED,
                    "CSRF protection needs form login (`security.form-login.enabled`)",
                ));
            }
            let other = [
                (FIELD_NAME, raw.field_name.is_some()),
                (HEADER_NAME, raw.header_name.is_some()),
                (TOKEN_ENDPOINT, raw.token_endpoint.is_some()),
            ];
            if let Some((key, _)) = other.into_iter().find(|(_, set)| *set) {
                return Err(ConfigError::invalid(
                    key,
                    "CSRF protection needs form login (`security.form-login.enabled`)",
                ));
            }
            return Ok(None);
        }
        let raw = raw.unwrap_or_default();
        let mut csrf = CsrfConfig::default();
        if let Some(enabled) = raw.enabled {
            csrf.enabled = enabled;
        }
        let names = [
            (FIELD_NAME, raw.field_name, &mut csrf.field_name),
            (HEADER_NAME, raw.header_name, &mut csrf.header_name),
        ];
        for (key, value, slot) in names {
            if let Some(value) = value {
                if !is_valid_field_name(&value) {
                    return Err(ConfigError::invalid(key, NAME_RULE));
                }
                *slot = value;
            }
        }
        if let Some(endpoint) = raw.token_endpoint {
            if !is_safe_redirect_target(&endpoint) {
                return Err(ConfigError::invalid(TOKEN_ENDPOINT, ENDPOINT_RULE));
            }
            csrf.token_endpoint = Some(endpoint);
        }
        Ok(Some(csrf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(body: &str, form: bool) -> Result<Option<CsrfConfig>, ConfigError> {
        let toml = format!("[security.csrf]\n{body}\n");
        CsrfProperties::from_config(&Config::from_str(&toml).unwrap(), form)
    }

    fn rejects(body: &str, key: &str, value: &str) {
        let err = read(body, true).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(key), "{text}");
        assert!(!text.contains(value), "{text} echoes {value}");
        assert!(
            !format!("{err:?}").contains(value),
            "{err:?} echoes {value}"
        );
    }

    #[test]
    fn nothing_to_apply_without_form_login() {
        let empty = Config::from_str("").unwrap();
        assert_eq!(CsrfProperties::from_config(&empty, false).unwrap(), None);
        assert_eq!(read("enabled = false", false).unwrap(), None);
        assert_eq!(read("", false).unwrap(), None);
    }

    #[test]
    fn follows_form_login_by_default() {
        let empty = Config::from_str("").unwrap();
        assert_eq!(
            CsrfProperties::from_config(&empty, true).unwrap(),
            Some(CsrfConfig::default())
        );
        assert_eq!(read("", true).unwrap(), Some(CsrfConfig::default()));
    }

    #[test]
    fn every_key_is_read() {
        let csrf = read(
            "enabled = true\nfield-name = \"tok_en\"\nheader-name = \"X-Tok-1\"\n\
             token-endpoint = \"/csrf\"",
            true,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            csrf,
            CsrfConfig::default()
                .field_name("tok_en")
                .header_name("X-Tok-1")
                .token_endpoint("/csrf")
        );
    }

    #[test]
    fn disabled_with_form_login_keeps_the_other_keys() {
        let csrf = read("enabled = false\nfield-name = \"t\"", true)
            .unwrap()
            .unwrap();
        assert_eq!(csrf, CsrfConfig::default().enabled(false).field_name("t"));
    }

    #[test]
    fn unknown_key_is_rejected_naming_it() {
        let err = read("bogus-key = 1", true).unwrap_err().to_string();
        assert!(err.contains("security.csrf"), "{err}");
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn bad_names_and_endpoint_name_the_key_not_the_value() {
        rejects("field-name = \"a b\"", FIELD_NAME, "a b");
        rejects("field-name = \"\"", FIELD_NAME, "\"\"");
        rejects("header-name = \"x:y\"", HEADER_NAME, "x:y");
        rejects(
            "token-endpoint = \"https://evil.example\"",
            TOKEN_ENDPOINT,
            "evil",
        );
        rejects("token-endpoint = \"csrf\"", TOKEN_ENDPOINT, "csrf\"");
    }

    #[test]
    fn enabled_without_form_login_names_the_key() {
        let err = read("enabled = true", false).unwrap_err();
        assert!(err.to_string().contains(ENABLED), "{err}");
    }

    #[test]
    fn other_keys_without_form_login_are_refused_naming_the_key() {
        for (body, key, value) in [
            ("field-name = \"secret-name\"", FIELD_NAME, "secret-name"),
            ("header-name = \"X-Secret\"", HEADER_NAME, "X-Secret"),
            ("token-endpoint = \"/secret\"", TOKEN_ENDPOINT, "/secret"),
        ] {
            let text = read(body, false).unwrap_err().to_string();
            assert!(text.contains(key), "{text}");
            assert!(!text.contains(value), "{text}");
        }
    }

    #[test]
    fn wrong_types_are_rejected() {
        assert!(read("enabled = \"yes\"", true).is_err());
        assert!(read("field-name = 3", true).is_err());
    }
}
