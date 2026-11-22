//! The per-user session limit and the principal key written by form login, through
//! `App::handle` with the in-memory store and a manual clock.

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use http::{HeaderValue, header};
use oxbloom_core::Clock;
use oxbloom_security::{
    Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity, InMemorySessionStore,
    InMemoryUserDetailsService, PasswordEncoder, PrincipalSessions, RequestSecurityExt,
    RequestSessionExt, SecurityBuildError, SessionBackend, SessionError, SessionFilter,
    SessionSettings, SessionStore, SessionSummary, UserDetails,
};
use oxbloom_web::{App, HttpMethod, Request, Response, Route};
use tracing_subscriber::fmt::MakeWriter;

const NAME: &str = "OXBLOOM_SESSION";
const HOST: &str = "example.com";
const FORM: &str = "application/x-www-form-urlencoded";
const LIMIT_LOG: &str = "the session limit could not be applied";

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
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

fn dao() -> DaoAuthenticationProvider {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    let users = InMemoryUserDetailsService::new(vec![
        UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
        UserDetails::new("bob", encoder.encode("hunter2"), []).roles(&["USER"]),
    ])
    .unwrap();
    DaoAuthenticationProvider::new(Arc::new(users), encoder)
}

fn security() -> HttpSecurity {
    HttpSecurity::new()
        .password_provider(Arc::new(dao()))
        .authorize(|a| {
            a.get("/token")
                .permit_all()
                .get("/whoami")
                .permit_all()
                .get("/keys")
                .permit_all()
                .any_request()
                .authenticated()
        })
}

struct Env {
    app: App,
    clock: Arc<ManualClock>,
    index: Arc<dyn PrincipalSessions>,
}

impl Env {
    fn new(
        configure: impl FnOnce(HttpSecurity, Arc<dyn PrincipalSessions>) -> HttpSecurity,
    ) -> Self {
        let clock = ManualClock::new();
        let settings = SessionSettings::default()
            .idle_timeout(Duration::from_secs(86_400))
            .absolute_timeout(Duration::from_secs(864_000));
        let store = Arc::new(InMemorySessionStore::new(&settings, clock.clone()));
        let index = store.clone().principal_sessions().unwrap();
        let filter = SessionFilter::new(SessionBackend::Server(store), settings)
            .unwrap()
            .with_clock(clock.clone());
        let chain = configure(security().form_login(|f| f), index.clone())
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
                |req: Request| async move {
                    req.authentication()
                        .map(|a| a.principal().to_string())
                        .unwrap_or_else(|| "anonymous".to_owned())
                },
            ))
            .unwrap()
            .route(Route::new(
                HttpMethod::Get,
                "/keys",
                |req: Request| async move {
                    let session = req.session().unwrap();
                    let principal = session
                        .get::<serde_json::Value>("oxbloom.security.principal")
                        .unwrap();
                    let revalidated = session
                        .get::<serde_json::Value>("oxbloom.security.revalidated-at")
                        .unwrap();
                    let stored = session
                        .get::<serde_json::Value>("oxbloom.security.authentication")
                        .unwrap();
                    let login = stored.and_then(|v| v.get("login").cloned());
                    format!("{}|{}|{}", json(principal), json(revalidated), json(login))
                },
            ))
            .unwrap();
        Self { app, clock, index }
    }

    fn browser(&self) -> Browser<'_> {
        Browser {
            env: self,
            jar: Mutex::new(None),
        }
    }
}

