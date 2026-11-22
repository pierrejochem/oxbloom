//! The security header settings.

use std::time::Duration;

use crate::http_security::SecurityBuildError;

/// The `Strict-Transport-Security` settings inside [`HeadersConfig`].
///
/// The header is added only to a response to an HTTPS request (see
/// [`Request::scheme`](oxbloom_web::Request::scheme)). The default is one year with
/// `includeSubDomains` and without `preload`. Nothing in here is secret.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_security::HstsConfig;
///
/// let hsts = HstsConfig::default();
/// assert!(hsts.enabled);
/// assert_eq!(hsts.max_age, Duration::from_secs(31_536_000));
/// assert!(hsts.include_subdomains);
/// assert!(!hsts.preload);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HstsConfig {
    /// Whether the header is added at all.
    pub enabled: bool,
    /// The `max-age` directive, in whole seconds. Zero is rejected by validation.
    pub max_age: Duration,
    /// Whether the `includeSubDomains` directive is added.
    pub include_subdomains: bool,
    /// Whether the `preload` directive is added.
    pub preload: bool,
}

impl Default for HstsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_age: Duration::from_secs(31_536_000),
            include_subdomains: true,
            preload: false,
        }
    }
}

impl HstsConfig {
    /// Turns the header on or off.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HstsConfig;
    ///
    /// assert!(!HstsConfig::default().enabled(false).enabled);
    /// ```
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the `max-age` directive. Only whole seconds are used.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::HstsConfig;
    ///
    /// let h = HstsConfig::default().max_age(Duration::from_secs(60));
    /// assert_eq!(h.max_age, Duration::from_secs(60));
    /// ```
    pub fn max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// Adds or drops the `includeSubDomains` directive.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HstsConfig;
    ///
    /// assert!(!HstsConfig::default().include_subdomains(false).include_subdomains);
    /// ```
    pub fn include_subdomains(mut self, include_subdomains: bool) -> Self {
        self.include_subdomains = include_subdomains;
        self
    }

    /// Adds or drops the `preload` directive.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HstsConfig;
    ///
    /// assert!(HstsConfig::default().preload(true).preload);
    /// ```
    pub fn preload(mut self, preload: bool) -> Self {
        self.preload = preload;
        self
    }

    /// The header value, for example `max-age=31536000; includeSubDomains`.
    pub(crate) fn value(&self) -> String {
        let mut value = format!("max-age={}", self.max_age.as_secs());
        if self.include_subdomains {
            value.push_str("; includeSubDomains");
        }
        if self.preload {
            value.push_str("; preload");
        }
        value
    }
}

/// The security header settings: one switch, four plain headers and the HSTS settings.
///
/// The defaults are `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
/// `X-Frame-Options: DENY`, `Cache-Control: no-store` (only on an authenticated response) and
/// `Strict-Transport-Security` (only on HTTPS). There is no default `Content-Security-Policy`.
/// A header is added only when the response lacks it. Passing `None` to a setter removes the
/// header from the set. [`validate`](Self::validate) checks the values; its error never
/// contains one.
///
/// # Examples
///
/// ```
/// use oxbloom_security::HeadersConfig;
///
/// let defaults = HeadersConfig::default();
/// assert!(defaults.enabled);
/// assert_eq!(defaults.frame_options.as_deref(), Some("DENY"));
/// assert!(defaults.validate().is_ok());
///
/// let custom = HeadersConfig::default().frame_options(Some("SAMEORIGIN")).referrer_policy(None);
/// assert_eq!(custom.frame_options.as_deref(), Some("SAMEORIGIN"));
/// assert_eq!(custom.referrer_policy, None);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadersConfig {
    /// Whether any header is added.
    pub enabled: bool,
    /// The `X-Content-Type-Options` value, or `None` for no such header.
    pub content_type_options: Option<String>,
    /// The `Referrer-Policy` value, or `None` for no such header.
    pub referrer_policy: Option<String>,
    /// The `X-Frame-Options` value, or `None` for no such header.
    pub frame_options: Option<String>,
    /// The `Cache-Control` value added to an authenticated response, or `None` for none.
    pub cache_control: Option<String>,
    /// The `Strict-Transport-Security` settings.
    pub hsts: HstsConfig,
}

