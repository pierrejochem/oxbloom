//! The CSRF check in the chain and in the login and logout handlers, through `App::handle`.
//!
//! The behavioural tests run against both session backends through one shared loop per test
//! (`for kind in KINDS`). A small cookie jar plays the browser. The token is fetched through a
//! test route that returns `req.csrf_token()`.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::SystemClock;
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthenticationError, AuthenticationProvider,
    AuthenticationRequest, CookieSessionBackend, CsrfConfig, DaoAuthenticationProvider,
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
const TOKEN_KEY: &str = "oxbloom.security.csrf-token";
const FORM: &str = "application/x-www-form-urlencoded";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Server,
    Cookie,
}

const KINDS: [Kind; 2] = [Kind::Server, Kind::Cookie];

/// A password provider that counts how often it is asked, around the real DAO provider.
struct Counting {
    inner: DaoAuthenticationProvider,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AuthenticationProvider for Counting {
    fn supports(&self, req: &AuthenticationRequest) -> bool {
        self.inner.supports(req)
    }

    async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.authenticate(req).await
    }
}

fn dao() -> DaoAuthenticationProvider {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let users = InMemoryUserDetailsService::new(vec![
        UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
    ])
    .unwrap();
    DaoAuthenticationProvider::new(Arc::new(users), encoder)
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

fn rules(security: HttpSecurity) -> HttpSecurity {
    security.authorize(|a| {
        a.get("/token")
            .permit_all()
            .get("/state")
            .permit_all()
            .path("/open")
            .permit_all()
            .any_request()
            .authenticated()
    })
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

/// A browser: an app, a cookie jar for the session cookie and the counters of the provider and
/// of the application's state-changing handlers.
struct Client {
    app: App,
    jar: Mutex<Option<String>>,
    provider_calls: Arc<AtomicUsize>,
    changes: Arc<AtomicUsize>,
}

impl Client {
    fn new(kind: Kind) -> Self {
        Self::with(kind, |s| s.form_login(|f| f))
    }

    /// A client whose security is `configure` applied to a builder that already has the
    /// counting password provider.
    fn with(kind: Kind, configure: impl FnOnce(HttpSecurity) -> HttpSecurity) -> Self {
        logs();
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let changes = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(Counting {
            inner: dao(),
            calls: provider_calls.clone(),
        });
        let security = rules(configure(HttpSecurity::new().password_provider(provider)));
        let mut app = App::new()
            .filter(session_filter(kind))
            .filter(security.build().unwrap())
            .route(Route::new(
                HttpMethod::Get,
                "/token",
                |req: Request| async move {
                    req.csrf_token()
                        .map(|t| t.as_str().to_owned())
                        .unwrap_or_else(|| "none".to_owned())
                },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/state",
                |req: Request| async move {
                    let principal = req
                        .authentication()
                        .map(|a| a.principal().to_string())
                        .unwrap_or_else(|| "anonymous".to_string());
                    let token = req
                        .session()
                        .and_then(|s| s.get::<String>(TOKEN_KEY).ok().flatten())
                        .unwrap_or_else(|| "none".to_owned());
                    format!("{principal}|{token}")
                },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/secret",
                |req: Request| async move {
                    req.authentication()
                        .map(|a| a.principal().to_string())
                        .unwrap_or_default()
                },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Post,
                "/debug",
                |req: Request| async move { format!("{req:?}") },
            ))
            .unwrap();
        for method in [
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            for path in ["/items", "/open"] {
                let changes = changes.clone();
                app = app
                    .route(Route::new(method.clone(), path, move |req: Request| {
                        let changes = changes.clone();
                        async move {
                            changes.fetch_add(1, Ordering::SeqCst);
                            req.authentication()
                                .map(|a| a.principal().to_string())
                                .unwrap_or_else(|| "anonymous".to_string())
                        }
                    }))
                    .unwrap();
            }
        }
        Self {
            app,
            jar: Mutex::new(None),
            provider_calls,
            changes,
        }
    }

    fn cookie(&self) -> Option<String> {
        self.jar.lock().unwrap().clone()
    }

    fn set_cookie(&self, value: Option<String>) {
        *self.jar.lock().unwrap() = value;
    }

    fn calls(&self) -> usize {
        self.provider_calls.load(Ordering::SeqCst)
    }

    fn changes(&self) -> usize {
        self.changes.load(Ordering::SeqCst)
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

    /// Fetches the token of the session, creating the session when there is none.
    async fn token(&self) -> String {
        let res = self.send(self.request(HttpMethod::Get, "/token")).await;
        assert_eq!(res.status(), 200);
        let token = body(&res);
        assert_eq!(token.len(), 43, "{token}");
        token
    }

    /// The principal and the raw stored token, as `principal|token`.
    async fn state(&self) -> String {
        body(&self.send(self.request(HttpMethod::Get, "/state")).await)
    }

    fn form(&self, method: HttpMethod, target: &str, body: &str) -> Request {
        let mut req = self.request(method, target);
        req.headers
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(FORM));
        req.body = body.as_bytes().to_vec().into();
        req
    }

    fn login_request(&self, user: &str, pass: &str, token: Option<&str>) -> Request {
        let mut text = format!("username={user}&password={pass}");
        if let Some(token) = token {
            text.push_str(&format!("&_csrf={token}"));
        }
        self.form(HttpMethod::Post, "/login", &text)
    }

    async fn login(&self, user: &str, pass: &str, token: Option<&str>) -> Response {
        self.send(self.login_request(user, pass, token)).await
    }

    /// Fetches a token and logs in as alice with it.
    async fn log_in(&self) {
        let token = self.token().await;
        let res = self.login("alice", "s3cret", Some(&token)).await;
        assert_redirect(&res, "/");
    }

    fn logout_request(&self, token: Option<&str>) -> Request {
        match token {
            Some(token) => self.form(HttpMethod::Post, "/logout", &format!("_csrf={token}")),
            None => self.request(HttpMethod::Post, "/logout"),
        }
    }

    async fn logout(&self, token: Option<&str>) -> Response {
        self.send(self.logout_request(token)).await
    }

    /// An unsafe request to `/items` with the token in the header, when given.
    async fn change(&self, method: HttpMethod, token: Option<&str>) -> Response {
        let mut req = self.request(method, "/items");
        if let Some(token) = token {
            req.headers
                .insert("x-csrf-token", HeaderValue::from_str(token).unwrap());
        }
        self.send(req).await
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

/// A 403 that set no cookie: the failed check changed nothing in the session.
fn assert_forbidden(res: &Response, what: &str) {
    assert_eq!(res.status(), StatusCode::FORBIDDEN, "{what}");
    assert!(
        set_cookies(res).is_empty(),
        "{what}: {:?}",
        set_cookies(res)
    );
}

fn basic(user: &str, pass: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{pass}")))
}

/// The token with its first character changed.
fn altered(token: &str) -> String {
    let first = if token.starts_with('A') { "B" } else { "A" };
    format!("{first}{}", &token[1..])
}

// --- the full flow ---

#[tokio::test]
async fn full_flow() {
    for kind in KINDS {
        let c = Client::new(kind);
        let before = c.token().await;
        assert!(
            c.cookie().is_some(),
            "{kind:?}: the token created a session"
        );
        assert_redirect(&c.login("alice", "s3cret", Some(&before)).await, "/");
        assert_eq!(
            body(&c.send(c.request(HttpMethod::Get, "/secret")).await),
            "alice"
        );

        let token = c.token().await;
        for method in [
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            let res = c.change(method.clone(), Some(&token)).await;
            assert_eq!(res.status(), 200, "{kind:?} {method:?}");
            assert_eq!(body(&res), "alice");
        }
        let res = c
            .send(c.form(HttpMethod::Post, "/items", &format!("a=1&_csrf={token}")))
            .await;
        assert_eq!(res.status(), 200, "{kind:?}: the form field");
        assert_eq!(c.changes(), 5);

        assert_redirect(&c.logout(Some(&token)).await, "/login?logout");
        assert!(c.state().await.starts_with("anonymous|"), "{kind:?}");
        let mut req = c.request(HttpMethod::Get, "/secret");
        req.headers.insert(header::ACCEPT, HTML.parse().unwrap());
        assert_redirect(&c.send(req).await, "/login");
    }
}

// --- failed checks ---

#[tokio::test]
async fn missing_wrong_stale_or_empty_token_is_403_and_changes_nothing() {
    for kind in KINDS {
        // Application routes.
        let c = Client::new(kind);
        let stale = c.token().await;
        let login_token = stale.clone();
        assert_redirect(&c.login("alice", "s3cret", Some(&login_token)).await, "/");
        let good = c.token().await;
        let state = c.state().await;
        for (name, token) in [
            ("missing", None),
            ("wrong", Some("wrong".to_owned())),
            ("stale", Some(stale.clone())),
            ("empty", Some(String::new())),
            ("altered", Some(altered(&good))),
            ("longer", Some(format!("{good}x"))),
            ("shorter", Some(good[..42].to_owned())),
        ] {
            for method in [HttpMethod::Post, HttpMethod::Put, HttpMethod::Delete] {
                let res = c.change(method.clone(), token.as_deref()).await;
                assert_forbidden(&res, &format!("{kind:?} {name} {method:?}"));
            }
            let field = token.clone().unwrap_or_default();
            let res = c
                .send(c.form(HttpMethod::Post, "/items", &format!("_csrf={field}")))
                .await;
            assert_forbidden(&res, &format!("{kind:?} {name} field"));
        }
        assert_eq!(c.changes(), 0, "{kind:?}: no handler ran");
        assert_eq!(c.state().await, state, "{kind:?}");

        // Logout.
        for (name, token) in [
            ("missing", None),
            ("wrong", Some("wrong".to_owned())),
            ("stale", Some(stale.clone())),
            ("empty", Some(String::new())),
            ("altered", Some(altered(&good))),
        ] {
            let res = c.logout(token.as_deref()).await;
            assert_forbidden(&res, &format!("{kind:?} logout {name}"));
            assert_eq!(c.state().await, state, "{kind:?} logout {name}");
        }

        // Login, from an anonymous session that holds a token.
        let c = Client::new(kind);
        let good = c.token().await;
        let state = c.state().await;
        let cookie = c.cookie();
        for (name, token) in [
            ("missing", None),
            ("wrong", Some("wrong".to_owned())),
            ("empty", Some(String::new())),
            ("altered", Some(altered(&good))),
        ] {
            let res = c.login("alice", "s3cret", token.as_deref()).await;
            assert_forbidden(&res, &format!("{kind:?} login {name}"));
            assert_eq!(c.cookie(), cookie);
            assert_eq!(c.state().await, state, "{kind:?} login {name}");
        }
        assert_eq!(c.calls(), 0, "{kind:?}: the provider was never asked");
    }
}

#[tokio::test]
async fn put_and_delete_with_a_cookie_and_no_token_are_403() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.log_in().await;
        for method in [HttpMethod::Put, HttpMethod::Delete, HttpMethod::Patch] {
            assert_forbidden(
                &c.change(method.clone(), None).await,
                &format!("{method:?}"),
            );
        }
        assert_eq!(c.changes(), 0);
    }
}

#[tokio::test]
async fn json_post_with_a_cookie_and_no_header_is_403() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.log_in().await;
        let token = c.token().await;
        let mut req = c.request(HttpMethod::Post, "/items");
        req.headers
            .insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        req.body = format!("{{\"_csrf\":\"{token}\"}}").into_bytes().into();
        assert_forbidden(&c.send(req).await, &format!("{kind:?}"));
        assert_eq!(c.changes(), 0);
    }
}

