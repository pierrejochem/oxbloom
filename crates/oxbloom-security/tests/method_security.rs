//! `#[method_security]` with `#[pre_authorize]`: every rule form, the combinators, custom checks,
//! the denial log, controller handlers and DI bean methods.

use std::future::Future;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use oxbloom_security::{
    AccessDenied, Authentication, GrantedAuthority, SecurityContext, method_security,
};
use tracing_subscriber::fmt::MakeWriter;

fn caller(name: &str, authorities: &[&str]) -> Option<Authentication> {
    Some(Authentication::new(
        name,
        authorities.iter().map(|a| GrantedAuthority::new(*a)),
    ))
}

fn admin() -> Option<Authentication> {
    caller("ada", &["ROLE_ADMIN"])
}

fn user() -> Option<Authentication> {
    caller("bob", &["ROLE_USER"])
}

/// Runs `future` with `auth` as the current caller (`None`: anonymous).
async fn as_caller<F: Future>(auth: Option<Authentication>, future: F) -> F::Output {
    let context = match auth {
        Some(a) => SecurityContext::authenticated(a),
        None => SecurityContext::anonymous(),
    };
    SecurityContext::scope(context, future).await
}

#[derive(Debug, PartialEq)]
enum ServiceError {
    Denied(AccessDenied),
}

impl From<AccessDenied> for ServiceError {
    fn from(denied: AccessDenied) -> Self {
        Self::Denied(denied)
    }
}

/// True when the caller is `owner` and the account id is below 100.
fn owns(auth: Option<&Authentication>, owner: &&str, id: &u64) -> bool {
    auth.is_some_and(|a| a.principal() == *owner) && *id < 100
}

#[derive(Default)]
struct Accounts {
    runs: AtomicU32,
}

#[method_security]
impl Accounts {
    #[pre_authorize(role = "ADMIN")]
    async fn close(&self, id: u64) -> Result<u64, AccessDenied> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(id)
    }

    #[pre_authorize(any_role("ADMIN", "AUDITOR"))]
    async fn audit(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(authority = "accounts:read")]
    async fn read(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(any_authority("accounts:read", "accounts:write"))]
    async fn browse(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(authenticated)]
    async fn profile(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(fully_authenticated)]
    async fn change_password(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(permit_all)]
    async fn status(&self) -> Result<&'static str, AccessDenied> {
        Ok("up")
    }

    #[pre_authorize(deny_all)]
    async fn wipe(&self) -> Result<(), AccessDenied> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[pre_authorize(all(role = "ADMIN", authority = "accounts:write"))]
    async fn transfer(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(any(role = "ADMIN", authority = "accounts:write"))]
    async fn deposit(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(not(role = "BANNED"))]
    async fn comment(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(all(authenticated, not(any_role("BANNED", "LOCKED"))))]
    async fn post(&self) -> Result<(), AccessDenied> {
        Ok(())
    }

    #[pre_authorize(custom = owns(owner, id))]
    fn rename(&self, owner: &str, id: u64, name: String) -> Result<String, ServiceError> {
        Ok(format!("{id}:{owner}:{name}"))
    }

    #[pre_authorize(role = "ADMIN")]
    fn count(&self) -> Result<u32, ServiceError> {
        Ok(self.runs.load(Ordering::SeqCst))
    }

    /// Not secured; it must stay as written.
    fn helper(&self) -> u32 {
        7
    }
}

#[tokio::test]
async fn role_allows_and_denies() {
    let svc = Accounts::default();
    assert_eq!(as_caller(admin(), svc.close(1)).await, Ok(1));
    assert_eq!(
        as_caller(user(), svc.close(1)).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        as_caller(None, svc.close(1)).await,
        Err(AccessDenied::Unauthenticated)
    );
}

#[tokio::test]
async fn any_role_allows_one_of_the_roles() {
    let svc = Accounts::default();
    assert!(as_caller(admin(), svc.audit()).await.is_ok());
    assert!(
        as_caller(caller("eve", &["ROLE_AUDITOR"]), svc.audit())
            .await
            .is_ok()
    );
    assert_eq!(
        as_caller(user(), svc.audit()).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        as_caller(None, svc.audit()).await,
        Err(AccessDenied::Unauthenticated)
    );
}

#[tokio::test]
async fn authority_matches_exactly() {
    let svc = Accounts::default();
    let reader = caller("r", &["accounts:read"]);
    assert!(as_caller(reader, svc.read()).await.is_ok());
    assert_eq!(
        as_caller(caller("w", &["accounts:write"]), svc.read()).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        as_caller(None, svc.read()).await,
        Err(AccessDenied::Unauthenticated)
    );
}