impl Default for HeadersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            content_type_options: Some("nosniff".to_owned()),
            referrer_policy: Some("no-referrer".to_owned()),
            frame_options: Some("DENY".to_owned()),
            cache_control: Some("no-store".to_owned()),
            hsts: HstsConfig::default(),
        }
    }
}

/// A header value must be visible ASCII, spaces included, and not empty.
fn value_ok(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_none_or(|v| !v.is_empty() && v.bytes().all(|b| (0x20..=0x7e).contains(&b)))
}

impl HeadersConfig {
    /// Turns all security headers on or off.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HeadersConfig;
    ///
    /// assert!(!HeadersConfig::default().enabled(false).enabled);
    /// ```
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the `X-Content-Type-Options` value; `None` removes the header.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HeadersConfig;
    ///
    /// assert_eq!(HeadersConfig::default().content_type_options(None).content_type_options, None);
    /// ```
    pub fn content_type_options(mut self, value: Option<&str>) -> Self {
        self.content_type_options = value.map(str::to_owned);
        self
    }

    /// Sets the `Referrer-Policy` value; `None` removes the header.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HeadersConfig;
    ///
    /// let c = HeadersConfig::default().referrer_policy(Some("same-origin"));
    /// assert_eq!(c.referrer_policy.as_deref(), Some("same-origin"));
    /// ```
    pub fn referrer_policy(mut self, value: Option<&str>) -> Self {
        self.referrer_policy = value.map(str::to_owned);
        self
    }

    /// Sets the `X-Frame-Options` value; `None` removes the header.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HeadersConfig;
    ///
    /// let c = HeadersConfig::default().frame_options(Some("SAMEORIGIN"));
    /// assert_eq!(c.frame_options.as_deref(), Some("SAMEORIGIN"));
    /// ```
    pub fn frame_options(mut self, value: Option<&str>) -> Self {
        self.frame_options = value.map(str::to_owned);
        self
    }

    /// Sets the `Cache-Control` value of an authenticated response; `None` removes it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HeadersConfig;
    ///
    /// let c = HeadersConfig::default().cache_control(Some("no-store, max-age=0"));
    /// assert_eq!(c.cache_control.as_deref(), Some("no-store, max-age=0"));
    /// ```
    pub fn cache_control(mut self, value: Option<&str>) -> Self {
        self.cache_control = value.map(str::to_owned);
        self
    }

    /// Tunes the `Strict-Transport-Security` header.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HeadersConfig;
    ///
    /// let c = HeadersConfig::default().hsts(|h| h.preload(true));
    /// assert!(c.hsts.preload);
    /// ```
    pub fn hsts(mut self, f: impl FnOnce(HstsConfig) -> HstsConfig) -> Self {
        self.hsts = f(self.hsts);
        self
    }

    /// Checks the values: each set value must be non-empty visible ASCII (spaces allowed, no
    /// control characters) and the HSTS `max_age` must be at least one second. The error has a
    /// fixed text and never contains a value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{HeadersConfig, SecurityBuildError};
    ///
    /// assert!(HeadersConfig::default().validate().is_ok());
    /// assert_eq!(
    ///     HeadersConfig::default().frame_options(Some("")).validate(),
    ///     Err(SecurityBuildError::InvalidHeadersConfig),
    /// );
    /// ```
    pub fn validate(&self) -> Result<(), SecurityBuildError> {
        let values_ok = value_ok(&self.content_type_options)
            && value_ok(&self.referrer_policy)
            && value_ok(&self.frame_options)
            && value_ok(&self.cache_control);
        if values_ok && self.hsts.max_age.as_secs() > 0 {
            Ok(())
        } else {
            Err(SecurityBuildError::InvalidHeadersConfig)
        }
    }
}
