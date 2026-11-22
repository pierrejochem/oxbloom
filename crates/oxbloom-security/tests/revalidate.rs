//! Revalidation of the stored authentication, through `App::handle` with a manual clock and a
//! user service whose users change during the test, over both session backends.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::{HeaderValue, header};
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthenticationError, AuthenticationProvider,
    AuthenticationRequest, CookieSessionBackend, DaoAuthenticationProvider, GrantedAuthority,
    HttpSecurity, InMemorySessionStore, PasswordEncoder, RequestSecurityExt, RequestSessionExt,
    SecurityBuildError, SessionBackend, SessionFilter, SessionSettings, UserDetails,
    UserDetailsError, UserDetailsService,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};
use tracing_subscriber::fmt::MakeWriter;

const NAME: &str = "OXBLOOM_SESSION";
const HOST: &str = "example.com";
const FORM: &str = "application/x-www-form-urlencoded";
const SECRET: &str = "0123456789abcdef0123456789abcdef";
const USER: &str = "revalidated-user";
const OTHER: &str = "basic-user";
const PASSWORD: &str = "s3cret";
const INTERVAL: Duration = Duration::from_secs(300);
const REVALIDATED_KEY: &str = "oxbloom.security.revalidated-at";
const AUTH_KEY: &str = "oxbloom.security.authentication";

#[derive(Debug, Clone, Copy)]
enum Kind {
    Server,
    Cookie,
}

const KINDS: [Kind; 2] = [Kind::Server, Kind::Cookie];

/// A clock that moves only when the test says so.
struct ManualClock(Mutex<SystemTime>);

impl ManualClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
        )))
    }

    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }

    fn secs(&self) -> u64 {
        self.now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

/// Users that the test can change while the application runs.
struct Users {
    hash: String,
    map: Mutex<HashMap<String, UserDetails>>,
}

impl Users {
    fn new() -> Arc<Self> {
        let encoder = Argon2PasswordEncoder::new(8, 1, 1).unwrap();
        let users = Arc::new(Self {
            hash: encoder.encode(PASSWORD),
            map: Mutex::new(HashMap::new()),
        });
        users.put(USER, &["USER", "ADMIN"], true, false);
        users.put(OTHER, &["USER"], true, false);
        users
    }

    fn put(&self, name: &str, roles: &[&str], enabled: bool, locked: bool) {
        let user = UserDetails::new(name, self.hash.clone(), [])
            .roles(roles)
            .enabled(enabled)
            .locked(locked);
        self.map.lock().unwrap().insert(name.to_owned(), user);
    }

    fn delete(&self, name: &str) {
        self.map.lock().unwrap().remove(name);
    }
}

#[async_trait]
impl UserDetailsService for Users {
    async fn load_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        Ok(self.map.lock().unwrap().get(username).cloned())
    }
}

/// The service revalidation uses: the same users, counted, and able to fail.
struct Counting {
    users: Arc<Users>,
    calls: AtomicUsize,
    fail: AtomicBool,
}

#[async_trait]
impl UserDetailsService for Counting {
    async fn load_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(UserDetailsError::new("down"));
        }
        self.users.load_user_by_username(username).await
    }
}

/// The password provider of the test: the DAO provider, plus details on every success.
struct WithDetails(DaoAuthenticationProvider);

#[async_trait]
impl AuthenticationProvider for WithDetails {
    fn supports(&self, req: &AuthenticationRequest) -> bool {
        self.0.supports(req)
    }

    async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        let mut details = serde_json::Map::new();
        details.insert("tenant".into(), serde_json::json!("acme"));
        Ok(self.0.authenticate(req).await?.with_details(details))
    }
}

/// A bearer provider that accepts the token `good` as the main user.
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
                Authentication::new(USER, [GrantedAuthority::new("ROLE_USER")]),
            ),
            _ => Err(AuthenticationError::InvalidToken),
        }
    }
}

struct Env {
    app: App,
    clock: Arc<ManualClock>,
    users: Arc<Users>,
    service: Arc<Counting>,
}

fn principal(req: &Request) -> String {
    req.authentication()
        .map(|a| a.principal().to_string())
        .unwrap_or_else(|| "anonymous".to_owned())
}

fn security(users: &Arc<Users>, service: Arc<Counting>, clock: Arc<ManualClock>) -> HttpSecurity {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let dao = DaoAuthenticationProvider::new(users.clone(), encoder);
    HttpSecurity::new()
        .password_provider(Arc::new(WithDetails(dao)))
        .bearer_provider(Arc::new(Bearer))
        .http_basic(|b| b)
        .form_login(|f| f)
        .clock(clock)
        .revalidate(INTERVAL, service)
        .authorize(|a| {
            a.get("/token")
                .permit_all()
                .get("/whoami")
                .permit_all()
                .get("/keys")
                .permit_all()
                .get("/forget")
                .permit_all()
                .get("/admin")
                .has_role("ADMIN")
                .any_request()
                .authenticated()
        })
}