#[tokio::test]
async fn any_authority_allows_one_of_the_authorities() {
    let svc = Accounts::default();
    assert!(
        as_caller(caller("w", &["accounts:write"]), svc.browse())
            .await
            .is_ok()
    );
    assert_eq!(
        as_caller(admin(), svc.browse()).await,
        Err(AccessDenied::Forbidden)
    );
}

#[tokio::test]
async fn authenticated_accepts_any_caller_and_refuses_anonymous() {
    let svc = Accounts::default();
    assert!(as_caller(caller("x", &[]), svc.profile()).await.is_ok());
    assert_eq!(
        as_caller(None, svc.profile()).await,
        Err(AccessDenied::Unauthenticated)
    );
}

#[tokio::test]
async fn fully_authenticated_refuses_a_remembered_caller_but_authenticated_accepts_it() {
    let svc = Accounts::default();
    let remembered = || Some(Authentication::new("ada", []).remembered());
    assert!(as_caller(remembered(), svc.profile()).await.is_ok());
    assert_eq!(
        as_caller(remembered(), svc.change_password()).await,
        Err(AccessDenied::Forbidden)
    );
    assert!(
        as_caller(caller("ada", &[]), svc.change_password())
            .await
            .is_ok()
    );
    assert_eq!(
        as_caller(None, svc.change_password()).await,
        Err(AccessDenied::Unauthenticated)
    );
}

#[tokio::test]
async fn permit_all_allows_anonymous() {
    let svc = Accounts::default();
    assert_eq!(as_caller(None, svc.status()).await, Ok("up"));
    assert_eq!(as_caller(admin(), svc.status()).await, Ok("up"));
}

