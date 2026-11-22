//! Auto login from the remember-me cookie, through `App::handle` over both session backends with
//! a manual clock.
//!
//! A browser logs in with the checkbox and keeps the cookie; a second browser that has only the
//! remember-me cookie, and no session, stands for a browser whose session is gone. The user
//! service is a test double that counts its calls and can fail.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthenticationError, AuthenticationProvider,
    AuthenticationRequest, CookieSessionBackend, DaoAuthenticationProvider, GrantedAuthority,
    HttpSecurity, InMemorySessionStore, PasswordEncoder, PrincipalSessions, RequestSecurityExt,
    RequestSessionExt, SessionBackend, SessionFilter, SessionRegistry, SessionSettings,
    SessionStore, UserDetails, UserDetailsError, UserDetailsService,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};
use tracing_subscriber::fmt::MakeWriter;

const SESSION: &str = "OXBLOOM_SESSION";
const REMEMBER: &str = "OXBLOOM_REMEMBER_ME";
const SESSION_SECRET: &str = "a-session-secret-of-at-least-32-bytes!";
const REMEMBER_SECRET: &str = "a-remember-me-secret-of-32-bytes";
const HOST: &str = "example.com";
const FORM: &str = "application/x-www-form-urlencoded";
const FOURTEEN_DAYS: u64 = 14 * 24 * 3600;
const CLEARING: &str = "OXBLOOM_REMEMBER_ME=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax";
const UNAVAILABLE_LOG: &str =
    "the user service failed during the remember-me login; the cookie is kept";
const NO_SESSION_LOG: &str = "remember-me has no session: add a SessionFilter to the application";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Server,
    Cookie,
}

const KINDS: [Kind; 2] = [Kind::Server, Kind::Cookie];

/// A clock that moves only when the test says so.
struct ManualClock(Mutex<SystemTime>);

impl ManualClock {
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock(Mutex::new(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
    )))
}

fn encoder() -> Argon2PasswordEncoder {
    Argon2PasswordEncoder::new(8, 1, 1).unwrap()
}

/// A user service the tests change while the app runs. It counts every call and fails while
/// `fail` is set.
struct Users {
    map: Mutex<HashMap<String, UserDetails>>,
    calls: AtomicUsize,
    fail: AtomicBool,
}

impl Users {
    fn new() -> Arc<Self> {
        let users = Arc::new(Self {
            map: Mutex::new(HashMap::new()),
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        });
        users.put(alice());
        users
    }

    fn put(&self, user: UserDetails) {
        self.map
            .lock()
            .unwrap()
            .insert(user.username().to_owned(), user);
    }

    fn remove(&self, name: &str) {
        self.map.lock().unwrap().remove(name);
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn fail(&self, fail: bool) {
        self.fail.store(fail, Ordering::SeqCst);
    }
}

#[async_trait]
impl UserDetailsService for Users {
    async fn load_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(UserDetailsError::new("the user store is down"));
        }
        Ok(self.map.lock().unwrap().get(username).cloned())
    }
}

fn alice() -> UserDetails {
    UserDetails::new("alice", encoder().encode("s3cret"), []).roles(&["USER"])
}

/// A bearer provider that knows the token `good`, for `bob`.
struct Bearer;

#[async_trait]
impl AuthenticationProvider for Bearer {
    fn supports(&self, req: &AuthenticationRequest) -> bool {
        matches!(req, AuthenticationRequest::BearerToken(_))
    }

    async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        match req {
            AuthenticationRequest::BearerToken(t) if t.expose() == "good" => Ok(
                Authentication::new("bob", [GrantedAuthority::new("ROLE_USER")]),
            ),
            _ => Err(AuthenticationError::InvalidToken),
        }
    }
}

type Index = Option<Arc<dyn PrincipalSessions>>;

/// Adds to the default chain; gets the user service and, for the server store, its index.
type Configure = Box<dyn FnOnce(HttpSecurity, Arc<Users>, Index) -> HttpSecurity>;

struct Env {
    app: App,
    clock: Arc<ManualClock>,
    users: Arc<Users>,
    index: Index,
}

