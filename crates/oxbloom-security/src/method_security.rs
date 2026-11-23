//! Runtime support for method security: the task-local security context, [`AccessDenied`] and the
//! rule functions shared by URL rules and method rules.

use std::fmt;
use std::future::Future;

use http::StatusCode;
use oxbloom_web::HttpError;
use tokio::task::JoinHandle;

use crate::{Authentication, SecurityContext};

tokio::task_local! {
    static CURRENT: SecurityContext;
}

/// Why a method security check refused a call: the caller is anonymous or lacks the permission.
///
/// Convert it into an [`HttpError`] with `?` or `.into()`: `Unauthenticated` becomes 401 and
/// `Forbidden` becomes 403.
///
/// # Examples
///
/// ```
/// use oxbloom_security::AccessDenied;
/// use oxbloom_web::{HttpError, IntoResponse};
///
/// let res = HttpError::from(AccessDenied::Forbidden).into_response();
/// assert_eq!(res.status(), 403);
/// assert_eq!(AccessDenied::Unauthenticated.to_string(), "authentication is required");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDenied {
    /// The caller is anonymous (401).
    Unauthenticated,
    /// The caller is authenticated but not allowed (403).
    Forbidden,
}

impl AccessDenied {
    /// Picks the denial that fits the caller: `Unauthenticated` for `None`, `Forbidden` for `Some`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{AccessDenied, Authentication};
    ///
    /// assert_eq!(AccessDenied::for_caller(None), AccessDenied::Unauthenticated);
    /// let alice = Authentication::new("alice", []);
    /// assert_eq!(AccessDenied::for_caller(Some(&alice)), AccessDenied::Forbidden);
    /// ```
    pub fn for_caller(auth: Option<&Authentication>) -> Self {
        match auth {
            None => Self::Unauthenticated,
            Some(_) => Self::Forbidden,
        }
    }
}

impl fmt::Display for AccessDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthenticated => f.write_str("authentication is required"),
            Self::Forbidden => f.write_str("access is denied"),
        }
    }
}

impl std::error::Error for AccessDenied {}

impl From<AccessDenied> for HttpError {
    fn from(denied: AccessDenied) -> Self {
        match denied {
            AccessDenied::Unauthenticated => HttpError::new(StatusCode::UNAUTHORIZED),
            AccessDenied::Forbidden => HttpError::new(StatusCode::FORBIDDEN),
        }
    }
}

impl SecurityContext {
    /// Returns a clone of the context of the running request, or the anonymous context outside
    /// any [`SecurityContext::scope`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::SecurityContext;
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// assert!(SecurityContext::current().authentication().is_none());
    /// # });
    /// ```
    pub fn current() -> SecurityContext {
        CURRENT
            .try_with(Clone::clone)
            .unwrap_or_else(|_| SecurityContext::anonymous())
    }

    /// Runs `future` with `context` as the current context. Tasks started with
    /// `tokio::spawn` inside do not inherit it; use [`spawn`] for that.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{Authentication, SecurityContext};
    ///
    /// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
    /// let ctx = SecurityContext::authenticated(Authentication::new("alice", []));
    /// let name = SecurityContext::scope(ctx, async {
    ///     SecurityContext::current().authentication().map(|a| a.principal().to_owned())
    /// })
    /// .await;
    /// assert_eq!(name.as_deref(), Some("alice"));
    /// # });
    /// ```
    pub async fn scope<F: Future>(context: SecurityContext, future: F) -> F::Output {
        CURRENT.scope(context, future).await
    }
}

/// Spawns a task that runs with the security context of the caller, which `tokio::spawn` does
/// not do.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Authentication, SecurityContext, spawn};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let ctx = SecurityContext::authenticated(Authentication::new("alice", []));
/// let name = SecurityContext::scope(ctx, async {
///     spawn(async {
///         SecurityContext::current().authentication().map(|a| a.principal().to_owned())
///     })
///     .await
///     .unwrap()
/// })
/// .await;
/// assert_eq!(name.as_deref(), Some("alice"));
/// # });
/// ```
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(SecurityContext::scope(SecurityContext::current(), future))
}

