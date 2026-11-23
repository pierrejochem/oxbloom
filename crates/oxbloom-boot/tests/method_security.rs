//! Method security through a booted application: a secured controller handler answers 401, 403
//! and 200 behind the real chain, a secured bean method sees the request principal,
//! `oxbloom_security::spawn` carries the principal into a task, and `with_authentication` runs a
//! service-level test without HTTP.

#![cfg(feature = "security")]
// `HttpError` is the error type of a handler; its size is not this test's concern.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use oxbloom_boot::SecurityCustomizer;
use oxbloom_di::ContainerBuilder;
use oxbloom_security::{
    AccessDenied, Argon2PasswordEncoder, Authentication, DelegatingPasswordEncoder, HttpSecurity,
    PasswordEncoder, SecurityContext, method_security,
};
use oxbloom_test::{RunningApp, TestApp, with_authentication};
use oxbloom_web::{Controller, HttpError, HttpMethod, Request, Route, controller};

/// The stored form of `raw`, hashed with a low-cost encoder so the tests stay fast.
fn hash(raw: &str) -> String {
    DelegatingPasswordEncoder::argon2(Argon2PasswordEncoder::new(8, 1, 1).unwrap()).encode(raw)
}

/// `alice` (password `wonderland`, role `USER`) and `root` (password `toor`, role `ADMIN`), with
/// HTTP Basic switched on.
fn config() -> String {
    format!(
        "[security.http-basic]\nenabled = true\n\
         [security.users.alice]\npassword = \"{}\"\nroles = [\"USER\"]\n\
         [security.users.root]\npassword = \"{}\"\nroles = [\"ADMIN\"]\n",
        hash("wonderland"),
        hash("toor")
    )
}

const ALICE: &str = "Basic YWxpY2U6d29uZGVybGFuZA==";
const ROOT: &str = "Basic cm9vdDp0b29y";

/// A bean whose methods are secured.
struct Ledger;

#[method_security]
impl Ledger {
    #[pre_authorize(role = "ADMIN")]
    async fn close_books(&self) -> Result<&'static str, AccessDenied> {
        Ok("closed")
    }

    #[pre_authorize(authenticated)]
    async fn whoami(&self) -> Result<String, AccessDenied> {
        Ok(SecurityContext::current()
            .authentication()
            .map(|a| a.principal().to_owned())
            .unwrap_or_default())
    }
}

struct Api {
    ledger: Arc<Ledger>,
}

#[controller("/api")]
#[method_security]
impl Api {
    #[get("/admin")]
    #[pre_authorize(role = "ADMIN")]
    async fn admin(&self, _req: Request) -> Result<&'static str, HttpError> {
        Ok("admin")
    }

    #[get("/whoami")]
    async fn whoami(&self, _req: Request) -> Result<String, HttpError> {
        Ok(self.ledger.whoami().await?)
    }

    #[get("/close")]
    async fn close(&self, _req: Request) -> Result<&'static str, HttpError> {
        Ok(self.ledger.close_books().await?)
    }
}

struct Spawner;

impl Controller for Spawner {
    fn routes(self: Arc<Self>) -> Vec<Route> {
        vec![Route::new(HttpMethod::Get, "/spawn", |_| async {
            let name = || {
                SecurityContext::current()
                    .authentication()
                    .map(|a| a.principal().to_owned())
                    .unwrap_or_else(|| "none".to_owned())
            };
            let carried = oxbloom_security::spawn(async move { name() })
                .await
                .unwrap();
            let bare = tokio::spawn(async move { name() }).await.unwrap();
            format!("spawn={carried};tokio={bare}")
        })]
    }
}

/// The chain lets every request through, so the method rules alone decide.
struct Rules;

impl SecurityCustomizer for Rules {
    fn customize(&self, http: HttpSecurity) -> HttpSecurity {
        http.authorize(|a| a.any_request().permit_all())
    }
}

fn wiring(b: ContainerBuilder) -> ContainerBuilder {
    let ledger = Arc::new(Ledger);
    b.instance(Api { ledger })
        .provides(|c: Arc<Api>| -> Arc<dyn Controller> { c })
        .instance(Spawner)
        .provides(|c: Arc<Spawner>| -> Arc<dyn Controller> { c })
        .instance(Rules)
        .provides(|c: Arc<Rules>| -> Arc<dyn SecurityCustomizer> { c })
}

async fn start() -> RunningApp {
    TestApp::secured()
        .config_str(config())
        .configure(wiring)
        .start()
        .await
}

#[tokio::test]
async fn secured_handler_answers_401_for_anonymous_and_403_for_the_wrong_role_and_200_for_the_right_role()
 {
    let app = start().await;
    let client = app.client();

    let anonymous = client.get("/api/admin").send().await;
    assert_eq!(anonymous.status(), 401);
    assert!(
        anonymous
            .header("content-type")
            .unwrap()
            .starts_with("application/problem+json")
    );

    let wrong = client
        .get("/api/admin")
        .header("authorization", ALICE)
        .send()
        .await;
    assert_eq!(wrong.status(), 403);
    assert!(
        wrong
            .header("content-type")
            .unwrap()
            .starts_with("application/problem+json")
    );

    let right = client
        .get("/api/admin")
        .header("authorization", ROOT)
        .send()
        .await;
    assert_eq!(right.status(), 200);
    assert_eq!(right.text(), "admin");
    app.stop().await;
}

#[tokio::test]
async fn a_secured_bean_method_called_from_a_handler_uses_the_request_principal() {
    let app = start().await;
    let client = app.client();

    let who = client
        .get("/api/whoami")
        .header("authorization", ALICE)
        .send()
        .await;
    assert_eq!(who.status(), 200);
    assert_eq!(who.text(), "alice");

    let denied = client
        .get("/api/close")
        .header("authorization", ALICE)
        .send()
        .await;
    assert_eq!(denied.status(), 403);
    let allowed = client
        .get("/api/close")
        .header("authorization", ROOT)
        .send()
        .await;
    assert_eq!(allowed.text(), "closed");

    let anonymous = client.get("/api/close").send().await;
    assert_eq!(anonymous.status(), 401);
    app.stop().await;
}

#[tokio::test]
async fn spawn_carries_the_principal_into_a_task_and_tokio_spawn_does_not() {
    let app = start().await;
    let res = app
        .client()
        .get("/spawn")
        .header("authorization", ALICE)
        .send()
        .await;
    assert_eq!(res.text(), "spawn=alice;tokio=none");
    app.stop().await;
}

#[tokio::test]
async fn with_authentication_runs_a_service_test_without_http() {
    let ledger = Ledger;
    let admin = Authentication::new(
        "ada",
        [oxbloom_security::GrantedAuthority::new("ROLE_ADMIN")],
    );
    let user = Authentication::new("bob", []);
    assert_eq!(
        with_authentication(admin, ledger.close_books()).await,
        Ok("closed")
    );
    assert_eq!(
        with_authentication(user, ledger.close_books()).await,
        Err(AccessDenied::Forbidden)
    );
    assert_eq!(
        ledger.close_books().await,
        Err(AccessDenied::Unauthenticated)
    );
}