impl Env {
    /// Form login with the CSRF check, Basic, a bearer provider and remember-me.
    fn new(kind: Kind) -> Self {
        Self::build(kind, Users::new(), clock(), true, Box::new(|s, _, _| s))
    }

    fn with(kind: Kind, configure: Configure) -> Self {
        Self::build(kind, Users::new(), clock(), true, configure)
    }

    fn build(
        kind: Kind,
        users: Arc<Users>,
        clock: Arc<ManualClock>,
        session_filter: bool,
        configure: Configure,
    ) -> Self {
        let settings = SessionSettings::default()
            .idle_timeout(Duration::from_secs(400 * 86_400))
            .absolute_timeout(Duration::from_secs(400 * 86_400));
        let (backend, index) = match kind {
            Kind::Server => {
                let store = Arc::new(InMemorySessionStore::new(&settings, clock.clone()));
                let index = store.clone().principal_sessions();
                (SessionBackend::Server(store), index)
            }
            Kind::Cookie => (
                SessionBackend::Cookie(Arc::new(
                    CookieSessionBackend::new(
                        &Secret::new(SESSION_SECRET.to_owned()),
                        &[],
                        &settings,
                        clock.clone(),
                    )
                    .unwrap(),
                )),
                None,
            ),
        };
        let filter = SessionFilter::new(backend, settings)
            .unwrap()
            .with_clock(clock.clone());
        let enc = Arc::new(encoder());
        let remember_users = users.clone();
        let remember_clock = clock.clone();
        let security = HttpSecurity::new()
            .password_provider(Arc::new(DaoAuthenticationProvider::new(users.clone(), enc)))
            .bearer_provider(Arc::new(Bearer))
            .http_basic(|b| b)
            .form_login(|f| f)
            .clock(clock.clone())
            .remember_me(move |r| {
                r.secret(Secret::new(REMEMBER_SECRET.to_owned()))
                    .user_details(remember_users)
                    .clock(remember_clock)
            })
            .authorize(|a| {
                a.get("/token")
                    .permit_all()
                    .get("/whoami")
                    .permit_all()
                    .get("/sets")
                    .permit_all()
                    .get("/plant")
                    .permit_all()
                    .get("/full")
                    .fully_authenticated()
                    .any_request()
                    .authenticated()
            });
        let chain = configure(security, users.clone(), index.clone())
            .build()
            .unwrap();
        logs();
        let mut app = App::new();
        if session_filter {
            app = app.filter(filter);
        }
        let app = app
            .filter(chain)
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
                HttpMethod::Get,
                "/whoami",
                |req: Request| async move {
                    req.authentication()
                        .map(|a| {
                            format!(
                                "{}|{}|{}",
                                a.principal(),
                                a.is_remembered(),
                                a.has_role("ADMIN")
                            )
                        })
                        .unwrap_or_else(|| "anonymous".to_owned())
                },
            ))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/full", |_| async { "full" }))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/auth", |_| async { "auth" }))
            .unwrap()
            .route(Route::new(HttpMethod::Post, "/change", |_| async {
                "changed"
            }))
            .unwrap()
            .route(Route::new(HttpMethod::Get, "/sets", |_| async {
                let mut res = Response::new("set".into());
                res.headers_mut().append(
                    header::SET_COOKIE,
                    HeaderValue::from_static("OXBLOOM_REMEMBER_ME=app; Path=/"),
                );
                res
            }))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/plant",
                |req: Request| async move {
                    let auth = Authentication::new("alice", [GrantedAuthority::new("ROLE_USER")])
                        .remembered();
                    let value = serde_json::json!({
                        "v": 1,
                        "login": 1_000_000,
                        "authentication": auth,
                    });
                    req.session()
                        .unwrap()
                        .set_reserved("oxbloom.security.authentication", &value)
                        .unwrap();
                    "planted"
                },
            ))
            .unwrap();
        Self {
            app,
            clock,
            users,
            index,
        }
    }

    fn browser(&self) -> Browser<'_> {
        Browser {
            env: self,
            session: Mutex::new(None),
            remember: Mutex::new(None),
        }
    }

    /// A browser with no session that holds only the remember-me cookie `value`.
    fn remembered_browser(&self, value: &str) -> Browser<'_> {
        let b = self.browser();
        *b.remember.lock().unwrap() = Some(value.to_owned());
        b
    }

    /// Logs in with the checkbox in a browser of its own and returns the remember-me cookie.
    async fn issue(&self) -> String {
        let b = self.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        assert_eq!(res.status(), StatusCode::FOUND, "{res:?}");
        value_of(&res, REMEMBER).expect("the login issues the cookie")
    }
}

