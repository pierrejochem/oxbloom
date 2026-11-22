//! Form login, logout, the saved request and the default login page through `App::handle`.
//!
//! The behavioural tests run against both session backends through one shared loop per test
//! (`for kind in KINDS`). A small cookie jar plays the browser: it keeps the session cookie the
//! responses set and drops it on `Max-Age=0`.
//!
//! These tests pin the Slice 6c behavior, so the CSRF check is off in every setup here; its own
//! tests are in `tests/csrf_flow.rs`.

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthenticationError, AuthenticationProvider,
    AuthenticationRequest, CookieSessionBackend, DaoAuthenticationProvider, FormLogin,
    GrantedAuthority, HttpSecurity, InMemorySessionStore, InMemoryUserDetailsService,
    PasswordEncoder, RequestSecurityExt, RequestSessionExt, SecurityBuildError, SessionBackend,
    SessionFilter, SessionSettings, UserDetails,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};
use tracing_subscriber::fmt::MakeWriter;

const NAME: &str = "OXBLOOM_SESSION";
const SECRET: &str = "a-session-secret-of-at-least-32-bytes!";
const HOST: &str = "example.com";
const HTML: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";
const SAVED_KEY: &str = "oxbloom.security.saved-request";
const AUTH_KEY: &str = "oxbloom.security.authentication";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Server,
    Cookie,
}

const KINDS: [Kind; 2] = [Kind::Server, Kind::Cookie];

fn dao() -> Arc<DaoAuthenticationProvider> {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let users = InMemoryUserDetailsService::new(vec![
        UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
        UserDetails::new("bob", encoder.encode("b0b-pass"), []).roles(&["USER"]),
        UserDetails::new("carol", encoder.encode("carol-pass"), [])
            .roles(&["USER"])
            .enabled(false),
        UserDetails::new("dave", encoder.encode("dave-pass"), [])
            .roles(&["USER"])
            .locked(true),
    ])
    .unwrap();
    Arc::new(DaoAuthenticationProvider::new(Arc::new(users), encoder))
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
                Authentication::new("token-user", [GrantedAuthority::new("ROLE_USER")]),
            ),
            _ => Err(AuthenticationError::InvalidToken),
        }
    }
}

fn base_security() -> HttpSecurity {
    HttpSecurity::new().password_provider(dao())
}

fn rules(security: HttpSecurity) -> HttpSecurity {
    security.authorize(|a| {
        a.get("/public")
            .permit_all()
            .get("/tamper/**")
            .permit_all()
            .any_request()
            .authenticated()
    })
}

/// Form login with the CSRF check off, as in Slice 6c.
fn form_login(security: HttpSecurity, f: impl FnOnce(FormLogin) -> FormLogin) -> HttpSecurity {
    security.form_login(f).csrf(|c| c.enabled(false))
}

fn default_security() -> HttpSecurity {
    rules(form_login(base_security(), |f| f))
}

fn principal_route(path: &'static str) -> Route {
    Route::new(HttpMethod::Get, path, |req: Request| async move {
        req.authentication()
            .map(|a| a.principal().to_string())
            .unwrap_or_else(|| "anonymous".to_string())
    })
}

/// Writes a raw value into the session, as a tampering attacker with store access could.
fn tamper_route(path: &'static str, key: &'static str, value: serde_json::Value) -> Route {
    Route::new(HttpMethod::Get, path, move |req: Request| {
        let value = value.clone();
        async move {
            req.session()
                .expect("the filter ran")
                .set(key, &value)
                .unwrap();
            "tampered"
        }
    })
}

fn routes(app: App) -> App {
    app.route(principal_route("/secret"))
        .unwrap()
        .route(principal_route("/public"))
        .unwrap()
        .route(principal_route("/login/other"))
        .unwrap()
        .route(tamper_route(
            "/tamper/slashes",
            SAVED_KEY,
            serde_json::json!("//evil.com"),
        ))
        .unwrap()
        .route(tamper_route(
            "/tamper/backslash",
            SAVED_KEY,
            serde_json::json!("/\\evil.com"),
        ))
        .unwrap()
        .route(tamper_route(
            "/tamper/absolute",
            SAVED_KEY,
            serde_json::json!("https://evil.com/x"),
        ))
        .unwrap()
        .route(tamper_route(
            "/tamper/crlf",
            SAVED_KEY,
            serde_json::json!("/a\r\nSet-Cookie: x=y"),
        ))
        .unwrap()
        .route(tamper_route(
            "/tamper/corrupt",
            AUTH_KEY,
            serde_json::json!({"v": 1, "login": 1, "authentication": {"principal": 5}}),
        ))
        .unwrap()
        .route(tamper_route(
            "/tamper/text",
            AUTH_KEY,
            serde_json::json!("not an authentication"),
        ))
        .unwrap()
}

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

