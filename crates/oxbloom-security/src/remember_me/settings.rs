//! The remember-me settings.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use oxbloom_config::Secret;
use oxbloom_core::{Clock, SystemClock};

use crate::UserDetailsService;
use crate::form_login::is_valid_field_name;
use crate::http_security::SecurityBuildError;
use crate::session::is_valid_cookie_name;

/// The shortest secret, current or old, in bytes.
const MIN_SECRET_BYTES: usize = 32;
/// The default cookie lifetime: 14 days.
const DEFAULT_LIFETIME: Duration = Duration::from_secs(14 * 24 * 3600);
/// The shortest allowed lifetime: 1 minute.
const MIN_LIFETIME: Duration = Duration::from_secs(60);
/// The longest allowed lifetime: 365 days.
const MAX_LIFETIME: Duration = Duration::from_secs(365 * 24 * 3600);

/// The remember-me settings inside [`HttpSecurity::remember_me`](crate::HttpSecurity::remember_me).
///
/// A secret of at least 32 bytes and a [`UserDetailsService`] are required. The lifetime is 14
/// days by default and must lie between 1 minute and 365 days; the form field is `remember-me`
/// and the cookie is `OXBLOOM_REMEMBER_ME` by default. The cookie name must be a non-empty HTTP
/// token and the field name must be non-empty and made of ASCII letters, digits, `-` and `_`.
/// Old secrets keep cookies issued under them valid after a rotation; each must also be at least
/// 32 bytes. The clock is [`SystemClock`] by default.
///
/// `Debug` never shows a secret, only how many old secrets there are and whether a user service
/// is set.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::time::Duration;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{
///     Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity, InMemoryUserDetailsService,
/// };
///
/// let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
/// let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
/// let chain = HttpSecurity::new()
///     .password_provider(Arc::new(DaoAuthenticationProvider::new(users.clone(), encoder)))
///     .form_login(|f| f)
///     .remember_me(|r| {
///         let r = r
///             .secret(Secret::new("a-remember-me-secret-of-32-bytes".to_owned()))
///             .user_details(users)
///             .lifetime(Duration::from_secs(7 * 24 * 3600));
///         assert!(!format!("{r:?}").contains("a-remember-me-secret"));
///         r
///     })
///     .authorize(|a| a.any_request().authenticated())
///     .build();
/// assert!(chain.is_ok());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct RememberMe {
    pub(crate) secret: Option<Secret<String>>,
    pub(crate) old_secrets: Vec<Secret<String>>,
    pub(crate) lifetime: Duration,
    pub(crate) field_name: String,
    pub(crate) cookie_name: String,
    pub(crate) user_details: Option<Arc<dyn UserDetailsService>>,
    pub(crate) clock: Arc<dyn Clock>,
}

impl RememberMe {
    pub(crate) fn new() -> Self {
        Self {
            secret: None,
            old_secrets: Vec::new(),
            lifetime: DEFAULT_LIFETIME,
            field_name: "remember-me".to_owned(),
            cookie_name: "OXBLOOM_REMEMBER_ME".to_owned(),
            user_details: None,
            clock: Arc::new(SystemClock),
        }
    }

    /// Sets the secret that signs new cookies. Required, at least 32 bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let _security = HttpSecurity::new()
    ///     .remember_me(|r| r.secret(Secret::new("a-remember-me-secret-of-32-bytes".to_owned())));
    /// ```
    pub fn secret(mut self, secret: Secret<String>) -> Self {
        self.secret = Some(secret);
        self
    }

    /// Sets the old secrets: cookies signed with one of them stay valid, and new cookies use the
    /// current secret. Each must be at least 32 bytes. Empty by default.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let _security = HttpSecurity::new().remember_me(|r| {
    ///     r.secret(Secret::new("the-new-remember-me-secret-32-byt".to_owned()))
    ///         .old_secrets(vec![Secret::new("the-old-remember-me-secret-32-byt".to_owned())])
    /// });
    /// ```
    pub fn old_secrets(mut self, old_secrets: Vec<Secret<String>>) -> Self {
        self.old_secrets = old_secrets;
        self
    }