/// One browser with its own session cookie and remember-me cookie.
struct Browser<'a> {
    env: &'a Env,
    session: Mutex<Option<String>>,
    remember: Mutex<Option<String>>,
}

impl Browser<'_> {
    fn request(&self, method: HttpMethod, path: &str) -> Request {
        let mut req = Request::new(method, path);
        req.headers.insert(header::HOST, HOST.parse().unwrap());
        let mut cookies = Vec::new();
        if let Some(value) = self.session.lock().unwrap().clone() {
            cookies.push(format!("{SESSION}={value}"));
        }
        if let Some(value) = self.remember.lock().unwrap().clone() {
            cookies.push(format!("{REMEMBER}={value}"));
        }
        if !cookies.is_empty() {
            req.headers
                .insert(header::COOKIE, cookies.join("; ").parse().unwrap());
        }
        req
    }

    async fn send(&self, req: Request) -> Response {
        let res = self.env.app.handle(req).await;
        for value in res.headers().get_all(header::SET_COOKIE) {
            let text = value.to_str().unwrap();
            for (name, jar) in [(SESSION, &self.session), (REMEMBER, &self.remember)] {
                if let Some(rest) = text.strip_prefix(&format!("{name}=")) {
                    let mut parts = rest.split("; ");
                    let value = parts.next().unwrap().to_owned();
                    let expired = parts.any(|p| p == "Max-Age=0");
                    *jar.lock().unwrap() = (!expired).then_some(value);
                }
            }
        }
        res
    }

    async fn get(&self, path: &str) -> Response {
        self.send(self.request(HttpMethod::Get, path)).await
    }

    async fn text(&self, path: &str) -> String {
        body(&self.get(path).await)
    }

    async fn who(&self) -> String {
        self.text("/whoami").await
    }

    fn form(&self, path: &str, body: &str) -> Request {
        let mut req = self.request(HttpMethod::Post, path);
        req.headers
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(FORM));
        req.body = body.as_bytes().to_vec().into();
        req
    }

    async fn login(&self, user: &str, pass: &str, extra: &str) -> Response {
        let token = self.text("/token").await;
        let body = format!("username={user}&password={pass}&_csrf={token}{extra}");
        self.send(self.form("/login", &body)).await
    }
}

fn body(res: &Response) -> String {
    String::from_utf8_lossy(res.body()).into_owned()
}

fn set_cookies(res: &Response) -> Vec<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

fn cookie_named(res: &Response, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    set_cookies(res)
        .into_iter()
        .find(|c| c.starts_with(&prefix))
}

fn value_of(res: &Response, name: &str) -> Option<String> {
    let text = cookie_named(res, name)?;
    let rest = &text[name.len() + 1..];
    Some(rest.split("; ").next().unwrap().to_owned())
}

/// A token with the parts of `token`, the expiry replaced.
fn with_expiry(token: &str, expiry: &str) -> String {
    let p: Vec<&str> = token.split('.').collect();
    format!("{}.{}.{expiry}.{}", p[0], p[1], p[3])
}

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

fn log_text() -> String {
    String::from_utf8(logs().0.lock().unwrap().clone()).unwrap()
}

// --- success ---

#[tokio::test]
async fn an_issued_cookie_logs_in_again_after_the_session_is_gone() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        let b = env.remembered_browser(&token);
        let res = b.get("/whoami").await;
        assert_eq!(res.status(), StatusCode::OK, "{kind:?}");
        assert_eq!(body(&res), "alice|true|false", "{kind:?}");
        assert!(cookie_named(&res, SESSION).is_some(), "{kind:?} {res:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
        // The session carries the login from now on, the cookie is not needed any more.
        *b.remember.lock().unwrap() = None;
        let calls = env.users.calls();
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        assert_eq!(env.users.calls(), calls, "no lookup for a session login");
        // The cookie was not reissued, its lifetime stays fixed.
    }
}

