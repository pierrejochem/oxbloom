//! The form login settings.

use crate::http_security::SecurityBuildError;

use super::redirect::is_safe_redirect_target;

/// The form login settings: the URLs and the names of the two form fields.
///
/// Nothing in here is secret, so `Debug` shows every field. Every URL must be a safe redirect
/// target (see [`is_safe_redirect_target`]); [`validate`](Self::validate) checks that, and the
/// field names.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{FormLogin, SecurityBuildError};
///
/// let defaults = FormLogin::default();
/// assert_eq!(defaults.login_page, "/login");
/// assert_eq!(defaults.failure_url, "/login?error");
/// assert!(defaults.validate().is_ok());
///
/// let bad = FormLogin::default().default_success_url("https://evil.example");
/// assert_eq!(bad.validate(), Err(SecurityBuildError::InvalidFormLoginUrl));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormLogin {
    /// Where the login page is served and where unauthenticated browsers are sent.
    pub login_page: String,
    /// Where the login form is posted.
    pub login_processing_url: String,
    /// Where the logout form is posted.
    pub logout_url: String,
    /// Where a successful login goes when no request was saved.
    pub default_success_url: String,
    /// Where a failed login goes.
    pub failure_url: String,
    /// Where a logout goes.
    pub logout_success_url: String,
    /// The name of the username form field.
    pub username_field: String,
    /// The name of the password form field.
    pub password_field: String,
    /// Whether Oxbloom serves the default login page.
    pub login_page_enabled: bool,
}

impl Default for FormLogin {
    fn default() -> Self {
        Self {
            login_page: "/login".to_owned(),
            login_processing_url: "/login".to_owned(),
            logout_url: "/logout".to_owned(),
            default_success_url: "/".to_owned(),
            failure_url: "/login?error".to_owned(),
            logout_success_url: "/login?logout".to_owned(),
            username_field: "username".to_owned(),
            password_field: "password".to_owned(),
            login_page_enabled: true,
        }
    }
}

pub(super) fn is_valid_field_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn path_of(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

macro_rules! setter {
    ($(#[$doc:meta])* $name:ident, $ty:ty, $value:expr) => {
        $(#[$doc])*
        pub fn $name(mut self, $name: $ty) -> Self {
            self.$name = $value($name);
            self
        }
    };
}

impl FormLogin {
    setter! {
        /// Sets the login page URL.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert_eq!(FormLogin::default().login_page("/signin").login_page, "/signin");
        /// ```
        login_page, impl Into<String>, Into::into
    }
    setter! {
        /// Sets the URL the login form is posted to.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// let f = FormLogin::default().login_processing_url("/signin/do");
        /// assert_eq!(f.login_processing_url, "/signin/do");
        /// ```
        login_processing_url, impl Into<String>, Into::into
    }
    setter! {
        /// Sets the logout URL.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert_eq!(FormLogin::default().logout_url("/signout").logout_url, "/signout");
        /// ```
        logout_url, impl Into<String>, Into::into
    }
    setter! {
        /// Sets the URL a successful login goes to when no request was saved.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert_eq!(FormLogin::default().default_success_url("/home").default_success_url, "/home");
        /// ```
        default_success_url, impl Into<String>, Into::into
    }
    setter! {
        /// Sets the URL a failed login goes to.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert_eq!(FormLogin::default().failure_url("/in?bad").failure_url, "/in?bad");
        /// ```
        failure_url, impl Into<String>, Into::into
    }
    setter! {
        /// Sets the URL a logout goes to.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert_eq!(FormLogin::default().logout_success_url("/bye").logout_success_url, "/bye");
        /// ```
        logout_success_url, impl Into<String>, Into::into
    }
    setter! {
        /// Sets the username field name.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert_eq!(FormLogin::default().username_field("user").username_field, "user");
        /// ```
        username_field, impl Into<String>, Into::into
    }
    setter! {
        /// Sets the password field name.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert_eq!(FormLogin::default().password_field("pass").password_field, "pass");
        /// ```
        password_field, impl Into<String>, Into::into
    }
    setter! {
        /// Turns the default login page on or off.
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::FormLogin;
        ///
        /// assert!(!FormLogin::default().login_page_enabled(false).login_page_enabled);
        /// ```
        login_page_enabled, bool, std::convert::identity
    }

    /// Checks the settings.
    ///
    /// Every URL must be a safe redirect target, the field names must be non-empty and made of
    /// ASCII letters, digits, `_` and `-`, the two field names must differ, and the logout URL
    /// must differ (by path) from the login page and the processing URL. The login page and the
    /// processing URL may be equal. An error never contains a value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{FormLogin, SecurityBuildError};
    ///
    /// assert!(FormLogin::default().validate().is_ok());
    /// assert_eq!(
    ///     FormLogin::default().logout_url("/login").validate(),
    ///     Err(SecurityBuildError::InvalidFormLoginUrl),
    /// );
    /// ```
    pub fn validate(&self) -> Result<(), SecurityBuildError> {
        let urls = [
            &self.login_page,
            &self.login_processing_url,
            &self.logout_url,
            &self.default_success_url,
            &self.failure_url,
            &self.logout_success_url,
        ];
        let logout = path_of(&self.logout_url);
        let ok = urls.iter().all(|u| is_safe_redirect_target(u))
            && is_valid_field_name(&self.username_field)
            && is_valid_field_name(&self.password_field)
            && self.username_field != self.password_field
            && logout != path_of(&self.login_page)
            && logout != path_of(&self.login_processing_url);
        if ok {
            Ok(())
        } else {
            Err(SecurityBuildError::InvalidFormLoginUrl)
        }
    }
}
