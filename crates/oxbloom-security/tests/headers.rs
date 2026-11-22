//! The security response headers through `App::handle`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::HeaderValue;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthenticationError, AuthenticationProvider,
    AuthenticationRequest, DaoAuthenticationProvider, GrantedAuthority, HeadersConfig, HstsConfig,
    HttpSecurity, InMemorySessionStore, InMemoryUserDetailsService, PasswordEncoder,
    SecurityBuildError, SessionBackend, SessionFilter, SessionSettings, UserDetails,
};
use oxbloom_web::{App, ClientInfo, HttpMethod, Request, Response, Route, Scheme};

const HSTS_DEFAULT: &str = "max-age=31536000; includeSubDomains";

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
                Authentication::new("tok", [GrantedAuthority::new("ROLE_USER")]),
            ),
            _ => Err(AuthenticationError::InvalidToken),
        }
    }
}

fn dao() -> Arc<DaoAuthenticationProvider> {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let users = InMemoryUserDetailsService::new(vec![
        UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
        UserDetails::new("bob", encoder.encode("pw"), []).roles(&["ADMIN"]),
    ])
    .unwrap();
    Arc::new(DaoAuthenticationProvider::new(Arc::new(users), encoder))
}

fn session_filter() -> SessionFilter {
    let settings = SessionSettings::default();
    let store = InMemorySessionStore::new(&settings, Arc::new(SystemClock));
    SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings).unwrap()
}

fn security(headers: impl FnOnce(HeadersConfig) -> HeadersConfig) -> HttpSecurity {
    HttpSecurity::new()
        .http_basic(|b| b)
        .password_provider(dao())
        .bearer_provider(Arc::new(FakeBearer))
        .form_login(|f| f)
        .csrf(|c| c.enabled(false))
        .headers(headers)
        .authorize(|a| {
            a.get("/open")
                .permit_all()
                .get("/own")
                .permit_all()
                .get("/own-sameorigin")
                .permit_all()
                .get("/two")
                .permit_all()
                .get("/cached")
                .permit_all()
                .get("/admin")
                .has_role("ADMIN")
                .any_request()
                .authenticated()
        })
}

fn text_route(app: App, path: &str, headers: Vec<(&'static str, &'static str)>) -> App {
    app.route(Route::new(HttpMethod::Get, path, move |_req: Request| {
        let headers = headers.clone();
        async move {
            let mut res = Response::new("ok".into());
            for (name, value) in headers {
                res.headers_mut().append(
                    http::HeaderName::from_static(name),
                    HeaderValue::from_static(value),
                );
            }
            res
        }
    }))
    .unwrap()
}

fn app_with(headers: impl FnOnce(HeadersConfig) -> HeadersConfig) -> App {
    let app = App::new()
        .filter(session_filter())
        .filter(security(headers).build().unwrap());
    let app = text_route(app, "/open", vec![]);
    let app = text_route(app, "/me", vec![]);
    let app = text_route(app, "/admin", vec![]);
    let app = text_route(
        app,
        "/own-sameorigin",
        vec![("x-frame-options", "SAMEORIGIN")],
    );
    let app = text_route(
        app,
        "/two",
        vec![
            ("referrer-policy", "origin"),
            ("referrer-policy", "same-origin"),
        ],
    );
    text_route(app, "/cached", vec![("cache-control", "max-age=60")])
}

fn app() -> App {
    app_with(|h| h)
}

fn get(path: &str) -> Request {
    Request::new(HttpMethod::Get, path)
}

fn with_header(mut req: Request, name: &'static str, value: &str) -> Request {
    req.headers
        .insert(http::HeaderName::from_static(name), value.parse().unwrap());
    req
}

fn basic(user: &str, pass: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{pass}")))
}

fn https(mut req: Request) -> Request {
    req.extensions_mut().insert(ClientInfo {
        addr: "127.0.0.1:1".parse().unwrap(),
        scheme: Scheme::Https,
        host: None,
    });
    req
}

