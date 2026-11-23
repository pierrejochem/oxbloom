//! A service method secured with `#[method_security]`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use oxbloom_di::component;
use oxbloom_security::{AccessDenied, method_security};

use crate::todo::TodoStore;

/// Administrative actions on the todos, guarded by method security instead of by a path rule.
///
/// The caller is read from the ambient security context, so the rule holds for every caller of
/// the method: the HTTP route `DELETE /api/admin/done`, a scheduled task or a test. A denied call
/// does not run the body.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_security::{AccessDenied, Authentication, GrantedAuthority, SecurityContext};
/// use todos::admin::TodoAdmin;
/// use todos::todo::TodoStore;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let store = Arc::new(TodoStore::default());
/// let admin = TodoAdmin { store };
///
/// // No caller in scope: anonymous.
/// assert_eq!(admin.purge_done(), Err(AccessDenied::Unauthenticated));
///
/// let root = Authentication::new("root", [GrantedAuthority::new("ROLE_ADMIN")]);
/// let ctx = SecurityContext::authenticated(root);
/// let removed = SecurityContext::scope(ctx, async { admin.purge_done() }).await;
/// assert_eq!(removed, Ok(0));
/// # }
/// ```
#[component]
pub struct TodoAdmin {
    /// The store the actions work on.
    pub store: Arc<TodoStore>,
}

#[method_security]
impl TodoAdmin {
    /// Removes every done todo and returns how many were removed. Needs the role `ADMIN`: an
    /// anonymous caller gets `AccessDenied::Unauthenticated` and any other caller
    /// `AccessDenied::Forbidden`.
    #[pre_authorize(role = "ADMIN")]
    pub fn purge_done(&self) -> Result<usize, AccessDenied> {
        Ok(self
            .store
            .remove_done_before(SystemTime::now() + Duration::from_secs(1)))
    }
}