#[tokio::test]
async fn auto_login_writes_the_revalidation_time() {
    for kind in KINDS {
        let env = Env::with(
            kind,
            Box::new(|s, users, _| s.revalidate(Duration::from_secs(60), users)),
        );
        let token = env.issue().await;
        env.clock.advance(Duration::from_secs(30));
        let b = env.remembered_browser(&token);
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        *b.remember.lock().unwrap() = None;
        // The check of the auto login counts: the next one is due an interval later.
        let calls = env.users.calls();
        env.clock.advance(Duration::from_secs(59));
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        assert_eq!(env.users.calls(), calls, "{kind:?}");
        env.clock.advance(Duration::from_secs(1));
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        assert_eq!(env.users.calls(), calls + 1, "{kind:?}");
    }
}

#[tokio::test]
async fn a_remembered_user_is_authenticated_but_not_fully() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        let b = env.remembered_browser(&token);
        let res = b.get("/full").await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{kind:?}");
        let res = b.get("/auth").await;
        assert_eq!(res.status(), StatusCode::OK, "{kind:?}");
        assert_eq!(body(&res), "auth");

        // A password login in the remembered session lifts the marker.
        let res = b.login("alice", "s3cret", "").await;
        assert_eq!(res.status(), StatusCode::FOUND, "{kind:?}");
        assert_eq!(b.who().await, "alice|false|false", "{kind:?}");
        assert_eq!(b.get("/full").await.status(), StatusCode::OK, "{kind:?}");
    }
}

#[tokio::test]
async fn a_password_login_drops_a_planted_marker() {
    for kind in KINDS {
        let env = Env::new(kind);
        let b = env.browser();
        assert_eq!(b.text("/plant").await, "planted");
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        let res = b.login("alice", "s3cret", "").await;
        assert_eq!(res.status(), StatusCode::FOUND, "{kind:?}");
        assert_eq!(b.who().await, "alice|false|false", "{kind:?}");
    }
}

#[tokio::test]
async fn a_post_that_logs_in_is_refused_and_the_next_get_works() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        let b = env.remembered_browser(&token);
        let res = b.send(b.form("/change", "a=b")).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{kind:?}");
        assert!(cookie_named(&res, SESSION).is_some(), "{kind:?} {res:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
        // The session of the refused request was kept.
        *b.remember.lock().unwrap() = None;
        let calls = env.users.calls();
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        assert_eq!(env.users.calls(), calls);
    }
}

// --- header credentials win ---

#[tokio::test]
async fn basic_with_a_wrong_password_is_401_and_never_logs_in() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        let b = env.remembered_browser(&token);
        let mut req = b.request(HttpMethod::Get, "/whoami");
        let basic = format!("Basic {}", STANDARD.encode("alice:wrong"));
        req.headers
            .insert(header::AUTHORIZATION, basic.parse().unwrap());
        let res = b.send(req).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{kind:?}");
        assert!(set_cookies(&res).is_empty(), "{kind:?} {res:?}");
    }
}

#[tokio::test]
async fn a_bearer_token_ignores_the_cookie() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        let calls = env.users.calls();
        let b = env.remembered_browser(&token);
        let mut req = b.request(HttpMethod::Get, "/whoami");
        req.headers
            .insert(header::AUTHORIZATION, "Bearer good".parse().unwrap());
        let res = b.send(req).await;
        assert_eq!(body(&res), "bob|false|false", "{kind:?}");
        assert!(set_cookies(&res).is_empty(), "{kind:?} {res:?}");
        assert_eq!(env.users.calls(), calls);

        let mut req = b.request(HttpMethod::Get, "/whoami");
        req.headers
            .insert(header::AUTHORIZATION, "Bearer bad".parse().unwrap());
        assert_eq!(b.send(req).await.status(), StatusCode::UNAUTHORIZED);

        // An unknown scheme is no credential either, but still keeps the cookie out.
        let mut req = b.request(HttpMethod::Get, "/whoami");
        req.headers
            .insert(header::AUTHORIZATION, "Negotiate abc".parse().unwrap());
        let res = b.send(req).await;
        assert_eq!(body(&res), "anonymous", "{kind:?}");
        assert!(set_cookies(&res).is_empty(), "{kind:?} {res:?}");
        assert_eq!(env.users.calls(), calls);
    }
}