#[tokio::test]
async fn token_in_the_query_is_ignored() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.log_in().await;
        let token = c.token().await;
        let req = c.request(
            HttpMethod::Post,
            &format!("/items?_csrf={token}&X-CSRF-TOKEN={token}"),
        );
        assert_forbidden(&c.send(req).await, &format!("{kind:?}"));
        let req = c.form(HttpMethod::Post, &format!("/items?_csrf={token}"), "a=1");
        assert_forbidden(&c.send(req).await, &format!("{kind:?} form"));
        assert_eq!(c.changes(), 0);
    }
}

#[tokio::test]
async fn first_header_value_wins() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.log_in().await;
        let token = c.token().await;
        let two = |first: &str, second: &str| {
            let mut req = c.request(HttpMethod::Post, "/items");
            req.headers
                .append("x-csrf-token", HeaderValue::from_str(first).unwrap());
            req.headers
                .append("x-csrf-token", HeaderValue::from_str(second).unwrap());
            req
        };
        assert_eq!(c.send(two(&token, "wrong")).await.status(), 200, "{kind:?}");
        assert_forbidden(&c.send(two("wrong", &token)).await, &format!("{kind:?}"));
        // Two form fields: the first wins as well.
        let first_good = c.form(
            HttpMethod::Post,
            "/items",
            &format!("_csrf={token}&_csrf=wrong"),
        );
        assert_eq!(c.send(first_good).await.status(), 200);
        let first_bad = c.form(
            HttpMethod::Post,
            "/items",
            &format!("_csrf=wrong&_csrf={token}"),
        );
        assert_forbidden(&c.send(first_bad).await, &format!("{kind:?} fields"));
        assert_eq!(c.changes(), 2);
    }
}

