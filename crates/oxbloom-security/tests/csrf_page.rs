//! The default login page with the CSRF token and the framing headers, and the token endpoint,
//! through `App::handle`.

use std::sync::{Arc, Mutex};

use http::{HeaderValue, header};
use oxbloom_config::Secret;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    Argon2PasswordEncoder, CookieSessionBackend, CsrfConfig, DaoAuthenticationProvider,
    HttpSecurity, InMemorySessionStore, InMemoryUserDetailsService, PasswordEncoder,
    RequestSessionExt, SessionBackend, SessionFilter, SessionSettings, UserDetails,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};

const NAME: &str = "OXBLOOM_SESSION";
const SECRET: &str = "a-session-secret-of-at-least-32-bytes!";
const CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'";

#[derive(Clone, Copy, Debug)]
enum Kind {
    Server,
    Cookie,
}

const KINDS: [Kind; 2] = [Kind::Server, Kind::Cookie];

fn session_filter(kind: Kind) -> SessionFilter {
    let settings = SessionSettings::default();
    let backend = match kind {
        Kind::Server => SessionBackend::Server(Arc::new(InMemorySessionStore::new(
            &settings,
            Arc::new(SystemClock),
        ))),
        Kind::Cookie => SessionBackend::Cookie(Arc::new(
            CookieSessionBackend::new(
                &Secret::new(SECRET.to_owned()),
                &[],
                &settings,
                Arc::new(SystemClock),
            )
            .unwrap(),
        )),
    };
    SessionFilter::new(backend, settings).unwrap()
}

struct Client {
    app: App,
    jar: Mutex<Option<String>>,
}

fn provider() -> Arc<DaoAuthenticationProvider> {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let users = InMemoryUserDetailsService::new(vec![
        UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
    ])
    .unwrap();
    Arc::new(DaoAuthenticationProvider::new(Arc::new(users), encoder))
}

fn security(csrf: Option<CsrfConfig>) -> HttpSecurity {
    let mut s = HttpSecurity::new()
        .password_provider(provider())
        .form_login(|f| f)
        .authorize(|a| {
            a.path("/login")
                .permit_all()
                .get("/token")
                .permit_all()
                .any_request()
                .authenticated()
        });
    if let Some(csrf) = csrf {
        s = s.csrf(|_| csrf);
    }
    s
}

impl Client {
    fn new(kind: Kind, csrf: Option<CsrfConfig>) -> Self {
        Self::build(Some(kind), csrf)
    }

    fn build(kind: Option<Kind>, csrf: Option<CsrfConfig>) -> Self {
        let mut app = App::new();
        if let Some(kind) = kind {
            app = app.filter(session_filter(kind));
        }
        let app = app
            .filter(security(csrf).build().expect("chain builds"))
            .route(Route::new(
                HttpMethod::Get,
                "/token",
                |req: Request| async move {
                    req.csrf_token()
                        .map(|t| t.as_str().to_owned())
                        .unwrap_or_default()
                },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Post,
                "/items",
                |_req: Request| async move { "changed" },
            ))
            .unwrap();
        Self {
            app,
            jar: Mutex::new(None),
        }
    }

    fn request(&self, method: HttpMethod, target: &str) -> Request {
        let mut req = Request::new(method, target);
        req.headers
            .insert(header::HOST, HeaderValue::from_static("example.com"));
        if let Some(value) = self.jar.lock().unwrap().clone() {
            req.headers
                .insert(header::COOKIE, format!("{NAME}={value}").parse().unwrap());
        }
        req
    }

    async fn send(&self, req: Request) -> Response {
        let res = self.app.handle(req).await;
        let prefix = format!("{NAME}=");
        for v in res.headers().get_all(header::SET_COOKIE) {
            if let Some(rest) = v.to_str().unwrap().strip_prefix(&prefix) {
                *self.jar.lock().unwrap() = Some(rest.split(';').next().unwrap().to_owned());
            }
        }
        res
    }

    async fn get(&self, target: &str) -> Response {
        self.send(self.request(HttpMethod::Get, target)).await
    }

    fn has_session(&self) -> bool {
        self.jar.lock().unwrap().is_some()
    }
}

fn body(res: &Response) -> String {
    String::from_utf8_lossy(res.body()).into_owned()
}

fn hidden_value(page: &str, name: &str) -> Option<String> {
    let marker = format!("<input type=\"hidden\" name=\"{name}\" value=\"");
    let start = page.find(&marker)? + marker.len();
    let end = page[start..].find('"')?;
    Some(page[start..start + end].to_owned())
}

