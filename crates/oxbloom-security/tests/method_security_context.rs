//! The filter chain scopes the request's security context around the downstream call, so a
//! handler sees it through `SecurityContext::current()`.

use async_trait::async_trait;
use http::{HeaderValue, header};
use oxbloom_security::{
    Authentication, AuthenticationError, AuthenticationProvider, AuthenticationRequest,
    GrantedAuthority, HttpSecurity, SecurityContext,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};

struct FakeBearer;

#[async_trait]
impl AuthenticationProvider for FakeBearer {
    fn supports(&self, req: &AuthenticationRequest) -> bool {
        matches!(req, AuthenticationRequest::BearerToken(_))
    }

    async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        match req {
            AuthenticationRequest::BearerToken(t) if t.expose() == "good" => Ok(
                Authentication::new("token-user", [GrantedAuthority::new("ROLE_USER")]),
            ),
            _ => Err(AuthenticationError::InvalidToken),
        }
    }
}

fn app() -> App {
    let chain = HttpSecurity::new()
        .bearer_provider(std::sync::Arc::new(FakeBearer))
        .authorize(|a| a.any_request().permit_all())
        .build()
        .unwrap();
    App::new()
        .filter(chain)
        .route(Route::new(
            HttpMethod::Get,
            "/who",
            |_req: Request| async move {
                SecurityContext::current()
                    .authentication()
                    .map(|a| a.principal().to_string())
                    .unwrap_or_else(|| "anonymous".to_string())
            },
        ))
        .unwrap()
}

async fn who(app: &App, bearer: Option<&str>) -> Response {
    let mut req = Request::new(HttpMethod::Get, "/who");
    if let Some(token) = bearer {
        req.headers.append(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
    }
    app.handle(req).await
}

#[tokio::test(flavor = "current_thread")]
async fn handler_sees_the_authenticated_principal_through_current() {
    let res = who(&app(), Some("good")).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "token-user");
}

#[tokio::test(flavor = "current_thread")]
async fn anonymous_handler_sees_an_anonymous_context() {
    let res = who(&app(), None).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "anonymous");
}

#[tokio::test(flavor = "current_thread")]
async fn the_form_login_page_sees_an_anonymous_context_inside_an_authenticated_outer_scope() {
    let chain = HttpSecurity::new()
        .password_provider(std::sync::Arc::new(FakeBearer))
        .form_login(|f| f)
        .authorize(|a| a.any_request().permit_all())
        .build()
        .unwrap();
    let app = App::new()
        .filter(chain)
        .route(Route::new(
            HttpMethod::Get,
            "/login",
            |_req: Request| async move {
                SecurityContext::current()
                    .authentication()
                    .map(|a| a.principal().to_string())
                    .unwrap_or_else(|| "anonymous".to_string())
            },
        ))
        .unwrap();
    let outer = SecurityContext::authenticated(Authentication::new(
        "outer",
        [GrantedAuthority::new("ROLE_USER")],
    ));
    let res =
        SecurityContext::scope(outer, app.handle(Request::new(HttpMethod::Get, "/login"))).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "anonymous");
}