// --- invalid cookies ---

/// The answers to `GET /whoami` and `GET /auth` from a browser that sends only `value`.
async fn probe(env: &Env, value: &str) -> Vec<(u16, String, Vec<String>)> {
    let mut answers = Vec::new();
    for path in ["/whoami", "/auth"] {
        let b = env.remembered_browser(value);
        let res = b.get(path).await;
        answers.push((res.status().as_u16(), body(&res), set_cookies(&res)));
    }
    answers
}

#[tokio::test]
async fn every_invalid_cookie_gets_the_same_anonymous_answer_and_a_clearing_cookie() {
    for kind in KINDS {
        let mut answers = Vec::new();
        let tampered_mac = |token: &str| {
            let p: Vec<&str> = token.rsplitn(2, '.').collect();
            format!("{}.{}", p[1], URL_SAFE_NO_PAD.encode([7u8; 32]))
        };
        type Shape = Box<dyn Fn(&Env, String) -> String>;
        let shapes: Vec<(&str, Shape)> = vec![
            ("tampered mac", Box::new(move |_, t| tampered_mac(&t))),
            ("malformed", Box::new(|_, _| "garbage".to_owned())),
            ("short", Box::new(|_, _| "v1.YWxpY2U.1".to_owned())),
            (
                "expired",
                Box::new(|env, t| {
                    env.clock.advance(Duration::from_secs(FOURTEEN_DAYS + 1));
                    t
                }),
            ),
            (
                "unknown user",
                Box::new(|env, t| {
                    env.users.remove("alice");
                    t
                }),
            ),
            (
                "disabled",
                Box::new(|env, t| {
                    env.users.put(alice().enabled(false));
                    t
                }),
            ),
            (
                "locked",
                Box::new(|env, t| {
                    env.users.put(alice().locked(true));
                    t
                }),
            ),
            (
                "changed password",
                Box::new(|env, t| {
                    env.users.put(
                        UserDetails::new("alice", encoder().encode("new-pass"), [])
                            .roles(&["USER"]),
                    );
                    t
                }),
            ),
            (
                "forged expiry i64::MAX",
                Box::new(|_, t| with_expiry(&t, &i64::MAX.to_string())),
            ),
            (
                "forged expiry u64::MAX",
                Box::new(|_, t| with_expiry(&t, &u64::MAX.to_string())),
            ),
        ];
        for (name, shape) in &shapes {
            let env = Env::new(kind);
            let token = env.issue().await;
            let value = shape(&env, token);
            let answer = probe(&env, &value).await;
            assert_eq!(answer[0].0, 200, "{kind:?} {name}");
            assert_eq!(answer[0].1, "anonymous", "{kind:?} {name}");
            assert_eq!(answer[0].2, vec![CLEARING.to_owned()], "{kind:?} {name}");
            assert_eq!(answer[1].0, 401, "{kind:?} {name}");
            assert_eq!(answer[1].2, vec![CLEARING.to_owned()], "{kind:?} {name}");
            answers.push((name, answer));
        }
        let first = &answers[0].1;
        for (name, answer) in &answers {
            assert_eq!(
                answer, first,
                "{kind:?} {name} differs from the first shape"
            );
        }
    }
}

#[tokio::test]
async fn an_expired_cookie_costs_no_user_service_call() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        env.clock.advance(Duration::from_secs(FOURTEEN_DAYS));
        let calls = env.users.calls();
        let answer = probe(&env, &token).await;
        assert_eq!(answer[0].1, "anonymous", "{kind:?}");
        assert_eq!(answer[0].2, vec![CLEARING.to_owned()]);
        assert_eq!(env.users.calls(), calls, "{kind:?}");
        // A malformed one costs none either.
        probe(&env, "v1.x.y.z").await;
        assert_eq!(env.users.calls(), calls, "{kind:?}");
    }
}

#[tokio::test]
async fn the_cookie_works_until_its_last_second() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        env.clock.advance(Duration::from_secs(FOURTEEN_DAYS - 1));
        let b = env.remembered_browser(&token);
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
    }
}

