use std::sync::Arc;

use oxbloom_web::{App, HttpMethod, IntoResponse, Request, Router, RouterConfig, controller};

struct UserController {
    greeting: String,
}

#[controller("/api/users/")]
impl UserController {
    #[get("/")]
    async fn list(&self, _req: Request) -> impl IntoResponse {
        format!("{}: list", self.greeting)
    }

    #[get("/{id}")]
    async fn show(&self, req: Request) -> impl IntoResponse {
        format!("{}: {}", self.greeting, req.param("id").unwrap())
    }

    #[post("")]
    async fn create(&self, req: Request) -> impl IntoResponse {
        (
            http::StatusCode::CREATED,
            String::from_utf8_lossy(&req.body).into_owned(),
        )
    }

    #[delete("/{id}")]
    async fn remove(&self, _req: Request) -> http::StatusCode {
        http::StatusCode::NO_CONTENT
    }

    /// Not a route; must survive unchanged.
    fn helper(&self) -> usize {
        1
    }
}

fn router() -> Router {
    let c = Arc::new(UserController {
        greeting: "hey".into(),
    });
    assert_eq!(c.helper(), 1);
    let mut r = Router::new(RouterConfig::default());
    for route in c.routes() {
        r.add(route).unwrap();
    }
    r
}

async fn call(r: &Router, m: HttpMethod, path: &str) -> http::Response<bytes::Bytes> {
    r.handle(Request::new(m, path)).await
}

#[tokio::test]
async fn routes_are_registered_with_joined_paths() {
    let r = router();
    assert_eq!(
        call(&r, HttpMethod::Get, "/api/users").await.body(),
        "hey: list"
    );
    assert_eq!(
        call(&r, HttpMethod::Get, "/api/users/7").await.body(),
        "hey: 7"
    );
    assert_eq!(
        call(&r, HttpMethod::Post, "/api/users").await.status(),
        http::StatusCode::CREATED
    );
    assert_eq!(
        call(&r, HttpMethod::Delete, "/api/users/7").await.status(),
        http::StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&r, HttpMethod::Put, "/api/users/7").await.status(),
        http::StatusCode::METHOD_NOT_ALLOWED
    );
}

#[test]
fn controller_plugs_into_app() {
    let c = Arc::new(UserController {
        greeting: "x".into(),
    });
    App::new().routes(c.routes()).unwrap();
}

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}

#[tokio::test]
async fn app_handle_dispatches_without_socket() {
    let c = Arc::new(UserController {
        greeting: "hey".into(),
    });
    let app = App::new().routes(c.routes()).unwrap();
    let res = app
        .handle(Request::new(HttpMethod::Get, "/api/users/7"))
        .await;
    assert_eq!(res.body(), "hey: 7");
}

#[test]
fn controller_trait_is_implemented() {
    fn routes_via_trait<C: oxbloom_web::Controller>(c: Arc<C>) -> usize {
        c.routes().len()
    }
    let c = Arc::new(UserController {
        greeting: "x".into(),
    });
    assert_eq!(routes_via_trait(c), 4);
}