fn json_field(text: &str, key: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(text).unwrap();
    v[key].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn the_page_carries_the_hidden_field_and_the_session_cookie() {
    for kind in KINDS {
        let c = Client::new(kind, None);
        let res = c.get("/login").await;
        assert_eq!(res.status(), 200, "{kind:?}");
        let page = body(&res);
        let token = hidden_value(&page, "_csrf").expect("hidden field");
        assert_eq!(token.len(), 43);
        assert!(c.has_session(), "{kind:?}: the page created the session");
    }
}

#[tokio::test]
async fn the_page_token_is_the_session_token() {
    for kind in KINDS {
        let c = Client::new(kind, None);
        let token = hidden_value(&body(&c.get("/login").await), "_csrf").unwrap();
        assert_eq!(body(&c.get("/token").await), token, "{kind:?}");
    }
}

#[tokio::test]
async fn the_page_headers_include_framing_protection() {
    for kind in KINDS {
        let c = Client::new(kind, None);
        let res = c.get("/login").await;
        assert_eq!(res.headers()["content-security-policy"], CSP);
        assert_eq!(res.headers()["x-frame-options"], "DENY");
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
        assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    }
}

#[tokio::test]
async fn csrf_disabled_page_has_no_field_but_keeps_the_framing_headers() {
    let c = Client::new(Kind::Server, Some(CsrfConfig::default().enabled(false)));
    let res = c.get("/login").await;
    assert_eq!(res.status(), 200);
    assert!(!body(&res).contains("hidden"));
    assert_eq!(res.headers()["content-security-policy"], CSP);
    assert_eq!(res.headers()["x-frame-options"], "DENY");
    assert!(!c.has_session(), "no token, no session");
}

#[tokio::test]
async fn the_page_without_a_session_filter_has_no_field() {
    let c = Client::build(None, None);
    let res = c.get("/login").await;
    assert_eq!(res.status(), 200);
    assert!(!body(&res).contains("hidden"));
    assert_eq!(res.headers()["x-frame-options"], "DENY");
}

#[tokio::test]
async fn the_page_created_session_is_the_only_anonymous_write() {
    for kind in KINDS {
        let c = Client::new(kind, None);
        let mut req = c.request(HttpMethod::Get, "/secret");
        req.headers
            .insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        let res = c.send(req).await;
        assert_eq!(res.status(), 401, "{kind:?}");
        assert!(
            !c.has_session(),
            "{kind:?}: a protected GET makes no session"
        );
        c.get("/login").await;
        assert!(c.has_session(), "{kind:?}");
    }
}

fn with_endpoint() -> CsrfConfig {
    CsrfConfig::default().token_endpoint("/csrf")
}

#[tokio::test]
async fn token_endpoint_is_off_by_default_404() {
    for kind in KINDS {
        let c = Client::new(kind, None);
        let res = c.get("/csrf").await;
        // No rule matches /csrf, so the chain denies it; the endpoint answered nothing.
        assert_ne!(res.status(), 200, "{kind:?}");
        assert!(!body(&res).contains("token"));
        assert!(!c.has_session());
    }
}

#[tokio::test]
async fn token_endpoint_answers_the_exact_json_and_headers() {
    for kind in KINDS {
        let c = Client::new(
            kind,
            Some(with_endpoint().field_name("f").header_name("X-T")),
        );
        let res = c.get("/csrf").await;
        assert_eq!(res.status(), 200, "{kind:?}");
        assert_eq!(res.headers()["content-type"], "application/json");
        assert_eq!(res.headers()["cache-control"], "no-store");
        let text = body(&res);
        let token = json_field(&text, "token");
        assert_eq!(token.len(), 43);
        assert_eq!(
            text,
            format!("{{\"headerName\":\"X-T\",\"parameterName\":\"f\",\"token\":\"{token}\"}}")
        );
    }
}

#[tokio::test]
async fn token_endpoint_creates_the_session_and_token() {
    for kind in KINDS {
        let c = Client::new(kind, Some(with_endpoint()));
        assert!(!c.has_session());
        let first = json_field(&body(&c.get("/csrf").await), "token");
        assert!(c.has_session(), "{kind:?}");
        assert_eq!(json_field(&body(&c.get("/csrf").await), "token"), first);
        assert_eq!(body(&c.get("/token").await), first);
        let page = body(&c.get("/login").await);
        assert_eq!(hidden_value(&page, "_csrf").unwrap(), first);
    }
}

#[tokio::test]
async fn token_endpoint_ignores_other_methods_and_paths() {
    for kind in KINDS {
        let c = Client::new(kind, Some(with_endpoint()));
        for method in [HttpMethod::Head, HttpMethod::Post, HttpMethod::Put] {
            let res = c.send(c.request(method.clone(), "/csrf")).await;
            assert_ne!(res.status(), 200, "{kind:?} {method:?}");
            assert!(!c.has_session(), "{kind:?} {method:?}");
        }
        for path in ["/csrf/", "/csrf2", "/other"] {
            let res = c.get(path).await;
            assert_ne!(res.status(), 200, "{kind:?} {path}");
        }
        assert!(!c.has_session());
    }
}

#[tokio::test]
async fn token_endpoint_without_a_session_filter_falls_through() {
    let c = Client::build(None, Some(with_endpoint()));
    let res = c.get("/csrf").await;
    assert_ne!(res.status(), 200);
    assert!(!body(&res).contains("token"));
}

#[tokio::test]
async fn the_endpoint_token_works_in_the_header_for_an_unsafe_request() {
    for kind in KINDS {
        let c = Client::new(kind, Some(with_endpoint()));
        let text = body(&c.get("/csrf").await);
        let (name, token) = (json_field(&text, "headerName"), json_field(&text, "token"));
        // Anonymous requests are not checked, so authenticate by the login form first is not
        // needed here: the check only guards the login and logout posts for anonymous users.
        let mut req = c.request(HttpMethod::Post, "/logout");
        req.headers.insert(
            http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(&token).unwrap(),
        );
        let ok = c.send(req).await;
        assert_eq!(ok.status(), 302, "{kind:?}: the endpoint token passes");
        let bad = c.send(c.request(HttpMethod::Post, "/logout")).await;
        assert_eq!(bad.status(), 403, "{kind:?}: no token fails");
    }
}
