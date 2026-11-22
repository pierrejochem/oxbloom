use std::collections::BTreeSet;
use std::fmt;

use oxbloom_config::Secret;
use oxbloom_web::Request;
use serde::{Deserialize, Serialize};

/// A single permission or role held by an authenticated principal, e.g. `ROLE_ADMIN` or `SCOPE_read`.
///
/// # Examples
///
/// ```
/// use oxbloom_security::GrantedAuthority;
///
/// let authority = GrantedAuthority::new("ROLE_ADMIN");
/// assert_eq!(authority.as_str(), "ROLE_ADMIN");
/// assert_eq!(authority.to_string(), "ROLE_ADMIN");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GrantedAuthority(String);

impl GrantedAuthority {
    /// Creates an authority from its full name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::GrantedAuthority;
    ///
    /// assert_eq!(GrantedAuthority::new("SCOPE_read").as_str(), "SCOPE_read");
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The authority name.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::GrantedAuthority;
    ///
    /// assert_eq!(GrantedAuthority::new("ROLE_USER").as_str(), "ROLE_USER");
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GrantedAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The `details` entry that marks an authentication restored from a remember-me cookie.
const REMEMBERED_DETAIL: &str = "oxbloom.remembered";

/// An authenticated principal with its authorities and extra details (e.g. token claims).
///
/// `Debug` prints only the principal and authority names, never the details. The serialized form
/// holds the principal, the authorities and the details, and never a credential.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Authentication, GrantedAuthority};
///
/// let auth = Authentication::new("alice", [GrantedAuthority::new("ROLE_ADMIN")]);
/// assert_eq!(auth.principal(), "alice");
/// assert!(auth.has_role("ADMIN"));
/// ```
#[derive(Clone, Serialize, Deserialize)]
pub struct Authentication {
    principal: String,
    authorities: BTreeSet<GrantedAuthority>,
    details: serde_json::Map<String, serde_json::Value>,
}

impl Authentication {
    /// Creates an authentication; duplicate authorities collapse and are kept sorted.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority};
    ///
    /// let auth = Authentication::new("bob", [GrantedAuthority::new("b"), GrantedAuthority::new("a")]);
    /// assert_eq!(auth.authorities().len(), 2);
    /// ```
    pub fn new(
        principal: impl Into<String>,
        authorities: impl IntoIterator<Item = GrantedAuthority>,
    ) -> Self {
        Self {
            principal: principal.into(),
            authorities: authorities.into_iter().collect(),
            details: serde_json::Map::new(),
        }
    }

    /// Attaches details such as token claims.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Authentication;
    ///
    /// let mut details = serde_json::Map::new();
    /// details.insert("tenant".into(), serde_json::json!("acme"));
    /// let auth = Authentication::new("alice", []).with_details(details);
    /// assert_eq!(auth.details()["tenant"], "acme");
    /// ```
    pub fn with_details(mut self, details: serde_json::Map<String, serde_json::Value>) -> Self {
        self.details = details;
        self
    }

    /// Marks the authentication as restored from a remember-me cookie, by setting the `details`
    /// entry `oxbloom.remembered` to `true`. The marker travels with the details, so it survives
    /// the stored session value and a revalidation; [`with_details`](Self::with_details) replaces
    /// the details and drops it. A remembered user passes `authenticated()` rules but not
    /// `fully_authenticated()` ones.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Authentication;
    ///
    /// let auth = Authentication::new("alice", []).remembered();
    /// assert!(auth.is_remembered());
    /// assert_eq!(auth.details()["oxbloom.remembered"], true);
    /// ```
    pub fn remembered(mut self) -> Self {
        self.details
            .insert(REMEMBERED_DETAIL.to_owned(), serde_json::Value::Bool(true));
        self
    }

    /// True when the authentication was restored from a remember-me cookie: the `details` entry
    /// `oxbloom.remembered` is the boolean `true`. A password, Basic or bearer login is not
    /// remembered.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Authentication;
    ///
    /// assert!(!Authentication::new("alice", []).is_remembered());
    /// assert!(Authentication::new("alice", []).remembered().is_remembered());
    /// ```
    pub fn is_remembered(&self) -> bool {
        self.details.get(REMEMBERED_DETAIL) == Some(&serde_json::Value::Bool(true))
    }

    /// Removes the remembered marker, whatever its value, and keeps the other details. A
    /// password login stores its authentication through this, so no provider can mark it.
    pub(crate) fn not_remembered(mut self) -> Self {
        self.details.remove(REMEMBERED_DETAIL);
        self
    }

    /// The principal name (for JWTs, the `sub` claim).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Authentication;
    ///
    /// assert_eq!(Authentication::new("alice", []).principal(), "alice");
    /// ```
    pub fn principal(&self) -> &str {
        &self.principal
    }