fn values(res: &Response, name: &str) -> Vec<String> {
    res.headers()
        .get_all(name)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

fn assert_unconditional(res: &Response) {
    assert_eq!(values(res, "x-content-type-options"), ["nosniff"]);
    assert_eq!(values(res, "referrer-policy"), ["no-referrer"]);
    assert_eq!(values(res, "x-frame-options"), ["DENY"]);
}

#[tokio::test(flavor = "current_thread")]
async fn unconditional_headers_on_every_kind_of_response() {
    let app = app();
    // 200 on a permit-all route.
    let res = app.handle(get("/open")).await;
    assert_eq!(res.status(), 200);
    assert_unconditional(&res);
    // 401 for an anonymous request.
    let res = app.handle(get("/me")).await;
    assert_eq!(res.status(), 401);
    assert_unconditional(&res);
    // 401 for a bad bearer token.
    let res = app
        .handle(with_header(get("/me"), "authorization", "Bearer bad"))
        .await;
    assert_eq!(res.status(), 401);
    assert_unconditional(&res);
    // 403 for a user without the role.
    let res = app
        .handle(with_header(
            get("/admin"),
            "authorization",
            &basic("alice", "s3cret"),
        ))
        .await;
    assert_eq!(res.status(), 403);
    assert_unconditional(&res);
    // The login page.
    let res = app.handle(get("/login")).await;
    assert_eq!(res.status(), 200);
    assert_unconditional(&res);
    // A form login that succeeds, and one that fails.
    for (pass, status) in [("s3cret", 302), ("wrong", 302)] {
        let mut login = Request::new(HttpMethod::Post, "/login");
        login.headers.insert(
            "content-type",
            "application/x-www-form-urlencoded".parse().unwrap(),
        );
        login.body = format!("username=alice&password={pass}").into();
        let res = app.handle(login).await;
        assert_eq!(res.status(), status);
        assert_unconditional(&res);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn no_store_only_when_authenticated() {
    let app = app();
    let res = app.handle(get("/open")).await;
    assert!(values(&res, "cache-control").is_empty());
    let res = app.handle(get("/me")).await;
    assert!(values(&res, "cache-control").is_empty());

    let basic_req = with_header(get("/me"), "authorization", &basic("alice", "s3cret"));
    let res = app.handle(basic_req).await;
    assert_eq!(res.status(), 200);
    assert_eq!(values(&res, "cache-control"), ["no-store"]);

    let bearer_req = with_header(get("/me"), "authorization", "Bearer good");
    let res = app.handle(bearer_req).await;
    assert_eq!(res.status(), 200);
    assert_eq!(values(&res, "cache-control"), ["no-store"]);
}

#[tokio::test(flavor = "current_thread")]
async fn no_store_after_a_session_login() {
    let app = app();
    let mut login = Request::new(HttpMethod::Post, "/login");
    login.headers.insert(
        "content-type",
        "application/x-www-form-urlencoded".parse().unwrap(),
    );
    login.body = "username=alice&password=s3cret".into();
    let res = app.handle(login).await;
    assert_eq!(res.status(), 302);
    let cookie = res.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let res = app.handle(with_header(get("/me"), "cookie", &cookie)).await;
    assert_eq!(res.status(), 200);
    assert_eq!(values(&res, "cache-control"), ["no-store"]);
}

#[tokio::test(flavor = "current_thread")]
async fn handler_cache_control_is_kept() {
    let app = app();
    let res = app
        .handle(with_header(
            get("/cached"),
            "authorization",
            &basic("alice", "s3cret"),
        ))
        .await;
    assert_eq!(values(&res, "cache-control"), ["max-age=60"]);
}

#[tokio::test(flavor = "current_thread")]
async fn handler_set_headers_survive_without_duplicates() {
    let app = app();
    let res = app.handle(get("/own-sameorigin")).await;
    assert_eq!(values(&res, "x-frame-options"), ["SAMEORIGIN"]);
    let res = app.handle(get("/two")).await;
    assert_eq!(values(&res, "referrer-policy"), ["origin", "same-origin"]);
}

#[tokio::test(flavor = "current_thread")]
async fn hsts_only_on_https() {
    let app = app();
    let res = app.handle(get("/open")).await;
    assert!(values(&res, "strict-transport-security").is_empty());
    let res = app.handle(https(get("/open"))).await;
    assert_eq!(values(&res, "strict-transport-security"), [HSTS_DEFAULT]);
    // A forwarded header on a plain request is not trusted by itself.
    let res = app
        .handle(with_header(get("/open"), "x-forwarded-proto", "https"))
        .await;
    assert!(values(&res, "strict-transport-security").is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn hsts_options_change_the_value() {
    let app = app_with(|h| {
        h.hsts(|s| {
            s.max_age(Duration::from_secs(60))
                .include_subdomains(false)
                .preload(true)
        })
    });
    let res = app.handle(https(get("/open"))).await;
    assert_eq!(
        values(&res, "strict-transport-security"),
        ["max-age=60; preload"]
    );
    let app = app_with(|h| h.hsts(|s| s.include_subdomains(false)));
    let res = app.handle(https(get("/open"))).await;
    assert_eq!(
        values(&res, "strict-transport-security"),
        ["max-age=31536000"]
    );
    let app = app_with(|h| h.hsts(|s| s.preload(true)));
    let res = app.handle(https(get("/open"))).await;
    assert_eq!(
        values(&res, "strict-transport-security"),
        ["max-age=31536000; includeSubDomains; preload"]
    );
    let app = app_with(|h| h.hsts(|s| s.enabled(false)));
    let res = app.handle(https(get("/open"))).await;
    assert!(values(&res, "strict-transport-security").is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn every_header_is_removable() {
    let app = app_with(|h| {
        h.content_type_options(None)
            .referrer_policy(None)
            .frame_options(None)
            .cache_control(None)
            .hsts(|s| s.enabled(false))
    });
    let res = app
        .handle(https(with_header(
            get("/me"),
            "authorization",
            &basic("alice", "s3cret"),
        )))
        .await;
    assert_eq!(res.status(), 200);
    for name in [
        "x-content-type-options",
        "referrer-policy",
        "x-frame-options",
        "cache-control",
        "strict-transport-security",
    ] {
        assert!(values(&res, name).is_empty(), "{name}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn disabled_adds_nothing() {
    let app = app_with(|h| h.enabled(false));
    let res = app
        .handle(https(with_header(
            get("/me"),
            "authorization",
            &basic("alice", "s3cret"),
        )))
        .await;
    assert_eq!(res.status(), 200);
    for name in [
        "x-content-type-options",
        "referrer-policy",
        "x-frame-options",
        "cache-control",
        "strict-transport-security",
    ] {
        assert!(values(&res, name).is_empty(), "{name}");
    }
}

#[test]
fn defaults_and_setters() {
    let d = HeadersConfig::default();
    assert!(d.enabled);
    assert_eq!(d.content_type_options.as_deref(), Some("nosniff"));
    assert_eq!(d.referrer_policy.as_deref(), Some("no-referrer"));
    assert_eq!(d.frame_options.as_deref(), Some("DENY"));
    assert_eq!(d.cache_control.as_deref(), Some("no-store"));
    assert_eq!(d.hsts, HstsConfig::default());
    assert_eq!(d.validate(), Ok(()));
    assert_eq!(d.clone(), d);
}

#[test]
fn validate_rejects_bad_values_with_a_fixed_text() {
    let bad = Err(SecurityBuildError::InvalidHeadersConfig);
    let secret = "leaky-value\u{1}";
    for c in [
        HeadersConfig::default().content_type_options(Some("")),
        HeadersConfig::default().referrer_policy(Some("no\nreferrer")),
        HeadersConfig::default().frame_options(Some(secret)),
        HeadersConfig::default().cache_control(Some("a\u{7f}b")),
        HeadersConfig::default().cache_control(Some("caf\u{e9}")),
        HeadersConfig::default().hsts(|h| h.max_age(Duration::ZERO)),
    ] {
        assert_eq!(c.validate(), bad, "{c:?}");
    }
    let text = SecurityBuildError::InvalidHeadersConfig.to_string();
    assert!(!text.contains("leaky"));
    let err = HttpSecurity::new()
        .authorize(|a| a.any_request().permit_all())
        .headers(|h| h.frame_options(Some(secret)))
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::InvalidHeadersConfig));
}