#[tokio::test]
async fn multipart_or_json_body_needs_the_header() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.log_in().await;
        let token = c.token().await;
        for content_type in [
            "multipart/form-data; boundary=x",
            "application/json",
            "text/plain",
        ] {
            let mut req = c.request(HttpMethod::Post, "/items");
            req.headers
                .insert(header::CONTENT_TYPE, content_type.parse().unwrap());
            req.body = format!("_csrf={token}").into_bytes().into();
            assert_forbidden(&c.send(req).await, &format!("{kind:?} {content_type}"));

            let mut req = c.request(HttpMethod::Post, "/items");
            req.headers
                .insert(header::CONTENT_TYPE, content_type.parse().unwrap());
            req.headers
                .insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
            req.body = "{}".into();
            assert_eq!(c.send(req).await.status(), 200, "{kind:?} {content_type}");
        }
    }
}

#[tokio::test]
async fn oversize_body_is_403() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.log_in().await;
        let token = c.token().await;
        let mut text = format!("_csrf={token}&pad=");
        while text.len() < 4096 {
            text.push('x');
        }
        let res = c.send(c.form(HttpMethod::Post, "/items", &text)).await;
        assert_eq!(res.status(), 200, "{kind:?}: 4096 bytes are read");
        text.push('x');
        let res = c.send(c.form(HttpMethod::Post, "/items", &text)).await;
        assert_forbidden(&res, &format!("{kind:?}: 4097 bytes"));
        assert_eq!(c.changes(), 1);
    }
}

