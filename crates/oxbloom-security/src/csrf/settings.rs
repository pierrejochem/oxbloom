//! The CSRF settings.

use crate::form_login::{is_safe_redirect_target, is_valid_field_name, path_of};
use crate::http_security::SecurityBuildError;

/// The CSRF settings: whether the check is on, the names the token travels under and an
/// optional endpoint that returns the token.
///
/// Nothing in here is secret, so `Debug` shows every field. The check is on by default; the
/// field name is `_csrf`, the header name is `X-CSRF-TOKEN` and there is no token endpoint.
/// [`validate`](Self::validate) checks the names and the endpoint.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{CsrfConfig, SecurityBuildError};
///
/// let defaults = CsrfConfig::default();
/// assert!(defaults.enabled);
/// assert_eq!(defaults.field_name, "_csrf");
/// assert_eq!(defaults.header_name, "X-CSRF-TOKEN");
/// assert_eq!(defaults.token_endpoint, None);
/// assert!(defaults.validate(&["/login", "/logout"]).is_ok());
///
/// let bad = CsrfConfig::default().token_endpoint("/login");
/// assert_eq!(bad.validate(&["/login"]), Err(SecurityBuildError::InvalidCsrfConfig));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsrfConfig {
    /// Whether the CSRF check runs.
    pub enabled: bool,
    /// The name of the hidden form field that carries the token.
    pub field_name: String,
    /// The name of the request header that carries the token.
    pub header_name: String,
    /// The path of an endpoint that returns the token as JSON, when there is one.
    pub token_endpoint: Option<String>,
}

impl Default for CsrfConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            field_name: "_csrf".to_owned(),
            header_name: "X-CSRF-TOKEN".to_owned(),
            token_endpoint: None,
        }
    }
}

impl CsrfConfig {
    /// Turns the CSRF check on or off.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::CsrfConfig;
    ///
    /// assert!(!CsrfConfig::default().enabled(false).enabled);
    /// ```
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the form field name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::CsrfConfig;
    ///
    /// assert_eq!(CsrfConfig::default().field_name("token").field_name, "token");
    /// ```
    pub fn field_name(mut self, field_name: impl Into<String>) -> Self {
        self.field_name = field_name.into();
        self
    }

    /// Sets the header name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::CsrfConfig;
    ///
    /// assert_eq!(CsrfConfig::default().header_name("X-Token").header_name, "X-Token");
    /// ```
    pub fn header_name(mut self, header_name: impl Into<String>) -> Self {
        self.header_name = header_name.into();
        self
    }

    /// Sets the token endpoint path.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::CsrfConfig;
    ///
    /// let c = CsrfConfig::default().token_endpoint("/csrf");
    /// assert_eq!(c.token_endpoint.as_deref(), Some("/csrf"));
    /// ```
    pub fn token_endpoint(mut self, token_endpoint: impl Into<String>) -> Self {
        self.token_endpoint = Some(token_endpoint.into());
        self
    }

    /// Checks the settings against the form login paths.
    ///
    /// The field and header names must be non-empty and made of ASCII letters, digits, `-` and
    /// `_`. The token endpoint, when set, must be a safe redirect target (see
    /// [`is_safe_redirect_target`](crate::is_safe_redirect_target)) and must differ, by the path
    /// before `?`, from every path in `form_login_paths`. An error never contains a value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{CsrfConfig, SecurityBuildError};
    ///
    /// assert!(CsrfConfig::default().token_endpoint("/csrf").validate(&["/login"]).is_ok());
    /// assert_eq!(
    ///     CsrfConfig::default().field_name("a b").validate(&[]),
    ///     Err(SecurityBuildError::InvalidCsrfConfig),
    /// );
    /// ```
    pub fn validate(&self, form_login_paths: &[&str]) -> Result<(), SecurityBuildError> {
        let endpoint_ok = self.token_endpoint.as_deref().is_none_or(|e| {
            is_safe_redirect_target(e) && form_login_paths.iter().all(|p| path_of(p) != path_of(e))
        });
        if is_valid_field_name(&self.field_name)
            && is_valid_field_name(&self.header_name)
            && endpoint_ok
        {
            Ok(())
        } else {
            Err(SecurityBuildError::InvalidCsrfConfig)
        }
    }
}