fn json(value: Option<serde_json::Value>) -> String {
    value.map_or_else(|| "none".to_owned(), |v| v.to_string())
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
        if let Some(value) = self.jar.lock().unwrap().clone() {
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

    async fn get(&self, path: &str) -> String {
        let res = self.send(self.request(HttpMethod::Get, path)).await;
        String::from_utf8_lossy(res.body()).into_owned()
    }

    /// Just over the touch interval of a minute later, so a request writes its touch back.
    fn tick(&self) {
        self.env.clock.advance(Duration::from_secs(61));
    }

    fn cookie(&self) -> String {
        self.jar.lock().unwrap().clone().unwrap_or_default()
    }

    async fn login_with(
        &self,
        user: &str,
        pass: &str,
        token: Option<&str>,
        origin: Option<&str>,
    ) -> Response {
        let mut text = format!("username={user}&password={pass}");
        if let Some(token) = token {
            text.push_str(&format!("&_csrf={token}"));
        }
        let mut req = self.request(HttpMethod::Post, "/login");
        req.headers
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(FORM));
        if let Some(origin) = origin {
            req.headers
                .insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        req.body = text.into_bytes().into();
        self.tick();
        self.send(req).await
    }

    /// Fetches a token and logs in; the answer must be the redirect of a success.
    async fn log_in(&self, user: &str, pass: &str) {
        let token = self.get("/token").await;
        let res = self.login_with(user, pass, Some(&token), None).await;
        assert_eq!(res.status(), 302, "{res:?}");
        assert_eq!(res.headers()[header::LOCATION], "/");
    }

    async fn who(&self) -> String {
        self.tick();
        self.get("/whoami").await
    }
}

fn limited(max: usize) -> Env {
    Env::new(|s, index| s.session_limit(index, max))
}

#[tokio::test]
async fn the_third_login_evicts_the_least_recently_used_session() {
    let env = limited(2);
    let (a, b, c) = (env.browser(), env.browser(), env.browser());
    a.log_in("alice", "s3cret").await;
    b.log_in("alice", "s3cret").await;
    assert_eq!(a.who().await, "alice");
    assert_eq!(b.who().await, "alice");
    c.log_in("alice", "s3cret").await;
    assert_eq!(a.who().await, "anonymous");
    assert_eq!(b.who().await, "alice");
    assert_eq!(c.who().await, "alice");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_logins_all_succeed_and_a_later_login_trims_the_extra_sessions() {
    const RACERS: usize = 8;
    let env = Arc::new(limited(2));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..RACERS {
        let env = env.clone();
        tasks.spawn(async move {
            let browser = env.browser();
            browser.log_in("alice", "s3cret").await;
            browser.cookie()
        });
    }
    let mut cookies = Vec::new();
    while let Some(done) = tasks.join_next().await {
        cookies.push(done.unwrap());
    }
    // Every login succeeded (`log_in` asserts the redirect) and got its own session.
    assert_eq!(cookies.len(), RACERS);
    assert!(cookies.iter().all(|c| !c.is_empty()));
    // At most limit + racers - 1 sessions.
    let after_race = env.index.sessions_of("alice").await.unwrap().len();
    assert!(after_race < 2 + RACERS, "{after_race} sessions");
    // One more login at a quiet moment leaves at most the limit.
    env.browser().log_in("alice", "s3cret").await;
    let after = env.index.sessions_of("alice").await.unwrap().len();
    assert!(after <= 2, "{after} sessions");
}

#[tokio::test]
async fn a_request_makes_its_session_the_newest() {
    let env = limited(2);
    let (a, b, c) = (env.browser(), env.browser(), env.browser());
    a.log_in("alice", "s3cret").await;
    b.log_in("alice", "s3cret").await;
    // A was the oldest; a request touches it, so B is now the oldest.
    assert_eq!(a.who().await, "alice");
    c.log_in("alice", "s3cret").await;
    assert_eq!(a.who().await, "alice");
    assert_eq!(b.who().await, "anonymous");
    assert_eq!(c.who().await, "alice");
}

#[tokio::test]
async fn another_users_sessions_are_untouched() {
    let env = limited(1);
    let (a, b, c) = (env.browser(), env.browser(), env.browser());
    b.log_in("bob", "hunter2").await;
    a.log_in("alice", "s3cret").await;
    c.log_in("alice", "s3cret").await;
    assert_eq!(a.who().await, "anonymous");
    assert_eq!(b.who().await, "bob");
    assert_eq!(c.who().await, "alice");
}

#[tokio::test]
async fn a_relogin_does_not_count_the_session_it_replaces() {
    let env = limited(2);
    let (a, b) = (env.browser(), env.browser());
    b.log_in("alice", "s3cret").await;
    a.log_in("alice", "s3cret").await;
    // B is the oldest. A logs in again from its authenticated session: that session is being
    // replaced, so only B is "another" session and the limit is not reached.
    a.log_in("alice", "s3cret").await;
    assert_eq!(a.who().await, "alice");
    assert_eq!(b.who().await, "alice");
}

#[tokio::test]
async fn a_failed_login_evicts_nothing() {
    let env = limited(2);
    let (a, b, c) = (env.browser(), env.browser(), env.browser());
    a.log_in("alice", "s3cret").await;
    b.log_in("alice", "s3cret").await;

    let token = c.get("/token").await;
    let wrong = c.login_with("alice", "nope", Some(&token), None).await;
    assert_eq!(wrong.status(), 302);
    assert_ne!(wrong.headers()[header::LOCATION], "/");
    let missing = c.login_with("alice", "s3cret", None, None).await;
    assert_eq!(missing.status(), 403);
    let foreign = c
        .login_with(
            "alice",
            "s3cret",
            Some(&token),
            Some("https://evil.example"),
        )
        .await;
    assert_eq!(foreign.status(), 403);

    assert_eq!(a.who().await, "alice");
    assert_eq!(b.who().await, "alice");
    assert_eq!(c.who().await, "anonymous");
}

#[tokio::test]
async fn without_a_limit_nothing_is_evicted() {
    let env = Env::new(|s, _| s);
    let browsers: Vec<_> = (0..4).map(|_| env.browser()).collect();
    for browser in &browsers {
        browser.log_in("alice", "s3cret").await;
    }
    for browser in &browsers {
        assert_eq!(browser.who().await, "alice");
    }
}

#[tokio::test]
async fn the_principal_key_is_written_without_a_limit() {
    let env = Env::new(|s, _| s);
    let a = env.browser();
    a.log_in("alice", "s3cret").await;
    let keys = a.get("/keys").await;
    let parts: Vec<&str> = keys.split('|').collect();
    assert_eq!(parts[0], "\"alice\"", "{keys}");
    // The revalidation time is the login time, from the same clock call.
    assert_ne!(parts[1], "none", "{keys}");
    assert_eq!(parts[1], parts[2], "{keys}");
}

/// An index whose eviction always fails.
struct Failing;

#[async_trait]
impl PrincipalSessions for Failing {
    async fn sessions_of(&self, _: &str) -> Result<Vec<SessionSummary>, SessionError> {
        Ok(Vec::new())
    }

    async fn delete_of(&self, _: &str, _: Option<&str>) -> Result<usize, SessionError> {
        Ok(0)
    }

    async fn evict_oldest(
        &self,
        _: &str,
        _: usize,
        _: Option<&str>,
    ) -> Result<usize, SessionError> {
        Err(SessionError::Store)
    }
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
async fn an_index_error_still_logs_in_and_logs_a_fixed_text() {
    let logs = logs();
    let env = Env::new(|s, _| s.session_limit(Arc::new(Failing), 2));
    let a = env.browser();
    a.log_in("alice", "s3cret").await;
    assert_eq!(a.who().await, "alice");

    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains(LIMIT_LOG), "{text}");
    assert!(!text.contains("alice"), "{text}");
    assert!(!text.contains("s3cret"), "{text}");
    assert!(!text.contains(&a.cookie()), "{text}");
}

#[tokio::test]
async fn no_session_id_or_username_reaches_the_logs_on_the_normal_path() {
    let logs = logs();
    let env = limited(1);
    let (a, b) = (env.browser(), env.browser());
    a.log_in("alice", "s3cret").await;
    b.log_in("alice", "s3cret").await;
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for secret in [a.cookie(), b.cookie()] {
        assert!(!text.contains(&secret), "{text}");
    }
    assert!(!text.contains("\"alice\""), "{text}");
    assert!(!text.contains("username=alice"), "{text}");
}

#[test]
fn build_rejects_a_zero_limit_and_a_limit_without_form_login() {
    let store = Arc::new(InMemorySessionStore::new(
        &SessionSettings::default(),
        ManualClock::new(),
    ));
    let index = store.principal_sessions().unwrap();
    let err = security()
        .form_login(|f| f)
        .session_limit(index.clone(), 0)
        .build()
        .err();
    assert_eq!(err, Some(SecurityBuildError::InvalidSessionLimit));
    let err = security().session_limit(index.clone(), 2).build().err();
    assert_eq!(err, Some(SecurityBuildError::SessionLimitWithoutFormLogin));
    let err = security().session_limit(index, 0).build().err();
    assert_eq!(err, Some(SecurityBuildError::SessionLimitWithoutFormLogin));
    assert_eq!(
        SecurityBuildError::SessionLimitWithoutFormLogin.to_string(),
        "session limit configured without form login"
    );
    assert_eq!(
        SecurityBuildError::InvalidSessionLimit.to_string(),
        "session limit must be at least one"
    );
}