// --- login CSRF and rotation ---

#[tokio::test]
async fn login_csrf() {
    for kind in KINDS {
        // A forged login post without a token, with and without a session.
        let c = Client::new(kind);
        let res = c.login("alice", "s3cret", None).await;
        assert_forbidden(&res, &format!("{kind:?} no session"));
        assert!(c.cookie().is_none());
        c.token().await;
        let res = c.login("alice", "s3cret", None).await;
        assert_forbidden(&res, &format!("{kind:?} with a session"));
        assert!(c.state().await.starts_with("anonymous|"));
        assert_eq!(c.calls(), 0, "{kind:?}: the provider was never asked");

        // A token from before the login is dead afterwards.
        let c = Client::new(kind);
        let before = c.token().await;
        assert_redirect(&c.login("alice", "s3cret", Some(&before)).await, "/");
        assert_forbidden(
            &c.change(HttpMethod::Post, Some(&before)).await,
            &format!("{kind:?} stale"),
        );
        assert_forbidden(
            &c.logout(Some(&before)).await,
            &format!("{kind:?} stale logout"),
        );
        assert!(c.state().await.starts_with("alice|"));

        // The same login form posted twice (the back button) fails the second time.
        let c = Client::new(kind);
        let token = c.token().await;
        assert_redirect(&c.login("alice", "s3cret", Some(&token)).await, "/");
        let calls = c.calls();
        let res = c.login("alice", "s3cret", Some(&token)).await;
        assert_forbidden(&res, &format!("{kind:?} twice"));
        assert_eq!(c.calls(), calls);
    }
}

