use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::{HeaderValue, header};
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthenticationEntryPoint, AuthenticationError,
    AuthenticationProvider, AuthenticationRequest, DaoAuthenticationProvider, GrantedAuthority,
    HttpSecurity, InMemoryUserDetailsService, PasswordEncoder, RequestSecurityExt,
    SecurityBuildError, SecurityFilterChain, UserDetails,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};

const BASIC_CHALLENGE: &str = "Basic realm=\"Oxbloom\", charset=\"UTF-8\"";

fn dao() -> Arc<DaoAuthenticationProvider> {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let users = InMemoryUserDetailsService::new(vec![
        UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
        UserDetails::new("bob", encoder.encode("pa:ss"), []).roles(&["ADMIN"]),
        UserDetails::new("carol", encoder.encode("x"), [])
            .roles(&["USER"])
            .enabled(false),
        UserDetails::new("dave", encoder.encode("x"), [])
            .roles(&["USER"])
            .locked(true),
    ])
    .unwrap();
    Arc::new(DaoAuthenticationProvider::new(Arc::new(users), encoder))
}

fn security() -> HttpSecurity {
    HttpSecurity::new()
        .http_basic(|b| b)
        .password_provider(dao())
}

fn rules(security: HttpSecurity) -> HttpSecurity {
    security.authorize(|a| {
        a.get("/public")
            .permit_all()
            .get("/admin")
            .has_role("ADMIN")
            .any_request()
            .authenticated()
    })
}

fn app(chain: SecurityFilterChain) -> App {
    let route = |path: &'static str| {
        Route::new(HttpMethod::Get, path, |req: Request| async move {
            req.authentication()
                .map(|a| a.principal().to_string())
                .unwrap_or_else(|| "anonymous".to_string())
        })
    };
    App::new()
        .filter(chain)
        .route(route("/public"))
        .unwrap()
        .route(route("/me"))
        .unwrap()
        .route(route("/admin"))
        .unwrap()
}

fn basic_app() -> App {
    app(rules(security()).build().unwrap())
}

fn basic(user: &str, pass: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{pass}")))
}

async fn get(app: &App, path: &str, auth: &[&str]) -> Response {
    let mut req = Request::new(HttpMethod::Get, path);
    for v in auth {
        req.headers
            .append(header::AUTHORIZATION, HeaderValue::from_str(v).unwrap());
    }
    app.handle(req).await
}

fn challenges(res: &Response) -> Vec<String> {
    res.headers()
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn valid_basic_reaches_the_route() {
    let res = get(&basic_app(), "/me", &[&basic("alice", "s3cret")]).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "alice");
}