    /// Sets how long a cookie stays valid from the time it is issued. Using the cookie does not
    /// extend it. The default is 14 days; the allowed range is 1 minute to 365 days.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let _security = HttpSecurity::new().remember_me(|r| r.lifetime(Duration::from_secs(3600)));
    /// ```
    pub fn lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// Sets the name of the login form field that asks for a cookie. The default is
    /// `remember-me`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let _security = HttpSecurity::new().remember_me(|r| r.field_name("stay-signed-in"));
    /// ```
    pub fn field_name(mut self, field_name: impl Into<String>) -> Self {
        self.field_name = field_name.into();
        self
    }

    /// Sets the cookie name. The default is `OXBLOOM_REMEMBER_ME`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let _security = HttpSecurity::new().remember_me(|r| r.cookie_name("REMEMBER"));
    /// ```
    pub fn cookie_name(mut self, cookie_name: impl Into<String>) -> Self {
        self.cookie_name = cookie_name.into();
        self
    }

    /// Sets the service that loads the user named in a cookie. Required; it must know every
    /// form login user.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{HttpSecurity, InMemoryUserDetailsService};
    ///
    /// let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
    /// let _security = HttpSecurity::new().remember_me(|r| r.user_details(users));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn user_details(mut self, user_details: Arc<dyn UserDetailsService>) -> Self {
        self.user_details = Some(user_details);
        self
    }

    /// Replaces the clock that decides the expiry of new cookies and whether a cookie has
    /// expired. The default is [`SystemClock`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let _security = HttpSecurity::new().remember_me(|r| r.clock(Arc::new(SystemClock)));
    /// ```
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Checks the settings. The error never contains a value.
    pub(crate) fn validate(&self) -> Result<(), SecurityBuildError> {
        let long_enough = |s: &Secret<String>| s.expose().len() >= MIN_SECRET_BYTES;
        let ok = self.secret.as_ref().is_some_and(long_enough)
            && self.old_secrets.iter().all(long_enough)
            && self.user_details.is_some()
            && (MIN_LIFETIME..=MAX_LIFETIME).contains(&self.lifetime)
            && is_valid_cookie_name(&self.cookie_name)
            && is_valid_field_name(&self.field_name);
        if ok {
            Ok(())
        } else {
            Err(SecurityBuildError::InvalidRememberMe)
        }
    }
}

impl fmt::Debug for RememberMe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RememberMe")
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .field("old_secrets", &self.old_secrets.len())
            .field("lifetime", &self.lifetime)
            .field("field_name", &self.field_name)
            .field("cookie_name", &self.cookie_name)
            .field("user_details", &self.user_details.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InMemoryUserDetailsService;

    fn valid() -> RememberMe {
        RememberMe::new()
            .secret(Secret::new("a-remember-me-secret-of-32-bytes".to_owned()))
            .user_details(Arc::new(InMemoryUserDetailsService::new(vec![]).unwrap()))
    }

    #[test]
    fn the_defaults() {
        let r = RememberMe::new();
        assert_eq!(r.lifetime, Duration::from_secs(14 * 24 * 3600));
        assert_eq!(r.field_name, "remember-me");
        assert_eq!(r.cookie_name, "OXBLOOM_REMEMBER_ME");
        assert!(r.secret.is_none() && r.old_secrets.is_empty() && r.user_details.is_none());
        assert_eq!(valid().validate(), Ok(()));
    }

    #[test]
    fn the_lifetime_bounds_are_inclusive() {
        for secs in [60, 365 * 24 * 3600] {
            let r = valid().lifetime(Duration::from_secs(secs));
            assert_eq!(r.validate(), Ok(()), "{secs}");
        }
        for secs in [0, 59, 365 * 24 * 3600 + 1] {
            let r = valid().lifetime(Duration::from_secs(secs));
            assert_eq!(
                r.validate(),
                Err(SecurityBuildError::InvalidRememberMe),
                "{secs}"
            );
        }
    }

    #[test]
    fn a_secret_of_exactly_32_bytes_passes() {
        let r = valid().old_secrets(vec![Secret::new("x".repeat(32))]);
        assert_eq!(r.validate(), Ok(()));
        let r = valid().old_secrets(vec![Secret::new("x".repeat(31))]);
        assert_eq!(r.validate(), Err(SecurityBuildError::InvalidRememberMe));
    }
}