#[tokio::test]
async fn the_token_changes_at_login() {
    for kind in KINDS {
        let c = Client::new(kind);
        let before = c.token().await;
        let cookie = c.cookie();
        assert_redirect(&c.login("alice", "s3cret", Some(&before)).await, "/");
        assert_ne!(c.cookie(), cookie, "{kind:?}: the id rotated");
        let after = c.token().await;
        assert_ne!(after, before, "{kind:?}");
        assert_eq!(c.state().await, format!("alice|{after}"));
        assert_eq!(c.change(HttpMethod::Post, Some(&after)).await.status(), 200);
    }
}

#[tokio::test]
async fn a_failed_login_keeps_the_token_and_the_session() {
    for kind in KINDS {
        let c = Client::new(kind);
        let token = c.token().await;
        let cookie = c.cookie();
        let res = c.login("alice", "wrong", Some(&token)).await;
        assert_redirect(&res, "/login?error");
        assert!(set_cookies(&res).is_empty(), "{kind:?}");
        assert_eq!(c.cookie(), cookie);
        assert_eq!(c.state().await, format!("anonymous|{token}"));
        assert_eq!(c.calls(), 1);
        // The same token still logs in.
        assert_redirect(&c.login("alice", "s3cret", Some(&token)).await, "/");
    }
}

#[tokio::test]
async fn session_expired_between_page_and_post_is_403() {
    for kind in KINDS {
        let c = Client::new(kind);
        let token = c.token().await;
        // The session is gone: the browser sends an id the server no longer knows.
        c.set_cookie(Some("A".repeat(43)));
        let res = c.login("alice", "s3cret", Some(&token)).await;
        assert_forbidden(&res, &format!("{kind:?} login"));
        assert_eq!(c.calls(), 0);

        // Logged in, then the session expired: the unsafe request is anonymous now.
        let c = Client::new(kind);
        c.log_in().await;
        let token = c.token().await;
        c.set_cookie(Some("A".repeat(43)));
        let res = c.logout(Some(&token)).await;
        assert_forbidden(&res, &format!("{kind:?} logout"));
        let res = c.change(HttpMethod::Post, Some(&token)).await;
        assert_eq!(res.status(), 401, "{kind:?}: no session, no authentication");
        assert_eq!(c.changes(), 0);
    }
}

// --- exemptions ---

#[tokio::test]
async fn basic_and_bearer_are_exempt_even_with_a_stray_session_cookie() {
    for kind in KINDS {
        let c = Client::with(kind, |s| {
            s.http_basic(|b| b)
                .bearer_provider(Arc::new(FakeBearer))
                .form_login(|f| f)
        });
        c.log_in().await;
        assert!(c.cookie().is_some());
        let mut req = c.request(HttpMethod::Post, "/items");
        req.headers.insert(
            header::AUTHORIZATION,
            basic("alice", "s3cret").parse().unwrap(),
        );
        let res = c.send(req).await;
        assert_eq!(res.status(), 200, "{kind:?} basic");
        assert_eq!(body(&res), "alice");

        let mut req = c.request(HttpMethod::Delete, "/items");
        req.headers
            .insert(header::AUTHORIZATION, "Bearer good".parse().unwrap());
        let res = c.send(req).await;
        assert_eq!(res.status(), 200, "{kind:?} bearer");
        assert_eq!(body(&res), "token-user");

        // Without a session cookie as well.
        c.set_cookie(None);
        let mut req = c.request(HttpMethod::Put, "/items");
        req.headers
            .insert(header::AUTHORIZATION, "Bearer good".parse().unwrap());
        assert_eq!(c.send(req).await.status(), 200);
        assert_eq!(c.changes(), 3);
    }
}