/// A browser: an app plus a cookie jar for the session cookie.
struct Client {
    app: App,
    jar: Mutex<Option<String>>,
}

impl Client {
    fn new(kind: Kind) -> Self {
        Self::with(kind, default_security())
    }

    fn with(kind: Kind, security: HttpSecurity) -> Self {
        Self::from_app(routes(
            App::new()
                .filter(session_filter(kind))
                .filter(security.build().unwrap()),
        ))
    }

    fn from_app(app: App) -> Self {
        logs();
        Self {
            app,
            jar: Mutex::new(None),
        }
    }

    fn cookie(&self) -> Option<String> {
        self.jar.lock().unwrap().clone()
    }

    fn set_cookie(&self, value: Option<String>) {
        *self.jar.lock().unwrap() = value;
    }

    fn request(&self, method: HttpMethod, target: &str) -> Request {
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p, Some(q.to_owned())),
            None => (target, None),
        };
        let mut req = Request::new(method, path);
        req.query = query;
        req.headers.insert(header::HOST, HOST.parse().unwrap());
        if let Some(value) = self.cookie() {
            req.headers
                .insert(header::COOKIE, format!("{NAME}={value}").parse().unwrap());
        }
        req
    }

    async fn send(&self, req: Request) -> Response {
        let res = self.app.handle(req).await;
        if let Some((value, max_age)) = session_cookie(&res) {
            self.set_cookie(if max_age == Some(0) {
                None
            } else {
                Some(value)
            });
        }
        res
    }

    async fn get_with_accept(&self, target: &str, accept: Option<&str>) -> Response {
        let mut req = self.request(HttpMethod::Get, target);
        if let Some(accept) = accept {
            req.headers.insert(header::ACCEPT, accept.parse().unwrap());
        }
        self.send(req).await
    }

    async fn browse(&self, target: &str) -> Response {
        self.get_with_accept(target, Some(HTML)).await
    }

    fn login_request(&self, body: &[u8]) -> Request {
        let mut req = self.request(HttpMethod::Post, "/login");
        req.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        req.body = body.to_vec().into();
        req
    }

    async fn login(&self, user: &str, pass: &str) -> Response {
        let body = format!("username={user}&password={pass}");
        self.send(self.login_request(body.as_bytes())).await
    }

    async fn logout(&self) -> Response {
        self.send(self.request(HttpMethod::Post, "/logout")).await
    }
}

fn set_cookies(res: &Response) -> Vec<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

fn session_cookie(res: &Response) -> Option<(String, Option<u64>)> {
    let prefix = format!("{NAME}=");
    let text = set_cookies(res)
        .into_iter()
        .find(|c| c.starts_with(&prefix))?;
    let mut parts = text.split("; ");
    let value = parts.next()?[prefix.len()..].to_owned();
    let max_age = parts
        .find_map(|p| p.strip_prefix("Max-Age="))
        .map(|n| n.parse().unwrap());
    Some((value, max_age))
}

fn location(res: &Response) -> &str {
    res.headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap())
        .unwrap_or("")
}

fn body(res: &Response) -> String {
    String::from_utf8_lossy(res.body()).into_owned()
}

fn assert_redirect(res: &Response, to: &str) {
    assert_eq!(res.status(), StatusCode::FOUND, "{res:?}");
    assert_eq!(location(res), to);
}

fn basic(user: &str, pass: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{pass}")))
}

// --- the full flow ---