impl Env {
    fn new(kind: Kind) -> Self {
        logs();
        let clock = ManualClock::new();
        let users = Users::new();
        let service = Arc::new(Counting {
            users: users.clone(),
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        });
        let settings = SessionSettings::default()
            .idle_timeout(Duration::from_secs(86_400))
            .absolute_timeout(Duration::from_secs(864_000));
        let backend = match kind {
            Kind::Server => SessionBackend::Server(Arc::new(InMemorySessionStore::new(
                &settings,
                clock.clone(),
            ))),
            Kind::Cookie => SessionBackend::Cookie(Arc::new(
                CookieSessionBackend::new(
                    &Secret::new(SECRET.to_owned()),
                    &[],
                    &settings,
                    clock.clone(),
                )
                .unwrap(),
            )),
        };
        let filter = SessionFilter::new(backend, settings)
            .unwrap()
            .with_clock(clock.clone());
        let chain = security(&users, service.clone(), clock.clone())
            .build()
            .unwrap();
        let app = App::new()
            .filter(filter)
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
                |req: Request| async move { principal(&req) },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/admin",
                |req: Request| async move { principal(&req) },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/roles",
                |req: Request| async move {
                    req.authentication()
                        .map(|a| {
                            let names: Vec<&str> =
                                a.authorities().iter().map(|g| g.as_str()).collect();
                            names.join(",")
                        })
                        .unwrap_or_default()
                },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Post,
                "/change",
                |req: Request| async move { principal(&req) },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/forget",
                |req: Request| async move {
                    if let Some(session) = req.session() {
                        session.remove_reserved(REVALIDATED_KEY);
                    }
                    "forgotten"
                },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/keys",
                |req: Request| async move {
                    let Some(session) = req.session() else {
                        return "no session".to_owned();
                    };
                    let revalidated = session
                        .get::<serde_json::Value>(REVALIDATED_KEY)
                        .unwrap()
                        .unwrap_or(serde_json::Value::Null);
                    let stored = session
                        .get::<serde_json::Value>(AUTH_KEY)
                        .unwrap()
                        .unwrap_or(serde_json::Value::Null);
                    serde_json::json!({ "revalidated": revalidated, "stored": stored }).to_string()
                },
            ))
            .unwrap();
        Self {
            app,
            clock,
            users,
            service,
        }
    }

    fn calls(&self) -> usize {
        self.service.calls.load(Ordering::SeqCst)
    }

    /// Moves the clock to just past the interval.
    fn pass_interval(&self) {
        self.clock.advance(INTERVAL + Duration::from_secs(1));
    }

    fn browser(&self) -> Browser<'_> {
        Browser {
            env: self,
            jar: Mutex::new(None),
        }
    }
}

/// One browser with its own session cookie.
struct Browser<'a> {
    env: &'a Env,
    jar: Mutex<Option<String>>,
}

impl Browser<'_> {
    fn request(&self, method: HttpMethod, path: &str) -> Request {
        let mut req = Request::new(method, path);
        req.headers.insert(header::HOST, HOST.parse().unwrap());
        if let Some(value) = self.cookie() {
            req.headers
                .insert(header::COOKIE, format!("{NAME}={value}").parse().unwrap());
        }
        req
    }

    async fn send(&self, req: Request) -> Response {
        let res = self.env.app.handle(req).await;
        for value in res.headers().get_all(header::SET_COOKIE) {
            let text = value.to_str().unwrap();
            if let Some(rest) = text.strip_prefix(&format!("{NAME}=")) {
                let mut parts = rest.split("; ");
                let id = parts.next().unwrap().to_owned();
                let expired = parts.any(|p| p == "Max-Age=0");
                *self.jar.lock().unwrap() = (!expired).then_some(id);
            }
        }
        res
    }

    fn cookie(&self) -> Option<String> {
        self.jar.lock().unwrap().clone()
    }

    fn set_cookie(&self, value: Option<String>) {
        *self.jar.lock().unwrap() = value;
    }

    async fn get(&self, path: &str) -> String {
        let res = self.send(self.request(HttpMethod::Get, path)).await;
        String::from_utf8_lossy(res.body()).into_owned()
    }

    async fn status(&self, path: &str) -> u16 {
        self.send(self.request(HttpMethod::Get, path))
            .await
            .status()
            .as_u16()
    }

    async fn post(&self, path: &str, token: Option<&str>) -> Response {
        let mut req = self.request(HttpMethod::Post, path);
        req.headers
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(FORM));
        if let Some(token) = token {
            req.body = format!("_csrf={token}").into_bytes().into();
        }
        self.send(req).await
    }

    async fn log_in(&self) {
        let token = self.get("/token").await;
        let mut req = self.request(HttpMethod::Post, "/login");
        req.headers
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(FORM));
        req.body = format!("username={USER}&password={PASSWORD}&_csrf={token}")
            .into_bytes()
            .into();
        let res = self.send(req).await;
        assert_eq!(res.status(), 302, "{res:?}");
        assert_eq!(res.headers()[header::LOCATION], "/");
    }

    async fn keys(&self) -> serde_json::Value {
        serde_json::from_str(&self.get("/keys").await).unwrap()
    }
}

