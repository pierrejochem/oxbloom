//! Authorization rules.

use std::fmt;
use std::sync::Arc;

use oxbloom_web::Request;

use crate::{__private, Authentication};

/// Custom authorization decision, used by [`AuthorizationRule::Custom`].
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Authentication, AuthorizationManager};
/// use oxbloom_web::{HttpMethod, Request};
///
/// struct Mine;
/// impl AuthorizationManager for Mine {
///     fn check(&self, auth: Option<&Authentication>, _req: &Request) -> bool {
///         auth.is_some_and(|a| a.principal() == "root")
///     }
/// }
/// let req = Request::new(HttpMethod::Get, "/");
/// assert!(!Mine.check(None, &req));
/// ```
pub trait AuthorizationManager: Send + Sync {
    /// Returns true when access is granted.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, AuthorizationManager};
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// struct Deny;
    /// impl AuthorizationManager for Deny {
    ///     fn check(&self, _: Option<&Authentication>, _: &Request) -> bool { false }
    /// }
    /// assert!(!Deny.check(None, &Request::new(HttpMethod::Get, "/")));
    /// ```
    fn check(&self, auth: Option<&Authentication>, req: &Request) -> bool;
}

/// What a matched request requires.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Authentication, AuthorizationRule, GrantedAuthority};
/// use oxbloom_web::{HttpMethod, Request};
///
/// let req = Request::new(HttpMethod::Get, "/admin");
/// let admin = Authentication::new("a", [GrantedAuthority::new("ROLE_ADMIN")]);
/// let rule = AuthorizationRule::HasRole("ADMIN".into());
/// assert!(rule.grants(Some(&admin), &req));
/// assert!(!rule.grants(None, &req));
///
/// // A remembered user is authenticated but not fully authenticated.
/// let remembered = Authentication::new("a", []).remembered();
/// assert!(AuthorizationRule::Authenticated.grants(Some(&remembered), &req));
/// assert!(!AuthorizationRule::FullyAuthenticated.grants(Some(&remembered), &req));
/// ```
#[derive(Clone)]
pub enum AuthorizationRule {
    /// Everyone, including anonymous callers.
    PermitAll,
    /// Nobody.
    DenyAll,
    /// Any authenticated caller, also one restored from a remember-me cookie.
    Authenticated,
    /// An authenticated caller that was not restored from a remember-me cookie (see
    /// [`Authentication::is_remembered`]). A password login, Basic and a bearer token count as
    /// fully authenticated; a remembered user gets the normal 401 or 403 answer.
    FullyAuthenticated,
    /// Authority `ROLE_<name>`.
    HasRole(String),
    /// Any one of the roles (each as `ROLE_<name>`).
    HasAnyRole(Vec<String>),
    /// Exactly this authority.
    HasAuthority(String),
    /// Any one of these authorities.
    HasAnyAuthority(Vec<String>),
    /// Delegates to a custom manager.
    Custom(Arc<dyn AuthorizationManager>),
}

impl fmt::Debug for AuthorizationRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermitAll => f.write_str("PermitAll"),
            Self::DenyAll => f.write_str("DenyAll"),
            Self::Authenticated => f.write_str("Authenticated"),
            Self::FullyAuthenticated => f.write_str("FullyAuthenticated"),
            Self::HasRole(r) => f.debug_tuple("HasRole").field(r).finish(),
            Self::HasAnyRole(r) => f.debug_tuple("HasAnyRole").field(r).finish(),
            Self::HasAuthority(a) => f.debug_tuple("HasAuthority").field(a).finish(),
            Self::HasAnyAuthority(a) => f.debug_tuple("HasAnyAuthority").field(a).finish(),
            Self::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