#[tokio::test]
async fn full_flow_with_saved_request() {
    for kind in KINDS {
        let c = Client::new(kind);
        let res = c.browse("/secret?page=2").await;
        assert_redirect(&res, "/login");
        let before = c.cookie().unwrap_or_else(|| panic!("{kind:?}: a session"));

        let res = c.login("alice", "s3cret").await;
        assert_redirect(&res, "/secret?page=2");
        let after = c.cookie().expect("a session after login");
        assert_ne!(before, after, "{kind:?}: the session id changes at login");

        let res = c.browse("/secret").await;
        assert_eq!(res.status(), 200, "{kind:?}");
        assert_eq!(body(&res), "alice");

        let res = c.logout().await;
        assert_redirect(&res, "/login?logout");
        let res = c.browse("/secret").await;
        assert_redirect(&res, "/login");

        if kind == Kind::Server {
            // The logged-in id was invalidated on the server.
            c.set_cookie(Some(after));
            assert_redirect(&c.browse("/secret").await, "/login");
        }
    }
}

#[tokio::test]
async fn fixation_a_planted_cookie_is_not_adopted_and_the_id_changes_at_login() {
    for kind in KINDS {
        let c = Client::new(kind);
        let planted = "A".repeat(43);
        c.set_cookie(Some(planted.clone()));
        let res = c.login("alice", "s3cret").await;
        assert_redirect(&res, "/");
        let issued = c.cookie().unwrap();
        assert_ne!(issued, planted, "{kind:?}");
        c.set_cookie(Some(planted));
        assert_redirect(&c.browse("/secret").await, "/login");

        // An attacker who holds a real anonymous session cannot keep it through the login.
        let c = Client::new(kind);
        c.browse("/secret").await;
        let attacker = c.cookie().unwrap();
        c.login("alice", "s3cret").await;
        assert_ne!(c.cookie().unwrap(), attacker, "{kind:?}");
        if kind == Kind::Server {
            c.set_cookie(Some(attacker));
            assert_redirect(&c.browse("/secret").await, "/login");
        }
    }
}

#[tokio::test]
async fn a_second_login_rotates_again() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.login("alice", "s3cret").await;
        let first = c.cookie().unwrap();
        let res = c.login("bob", "b0b-pass").await;
        assert_redirect(&res, "/");
        let second = c.cookie().unwrap();
        assert_ne!(first, second, "{kind:?}");
        assert_eq!(body(&c.browse("/secret").await), "bob");
        if kind == Kind::Server {
            c.set_cookie(Some(first));
            assert_redirect(&c.browse("/secret").await, "/login");
        }
    }
}

#[tokio::test]
async fn two_concurrent_logins_on_one_session_do_not_panic() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.login("alice", "s3cret").await;
        let one = c.login_request(b"username=alice&password=s3cret");
        let two = c.login_request(b"username=bob&password=b0b-pass");
        let (a, b) = tokio::join!(c.app.handle(one), c.app.handle(two));
        assert_redirect(&a, "/");
        assert_redirect(&b, "/");
    }
}

// --- failures ---

#[tokio::test]
async fn all_failures_look_the_same() {
    let mut oversize = b"username=alice&password=s3cret&pad=".to_vec();
    oversize.resize(4097, b'x');
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("wrong password", b"username=alice&password=wrong".to_vec()),
        ("unknown user", b"username=mallory&password=s3cret".to_vec()),
        ("disabled", b"username=carol&password=carol-pass".to_vec()),
        ("locked", b"username=dave&password=dave-pass".to_vec()),
        ("oversize", oversize),
        ("missing field", b"username=alice".to_vec()),
        ("invalid utf-8", b"username=alice&password=%FF".to_vec()),
    ];
    for kind in KINDS {
        for (name, body_bytes) in &cases {
            // Without a session: the failure creates none.
            let c = Client::new(kind);
            let res = c.send(c.login_request(body_bytes)).await;
            assert_redirect(&res, "/login?error");
            assert!(set_cookies(&res).is_empty(), "{kind:?} {name}");
            assert!(!location(&res).contains("alice"));

            // With an anonymous session holding a saved request: the session is unchanged.
            let c = Client::new(kind);
            c.browse("/secret").await;
            let before = c.cookie().unwrap();
            let res = c.send(c.login_request(body_bytes)).await;
            assert_redirect(&res, "/login?error");
            assert!(set_cookies(&res).is_empty(), "{kind:?} {name}");
            assert_eq!(c.cookie().unwrap(), before);
            assert_redirect(&c.browse("/secret").await, "/login");
            // The saved request survived the failure.
            assert_redirect(&c.login("alice", "s3cret").await, "/secret");
        }

        let c = Client::new(kind);
        let mut req = c.login_request(b"username=alice&password=s3cret");
        req.headers
            .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        let res = c.send(req).await;
        assert_eq!(res.status(), 400, "{kind:?}");
        assert!(session_cookie(&res).is_none());

        let mut req = c.login_request(b"username=alice&password=s3cret");
        req.headers.remove(header::CONTENT_TYPE);
        assert_eq!(c.send(req).await.status(), 400);

        let mut req = c.login_request(b"username=alice&password=s3cret");
        req.headers.insert(
            header::CONTENT_TYPE,
            "application/x-www-form-urlencoded; charset=UTF-8"
                .parse()
                .unwrap(),
        );
        assert_redirect(&c.send(req).await, "/");
    }
}

