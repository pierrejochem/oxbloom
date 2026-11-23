//! `#[method_security]` with `#[pre_authorize]`, `#[post_authorize]`, `#[pre_filter]` and
//! `#[post_filter]`: every rule form, the combinators, custom checks, the filters, the order of
//! the checks, the denial log, controller handlers and DI bean methods.
//!
//! The whole file denies warnings, so the generated code must not cause a warning such as
//! `unused_braces` on a one-line body or `unused_mut` on a parameter that is already `mut`.
#![deny(warnings)]

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
    NotFound,
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

/// `#[method_security]` must keep securing its methods when `#[configuration]` is on the same
/// impl, in either order. A `#[configuration]` that stripped the method attributes would make
/// the secured method fail open, so both orders are pinned here.
#[cfg(feature = "di")]
mod configuration {
    use oxbloom_di::configuration;
    use oxbloom_security::{AccessDenied, SecurityContext, method_security};

    use super::{admin, as_caller};

    struct Pool;

    struct Outer;

    #[method_security]
    #[configuration]
    impl Outer {
        #[bean]
        fn pool(&self) -> Pool {
            Pool
        }

        #[pre_authorize(role = "ADMIN")]
        fn secured(&self) -> Result<&'static str, AccessDenied> {
            Ok("outer")
        }
    }

    struct Inner;

    #[configuration]
    #[method_security]
    impl Inner {
        #[bean]
        fn pool(&self) -> Pool {
            Pool
        }

        #[pre_authorize(role = "ADMIN")]
        fn secured(&self) -> Result<&'static str, AccessDenied> {
            Ok("inner")
        }
    }

    #[tokio::test]
    async fn method_security_before_configuration_still_secures_the_method() {
        assert_eq!(Outer.secured(), Err(AccessDenied::Unauthenticated));
        assert_eq!(
            SecurityContext::scope(SecurityContext::anonymous(), async { Outer.secured() }).await,
            Err(AccessDenied::Unauthenticated)
        );
        assert_eq!(
            as_caller(admin(), async { Outer.secured() }).await,
            Ok("outer")
        );
    }

    #[tokio::test]
    async fn method_security_after_configuration_still_secures_the_method() {
        assert_eq!(Inner.secured(), Err(AccessDenied::Unauthenticated));
        assert_eq!(
            SecurityContext::scope(SecurityContext::anonymous(), async { Inner.secured() }).await,
            Err(AccessDenied::Unauthenticated)
        );
        assert_eq!(
            as_caller(admin(), async { Inner.secured() }).await,
            Ok("inner")
        );
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

/// A document with an owner, for the checks on a returned value. The token shows whether the
/// document was dropped.
#[derive(Debug, Clone)]
struct Document {
    owner: String,
    title: String,
    _token: Arc<()>,
}

impl PartialEq for Document {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner && self.title == other.title
    }
}

fn document(owner: &str, title: &str, token: Arc<()>) -> Document {
    Document {
        owner: owner.to_owned(),
        title: title.to_owned(),
        _token: token,
    }
}

/// True when the caller owns the returned document.
fn owns_document(auth: Option<&Authentication>, doc: &Document) -> bool {
    auth.is_some_and(|a| a.principal() == doc.owner)
}

/// True when the caller owns the document and asked for it under this title.
fn owns_titled(auth: Option<&Authentication>, doc: &Document, title: &String) -> bool {
    owns_document(auth, doc) && doc.title == *title
}

/// A post check that must never run.
fn never(_auth: Option<&Authentication>, _value: &u32) -> bool {
    panic!("the post check ran")
}

/// Keeps the even numbers for an authenticated caller and nothing for an anonymous one.
fn even_for_callers(auth: Option<&Authentication>, n: &u32) -> bool {
    auth.is_some() && n.is_multiple_of(2)
}

#[derive(Default)]
struct Library;

#[method_security]
impl Library {
    #[post_authorize(custom = owns_document(returned))]
    fn fetch(&self, owner: &str, token: Arc<()>) -> Result<Document, AccessDenied> {
        Ok(document(owner, "report", token))
    }

    #[post_authorize(custom = owns_titled(returned, title))]
    fn fetch_titled(&self, owner: &str, title: String) -> Result<Document, AccessDenied> {
        Ok(document(owner, &title, Arc::new(())))
    }

    #[post_authorize(custom = never(returned))]
    fn missing(&self) -> Result<u32, ServiceError> {
        Err(ServiceError::NotFound)
    }

    #[post_authorize(custom = owns_document(returned))]
    fn find(&self, owner: &str, early: bool) -> Result<Document, AccessDenied> {
        if early {
            return Ok(document(owner, "early", Arc::new(())));
        }
        Ok(document(owner, "late", Arc::new(())))
    }

