//! The reader of the `security.headers` keys.

use std::time::Duration;

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;

use super::config::{HeadersConfig, value_ok};

const SECTION: &str = "security.headers";
const CONTENT_TYPE_OPTIONS: &str = "security.headers.content-type-options";
const FRAME_OPTIONS: &str = "security.headers.frame-options";
const REFERRER_POLICY: &str = "security.headers.referrer-policy";
const CACHE_CONTROL: &str = "security.headers.cache-control";
const HSTS_MAX_AGE: &str = "security.headers.hsts.max-age";

/// The string value that removes a header.
const OFF: &str = "off";
const VALUE_RULE: &str =
    "must be `off` or a non-empty header value of visible ASCII characters and spaces";
const MAX_AGE_RULE: &str = "must be at least one second";

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawHeaders {
    enabled: Option<bool>,
    #[serde(rename = "content-type-options")]
    content_type_options: Option<String>,
    #[serde(rename = "frame-options")]
    frame_options: Option<String>,
    #[serde(rename = "referrer-policy")]
    referrer_policy: Option<String>,
    #[serde(rename = "cache-control")]
    cache_control: Option<String>,
    hsts: RawHsts,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawHsts {
    enabled: Option<bool>,
    #[serde(rename = "max-age")]
    max_age: Option<Duration>,
    #[serde(rename = "include-subdomains")]
    include_subdomains: Option<bool>,
    preload: Option<bool>,
}

/// The reader of the `security.headers` settings.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::HeadersProperties;
///
/// let config = Config::from_str("[security.headers]\nframe-options = \"SAMEORIGIN\"")?;
/// let headers = HeadersProperties::from_config(&config)?;
/// assert_eq!(headers.frame_options.as_deref(), Some("SAMEORIGIN"));
/// assert_eq!(headers.referrer_policy.as_deref(), Some("no-referrer"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Copy)]
pub struct HeadersProperties;

impl HeadersProperties {
    /// Reads the `security.headers` keys of `config` into a [`HeadersConfig`].
    ///
    /// The keys are `enabled` (default `true`), `content-type-options` (`nosniff`),
    /// `frame-options` (`DENY`), `referrer-policy` (`no-referrer`), `cache-control`
    /// (`no-store`, added only to an authenticated response) and, under `hsts`, `enabled`
    /// (`true`), `max-age` (a duration, default one year, at least one second),
    /// `include-subdomains` (`true`) and `preload` (`false`). The string `off` removes a
    /// header; `enabled = false` removes all of them, `hsts.enabled = false` removes HSTS. A
    /// missing section gives the defaults.
    ///
    /// # Errors
    ///
    /// A [`ConfigError`] naming the key for an unknown key under `security.headers` or
    /// `security.headers.hsts`, a value of the wrong type, a header value that is empty or has
    /// a character outside visible ASCII and space, and an HSTS max age under one second. The
    /// error never shows the configured value.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_config::Config;
    /// use oxbloom_security::{HeadersConfig, HeadersProperties};
    ///
    /// // Nothing configured: the defaults.
    /// let config = Config::from_str("")?;
    /// assert_eq!(HeadersProperties::from_config(&config)?, HeadersConfig::default());
    ///
    /// // `off` removes a header; the HSTS table tunes the HSTS header.
    /// let config = Config::from_str(
    ///     "[security.headers]\nreferrer-policy = \"off\"\n\
    ///      [security.headers.hsts]\nmax-age = \"1h\"\npreload = true",
    /// )?;
    /// let headers = HeadersProperties::from_config(&config)?;
    /// assert_eq!(headers.referrer_policy, None);
    /// assert_eq!(headers.hsts.max_age, Duration::from_secs(3600));
    /// assert!(headers.hsts.preload);
    ///
    /// // A bad value is an error that names the key and not the value.
    /// let config = Config::from_str("[security.headers]\nframe-options = \"\"")?;
    /// let err = HeadersProperties::from_config(&config).unwrap_err();
    /// assert!(err.to_string().contains("security.headers.frame-options"));
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<HeadersConfig, ConfigError> {
        let raw = match config.get::<RawHeaders>(SECTION) {
            Ok(raw) => raw,
            Err(ConfigError::Missing { key }) if key == SECTION => RawHeaders::default(),
            Err(e) => return Err(e),
        };
        let mut headers = HeadersConfig::default();
        if let Some(enabled) = raw.enabled {
            headers.enabled = enabled;
        }
        let values = [
            (
                CONTENT_TYPE_OPTIONS,
                raw.content_type_options,
                &mut headers.content_type_options,
            ),
            (FRAME_OPTIONS, raw.frame_options, &mut headers.frame_options),
            (
                REFERRER_POLICY,
                raw.referrer_policy,
                &mut headers.referrer_policy,
            ),
            (CACHE_CONTROL, raw.cache_control, &mut headers.cache_control),
        ];
        for (key, value, slot) in values {
            if let Some(value) = value {
                *slot = header_value(key, value)?;
            }
        }
        let hsts = raw.hsts;
        if let Some(enabled) = hsts.enabled {
            headers.hsts.enabled = enabled;
        }
        if let Some(max_age) = hsts.max_age {
            if max_age < Duration::from_secs(1) {
                return Err(ConfigError::invalid(HSTS_MAX_AGE, MAX_AGE_RULE));
            }
            headers.hsts.max_age = max_age;
        }
        if let Some(include_subdomains) = hsts.include_subdomains {
            headers.hsts.include_subdomains = include_subdomains;
        }
        if let Some(preload) = hsts.preload {
            headers.hsts.preload = preload;
        }
        Ok(headers)
    }
}

/// `None` for `off`, the value when it is a valid header value, otherwise an error naming `key`.
fn header_value(key: &str, value: String) -> Result<Option<String>, ConfigError> {
    if value.eq_ignore_ascii_case(OFF) {
        return Ok(None);
    }
    let value = Some(value);
    if value_ok(&value) {
        Ok(value)
    } else {
        Err(ConfigError::invalid(key, VALUE_RULE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(toml: &str) -> Result<HeadersConfig, ConfigError> {
        HeadersProperties::from_config(&Config::from_str(toml).unwrap())
    }

    fn rejects(toml: &str, key: &str, value: &str) {
        let err = read(toml).unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(text.contains(key), "{text}");
        assert!(!text.contains(value), "{text} echoes {value}");
    }

    #[test]
    fn defaults_without_the_section() {
        assert_eq!(read("").unwrap(), HeadersConfig::default());
        assert_eq!(
            read("[security.headers]").unwrap(),
            HeadersConfig::default()
        );
    }

    #[test]
    fn off_removes_each_header() {
        let headers = read(
            "[security.headers]\ncontent-type-options = \"off\"\nframe-options = \"off\"\n\
             referrer-policy = \"off\"\ncache-control = \"off\"",
        )
        .unwrap();
        assert_eq!(
            headers,
            HeadersConfig::default()
                .content_type_options(None)
                .frame_options(None)
                .referrer_policy(None)
                .cache_control(None)
        );
        // The word `off` turns a header off in any letter case.
        for word in ["OFF", "Off"] {
            let headers = read(&format!("[security.headers]\nframe-options = \"{word}\"")).unwrap();
            assert_eq!(headers.frame_options, None, "{word}");
        }
    }

    #[test]
    fn switches_are_read() {
        assert!(!read("[security.headers]\nenabled = false").unwrap().enabled);
        let headers = read(
            "[security.headers.hsts]\nenabled = false\ninclude-subdomains = false\n\
             preload = true\nmax-age = \"1s\"",
        )
        .unwrap();
        assert!(!headers.hsts.enabled);
        assert!(!headers.hsts.include_subdomains);
        assert!(headers.hsts.preload);
        assert_eq!(headers.hsts.max_age, Duration::from_secs(1));
    }

    #[test]
    fn bad_values_name_the_key_not_the_value() {
        rejects(
            "[security.headers]\nframe-options = \"a\\tsecret\"",
            FRAME_OPTIONS,
            "secret",
        );
        rejects(
            "[security.headers]\nreferrer-policy = \"\"",
            REFERRER_POLICY,
            "\"\"",
        );
        rejects(
            "[security.headers]\ncontent-type-options = \"n\u{f6}sniff\"",
            CONTENT_TYPE_OPTIONS,
            "sniff",
        );
        rejects(
            "[security.headers]\ncache-control = \"x\\u007fsecret\"",
            CACHE_CONTROL,
            "secret",
        );
        rejects(
            "[security.headers.hsts]\nmax-age = \"999ms\"",
            HSTS_MAX_AGE,
            "999",
        );
        rejects("[security.headers.hsts]\nmax-age = 0", HSTS_MAX_AGE, "= 0");
    }

    #[test]
    fn unknown_keys_and_wrong_types_are_rejected() {
        let err = read("[security.headers]\nbogus-key = 1").unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
        let err = read("[security.headers.hsts]\nbogus-key = 1").unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
        assert!(read("[security.headers]\nenabled = \"yes\"").is_err());
        assert!(read("[security.headers]\nframe-options = 3").is_err());
        assert!(read("[security.headers.hsts]\npreload = \"no\"").is_err());
    }
}