// --- the saved request ---

#[tokio::test]
async fn saved_request_safety() {
    for kind in KINDS {
        // Paths that would be open redirects are never saved.
        for path in ["//evil.com", "/\\evil.com", "//evil.com/x"] {
            let c = Client::new(kind);
            let res = c.browse(path).await;
            assert_redirect(&res, "/login");
            let res = c.login("alice", "s3cret").await;
            assert_redirect(&res, "/");
        }

        // A tampered session never redirects off-site, and the bad value is consumed.
        for tamper in [
            "/tamper/slashes",
            "/tamper/backslash",
            "/tamper/absolute",
            "/tamper/crlf",
        ] {
            let c = Client::new(kind);
            assert_eq!(c.browse(tamper).await.status(), 200);
            let res = c.login("alice", "s3cret").await;
            assert_redirect(&res, "/");
            let res = c.login("alice", "s3cret").await;
            assert_redirect(&res, "/");
        }

        // A saved request is consumed after use.
        let c = Client::new(kind);
        c.browse("/secret").await;
        assert_redirect(&c.login("alice", "s3cret").await, "/secret");
        assert_redirect(&c.login("alice", "s3cret").await, "/");

        // API clients, other methods and requests without `Accept` get 401 and save nothing.
        for accept in [
            Some("application/json"),
            None,
            Some("*/*"),
            Some("application/json, text/html"),
            Some("text/html;q=0.5, application/json"),
            Some("text/html;q=0"),
        ] {
            let c = Client::new(kind);
            let res = c.get_with_accept("/secret", accept).await;
            assert_eq!(res.status(), 401, "{kind:?} {accept:?}");
            assert!(set_cookies(&res).is_empty(), "{kind:?} {accept:?}");
        }
        for accept in [
            "text/html",
            "text/html, application/json",
            "application/xhtml+xml",
        ] {
            let c = Client::new(kind);
            let res = c.get_with_accept("/secret", Some(accept)).await;
            assert_redirect(&res, "/login");
        }
        let c = Client::new(kind);
        let mut req = c.request(HttpMethod::Post, "/secret");
        req.headers.insert(header::ACCEPT, HTML.parse().unwrap());
        let res = c.send(req).await;
        assert_eq!(res.status(), 401);
        assert!(set_cookies(&res).is_empty());

        // Requests for the form login URLs never overwrite the saved request.
        let c = Client::new(kind);
        c.browse("/secret?x=1").await;
        assert_redirect(&c.browse("/logout").await, "/login");
        assert_eq!(c.browse("/login").await.status(), 200);
        assert_eq!(c.browse("/login?error").await.status(), 200);
        assert_redirect(&c.login("alice", "s3cret").await, "/secret?x=1");
    }
}

// --- same origin ---