impl AuthorizationRule {
    /// Returns true when the rule grants access to this caller and request.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::AuthorizationRule;
    /// use oxbloom_web::{HttpMethod, Request};
    ///
    /// let req = Request::new(HttpMethod::Get, "/");
    /// assert!(AuthorizationRule::PermitAll.grants(None, &req));
    /// assert!(!AuthorizationRule::DenyAll.grants(None, &req));
    /// ```
    pub fn grants(&self, auth: Option<&Authentication>, req: &Request) -> bool {
        match self {
            Self::PermitAll => true,
            Self::DenyAll => false,
            Self::Authenticated => __private::authenticated(auth),
            Self::FullyAuthenticated => __private::fully_authenticated(auth),
            Self::HasRole(r) => __private::has_role(auth, r),
            Self::HasAnyRole(rs) => {
                let rs: Vec<&str> = rs.iter().map(String::as_str).collect();
                __private::has_any_role(auth, &rs)
            }
            Self::HasAuthority(x) => __private::has_authority(auth, x),
            Self::HasAnyAuthority(xs) => {
                let xs: Vec<&str> = xs.iter().map(String::as_str).collect();
                __private::has_any_authority(auth, &xs)
            }
            Self::Custom(m) => m.check(auth, req),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GrantedAuthority;
    use oxbloom_web::HttpMethod;

    fn req() -> Request {
        Request::new(HttpMethod::Get, "/")
    }

    fn user(auths: &[&str]) -> Authentication {
        Authentication::new("u", auths.iter().map(|a| GrantedAuthority::new(*a)))
    }

    #[test]
    fn permit_all_grants_anonymous() {
        assert!(AuthorizationRule::PermitAll.grants(None, &req()));
    }

    #[test]
    fn deny_all_never_grants() {
        let u = user(&["ROLE_ADMIN"]);
        assert!(!AuthorizationRule::DenyAll.grants(Some(&u), &req()));
        assert!(!AuthorizationRule::DenyAll.grants(None, &req()));
    }

    #[test]
    fn authenticated_needs_authentication() {
        assert!(!AuthorizationRule::Authenticated.grants(None, &req()));
        assert!(AuthorizationRule::Authenticated.grants(Some(&user(&[])), &req()));
    }

    #[test]
    fn fully_authenticated_refuses_anonymous_and_remembered() {
        let rule = AuthorizationRule::FullyAuthenticated;
        assert!(!rule.grants(None, &req()));
        assert!(rule.grants(Some(&user(&[])), &req()));
        assert!(!rule.grants(Some(&user(&[]).remembered()), &req()));
    }

    #[test]
    fn has_role_requires_role_prefixed_authority() {
        let rule = AuthorizationRule::HasRole("ADMIN".into());
        assert!(rule.grants(Some(&user(&["ROLE_ADMIN"])), &req()));
        assert!(!rule.grants(Some(&user(&["ADMIN"])), &req()));
        assert!(!rule.grants(None, &req()));
    }

    #[test]
    fn has_any_role_accepts_one_of() {
        let rule = AuthorizationRule::HasAnyRole(vec!["A".into(), "B".into()]);
        assert!(rule.grants(Some(&user(&["ROLE_B"])), &req()));
        assert!(!rule.grants(Some(&user(&["ROLE_C"])), &req()));
        assert!(!rule.grants(None, &req()));
    }

    #[test]
    fn has_authority_exact() {
        let rule = AuthorizationRule::HasAuthority("SCOPE_read".into());
        assert!(rule.grants(Some(&user(&["SCOPE_read"])), &req()));
        assert!(!rule.grants(Some(&user(&["SCOPE_reads"])), &req()));
        let any = AuthorizationRule::HasAnyAuthority(vec!["x".into(), "y".into()]);
        assert!(any.grants(Some(&user(&["y"])), &req()));
        assert!(!any.grants(None, &req()));
    }

    #[test]
    fn custom_manager_is_consulted() {
        struct OnlyRoot;
        impl AuthorizationManager for OnlyRoot {
            fn check(&self, auth: Option<&Authentication>, req: &Request) -> bool {
                req.path == "/" && auth.is_some_and(|a| a.principal() == "root")
            }
        }
        let rule = AuthorizationRule::Custom(Arc::new(OnlyRoot));
        let root = Authentication::new("root", []);
        assert!(rule.grants(Some(&root), &req()));
        assert!(!rule.grants(Some(&user(&[])), &req()));
        assert!(!rule.grants(None, &req()));
    }
}