    #[post_authorize(authenticated)]
    fn parse(&self, text: &str) -> Result<u32, ServiceError> {
        let n: u32 = text.parse().map_err(|_| ServiceError::NotFound)?;
        Ok(n)
    }

    #[pre_filter(arg = ids, custom = even_for_callers)]
    fn keep(&self, ids: Vec<u32>) -> Result<Vec<u32>, AccessDenied> {
        Ok(ids)
    }

    /// A pre filter needs no `Result`, because a filter never denies.
    #[pre_filter(arg = ids, custom = even_for_callers)]
    fn count(&self, ids: Vec<u32>) -> usize {
        ids.len()
    }

    /// The parameter is already `mut`; the filter runs before the body pushes 100.
    #[pre_filter(arg = ids, custom = even_for_callers)]
    fn keep_and_push(&self, mut ids: Vec<u32>) -> Vec<u32> {
        ids.push(100);
        ids
    }

    #[post_filter(custom = even_for_callers)]
    fn list(&self, ids: Vec<u32>) -> Result<Vec<u32>, AccessDenied> {
        Ok(ids)
    }

    #[post_filter(custom = even_for_callers)]
    fn list_missing(&self) -> Result<Vec<u32>, ServiceError> {
        Err(ServiceError::NotFound)
    }

    #[post_authorize(custom = owns_document(returned))]
    async fn load(&self, owner: &str, early: bool) -> Result<Document, AccessDenied> {
        tokio::task::yield_now().await;
        if early {
            return Ok(document(owner, "early", Arc::new(())));
        }
        Ok(document(owner, "late", Arc::new(())))
    }

    #[post_filter(custom = even_for_callers)]
    async fn load_list(&self, ids: Vec<u32>) -> Result<Vec<u32>, AccessDenied> {
        tokio::task::yield_now().await;
        Ok(ids)
    }

    #[pre_authorize(authenticated)]
    #[pre_filter(arg = ids, custom = even_for_callers)]
    #[post_authorize(authenticated)]
    #[post_filter(custom = even_for_callers)]
    async fn load_all(&self, ids: Vec<u32>) -> Result<Vec<u32>, AccessDenied> {
        tokio::task::yield_now().await;
        Ok(ids.into_iter().chain([7, 8]).collect())
    }

    /// One-line bodies: no `unused_braces` warning may appear for them. rustc reports that
    /// lint only for braces on one line, so rustfmt must keep these bodies on one line.
    #[rustfmt::skip]
    #[pre_authorize(authenticated)]
    fn one(&self) -> Result<u8, AccessDenied> { Ok(1) }

    #[rustfmt::skip]
    #[post_authorize(authenticated)]
    fn two(&self) -> Result<u8, AccessDenied> { Ok(2) }

    #[rustfmt::skip]
    #[pre_authorize(authenticated)]
    async fn three(&self) -> Result<u8, AccessDenied> { Ok(3) }
}

#[tokio::test]
async fn post_authorize_sees_the_returned_value() {
    let lib = Library;
    let token = Arc::new(());
    let mine = as_caller(caller("alice", &[]), async {
        lib.fetch("alice", token.clone())
    })
    .await;
    assert_eq!(mine, Ok(document("alice", "report", Arc::new(()))));
    drop(mine);

    let theirs = as_caller(caller("bob", &[]), async {
        lib.fetch("alice", token.clone())
    })
    .await;
    assert_eq!(theirs, Err(AccessDenied::Forbidden));
    assert_eq!(
        Arc::strong_count(&token),
        1,
        "the denied value was not dropped"
    );

    let anonymous = as_caller(None, async { lib.fetch("alice", token.clone()) }).await;
    assert_eq!(anonymous, Err(AccessDenied::Unauthenticated));
}

#[tokio::test]
async fn post_authorize_can_also_read_a_parameter() {
    let lib = Library;
    let alice = || caller("alice", &[]);
    let ok = as_caller(alice(), async { lib.fetch_titled("alice", "notes".into()) }).await;
    assert_eq!(ok.map(|d| d.title), Ok("notes".to_owned()));
    let other = as_caller(caller("bob", &[]), async {
        lib.fetch_titled("alice", "notes".into())
    })
    .await;
    assert_eq!(other, Err(AccessDenied::Forbidden));
}

#[tokio::test]
async fn post_authorize_skipped_on_err() {
    let lib = Library;
    assert_eq!(
        as_caller(None, async { lib.missing() }).await,
        Err(ServiceError::NotFound)
    );
}