#[tokio::test]
async fn origin_checks_on_both_posts() {
    for kind in KINDS {
        let c = Client::new(kind);
        for (name, value) in [
            ("origin", "http://evil.com"),
            ("origin", "null"),
            ("referer", "http://example.com.evil.com/login"),
            ("origin", "https://example.com"),
        ] {
            let mut req = c.login_request(b"username=alice&password=s3cret");
            req.headers.insert(
                header::HeaderName::from_static(name),
                value.parse().unwrap(),
            );
            let res = c.send(req).await;
            assert_eq!(res.status(), 403, "{kind:?} {name} {value}");
            assert!(set_cookies(&res).is_empty());
        }
        assert_redirect(&c.browse("/secret").await, "/login");

        for (name, value) in [
            ("origin", "http://example.com"),
            ("origin", "http://example.com:80"),
            ("referer", "http://example.com/login?x"),
        ] {
            let c = Client::new(kind);
            let mut req = c.login_request(b"username=alice&password=s3cret");
            req.headers.insert(
                header::HeaderName::from_static(name),
                value.parse().unwrap(),
            );
            assert_redirect(&c.send(req).await, "/");
            assert_eq!(c.browse("/secret").await.status(), 200);

            let mut req = c.request(HttpMethod::Post, "/logout");
            req.headers.insert(
                header::HeaderName::from_static("origin"),
                "http://evil.com".parse().unwrap(),
            );
            let res = c.send(req).await;
            assert_eq!(res.status(), 403);
            assert!(set_cookies(&res).is_empty());
            assert_eq!(c.browse("/secret").await.status(), 200, "still logged in");

            let mut req = c.request(HttpMethod::Post, "/logout");
            req.headers.insert(
                header::HeaderName::from_static(name),
                value.parse().unwrap(),
            );
            assert_redirect(&c.send(req).await, "/login?logout");
            assert_redirect(&c.browse("/secret").await, "/login");
        }

        // `GET /logout` never logs out; it follows the rules like any other path.
        let c = Client::new(kind);
        c.login("alice", "s3cret").await;
        let res = c.browse("/logout").await;
        assert_eq!(res.status(), 404);
        assert_eq!(c.browse("/secret").await.status(), 200);
    }
}

// --- Basic and bearer ---

#[tokio::test]
async fn basic_and_bearer_create_no_session() {
    for kind in KINDS {
        let c = Client::with(
            kind,
            rules(form_login(
                base_security()
                    .http_basic(|b| b)
                    .bearer_provider(Arc::new(FakeBearer)),
                |f| f,
            )),
        );
        let mut req = c.request(HttpMethod::Get, "/secret");
        req.headers.insert(
            header::AUTHORIZATION,
            basic("alice", "s3cret").parse().unwrap(),
        );
        req.headers.insert(header::ACCEPT, HTML.parse().unwrap());
        let res = c.send(req).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(&res), "alice");
        assert!(set_cookies(&res).is_empty(), "{kind:?}");

        let mut req = c.request(HttpMethod::Get, "/secret");
        req.headers
            .insert(header::AUTHORIZATION, "Bearer good".parse().unwrap());
        let res = c.send(req).await;
        assert_eq!(body(&res), "token-user");
        assert!(set_cookies(&res).is_empty());

        // A failed Basic login from a browser is a 401 with the challenge, not a redirect.
        let mut req = c.request(HttpMethod::Get, "/secret");
        req.headers.insert(
            header::AUTHORIZATION,
            basic("alice", "nope").parse().unwrap(),
        );
        req.headers.insert(header::ACCEPT, HTML.parse().unwrap());
        let res = c.send(req).await;
        assert_eq!(res.status(), 401);
        assert!(set_cookies(&res).is_empty());
    }
}

#[tokio::test]
async fn basic_wins_over_a_session() {
    for kind in KINDS {
        let c = Client::with(
            kind,
            rules(form_login(base_security().http_basic(|b| b), |f| f)),
        );
        c.login("alice", "s3cret").await;
        assert_eq!(body(&c.browse("/secret").await), "alice");
        let mut req = c.request(HttpMethod::Get, "/secret");
        req.headers.insert(
            header::AUTHORIZATION,
            basic("bob", "b0b-pass").parse().unwrap(),
        );
        assert_eq!(body(&c.send(req).await), "bob", "{kind:?}");
    }
}

// --- the default page ---