#[tokio::test]
async fn the_login_writes_the_time_of_the_configured_clock() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        let keys = a.keys().await;
        assert_eq!(keys["revalidated"], env.clock.secs(), "{kind:?} {keys}");
        assert_eq!(keys["stored"]["login"], env.clock.secs(), "{kind:?} {keys}");
    }
}

#[tokio::test]
async fn nothing_happens_before_the_interval() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        env.users.put(USER, &["USER"], false, false);
        env.clock.advance(INTERVAL - Duration::from_secs(1));
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 0, "{kind:?}");
    }
}

#[tokio::test]
async fn a_disabled_a_locked_and_a_deleted_user_end_the_session() {
    for kind in KINDS {
        for change in ["disabled", "locked", "deleted"] {
            let env = Env::new(kind);
            let a = env.browser();
            a.log_in().await;
            let old = a.cookie();
            match change {
                "disabled" => env.users.put(USER, &["USER", "ADMIN"], false, false),
                "locked" => env.users.put(USER, &["USER", "ADMIN"], true, true),
                _ => env.users.delete(USER),
            }
            env.pass_interval();
            assert_eq!(a.get("/whoami").await, "anonymous", "{kind:?} {change}");
            assert_eq!(env.calls(), 1, "{kind:?} {change}");
            // A later request with the cookie the browser now holds is anonymous too.
            assert_eq!(a.get("/whoami").await, "anonymous", "{kind:?} {change}");
            // So is a replay of the cookie from before the end.
            a.set_cookie(old);
            assert_eq!(a.get("/whoami").await, "anonymous", "{kind:?} {change}");
            assert_eq!(a.status("/roles").await, 401, "{kind:?} {change}");
        }
    }
}

#[tokio::test]
async fn changed_roles_are_stored_and_seen_by_the_next_handler() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        assert_eq!(a.status("/admin").await, 200, "{kind:?}");
        env.users.put(USER, &["USER", "AUDITOR"], true, false);
        env.pass_interval();
        assert_eq!(a.status("/admin").await, 403, "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
        // The fresh roles were stored: the next request needs no check to see them.
        assert_eq!(a.get("/roles").await, "ROLE_AUDITOR,ROLE_USER", "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
        let keys = a.keys().await;
        let stored = keys["stored"]["authentication"].to_string();
        assert!(stored.contains("ROLE_AUDITOR"), "{kind:?} {keys}");
        assert!(!stored.contains("ROLE_ADMIN"), "{kind:?} {keys}");
    }
}

#[tokio::test]
async fn an_update_keeps_the_details_and_the_login_time() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        let login = env.clock.secs();
        env.users.put(USER, &["USER"], true, false);
        env.pass_interval();
        assert_eq!(a.get("/roles").await, "ROLE_USER", "{kind:?}");
        let keys = a.keys().await;
        assert_eq!(keys["stored"]["login"], login, "{kind:?} {keys}");
        assert_eq!(
            keys["stored"]["authentication"]["details"]["tenant"], "acme",
            "{kind:?} {keys}"
        );
        assert_eq!(keys["revalidated"], env.clock.secs(), "{kind:?} {keys}");
    }
}

#[tokio::test]
async fn no_change_only_advances_the_time() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        let before = a.keys().await;
        env.pass_interval();
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
        let keys = a.keys().await;
        assert_eq!(keys["revalidated"], env.clock.secs(), "{kind:?} {keys}");
        assert_eq!(keys["stored"], before["stored"], "{kind:?} {keys}");
        // The next check comes only after another interval.
        env.clock.advance(INTERVAL - Duration::from_secs(1));
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
        env.clock.advance(Duration::from_secs(1));
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 2, "{kind:?}");
    }
}

#[tokio::test]
async fn a_service_error_keeps_the_session_and_the_next_request_retries() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        let login = env.clock.secs();
        env.service.fail.store(true, Ordering::SeqCst);
        env.pass_interval();
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
        let keys = a.keys().await;
        assert_eq!(env.calls(), 2, "{kind:?}");
        assert_eq!(keys["revalidated"], login, "{kind:?} {keys}");
        // Back up: the next request checks again and the time advances.
        env.service.fail.store(false, Ordering::SeqCst);
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 3, "{kind:?}");
        assert_eq!(a.keys().await["revalidated"], env.clock.secs(), "{kind:?}");
        assert_eq!(env.calls(), 3, "{kind:?}");
    }
}