    /// The granted authorities, sorted and unique.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority};
    ///
    /// let auth = Authentication::new("a", [GrantedAuthority::new("x")]);
    /// assert!(auth.authorities().contains(&GrantedAuthority::new("x")));
    /// ```
    pub fn authorities(&self) -> &BTreeSet<GrantedAuthority> {
        &self.authorities
    }

    /// The attached details; empty unless set with [`Authentication::with_details`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::Authentication;
    ///
    /// assert!(Authentication::new("a", []).details().is_empty());
    /// ```
    pub fn details(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.details
    }

    /// True if the exact authority name is granted.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority};
    ///
    /// let auth = Authentication::new("a", [GrantedAuthority::new("SCOPE_read")]);
    /// assert!(auth.has_authority("SCOPE_read"));
    /// assert!(!auth.has_authority("read"));
    /// ```
    pub fn has_authority(&self, name: &str) -> bool {
        self.authorities.contains(&GrantedAuthority::new(name))
    }

    /// True if `ROLE_<name>` is granted.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority};
    ///
    /// let auth = Authentication::new("a", [GrantedAuthority::new("ROLE_ADMIN")]);
    /// assert!(auth.has_role("ADMIN"));
    /// assert!(!auth.has_role("USER"));
    /// ```
    pub fn has_role(&self, name: &str) -> bool {
        self.has_authority(&format!("ROLE_{name}"))
    }
}

impl fmt::Debug for Authentication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Authentication")
            .field("principal", &self.principal)
            .field(
                "authorities",
                &self
                    .authorities
                    .iter()
                    .map(GrantedAuthority::as_str)
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

/// Credentials presented by a client, handed to an authentication provider.
///
/// `Debug` never prints a token, a password or the username text. For a username and password it
/// prints the username length only.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Secret;
/// use oxbloom_security::AuthenticationRequest;
///
/// let request = AuthenticationRequest::BearerToken(Secret::new("token".to_string()));
/// assert_eq!(format!("{request:?}"), "BearerToken([REDACTED])");
///
/// let request = AuthenticationRequest::UsernamePassword {
///     username: "alice".to_string(),
///     password: Secret::new("hunter2".to_string()),
/// };
/// let debug = format!("{request:?}");
/// assert!(debug.contains("[REDACTED]") && !debug.contains("alice") && !debug.contains("hunter2"));
/// ```
#[derive(Clone)]
pub enum AuthenticationRequest {
    /// A bearer token taken from the `Authorization` header.
    BearerToken(Secret<String>),
    /// A username and password, for example from a login form or HTTP Basic.
    UsernamePassword {
        /// The username as presented.
        username: String,
        /// The password as presented.
        password: Secret<String>,
    },
}

impl fmt::Debug for AuthenticationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BearerToken(token) => f.debug_tuple("BearerToken").field(token).finish(),
            Self::UsernamePassword { username, password } => f
                .debug_struct("UsernamePassword")
                .field("username_len", &username.len())
                .field("password", password)
                .finish(),
        }
    }
}

/// The security state of one request: anonymous or authenticated.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Authentication, SecurityContext};
///
/// assert!(SecurityContext::anonymous().authentication().is_none());
/// let ctx = SecurityContext::authenticated(Authentication::new("alice", []));
/// assert_eq!(ctx.authentication().map(|a| a.principal()), Some("alice"));
/// ```
#[derive(Clone, Default)]
pub struct SecurityContext {
    authentication: Option<Authentication>,
}

impl SecurityContext {
    /// A context with no authenticated principal.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::SecurityContext;
    ///
    /// assert!(SecurityContext::anonymous().authentication().is_none());
    /// ```
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// A context for an authenticated principal.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, SecurityContext};
    ///
    /// let ctx = SecurityContext::authenticated(Authentication::new("alice", []));
    /// assert!(ctx.authentication().is_some());
    /// ```
    pub fn authenticated(authentication: Authentication) -> Self {
        Self {
            authentication: Some(authentication),
        }
    }

    /// The authentication, or `None` for an anonymous request.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, SecurityContext};
    ///
    /// let ctx = SecurityContext::authenticated(Authentication::new("alice", []));
    /// assert_eq!(ctx.authentication().unwrap().principal(), "alice");
    /// ```
    pub fn authentication(&self) -> Option<&Authentication> {
        self.authentication.as_ref()
    }
}

impl fmt::Debug for SecurityContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.authentication {
            Some(a) => f.debug_tuple("SecurityContext").field(a).finish(),
            None => f.write_str("SecurityContext(anonymous)"),
        }
    }
}