#[tokio::test]
async fn a_cookie_issued_under_a_longer_lifetime_is_refused_without_a_lookup() {
    for kind in KINDS {
        // Issued under the default 14 days, checked by an app that now allows one hour.
        let env = Env::new(kind);
        let token = env.issue().await;
        let short = Env::build(
            kind,
            env.users.clone(),
            env.clock.clone(),
            true,
            Box::new(|s, _, _| s.remember_me(|r| r.lifetime(Duration::from_secs(3600)))),
        );
        let calls = short.users.calls();
        let answer = probe(&short, &token).await;
        assert_eq!(answer[0].1, "anonymous", "{kind:?}");
        assert_eq!(answer[0].2, vec![CLEARING.to_owned()], "{kind:?}");
        assert_eq!(short.users.calls(), calls, "{kind:?}");
    }
}

#[tokio::test]
async fn a_cookie_issued_under_the_current_lifetime_works_at_the_boundary() {
    for kind in KINDS {
        let env = Env::build(
            kind,
            Users::new(),
            clock(),
            true,
            Box::new(|s, _, _| s.remember_me(|r| r.lifetime(Duration::from_secs(3600)))),
        );
        let token = env.issue().await;
        // Its expiry is exactly now plus the lifetime: allowed.
        let b = env.remembered_browser(&token);
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        // One second later it is below the bound still.
        env.clock.advance(Duration::from_secs(1));
        let b = env.remembered_browser(&token);
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
    }
}

#[tokio::test]
async fn the_security_headers_are_on_the_clearing_cookie_and_the_403_answers() {
    for kind in KINDS {
        let env = Env::new(kind);
        let res = env.remembered_browser("garbage").get("/whoami").await;
        assert_eq!(set_cookies(&res), vec![CLEARING.to_owned()], "{kind:?}");
        assert_eq!(
            res.headers()["x-content-type-options"],
            "nosniff",
            "{kind:?}"
        );
        assert!(res.headers().contains_key("referrer-policy"), "{kind:?}");

        let token = env.issue().await;
        let b = env.remembered_browser(&token);
        let res = b.send(b.form("/change", "a=b")).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{kind:?}");
        assert_eq!(
            res.headers()["x-content-type-options"],
            "nosniff",
            "{kind:?}"
        );
        assert!(res.headers().contains_key("referrer-policy"), "{kind:?}");
    }
}

#[tokio::test]
async fn the_clearing_cookie_is_not_added_when_the_response_sets_the_cookie() {
    for kind in KINDS {
        let env = Env::new(kind);
        let b = env.remembered_browser("garbage");
        let res = b.get("/sets").await;
        assert_eq!(body(&res), "set", "{kind:?}");
        assert_eq!(
            set_cookies(&res),
            vec!["OXBLOOM_REMEMBER_ME=app; Path=/".to_owned()],
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn a_cookie_header_over_eight_kib_counts_as_no_cookie() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        let mut req = Request::new(HttpMethod::Get, "/whoami");
        req.headers.insert(header::HOST, HOST.parse().unwrap());
        let header = format!("pad={}; {REMEMBER}={token}", "x".repeat(8 * 1024));
        req.headers.insert(header::COOKIE, header.parse().unwrap());
        let res = env.app.handle(req).await;
        assert_eq!(body(&res), "anonymous", "{kind:?}");
        assert!(set_cookies(&res).is_empty(), "{kind:?}");
    }
}

// --- the user service fails ---

#[tokio::test]
async fn a_user_service_error_keeps_the_cookie_and_stays_anonymous() {
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        env.users.fail(true);
        let b = env.remembered_browser(&token);
        let res = b.get("/whoami").await;
        assert_eq!(body(&res), "anonymous", "{kind:?}");
        assert!(set_cookies(&res).is_empty(), "{kind:?} {res:?}");
        assert!(log_text().contains(UNAVAILABLE_LOG));
        env.users.fail(false);
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
    }
    assert!(!log_text().contains("store is down"));
}

// --- the session limit and revalidation ---