#[tokio::test]
async fn early_return_in_the_body_still_reaches_post_authorize() {
    let lib = Library;
    let bob = || caller("bob", &[]);
    assert_eq!(
        as_caller(bob(), async { lib.find("alice", true) }).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        as_caller(bob(), async { lib.find("bob", true) })
            .await
            .map(|d| d.title),
        Ok("early".to_owned())
    );
    assert_eq!(
        as_caller(bob(), async { lib.find("alice", false) }).await,
        Err(AccessDenied::Forbidden)
    );
    // `?` in the body leaves only the body, so its error passes through the post check.
    assert_eq!(
        as_caller(bob(), async { lib.parse("x") }).await,
        Err(ServiceError::NotFound)
    );
    assert_eq!(as_caller(bob(), async { lib.parse("4") }).await, Ok(4));
    assert_eq!(
        lib.parse("4"),
        Err(ServiceError::Denied(AccessDenied::Unauthenticated))
    );
}

#[tokio::test]
async fn pre_filter_retains_matching_elements() {
    let lib = Library;
    let bob = || caller("bob", &[]);
    assert_eq!(
        as_caller(bob(), async { lib.keep(vec![1, 2, 3, 4]) }).await,
        Ok(vec![2, 4])
    );
    assert_eq!(
        as_caller(bob(), async { lib.count(vec![1, 2, 3, 4, 6]) }).await,
        3
    );
    assert_eq!(
        as_caller(bob(), async { lib.keep_and_push(vec![1, 2, 3]) }).await,
        vec![2, 100]
    );
}

#[tokio::test]
async fn post_filter_retains_matching_elements() {
    let lib = Library;
    assert_eq!(
        as_caller(caller("bob", &[]), async { lib.list(vec![1, 2, 3, 4]) }).await,
        Ok(vec![2, 4])
    );
}

#[tokio::test]
async fn a_filter_that_removes_everything_gives_an_empty_vec() {
    let lib = Library;
    let bob = || caller("bob", &[]);
    assert_eq!(
        as_caller(bob(), async { lib.keep(vec![1, 3]) }).await,
        Ok(vec![])
    );
    assert_eq!(
        as_caller(bob(), async { lib.list(vec![5, 7]) }).await,
        Ok(vec![])
    );
}

#[tokio::test]
async fn filters_never_deny() {
    let lib = Library;
    assert_eq!(
        as_caller(None, async { lib.keep(vec![2, 4]) }).await,
        Ok(vec![])
    );
    assert_eq!(
        as_caller(None, async { lib.list(vec![2, 4]) }).await,
        Ok(vec![])
    );
    assert_eq!(lib.count(vec![2, 4]), 0);
}

#[tokio::test]
async fn post_filter_passes_an_err_through() {
    let lib = Library;
    assert_eq!(
        as_caller(caller("bob", &[]), async { lib.list_missing() }).await,
        Err(ServiceError::NotFound)
    );
}

#[tokio::test]
async fn async_methods_with_post_checks() {
    let lib = Library;
    let bob = || caller("bob", &[]);
    assert_eq!(
        as_caller(bob(), lib.load("alice", true)).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        as_caller(bob(), lib.load("bob", false))
            .await
            .map(|d| d.title),
        Ok("late".to_owned())
    );
    assert_eq!(
        as_caller(bob(), lib.load_list(vec![1, 2, 3, 4])).await,
        Ok(vec![2, 4])
    );
    assert_eq!(
        as_caller(bob(), lib.load_all(vec![1, 2, 3, 4])).await,
        Ok(vec![2, 4, 8])
    );
    assert_eq!(
        as_caller(None, lib.load_all(vec![2])).await,
        Err(AccessDenied::Unauthenticated)
    );
}

#[tokio::test]
async fn one_line_bodies_work() {
    let lib = Library;
    let bob = || caller("bob", &[]);
    assert_eq!(as_caller(bob(), async { lib.one() }).await, Ok(1));
    assert_eq!(as_caller(bob(), async { lib.two() }).await, Ok(2));
    assert_eq!(as_caller(bob(), lib.three()).await, Ok(3));
}

/// The calls of the ordered method, in the order they ran. Only `Ordered` writes to it.
static ORDER: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn record_pre_filter(_auth: Option<&Authentication>, _n: &u32) -> bool {
    ORDER.lock().unwrap().push("pre_filter");
    true
}

fn record_pre_authorize(_auth: Option<&Authentication>) -> bool {
    ORDER.lock().unwrap().push("pre_authorize");
    true
}

fn record_post_authorize(_auth: Option<&Authentication>, _value: &Vec<u32>) -> bool {
    ORDER.lock().unwrap().push("post_authorize");
    true
}

fn record_post_filter(_auth: Option<&Authentication>, _n: &u32) -> bool {
    ORDER.lock().unwrap().push("post_filter");
    true
}

struct Ordered;

