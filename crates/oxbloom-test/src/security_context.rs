use std::future::Future;

use oxbloom_security::{Authentication, SecurityContext};

/// Runs `future` with `authentication` as the current caller, so a secured service can be
/// tested without HTTP.
///
/// This is a thin wrapper over `SecurityContext::scope(SecurityContext::authenticated(..), ..)`.
/// Code that runs outside the future, or in a task started with a bare `tokio::spawn`, does not
/// see the caller; use `oxbloom_security::spawn` for that. Needs the feature `security`.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{Authentication, GrantedAuthority, SecurityContext};
/// use oxbloom_test::with_authentication;
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let admin = Authentication::new("ada", [GrantedAuthority::new("ROLE_ADMIN")]);
/// let name = with_authentication(admin, async {
///     SecurityContext::current()
///         .authentication()
///         .map(|a| a.principal().to_owned())
/// })
/// .await;
/// assert_eq!(name.as_deref(), Some("ada"));
/// # Ok::<(), std::io::Error>(())
/// # })?;
/// # Ok::<(), std::io::Error>(())
/// ```
pub async fn with_authentication<F: Future>(
    authentication: Authentication,
    future: F,
) -> F::Output {
    SecurityContext::scope(SecurityContext::authenticated(authentication), future).await
}