#[tokio::test]
async fn basic_with_a_wrong_password_is_401_not_403() {
    for kind in KINDS {
        let c = Client::with(kind, |s| s.http_basic(|b| b).form_login(|f| f));
        c.log_in().await;
        let mut req = c.request(HttpMethod::Post, "/items");
        req.headers.insert(
            header::AUTHORIZATION,
            basic("alice", "nope").parse().unwrap(),
        );
        let res = c.send(req).await;
        assert_eq!(res.status(), 401, "{kind:?}");
        assert!(res.headers().contains_key(header::WWW_AUTHENTICATE));
        assert_eq!(c.changes(), 0);
    }
}

#[tokio::test]
async fn anonymous_unsafe_requests_with_no_session_are_exempt() {
    for kind in KINDS {
        let c = Client::new(kind);
        for method in [
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            let res = c.send(c.request(method.clone(), "/open")).await;
            assert_eq!(res.status(), 200, "{kind:?} {method:?}");
            assert_eq!(body(&res), "anonymous");
            assert!(set_cookies(&res).is_empty(), "the check creates no session");
        }
        assert_eq!(c.changes(), 4);
        // An anonymous unsafe request to a protected route is a 401 from the rules, not a 403.
        assert_eq!(c.change(HttpMethod::Post, None).await.status(), 401);
    }
}

#[tokio::test]
async fn safe_methods_are_never_checked() {
    for kind in KINDS {
        let c = Client::new(kind);
        c.log_in().await;
        let res = c.send(c.request(HttpMethod::Get, "/secret")).await;
        assert_eq!(res.status(), 200, "{kind:?}");
        assert_eq!(body(&res), "alice");
        let res = c.send(c.request(HttpMethod::Head, "/secret")).await;
        assert_eq!(res.status(), 200, "{kind:?}");
        for method in [HttpMethod::Options, HttpMethod::Trace] {
            let res = c.send(c.request(method.clone(), "/secret")).await;
            assert_ne!(res.status(), 403, "{kind:?} {method:?}");
        }
    }
}

#[tokio::test]
async fn origin_check_still_runs_first_on_login_and_logout() {
    for kind in KINDS {
        let c = Client::new(kind);
        let token = c.token().await;
        let mut req = c.login_request("alice", "s3cret", Some(&token));
        req.headers
            .insert(header::ORIGIN, "http://evil.com".parse().unwrap());
        assert_forbidden(&c.send(req).await, &format!("{kind:?} login"));
        assert_eq!(c.calls(), 0);
        assert!(c.state().await.starts_with("anonymous|"));

        assert_redirect(&c.login("alice", "s3cret", Some(&token)).await, "/");
        let token = c.token().await;
        let mut req = c.logout_request(Some(&token));
        req.headers
            .insert(header::ORIGIN, "http://evil.com".parse().unwrap());
        assert_forbidden(&c.send(req).await, &format!("{kind:?} logout"));
        assert!(c.state().await.starts_with("alice|"));
    }
}

// --- settings ---

#[tokio::test]
async fn csrf_disabled_leaves_6c_behavior() {
    for kind in KINDS {
        let c = Client::with(kind, |s| s.form_login(|f| f).csrf(|c| c.enabled(false)));
        assert_redirect(&c.login("alice", "s3cret", None).await, "/");
        assert_eq!(c.change(HttpMethod::Post, None).await.status(), 200);
        assert_eq!(c.change(HttpMethod::Delete, None).await.status(), 200);
        assert_redirect(&c.logout(None).await, "/login?logout");
        assert!(c.state().await.starts_with("anonymous|"));
    }
}

#[tokio::test]
async fn configured_names_are_used() {
    for kind in KINDS {
        let c = Client::with(kind, |s| {
            s.form_login(|f| f)
                .csrf(|c| c.field_name("token").header_name("X-Token"))
        });
        let token = c.token().await;
        let req = c.form(
            HttpMethod::Post,
            "/login",
            &format!("username=alice&password=s3cret&token={token}"),
        );
        assert_redirect(&c.send(req).await, "/");
        let token = c.token().await;
        assert_forbidden(
            &c.change(HttpMethod::Post, Some(&token)).await,
            "the default header is not read",
        );
        let mut req = c.request(HttpMethod::Post, "/items");
        req.headers
            .insert("x-token", HeaderValue::from_str(&token).unwrap());
        assert_eq!(c.send(req).await.status(), 200, "{kind:?}");
    }
}