#[tokio::test]
async fn a_session_without_the_time_is_checked_once() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        assert_eq!(a.get("/forget").await, "forgotten", "{kind:?}");
        assert_eq!(env.calls(), 0, "{kind:?}");
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
        let keys = a.keys().await;
        assert_eq!(keys["revalidated"], env.clock.secs(), "{kind:?} {keys}");
        assert_eq!(a.get("/whoami").await, USER, "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
    }
}

#[tokio::test]
async fn basic_and_bearer_requests_never_touch_the_service() {
    for kind in KINDS {
        let env = Env::new(kind);
        let a = env.browser();
        a.log_in().await;
        env.users.put(USER, &["USER"], false, false);
        env.pass_interval();
        // Basic as another user, with the session cookie of the disabled user: the header wins.
        let mut req = a.request(HttpMethod::Get, "/whoami");
        let basic = STANDARD.encode(format!("{OTHER}:{PASSWORD}"));
        req.headers.insert(
            header::AUTHORIZATION,
            format!("Basic {basic}").parse().unwrap(),
        );
        let res = a.send(req).await;
        assert_eq!(res.body(), OTHER, "{kind:?}");
        let mut req = a.request(HttpMethod::Get, "/whoami");
        req.headers
            .insert(header::AUTHORIZATION, "Bearer good".parse().unwrap());
        let res = a.send(req).await;
        assert_eq!(res.body(), USER, "{kind:?}");
        assert_eq!(env.calls(), 0, "{kind:?}");
    }
}

#[tokio::test]
async fn the_check_runs_before_the_csrf_check() {
    for kind in KINDS {
        let env = Env::new(kind);
        // An authenticated post still needs the token.
        let a = env.browser();
        a.log_in().await;
        let token = a.get("/token").await;
        assert_eq!(a.post("/change", None).await.status(), 403, "{kind:?}");
        assert_eq!(
            a.post("/change", Some(&token)).await.status(),
            200,
            "{kind:?}"
        );
        // A disabled user's forged post is anonymous: 401, not the 403 of a missing token.
        env.users.put(USER, &["USER", "ADMIN"], false, false);
        env.pass_interval();
        assert_eq!(a.post("/change", None).await.status(), 401, "{kind:?}");
        assert_eq!(env.calls(), 1, "{kind:?}");
    }
}

#[test]
fn build_rejects_a_zero_interval_and_a_missing_form_login() {
    let users = Users::new();
    let service = || {
        Arc::new(Counting {
            users: users.clone(),
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        })
    };
    let err = HttpSecurity::new()
        .password_provider(Arc::new(DaoAuthenticationProvider::new(
            users.clone(),
            Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap()),
        )))
        .form_login(|f| f)
        .revalidate(Duration::ZERO, service())
        .authorize(|a| a.any_request().authenticated())
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::InvalidRevalidateInterval));
    let err = HttpSecurity::new()
        .revalidate(INTERVAL, service())
        .authorize(|a| a.any_request().authenticated())
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::RevalidateWithoutFormLogin));
    let err = HttpSecurity::new()
        .revalidate(Duration::ZERO, service())
        .authorize(|a| a.any_request().authenticated())
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::RevalidateWithoutFormLogin));
    assert_eq!(
        SecurityBuildError::RevalidateWithoutFormLogin.to_string(),
        "revalidation configured without form login"
    );
    assert_eq!(
        SecurityBuildError::InvalidRevalidateInterval.to_string(),
        "revalidation interval must be greater than zero"
    );
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

#[tokio::test]
async fn no_username_or_session_id_reaches_the_logs() {
    let logs = logs();
    for kind in KINDS {
        let env = Env::new(kind);
        let (a, b, c) = (env.browser(), env.browser(), env.browser());
        a.log_in().await;
        b.log_in().await;
        c.log_in().await;
        let cookies = [a.cookie(), b.cookie(), c.cookie()];
        // An error, an update and an end.
        env.service.fail.store(true, Ordering::SeqCst);
        env.pass_interval();
        assert_eq!(a.get("/whoami").await, USER);
        env.service.fail.store(false, Ordering::SeqCst);
        env.users.put(USER, &["USER"], true, false);
        assert_eq!(b.get("/roles").await, "ROLE_USER");
        env.users.put(USER, &["USER"], false, false);
        assert_eq!(c.get("/whoami").await, "anonymous");
        let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(
            text.contains("the user service failed during revalidation"),
            "{kind:?} {text}"
        );
        assert!(!text.contains(USER), "{kind:?} {text}");
        for cookie in cookies.iter().flatten() {
            assert!(!text.contains(cookie.as_str()), "{kind:?} {text}");
        }
    }
}