#[tokio::test(flavor = "current_thread")]
async fn wrong_password_is_401_with_basic_challenge() {
    let app = basic_app();
    for (user, pass) in [
        ("alice", "nope"),
        ("nobody", "x"),
        ("carol", "x"),
        ("dave", "x"),
    ] {
        let res = get(&app, "/public", &[&basic(user, pass)]).await;
        assert_eq!(res.status(), 401, "{user}");
        assert_eq!(challenges(&res), [BASIC_CHALLENGE], "{user}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn anonymous_gets_the_basic_challenge() {
    let res = get(&basic_app(), "/me", &[]).await;
    assert_eq!(res.status(), 401);
    assert_eq!(challenges(&res), [BASIC_CHALLENGE]);
}

#[tokio::test(flavor = "current_thread")]
async fn custom_realm_is_used() {
    let chain = rules(
        HttpSecurity::new()
            .http_basic(|b| b.realm("Admin area"))
            .password_provider(dao()),
    )
    .build()
    .unwrap();
    let res = get(&app(chain), "/me", &[]).await;
    assert_eq!(
        challenges(&res),
        ["Basic realm=\"Admin area\", charset=\"UTF-8\""]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn role_protected_route_is_403_for_the_wrong_role() {
    let app = basic_app();
    let res = get(&app, "/admin", &[&basic("alice", "s3cret")]).await;
    assert_eq!(res.status(), 403);
    let res = get(&app, "/admin", &[&basic("bob", "pa:ss")]).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "bob");
}

#[tokio::test(flavor = "current_thread")]
async fn password_may_contain_a_colon() {
    let res = get(&basic_app(), "/me", &[&basic("bob", "pa:ss")]).await;
    assert_eq!(res.status(), 200);
}

#[tokio::test(flavor = "current_thread")]
async fn username_splits_at_the_first_colon() {
    // "bob:pa" with password "ss" would be a different user; the first colon wins.
    let header = format!("Basic {}", STANDARD.encode("bob:pa:ss"));
    let res = get(&basic_app(), "/me", &[&header]).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "bob");
}

#[tokio::test(flavor = "current_thread")]
async fn scheme_is_case_insensitive_and_spaces_are_tolerated() {
    let token = STANDARD.encode("alice:s3cret");
    for v in [
        format!("basic {token}"),
        format!("BASIC {token}"),
        format!("Basic  {token}"),
    ] {
        let res = get(&basic_app(), "/me", &[&v]).await;
        assert_eq!(res.status(), 200, "{v}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn malformed_headers_are_401_never_400() {
    let app = basic_app();
    let no_colon = format!("Basic {}", STANDARD.encode("alice"));
    let good = basic("alice", "s3cret");
    for headers in [
        vec!["Basic !!!not-base64".to_string()],
        vec![no_colon],
        vec!["Basic".to_string()],
        vec!["Basic ".to_string()],
        vec![good.clone(), good],
    ] {
        let refs: Vec<&str> = headers.iter().map(String::as_str).collect();
        let res = get(&app, "/public", &refs).await;
        assert_eq!(res.status(), 401, "{headers:?}");
        assert_eq!(challenges(&res), [BASIC_CHALLENGE], "{headers:?}");
    }
    // Valid base64 of bytes that are not UTF-8.
    let mut req = Request::new(HttpMethod::Get, "/public");
    let bad = format!("Basic {}", STANDARD.encode([0xff, b':', 0xfe]));
    req.headers
        .insert(header::AUTHORIZATION, HeaderValue::from_str(&bad).unwrap());
    let res = app.handle(req).await;
    assert_eq!(res.status(), 401);
    // A header value that is not even text.
    let mut req = Request::new(HttpMethod::Get, "/public");
    req.headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_bytes(b"Basic \xff\xfe").unwrap(),
    );
    assert_eq!(app.handle(req).await.status(), 401);
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_credentials_are_401() {
    let app = basic_app();
    let long = "a".repeat(1025);
    for (user, pass) in [(long.as_str(), "x"), ("alice", long.as_str())] {
        let res = get(&app, "/me", &[&basic(user, pass)]).await;
        assert_eq!(res.status(), 401);
        assert_eq!(challenges(&res), [BASIC_CHALLENGE]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn bearer_header_is_left_to_the_bearer_path() {
    let app = basic_app();
    let res = get(&app, "/public", &["Bearer x"]).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "anonymous");
    let res = get(&app, "/me", &["Bearer x"]).await;
    assert_eq!(res.status(), 401);
    assert_eq!(challenges(&res), [BASIC_CHALLENGE]);
}

#[tokio::test(flavor = "current_thread")]
async fn basic_header_is_anonymous_without_http_basic() {
    let chain = rules(HttpSecurity::new().password_provider(dao()))
        .build()
        .unwrap();
    let app = app(chain);
    let auth = basic("alice", "s3cret");
    let res = get(&app, "/public", &[&auth]).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.body(), "anonymous");
    let res = get(&app, "/me", &[&auth]).await;
    assert_eq!(res.status(), 401);
    assert_eq!(challenges(&res), ["Bearer"]);
}

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
                Authentication::new("tokenuser", [GrantedAuthority::new("ROLE_USER")]),
            ),
            _ => Err(AuthenticationError::InvalidToken),
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn basic_and_bearer_share_a_chain() {
    let chain = rules(security().bearer_provider(Arc::new(FakeBearer)))
        .build()
        .unwrap();
    let app = app(chain);
    let res = get(&app, "/me", &[&basic("alice", "s3cret")]).await;
    assert_eq!(res.body(), "alice");
    let res = get(&app, "/me", &["Bearer good"]).await;
    assert_eq!(res.body(), "tokenuser");
    let res = get(&app, "/me", &[]).await;
    assert_eq!(res.status(), 401);
    assert_eq!(challenges(&res), [BASIC_CHALLENGE, "Bearer"]);
    let res = get(&app, "/me", &["Bearer bad"]).await;
    assert_eq!(challenges(&res), ["Bearer error=\"invalid_token\""]);
    let res = get(&app, "/me", &[&basic("alice", "wrong")]).await;
    assert_eq!(challenges(&res), [BASIC_CHALLENGE]);
}

#[test]
fn build_errors() {
    let err = HttpSecurity::new()
        .http_basic(|b| b)
        .authorize(|a| a.any_request().authenticated())
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::HttpBasicWithoutProvider));
    for realm in ["a\"b", "a\\b", "a\nb"] {
        let err = HttpSecurity::new()
            .http_basic(|b| b.realm(realm))
            .password_provider(dao())
            .authorize(|a| a.any_request().authenticated())
            .build()
            .err();
        assert_eq!(err, Some(SecurityBuildError::InvalidRealm), "{realm:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn debug_of_request_hides_credentials() {
    let debug = Route::new(HttpMethod::Get, "/debug", |req: Request| async move {
        format!("{req:?} {:?}", req.headers)
    });
    let chain = security()
        .authorize(|a| a.any_request().authenticated())
        .build()
        .unwrap();
    let app = App::new().filter(chain).route(debug).unwrap();
    let auth = basic("alice", "s3cret");
    let res = get(&app, "/debug", &[&auth]).await;
    assert_eq!(res.status(), 200);
    let text = String::from_utf8_lossy(res.body()).into_owned();
    assert!(
        !text.contains(auth.strip_prefix("Basic ").unwrap()),
        "{text}"
    );
    assert!(!text.contains("s3cret"), "{text}");
}

#[tokio::test(flavor = "current_thread")]
async fn custom_entry_point_wins() {
    struct Teapot;
    impl AuthenticationEntryPoint for Teapot {
        fn commence(&self, _req: &Request, _error: Option<&AuthenticationError>) -> Response {
            let mut res = Response::new(Default::default());
            *res.status_mut() = http::StatusCode::IM_A_TEAPOT;
            res
        }
    }
    let chain = rules(security().entry_point(Arc::new(Teapot)))
        .build()
        .unwrap();
    let app = app(chain);
    assert_eq!(get(&app, "/me", &[]).await.status(), 418);
    assert_eq!(
        get(&app, "/me", &[&basic("alice", "bad")]).await.status(),
        418
    );
}