/// Reads the authenticated principal of a request from its extensions.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Authentication, RequestSecurityExt, SecurityContext};
/// use oxbloom_web::{HttpMethod, Request};
///
/// let mut req = Request::new(HttpMethod::Get, "/me");
/// assert!(req.authentication().is_none());
/// req.extensions_mut()
///     .insert(SecurityContext::authenticated(Authentication::new("alice", [])));
/// assert_eq!(req.authentication().unwrap().principal(), "alice");
/// ```
pub trait RequestSecurityExt {
    /// The authentication stored by the security filter, if any.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::RequestSecurityExt;
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// assert!(Request::new(HttpMethod::Get, "/").authentication().is_none());
    /// ```
    fn authentication(&self) -> Option<&Authentication>;
}

impl RequestSecurityExt for Request {
    fn authentication(&self) -> Option<&Authentication> {
        self.extensions().get::<SecurityContext>()?.authentication()
    }
}

/// Why authentication failed. Messages are fixed text and never contain token or claim data.
///
/// # Examples
///
/// ```
/// use oxbloom_security::AuthenticationError;
///
/// assert_eq!(AuthenticationError::Expired.to_string(), "token expired");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationError {
    /// The credentials were rejected.
    BadCredentials,
    /// The token is past its expiry.
    Expired,
    /// The token is malformed or failed validation.
    InvalidToken,
    /// A required backend (e.g. the key source) could not be reached.
    ProviderUnavailable,
    /// The credentials were right but the account is disabled.
    AccountDisabled,
    /// The credentials were right but the account is locked.
    AccountLocked,
}

impl fmt::Display for AuthenticationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BadCredentials => "bad credentials",
            Self::Expired => "token expired",
            Self::InvalidToken => "invalid token",
            Self::ProviderUnavailable => "authentication provider unavailable",
            Self::AccountDisabled => "account disabled",
            Self::AccountLocked => "account locked",
        })
    }
}

impl std::error::Error for AuthenticationError {}

#[cfg(test)]
mod tests {
    use super::*;
    use oxbloom_web::{HttpMethod, Request};

    fn auth(authorities: &[&str]) -> Authentication {
        Authentication::new(
            "alice",
            authorities.iter().map(|a| GrantedAuthority::new(*a)),
        )
    }

    #[test]
    fn has_role_checks_role_prefix() {
        let a = auth(&["ROLE_ADMIN", "SCOPE_read"]);
        assert!(a.has_role("ADMIN"));
        assert!(!a.has_role("ROLE_ADMIN"));
        assert!(!a.has_role("read"));
        assert!(a.has_authority("SCOPE_read"));
        assert!(!a.has_authority("ADMIN"));
    }

    #[test]
    fn authorities_are_sorted_and_unique() {
        let a = auth(&["b", "a", "b"]);
        let names: Vec<&str> = a
            .authorities()
            .iter()
            .map(GrantedAuthority::as_str)
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert_eq!(GrantedAuthority::new("a").to_string(), "a");
    }

    #[test]
    fn authentication_debug_has_no_claim_values() {
        let mut details = serde_json::Map::new();
        details.insert("k".into(), serde_json::json!("secret-claim"));
        let a = auth(&["ROLE_USER"]).with_details(details);
        assert_eq!(a.details()["k"], "secret-claim");
        let dbg = format!("{a:?}");
        assert!(!dbg.contains("secret-claim"));
        assert!(dbg.contains("alice") && dbg.contains("ROLE_USER"));
        let ctx = format!("{:?}", SecurityContext::authenticated(a));
        assert!(!ctx.contains("secret-claim"));
    }

    #[test]
    fn security_context_defaults_to_anonymous() {
        assert!(SecurityContext::anonymous().authentication().is_none());
        let ctx = SecurityContext::authenticated(auth(&[]));
        assert_eq!(ctx.authentication().unwrap().principal(), "alice");
    }

    #[test]
    fn request_ext_reads_context_from_extensions() {
        let mut req = Request::new(HttpMethod::Get, "/");
        assert!(req.authentication().is_none());
        req.extensions_mut().insert(SecurityContext::anonymous());
        assert!(req.authentication().is_none());
        req.extensions_mut()
            .insert(SecurityContext::authenticated(auth(&[])));
        assert_eq!(req.authentication().unwrap().principal(), "alice");
    }

    #[test]
    fn error_display_is_fixed_text() {
        assert_eq!(
            AuthenticationError::BadCredentials.to_string(),
            "bad credentials"
        );
        assert_eq!(AuthenticationError::Expired.to_string(), "token expired");
        assert_eq!(
            AuthenticationError::InvalidToken.to_string(),
            "invalid token"
        );
        assert_eq!(
            AuthenticationError::ProviderUnavailable.to_string(),
            "authentication provider unavailable"
        );
        assert_eq!(
            AuthenticationError::AccountDisabled.to_string(),
            "account disabled"
        );
        assert_eq!(
            AuthenticationError::AccountLocked.to_string(),
            "account locked"
        );
    }

    #[test]
    fn error_is_send_sync_static() {
        fn check<T: std::error::Error + Send + Sync + 'static>() {}
        check::<AuthenticationError>();
    }
}