#[tokio::test]
async fn default_page() {
    for kind in KINDS {
        let c = Client::new(kind);
        let res = c.browse("/login").await;
        assert_eq!(res.status(), 200);
        let h = res.headers();
        assert_eq!(h[header::CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(h[header::CACHE_CONTROL], "no-store");
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(
            h[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'"
        );
        assert_eq!(h[header::X_FRAME_OPTIONS], "DENY");
        let text = body(&res);
        assert!(text.contains("action=\"/login\""));
        assert!(!text.contains("Invalid username"));
        assert!(
            set_cookies(&res).is_empty(),
            "with CSRF off the page creates no session"
        );

        assert!(body(&c.browse("/login?error").await).contains("Invalid username or password."));
        assert!(body(&c.browse("/login?logout").await).contains("You have been logged out."));
        let hostile = body(
            &c.browse("/login?error=%3Cscript%3Ealert(1)%3C/script%3E")
                .await,
        );
        assert!(hostile.contains("Invalid username or password."));
        assert!(!hostile.contains("<script") && !hostile.contains("alert"));

        // The application's own login page wins.
        let app = App::new()
            .filter(session_filter(kind))
            .filter(default_security().build().unwrap())
            .route(Route::new(
                HttpMethod::Get,
                "/login",
                |_req: Request| async { "my own page" },
            ))
            .unwrap();
        let own = Client::from_app(app);
        let res = own.browse("/login").await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(&res), "my own page");

        // Turned off, the 404 stays a 404.
        let off = Client::with(
            kind,
            rules(form_login(base_security(), |f| f.login_page_enabled(false))),
        );
        assert_eq!(off.browse("/login").await.status(), 404);
    }
}

#[tokio::test]
async fn login_url_exemptions_apply_only_at_the_configured_paths() {
    for kind in KINDS {
        let c = Client::new(kind);
        assert_redirect(&c.browse("/login/other").await, "/login");
        let res = c.send(c.request(HttpMethod::Post, "/login/other")).await;
        assert_eq!(res.status(), 401);
        let res = c.send(c.request(HttpMethod::Post, "/logout/x")).await;
        assert_eq!(res.status(), 401);
        let res = c.send(c.request(HttpMethod::Put, "/login")).await;
        assert_eq!(res.status(), 401);
        c.login("alice", "s3cret").await;
        assert_eq!(body(&c.browse("/login/other").await), "alice");
    }
}

#[test]
fn build_errors() {
    let err = rules(HttpSecurity::new().form_login(|f| f)).build().err();
    assert_eq!(err, Some(SecurityBuildError::FormLoginWithoutProvider));
    let err = rules(base_security().form_login(|f| f.default_success_url("https://evil")))
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::InvalidFormLoginUrl));
    assert_eq!(
        SecurityBuildError::InvalidFormLoginUrl.to_string(),
        "invalid form login url or field name"
    );
    assert!(FormLogin::default().validate().is_ok());
}

#[tokio::test]
async fn a_corrupt_stored_authentication_is_anonymous_not_500() {
    for kind in KINDS {
        for tamper in ["/tamper/corrupt", "/tamper/text"] {
            let c = Client::new(kind);
            assert_eq!(c.browse(tamper).await.status(), 200);
            assert_redirect(&c.browse("/secret").await, "/login");
            assert_eq!(body(&c.browse("/public").await), "anonymous");
        }
    }
}

#[tokio::test]
async fn no_session_filter_means_a_failed_login_not_a_panic() {
    let app = routes(App::new().filter(default_security().build().unwrap()));
    let c = Client::from_app(app);
    let res = c.login("alice", "s3cret").await;
    assert_redirect(&res, "/login?error");
    // The browser is still sent to the login page; nothing can be saved.
    assert_redirect(&c.browse("/secret").await, "/login");
}

// --- logs ---

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

/// The log output of the tests in this binary. A global subscriber avoids the race of a
/// thread-scoped one with the callsite cache of concurrent tests. It is installed when the first
/// test that needs the buffer starts (the log-leak test), so logs of other tests that run later
/// land in the same buffer, which only makes the leak check stricter.
fn logs() -> LogBuffer {
    static LOGS: OnceLock<LogBuffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let logs = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(logs.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        logs
    })
    .clone()
}

#[tokio::test]
async fn debug_and_logs_never_contain_the_username_or_password() {
    let logs = logs();

    for kind in KINDS {
        let c = Client::new(kind);
        c.browse("/secret?q=hidden-saved-query").await;
        assert_redirect(&c.login("alice", "wrong-pass-xyz").await, "/login?error");
        assert_redirect(&c.login("mallory-user", "s3cret").await, "/login?error");
        let res = c.login("alice", "s3cret").await;
        assert!(!format!("{res:?}").contains("alice"));
        c.logout().await;
    }
    let no_session = Client::from_app(routes(
        App::new().filter(default_security().build().unwrap()),
    ));
    no_session.login("alice", "s3cret").await;

    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for secret in [
        "alice",
        "mallory-user",
        "wrong-pass-xyz",
        "s3cret",
        "hidden-saved-query",
    ] {
        assert!(!text.contains(secret), "{secret} leaked into:\n{text}");
    }
    assert!(text.contains("BadCredentials"), "{text}");
    assert!(text.contains("username_len=5"), "{text}");
    assert!(text.contains("username_len=12"), "{text}");
    assert!(text.contains("no session"), "{text}");
}