#[tokio::test]
async fn auto_login_counts_against_the_session_limit_and_evicts() {
    // The per-user index exists for the server store only.
    let env = Env::with(
        Kind::Server,
        Box::new(|s, _, index| s.session_limit(index.unwrap(), 2)),
    );
    let registry = SessionRegistry::new(env.index.clone().unwrap());
    let a = env.browser();
    a.login("alice", "s3cret", "").await;
    env.clock.advance(Duration::from_secs(1));
    let c = env.browser();
    let res = c.login("alice", "s3cret", "&remember-me=on").await;
    let token = value_of(&res, REMEMBER).unwrap();
    *c.remember.lock().unwrap() = None;
    env.clock.advance(Duration::from_secs(1));
    assert_eq!(registry.sessions_of("alice").await.unwrap().len(), 2);

    let b = env.remembered_browser(&token);
    assert_eq!(b.who().await, "alice|true|false");
    assert_eq!(registry.sessions_of("alice").await.unwrap().len(), 2);
    assert_eq!(a.who().await, "anonymous", "the oldest session is evicted");
    assert_eq!(c.who().await, "alice|false|false");
    assert_eq!(b.who().await, "alice|true|false");
}

#[tokio::test]
async fn revalidation_keeps_the_marker_and_ends_a_disabled_user() {
    for kind in KINDS {
        let env = Env::with(
            kind,
            Box::new(|s, users, _| s.revalidate(Duration::from_secs(60), users)),
        );
        let token = env.issue().await;
        let b = env.remembered_browser(&token);
        assert_eq!(b.who().await, "alice|true|false", "{kind:?}");
        *b.remember.lock().unwrap() = None;

        env.users.put(alice().roles(&["ADMIN"]));
        env.clock.advance(Duration::from_secs(61));
        assert_eq!(b.who().await, "alice|true|true", "{kind:?}");

        env.users.put(alice().enabled(false));
        env.clock.advance(Duration::from_secs(61));
        assert_eq!(b.who().await, "anonymous", "{kind:?}");

        // With the cookie back, the disabled user is refused and the cookie cleared.
        let b = env.remembered_browser(&token);
        let res = b.get("/whoami").await;
        assert_eq!(body(&res), "anonymous", "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER).as_deref(), Some(CLEARING));
    }
}

// --- without a session filter ---

#[tokio::test]
async fn without_a_session_filter_there_is_no_auto_login() {
    for kind in KINDS {
        let users = Users::new();
        let clock = clock();
        let with = Env::build(
            kind,
            users.clone(),
            clock.clone(),
            true,
            Box::new(|s, _, _| s),
        );
        let token = with.issue().await;
        let without = Env::build(kind, users.clone(), clock, false, Box::new(|s, _, _| s));
        let calls = users.calls();
        let b = without.remembered_browser(&token);
        let res = b.get("/whoami").await;
        assert_eq!(body(&res), "anonymous", "{kind:?}");
        assert!(set_cookies(&res).is_empty(), "{kind:?} {res:?}");
        assert_eq!(users.calls(), calls, "{kind:?}");
        assert!(log_text().contains(NO_SESSION_LOG));
    }
}

// --- nothing secret is shown ---

#[tokio::test]
async fn no_secret_token_or_username_reaches_a_log_or_debug() {
    let mut tokens = Vec::new();
    for kind in KINDS {
        let env = Env::new(kind);
        let token = env.issue().await;
        let b = env.remembered_browser(&token);
        let res = b.get("/whoami").await;
        assert!(!format!("{res:?}").contains(&token), "{kind:?}");
        // Every failure path too.
        env.users.fail(true);
        probe(&env, &token).await;
        env.users.fail(false);
        let forged = with_expiry(&token, "99999999999");
        let answer = probe(&env, &forged).await;
        assert!(!format!("{answer:?}").contains(&forged));
        env.users.put(alice().enabled(false));
        probe(&env, &token).await;
        tokens.push(token);
    }
    let text = log_text();
    assert!(!text.contains(REMEMBER_SECRET), "{text}");
    for token in &tokens {
        assert!(!text.contains(token.as_str()), "{text}");
        assert!(!text.contains(token.rsplit('.').next().unwrap()), "{text}");
    }
    assert!(!text.contains(&URL_SAFE_NO_PAD.encode("alice")), "{text}");
    assert!(!text.contains("\"alice\""), "{text}");
    assert!(!text.contains("alice"), "{text}");
}