#[method_security]
impl Ordered {
    // The attributes are listed out of order on purpose: the order of execution is fixed.
    #[post_filter(custom = record_post_filter)]
    #[post_authorize(custom = record_post_authorize(returned))]
    #[pre_authorize(custom = record_pre_authorize())]
    #[pre_filter(arg = ids, custom = record_pre_filter)]
    fn run(&self, ids: Vec<u32>) -> Result<Vec<u32>, AccessDenied> {
        ORDER.lock().unwrap().push("body");
        Ok(ids)
    }
}

#[test]
fn order_is_pre_filter_pre_authorize_body_post_authorize_post_filter() {
    assert_eq!(Ordered.run(vec![1]), Ok(vec![1]));
    assert_eq!(
        *ORDER.lock().unwrap(),
        [
            "pre_filter",
            "pre_authorize",
            "body",
            "post_authorize",
            "post_filter"
        ]
    );
}

/// How often the post checks of `Guarded` ran.
static GUARDED_POST_RUNS: AtomicU32 = AtomicU32::new(0);

fn count_post_authorize(_auth: Option<&Authentication>, _value: &Vec<u32>) -> bool {
    GUARDED_POST_RUNS.fetch_add(1, Ordering::SeqCst);
    true
}

fn count_post_filter(_auth: Option<&Authentication>, _n: &u32) -> bool {
    GUARDED_POST_RUNS.fetch_add(1, Ordering::SeqCst);
    true
}

struct Guarded;

#[method_security]
impl Guarded {
    #[pre_authorize(role = "ADMIN")]
    #[post_authorize(custom = count_post_authorize(returned))]
    #[post_filter(custom = count_post_filter)]
    async fn run(&self) -> Result<Vec<u32>, AccessDenied> {
        Ok(vec![1])
    }
}

#[tokio::test]
async fn a_denied_pre_authorize_does_not_run_post_checks() {
    assert_eq!(
        as_caller(user(), Guarded.run()).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(GUARDED_POST_RUNS.load(Ordering::SeqCst), 0);
    assert_eq!(as_caller(admin(), Guarded.run()).await, Ok(vec![1]));
    assert_eq!(GUARDED_POST_RUNS.load(Ordering::SeqCst), 2);
}

/// Async methods with post checks whose bodies rely on the declared return type: an early
/// `return` that needs a coercion, and tails that diverge. The file denies warnings, so a
/// diverging tail must not cause `unreachable_code`.
struct Shelf {
    name: String,
}

#[method_security]
impl Shelf {
    #[post_authorize(authenticated)]
    async fn label(&self, early: bool) -> Result<&str, AccessDenied> {
        tokio::task::yield_now().await;
        if early {
            return Ok(&self.name);
        }
        Ok("default")
    }

    #[post_authorize(authenticated)]
    async fn boxed(&self, early: bool) -> Result<Box<dyn std::fmt::Debug>, AccessDenied> {
        if early {
            return Ok(Box::new(1));
        }
        Ok(Box::new("late"))
    }

    /// A `return` inside a closure belongs to the closure and stays as written.
    #[allow(clippy::needless_return)]
    #[post_filter(custom = even_for_callers)]
    async fn trailing(&self) -> Result<Vec<u32>, AccessDenied> {
        tokio::task::yield_now().await;
        let double = |n: u32| -> u32 {
            return n * 2;
        };
        return Ok(vec![double(1), 3]);
    }

    #[post_authorize(authenticated)]
    async fn unfinished(&self) -> Result<u32, AccessDenied> {
        todo!()
    }
}

#[tokio::test]
async fn async_post_checks_keep_the_declared_return_type_for_early_returns() {
    let shelf = Shelf {
        name: "poetry".into(),
    };
    let bob = || caller("bob", &[]);
    assert_eq!(as_caller(bob(), shelf.label(true)).await, Ok("poetry"));
    assert_eq!(as_caller(bob(), shelf.label(false)).await, Ok("default"));
    assert_eq!(
        as_caller(None, shelf.label(true)).await,
        Err(AccessDenied::Unauthenticated)
    );
    let early = as_caller(bob(), shelf.boxed(true)).await.unwrap();
    assert_eq!(format!("{early:?}"), "1");
    let late = as_caller(bob(), shelf.boxed(false)).await.unwrap();
    assert_eq!(format!("{late:?}"), "\"late\"");
    assert!(as_caller(None, shelf.boxed(true)).await.is_err());
    assert_eq!(as_caller(bob(), shelf.trailing()).await, Ok(vec![2]));
}

#[tokio::test]
#[should_panic(expected = "not yet implemented")]
async fn an_async_body_with_a_diverging_tail_compiles_and_runs() {
    let shelf = Shelf {
        name: String::new(),
    };
    let _ = as_caller(caller("bob", &[]), shelf.unfinished()).await;
}