#[test]
fn build_errors() {
    let provider = || Arc::new(dao());
    let build = |s: HttpSecurity| rules(s.password_provider(provider())).build().err();
    assert_eq!(
        build(HttpSecurity::new().csrf(|c| c)),
        Some(SecurityBuildError::CsrfWithoutFormLogin)
    );
    assert_eq!(
        build(HttpSecurity::new().csrf(|c| c.enabled(false))),
        Some(SecurityBuildError::CsrfWithoutFormLogin)
    );
    for bad in [
        CsrfConfig::default().field_name("a b"),
        CsrfConfig::default().header_name(""),
        CsrfConfig::default().token_endpoint("/login"),
        CsrfConfig::default().token_endpoint("/logout"),
        CsrfConfig::default().token_endpoint("//evil.com"),
    ] {
        let err = build(
            HttpSecurity::new()
                .form_login(|f| f)
                .csrf(move |_| bad.clone()),
        );
        assert_eq!(err, Some(SecurityBuildError::InvalidCsrfConfig));
    }
    assert_eq!(build(HttpSecurity::new().form_login(|f| f)), None);
    assert_eq!(
        build(
            HttpSecurity::new()
                .form_login(|f| f)
                .csrf(|c| c.token_endpoint("/csrf"))
        ),
        None
    );
    for err in [
        SecurityBuildError::CsrfWithoutFormLogin,
        SecurityBuildError::InvalidCsrfConfig,
    ] {
        assert!(!err.to_string().is_empty());
    }
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

/// The log output of the tests in this binary, through one global subscriber that the first
/// client installs, so every test's logs land in the same buffer.
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

/// Everything a response shows: its `Debug`, its headers and its body.
fn shown(res: &Response) -> String {
    let mut text = format!("{res:?}");
    for (k, v) in res.headers() {
        text.push_str(k.as_str());
        text.push_str(&String::from_utf8_lossy(v.as_bytes()));
    }
    text.push_str(&body(res));
    text
}

#[tokio::test]
async fn no_token_in_logs_errors_or_debug() {
    let logs = logs();
    let mut tokens = Vec::new();
    let mut seen = String::new();
    for kind in KINDS {
        let c = Client::new(kind);
        let before = c.token().await;
        tokens.push(before.clone());
        seen.push_str(&shown(&c.login("alice", "wrong", Some(&before)).await));
        seen.push_str(&shown(
            &c.login("alice", "s3cret", Some(&altered(&before))).await,
        ));
        seen.push_str(&shown(&c.login("alice", "s3cret", Some(&before)).await));
        let token = c.token().await;
        tokens.push(token.clone());
        seen.push_str(&shown(&c.change(HttpMethod::Post, Some(&before)).await));
        seen.push_str(&shown(&c.change(HttpMethod::Put, Some(&token)).await));

        // The chain marks the token header sensitive, so the request's `Debug` hides it.
        let mut req = c.request(HttpMethod::Post, "/debug");
        req.headers
            .insert("x-csrf-token", HeaderValue::from_str(&token).unwrap());
        let res = c.send(req).await;
        assert_eq!(res.status(), 200, "{kind:?}");
        assert!(body(&res).contains("Sensitive"), "{}", body(&res));
        seen.push_str(&shown(&res));

        seen.push_str(&shown(&c.logout(Some(&altered(&token))).await));
        seen.push_str(&shown(&c.logout(Some(&token)).await));
    }
    for err in [
        SecurityBuildError::CsrfWithoutFormLogin,
        SecurityBuildError::InvalidCsrfConfig,
    ] {
        seen.push_str(&format!("{err} {err:?}"));
    }
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for token in &tokens {
        assert!(
            !text.contains(token.as_str()),
            "a token leaked into the logs:\n{text}"
        );
        assert!(
            !seen.contains(token.as_str()),
            "a token leaked into:\n{seen}"
        );
        assert!(
            !seen.contains(&altered(token)),
            "a submitted value leaked:\n{seen}"
        );
    }
}