#[tokio::test]
async fn deny_all_refuses_everyone() {
    let svc = Accounts::default();
    assert_eq!(
        as_caller(admin(), svc.wipe()).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        as_caller(None, svc.wipe()).await,
        Err(AccessDenied::Unauthenticated)
    );
    assert_eq!(svc.runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn all_requires_every_rule() {
    let svc = Accounts::default();
    let both = caller("a", &["ROLE_ADMIN", "accounts:write"]);
    assert!(as_caller(both, svc.transfer()).await.is_ok());
    assert_eq!(
        as_caller(admin(), svc.transfer()).await,
        Err(AccessDenied::Forbidden)
    );
}

#[tokio::test]
async fn any_accepts_one_rule() {
    let svc = Accounts::default();
    assert!(as_caller(admin(), svc.deposit()).await.is_ok());
    assert!(
        as_caller(caller("w", &["accounts:write"]), svc.deposit())
            .await
            .is_ok()
    );
    assert_eq!(
        as_caller(user(), svc.deposit()).await,
        Err(AccessDenied::Forbidden)
    );
}

#[tokio::test]
async fn not_inverts_the_rule() {
    let svc = Accounts::default();
    assert!(as_caller(user(), svc.comment()).await.is_ok());
    assert!(as_caller(None, svc.comment()).await.is_ok());
    assert_eq!(
        as_caller(caller("x", &["ROLE_BANNED"]), svc.comment()).await,
        Err(AccessDenied::Forbidden)
    );
}

#[tokio::test]
async fn combinators_nest() {
    let svc = Accounts::default();
    assert!(as_caller(user(), svc.post()).await.is_ok());
    assert_eq!(
        as_caller(caller("x", &["ROLE_LOCKED"]), svc.post()).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        as_caller(None, svc.post()).await,
        Err(AccessDenied::Unauthenticated)
    );
}

#[tokio::test]
async fn custom_check_receives_the_named_arguments() {
    let svc = Accounts::default();
    let alice = || caller("alice", &[]);
    let allowed = as_caller(alice(), async { svc.rename("alice", 7, "main".into()) }).await;
    assert_eq!(allowed, Ok("7:alice:main".to_owned()));
    let other_owner = as_caller(alice(), async { svc.rename("bob", 7, "main".into()) }).await;
    assert_eq!(
        other_owner,
        Err(ServiceError::Denied(AccessDenied::Forbidden))
    );
    let large_id = as_caller(alice(), async { svc.rename("alice", 700, "main".into()) }).await;
    assert_eq!(large_id, Err(ServiceError::Denied(AccessDenied::Forbidden)));
    let anonymous = as_caller(None, async { svc.rename("alice", 7, "main".into()) }).await;
    assert_eq!(
        anonymous,
        Err(ServiceError::Denied(AccessDenied::Unauthenticated))
    );
}

#[tokio::test]
async fn a_denied_call_does_not_run_the_body() {
    let svc = Accounts::default();
    assert!(as_caller(user(), svc.close(1)).await.is_err());
    assert!(as_caller(admin(), svc.wipe()).await.is_err());
    assert_eq!(svc.runs.load(Ordering::SeqCst), 0);
    assert!(as_caller(admin(), svc.close(1)).await.is_ok());
    assert_eq!(svc.runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn works_on_async_and_sync_methods() {
    let svc = Accounts::default();
    assert_eq!(as_caller(admin(), svc.close(3)).await, Ok(3));
    assert_eq!(as_caller(admin(), async { svc.count() }).await, Ok(1));
    assert_eq!(
        as_caller(user(), async { svc.count() }).await,
        Err(ServiceError::Denied(AccessDenied::Forbidden))
    );
    assert_eq!(svc.helper(), 7);
}

#[tokio::test]
async fn no_scope_means_anonymous() {
    let svc = Accounts::default();
    assert_eq!(svc.close(1).await, Err(AccessDenied::Unauthenticated));
    assert_eq!(
        svc.count(),
        Err(ServiceError::Denied(AccessDenied::Unauthenticated))
    );
    assert_eq!(svc.status().await, Ok("up"));
}

#[test]
fn a_sync_method_works_without_a_runtime() {
    let svc = Accounts::default();
    assert_eq!(
        svc.count(),
        Err(ServiceError::Denied(AccessDenied::Unauthenticated))
    );
}

mod controller {
    use std::sync::Arc;

    use oxbloom_security::{SecurityContext, method_security};
    use oxbloom_web::{App, HttpError, HttpMethod, Request, Response, controller};

    use super::{admin, user};

    struct Admin;

    #[controller("/x")]
    #[method_security]
    impl Admin {
        #[get("/a")]
        #[pre_authorize(role = "ADMIN")]
        async fn a(&self, _req: Request) -> Result<&'static str, HttpError> {
            Ok("secret")
        }

        #[get("/open")]
        async fn open(&self, _req: Request) -> &'static str {
            "open"
        }
    }

    struct Reports;

    #[method_security]
    #[controller("/r")]
    impl Reports {
        #[pre_authorize(authenticated)]
        #[get("")]
        async fn index(&self, _req: Request) -> Result<&'static str, HttpError> {
            Ok("reports")
        }
    }

    fn app() -> App {
        App::new()
            .routes(Arc::new(Admin).routes())
            .unwrap()
            .routes(Arc::new(Reports).routes())
            .unwrap()
    }

    async fn get(auth: Option<oxbloom_security::Authentication>, path: &str) -> Response {
        let context = match auth {
            Some(a) => SecurityContext::authenticated(a),
            None => SecurityContext::anonymous(),
        };
        SecurityContext::scope(context, app().handle(Request::new(HttpMethod::Get, path))).await
    }

    #[tokio::test]
    async fn controller_handler_can_be_secured() {
        let ok = get(admin(), "/x/a").await;
        assert_eq!(ok.status(), 200);
        assert_eq!(ok.body(), "secret");
        assert_eq!(get(user(), "/x/a").await.status(), 403);
        assert_eq!(get(None, "/x/a").await.status(), 401);
        assert_eq!(get(None, "/x/open").await.status(), 200);
    }

    #[tokio::test]
    async fn method_security_above_controller_also_works() {
        assert_eq!(get(user(), "/r").await.status(), 200);
        assert_eq!(get(None, "/r").await.status(), 401);
    }
}

#[cfg(feature = "di")]
mod di {
    use oxbloom_di::{ContainerBuilder, component};
    use oxbloom_security::{AccessDenied, method_security};

    use super::{admin, as_caller, user};

    #[component]
    struct Ledger;

    #[method_security]
    impl Ledger {
        #[pre_authorize(role = "ADMIN")]
        async fn close_books(&self) -> Result<&'static str, AccessDenied> {
            Ok("closed")
        }
    }

    #[tokio::test]
    async fn a_secured_di_bean_method_denies() {
        let container = ContainerBuilder::new()
            .component::<Ledger>()
            .build()
            .unwrap();
        let ledger = container.get::<Ledger>().unwrap();
        assert_eq!(
            as_caller(user(), ledger.close_books()).await,
            Err(AccessDenied::Forbidden)
        );
        assert_eq!(as_caller(admin(), ledger.close_books()).await, Ok("closed"));
    }
}

/// Captures the log output written while the subscriber guard lives.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn the_denial_log_names_the_method_and_the_principal_but_no_argument() {
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let svc = Accounts::default();
    let denied = as_caller(caller("mallory", &[]), async {
        svc.rename("victim-owner", 4242, "secret-name".into())
    })
    .await;
    assert!(denied.is_err());

    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("oxbloom::security"), "{text}");
    assert!(text.contains("Accounts::rename"), "{text}");
    assert!(text.contains("mallory"), "{text}");
    for value in ["victim-owner", "4242", "secret-name"] {
        assert!(!text.contains(value), "argument {value} leaked: {text}");
    }
}
