//! Session cookie and lifetime settings.

use std::str::FromStr;
use std::time::Duration;

use super::error::SessionError;

/// The `SameSite` attribute of the session cookie.
///
/// # Examples
///
/// ```
/// use oxbloom_security::SameSite;
///
/// assert_eq!("strict".parse::<SameSite>(), Ok(SameSite::Strict));
/// assert!("Strict".parse::<SameSite>().is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    /// Sent on same-site requests and top-level navigations. The default.
    Lax,
    /// Sent on same-site requests only.
    Strict,
    /// Sent on every request. Requires [`SecureMode::Always`].
    None,
}

impl FromStr for SameSite {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "lax" => Ok(Self::Lax),
            "strict" => Ok(Self::Strict),
            "none" => Ok(Self::None),
            _ => Err(SessionError::InvalidSettings),
        }
    }
}

/// When the session cookie carries the `Secure` attribute.
///
/// # Examples
///
/// ```
/// use oxbloom_security::SecureMode;
///
/// assert_eq!("always".parse::<SecureMode>(), Ok(SecureMode::Always));
/// assert!("ALWAYS".parse::<SecureMode>().is_err());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureMode {
    /// `Secure` when the request arrived over HTTPS. The default.
    Auto,
    /// Always `Secure`.
    Always,
    /// Never `Secure`.
    Never,
}

impl FromStr for SecureMode {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(SessionError::InvalidSettings),
        }
    }
}

/// The cookie and lifetime settings of the session layer.
///
/// The cookie is always `HttpOnly`. Nothing in here is secret, so `Debug` shows every field.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use oxbloom_security::{SameSite, SecureMode, SessionSettings};
///
/// let defaults = SessionSettings::default();
/// assert_eq!(defaults.cookie_name, "OXBLOOM_SESSION");
/// assert_eq!(defaults.idle_timeout, Duration::from_secs(30 * 60));
///
/// let bad = SessionSettings::default().same_site(SameSite::None);
/// assert!(bad.validate().is_err());
/// assert!(bad.secure(SecureMode::Always).validate().is_ok());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSettings {
    /// The cookie name. Must be a non-empty HTTP token.
    pub cookie_name: String,
    /// The cookie `Path`. Must start with `/`.
    pub path: String,
    /// When the cookie is `Secure`.
    pub secure: SecureMode,
    /// The cookie `SameSite` attribute.
    pub same_site: SameSite,
    /// How long a session may sit unused before it expires. At least one second.
    pub idle_timeout: Duration,
    /// How long a session may live in total, however active it is.
    pub absolute_timeout: Duration,
    /// The most sessions the store holds at once.
    pub max_sessions: usize,
    /// The largest cookie, in bytes, the layer will issue. At least 256.
    pub max_cookie_bytes: usize,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            cookie_name: "OXBLOOM_SESSION".to_owned(),
            path: "/".to_owned(),
            secure: SecureMode::Auto,
            same_site: SameSite::Lax,
            idle_timeout: Duration::from_secs(30 * 60),
            absolute_timeout: Duration::from_secs(8 * 60 * 60),
            max_sessions: 100_000,
            max_cookie_bytes: 3072,
        }
    }
}

/// The shortest idle timeout: below one second every write would expire at once.
const MIN_IDLE_TIMEOUT: Duration = Duration::from_secs(1);

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c)
}

impl SessionSettings {
    /// Sets the cookie name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::SessionSettings;
    ///
    /// let s = SessionSettings::default().cookie_name("SID");
    /// assert_eq!(s.cookie_name, "SID");
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn cookie_name(mut self, name: impl Into<String>) -> Self {
        self.cookie_name = name.into();
        self
    }

    /// Sets the cookie path.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::SessionSettings;
    ///
    /// let s = SessionSettings::default().path("/app");
    /// assert_eq!(s.path, "/app");
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Sets when the cookie is `Secure`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{SecureMode, SessionSettings};
    ///
    /// let s = SessionSettings::default().secure(SecureMode::Never);
    /// assert_eq!(s.secure, SecureMode::Never);
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn secure(mut self, secure: SecureMode) -> Self {
        self.secure = secure;
        self
    }

    /// Sets the `SameSite` attribute.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{SameSite, SessionSettings};
    ///
    /// let s = SessionSettings::default().same_site(SameSite::Strict);
    /// assert_eq!(s.same_site, SameSite::Strict);
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = same_site;
        self
    }

    /// Sets the idle timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::SessionSettings;
    ///
    /// let s = SessionSettings::default().idle_timeout(Duration::from_secs(60));
    /// assert_eq!(s.idle_timeout, Duration::from_secs(60));
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Sets the absolute timeout.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use oxbloom_security::SessionSettings;
    ///
    /// let s = SessionSettings::default().absolute_timeout(Duration::from_secs(7200));
    /// assert_eq!(s.absolute_timeout, Duration::from_secs(7200));
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn absolute_timeout(mut self, timeout: Duration) -> Self {
        self.absolute_timeout = timeout;
        self
    }

    /// Sets the session limit.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::SessionSettings;
    ///
    /// let s = SessionSettings::default().max_sessions(10);
    /// assert_eq!(s.max_sessions, 10);
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn max_sessions(mut self, max: usize) -> Self {
        self.max_sessions = max;
        self
    }

    /// Sets the cookie size limit in bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::SessionSettings;
    ///
    /// let s = SessionSettings::default().max_cookie_bytes(4000);
    /// assert_eq!(s.max_cookie_bytes, 4000);
    /// assert!(s.validate().is_ok());
    /// ```
    #[must_use]
    pub fn max_cookie_bytes(mut self, max: usize) -> Self {
        self.max_cookie_bytes = max;
        self
    }

    /// Checks the settings.
    ///
    /// # Errors
    ///
    /// [`SessionError::InvalidSettings`] for an empty or non-token cookie name, a path that is
    /// not a plain path (it must start with `/` and hold no `;`, whitespace or control
    /// character), `SameSite::None` without `SecureMode::Always`, an idle timeout below one
    /// second (every write would expire at once), a zero absolute timeout, an idle timeout above
    /// the absolute timeout, `max_sessions` of zero or `max_cookie_bytes` below 256.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{SessionError, SessionSettings};
    ///
    /// assert!(SessionSettings::default().validate().is_ok());
    /// assert_eq!(
    ///     SessionSettings::default().cookie_name("a;b").validate(),
    ///     Err(SessionError::InvalidSettings),
    /// );
    /// ```
    pub fn validate(&self) -> Result<(), SessionError> {
        let name_ok = !self.cookie_name.is_empty() && self.cookie_name.chars().all(is_token_char);
        let path_ok = self.path.starts_with('/')
            && !self
                .path
                .chars()
                .any(|c| c == ';' || c.is_whitespace() || c.is_control() || !c.is_ascii());
        let same_site_ok = self.same_site != SameSite::None || self.secure == SecureMode::Always;
        let timeouts_ok = self.idle_timeout >= MIN_IDLE_TIMEOUT
            && !self.absolute_timeout.is_zero()
            && self.idle_timeout <= self.absolute_timeout;
        if name_ok
            && path_ok
            && same_site_ok
            && timeouts_ok
            && self.max_sessions > 0
            && self.max_cookie_bytes >= 256
        {
            Ok(())
        } else {
            Err(SessionError::InvalidSettings)
        }
    }
}