/// Support for the code `#[method_security]` generates and for [`AuthorizationRule`]; not a
/// stable API.
///
/// [`AuthorizationRule`]: crate::AuthorizationRule
#[doc(hidden)]
pub mod __private {
    use super::AccessDenied;
    use crate::Authentication;

    /// True when the caller holds `ROLE_<role>`.
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority, __private};
    ///
    /// let admin = Authentication::new("a", [GrantedAuthority::new("ROLE_ADMIN")]);
    /// assert!(__private::has_role(Some(&admin), "ADMIN"));
    /// ```
    pub fn has_role(auth: Option<&Authentication>, role: &str) -> bool {
        auth.is_some_and(|a| a.has_role(role))
    }

    /// True when the caller holds any of the roles.
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority, __private};
    ///
    /// let user = Authentication::new("u", [GrantedAuthority::new("ROLE_USER")]);
    /// assert!(__private::has_any_role(Some(&user), &["ADMIN", "USER"]));
    /// ```
    pub fn has_any_role(auth: Option<&Authentication>, roles: &[&str]) -> bool {
        auth.is_some_and(|a| roles.iter().any(|r| a.has_role(r)))
    }

    /// True when the caller holds exactly this authority.
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority, __private};
    ///
    /// let reader = Authentication::new("r", [GrantedAuthority::new("read")]);
    /// assert!(__private::has_authority(Some(&reader), "read"));
    /// ```
    pub fn has_authority(auth: Option<&Authentication>, name: &str) -> bool {
        auth.is_some_and(|a| a.has_authority(name))
    }

    /// True when the caller holds any of the authorities.
    ///
    /// ```
    /// use oxbloom_security::{Authentication, GrantedAuthority, __private};
    ///
    /// let reader = Authentication::new("r", [GrantedAuthority::new("read")]);
    /// assert!(__private::has_any_authority(Some(&reader), &["write", "read"]));
    /// ```
    pub fn has_any_authority(auth: Option<&Authentication>, names: &[&str]) -> bool {
        auth.is_some_and(|a| names.iter().any(|n| a.has_authority(n)))
    }

    /// True for any authenticated caller, also a remembered one.
    ///
    /// ```
    /// use oxbloom_security::{Authentication, __private};
    ///
    /// assert!(__private::authenticated(Some(&Authentication::new("u", []).remembered())));
    /// assert!(!__private::authenticated(None));
    /// ```
    pub fn authenticated(auth: Option<&Authentication>) -> bool {
        auth.is_some()
    }

    /// True for an authenticated caller that was not restored from a remember-me cookie.
    ///
    /// ```
    /// use oxbloom_security::{Authentication, __private};
    ///
    /// let remembered = Authentication::new("u", []).remembered();
    /// assert!(!__private::fully_authenticated(Some(&remembered)));
    /// assert!(__private::fully_authenticated(Some(&Authentication::new("u", []))));
    /// ```
    pub fn fully_authenticated(auth: Option<&Authentication>) -> bool {
        auth.is_some_and(|a| !a.is_remembered())
    }

    /// Logs the refusal at debug level (method and principal only, never arguments or results)
    /// and returns the matching [`AccessDenied`].
    ///
    /// ```
    /// use oxbloom_security::{AccessDenied, __private};
    ///
    /// assert_eq!(__private::deny("Service::delete", None), AccessDenied::Unauthenticated);
    /// ```
    pub fn deny(method: &'static str, auth: Option<&Authentication>) -> AccessDenied {
        let principal = auth.map_or("anonymous", Authentication::principal);
        tracing::debug!(target: "oxbloom::security", method, principal, "method access denied");
        AccessDenied::for_caller(auth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Authentication, GrantedAuthority, SecurityContext};
    use oxbloom_web::{HttpError, IntoResponse};

    fn user(auths: &[&str]) -> Authentication {
        Authentication::new("u", auths.iter().map(|a| GrantedAuthority::new(*a)))
    }

    fn ctx(name: &str) -> SecurityContext {
        SecurityContext::authenticated(Authentication::new(name, []))
    }

    #[tokio::test]
    async fn current_is_anonymous_without_a_scope() {
        assert!(SecurityContext::current().authentication().is_none());
    }

    #[tokio::test]
    async fn scope_makes_the_context_current_and_restores_nothing_after() {
        let seen = SecurityContext::scope(ctx("alice"), async {
            SecurityContext::current()
                .authentication()
                .map(|a| a.principal().to_owned())
        })
        .await;
        assert_eq!(seen.as_deref(), Some("alice"));
        assert!(SecurityContext::current().authentication().is_none());
    }

    #[tokio::test]
    async fn spawn_carries_the_context() {
        let seen = SecurityContext::scope(ctx("alice"), async {
            spawn(async {
                SecurityContext::current()
                    .authentication()
                    .map(|a| a.principal().to_owned())
            })
            .await
            .unwrap()
        })
        .await;
        assert_eq!(seen.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn tokio_spawn_does_not_carry_the_context() {
        let anonymous = SecurityContext::scope(ctx("alice"), async {
            tokio::spawn(async { SecurityContext::current().authentication().is_none() })
                .await
                .unwrap()
        })
        .await;
        assert!(anonymous);
    }

    #[test]
    fn for_caller_maps_anonymous_to_unauthenticated_and_authenticated_to_forbidden() {
        assert_eq!(
            AccessDenied::for_caller(None),
            AccessDenied::Unauthenticated
        );
        assert_eq!(
            AccessDenied::for_caller(Some(&user(&[]))),
            AccessDenied::Forbidden
        );
    }

    #[test]
    fn access_denied_converts_to_401_and_403() {
        let status = |d: AccessDenied| HttpError::from(d).into_response().status();
        assert_eq!(status(AccessDenied::Unauthenticated), 401);
        assert_eq!(status(AccessDenied::Forbidden), 403);
    }

    #[test]
    fn access_denied_displays_and_is_an_error() {
        let e: &dyn std::error::Error = &AccessDenied::Forbidden;
        assert!(!e.to_string().is_empty());
        assert_ne!(
            AccessDenied::Forbidden.to_string(),
            AccessDenied::Unauthenticated.to_string()
        );
    }

    #[test]
    fn has_role_matches_role_prefix() {
        let u = user(&["ROLE_ADMIN"]);
        assert!(__private::has_role(Some(&u), "ADMIN"));
        assert!(!__private::has_role(Some(&u), "USER"));
        assert!(!__private::has_role(None, "ADMIN"));
    }

    #[test]
    fn has_any_role_matches_one() {
        let u = user(&["ROLE_USER"]);
        assert!(__private::has_any_role(Some(&u), &["ADMIN", "USER"]));
        assert!(!__private::has_any_role(Some(&u), &["ADMIN"]));
        assert!(!__private::has_any_role(None, &["USER"]));
    }

    #[test]
    fn has_authority_matches_exactly() {
        let u = user(&["read"]);
        assert!(__private::has_authority(Some(&u), "read"));
        assert!(!__private::has_authority(Some(&u), "write"));
        assert!(!__private::has_authority(None, "read"));
    }

    #[test]
    fn has_any_authority_matches_one() {
        let u = user(&["read"]);
        assert!(__private::has_any_authority(Some(&u), &["write", "read"]));
        assert!(!__private::has_any_authority(Some(&u), &["write"]));
        assert!(!__private::has_any_authority(None, &["read"]));
    }

    #[test]
    fn authenticated_accepts_remembered() {
        assert!(!__private::authenticated(None));
        assert!(__private::authenticated(Some(&user(&[]))));
        assert!(__private::authenticated(Some(&user(&[]).remembered())));
    }

    #[test]
    fn fully_authenticated_refuses_remembered() {
        assert!(!__private::fully_authenticated(None));
        assert!(__private::fully_authenticated(Some(&user(&[]))));
        assert!(!__private::fully_authenticated(Some(
            &Authentication::new("u", []).remembered()
        )));
    }

    #[test]
    fn deny_maps_the_caller() {
        assert_eq!(__private::deny("m", None), AccessDenied::Unauthenticated);
        assert_eq!(
            __private::deny("m", Some(&user(&[]))),
            AccessDenied::Forbidden
        );
    }
}
