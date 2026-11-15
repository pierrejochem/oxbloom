//! Behaviour of the session filter through `App::handle`.
//!
//! The behavioural tests run against both backends through one shared function per test
//! (`for kind in KINDS`). A small cookie jar plays the browser: it keeps the session cookie the
//! responses set and drops it on `Max-Age=0`. Time is driven by a manual clock that the filter,
//! the store and the cookie backend share.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_security::{
    CookieSessionBackend, InMemorySessionStore, RequestSessionExt, SESSION_ORDER, SameSite,
    SecureMode, SessionBackend, SessionData, SessionError, SessionFilter, SessionSettings,
    SessionStore,
};
use oxbloom_web::{
    App, ClientInfo, Filter, ForwardStrategy, ForwardedConfig, HttpMethod, Request, Response,
    Route, Scheme, TrustedProxies,
};
use tracing_subscriber::fmt::MakeWriter;

const NAME: &str = "OXBLOOM_SESSION";
const SECRET: &str = "a-session-secret-of-at-least-32-bytes!";
const HIDDEN: &str = "hidden-session-value";

/// A clock that moves only when the test says so.
struct ManualClock(Mutex<SystemTime>);

impl ManualClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
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

/// Wraps the in-memory store, counts the calls, and fails on demand.
struct CountingStore {
    inner: InMemorySessionStore,
    saves: AtomicUsize,
    maintains: AtomicUsize,
    fail_load: bool,
    fail_save: bool,
}

impl CountingStore {
    fn new(settings: &SessionSettings, clock: Arc<ManualClock>) -> Self {
        Self {
            inner: InMemorySessionStore::new(settings, clock),
            saves: AtomicUsize::new(0),
            maintains: AtomicUsize::new(0),
            fail_load: false,
            fail_save: false,
        }
    }

    fn saves(&self) -> usize {
        self.saves.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SessionStore for CountingStore {
    async fn load(&self, id: &str) -> Result<Option<SessionData>, SessionError> {
        if self.fail_load {
            return Err(SessionError::Store);
        }
        self.inner.load(id).await
    }

    async fn save(&self, id: &str, data: &SessionData, ttl: Duration) -> Result<(), SessionError> {
        if self.fail_save {
            return Err(SessionError::Store);
        }
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.inner.save(id, data, ttl).await
    }

    async fn delete(&self, id: &str) -> Result<(), SessionError> {
        self.inner.delete(id).await
    }

    async fn update(
        &self,
        id: &str,
        data: &SessionData,
        ttl: Duration,
    ) -> Result<bool, SessionError> {
        if self.fail_save {
            return Err(SessionError::Store);
        }
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.inner.update(id, data, ttl).await
    }

    async fn rotate(&self, old: &str, new: &str) -> Result<bool, SessionError> {
        self.inner.rotate(old, new).await
    }

    async fn maintain(&self) {
        self.maintains.fetch_add(1, Ordering::SeqCst);
        self.inner.maintain().await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Server,
    Cookie,
}

const KINDS: [Kind; 2] = [Kind::Server, Kind::Cookie];

/// One application with the session filter and a few routes, plus what the tests inspect.
struct Harness {
    app: App,
    clock: Arc<ManualClock>,
    store: Option<Arc<CountingStore>>,
    jar: Mutex<Option<String>>,
}

fn backend_for(
    kind: Kind,
    settings: &SessionSettings,
    clock: &Arc<ManualClock>,
) -> (SessionBackend, Option<Arc<CountingStore>>) {
    match kind {
        Kind::Server => {
            let store = Arc::new(CountingStore::new(settings, clock.clone()));
            (SessionBackend::Server(store.clone()), Some(store))
        }
        Kind::Cookie => {
            let backend = CookieSessionBackend::new(
                &Secret::new(SECRET.to_owned()),
                &[],
                settings,
                clock.clone(),
            )
            .unwrap();
            (SessionBackend::Cookie(Arc::new(backend)), None)
        }
    }
}

fn routes(app: App) -> App {
    app.route(Route::new(
        HttpMethod::Get,
        "/none",
        |_req: Request| async { "untouched" },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/read",
        |req: Request| async move {
            let session = req.session().expect("the filter ran");
            session.get::<String>("v").unwrap().unwrap_or_default()
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/write/{v}",
        |req: Request| async move {
            let session = req.session().expect("the filter ran");
            let v = req.param("v").unwrap().to_owned();
            session.set("v", &v).unwrap();
            v
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/write-hidden",
        |req: Request| async move {
            req.session().unwrap().set("v", HIDDEN).unwrap();
            "written"
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/invalidate",
        |req: Request| async move {
            req.session().unwrap().invalidate();
            "bye"
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/rotate",
        |req: Request| async move {
            req.session().unwrap().rotate_id();
            "rotated"
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/invalidate-then-set",
        |req: Request| async move {
            let session = req.session().unwrap();
            session.invalidate();
            session.set("fresh", &1u32).unwrap();
            "restarted"
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/restart-rotate",
        |req: Request| async move {
            let session = req.session().unwrap();
            session.invalidate();
            session.set("fresh", &1u32).unwrap();
            session.rotate_id();
            "restarted and rotated"
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/rotate-then-invalidate",
        |req: Request| async move {
            let session = req.session().unwrap();
            session.rotate_id();
            session.invalidate();
            "gone"
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/fresh",
        |req: Request| async move {
            req.session()
                .unwrap()
                .get::<u32>("fresh")
                .unwrap()
                .map(|n| n.to_string())
                .unwrap_or_default()
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/cached",
        |req: Request| async move {
            req.session().unwrap().set("v", "x").unwrap();
            let mut res = Response::new("cached".into());
            res.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("max-age=60"),
            );
            res
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/own-cookie",
        |req: Request| async move {
            req.session().unwrap().set("v", "x").unwrap();
            let mut res = Response::new("own".into());
            res.headers_mut()
                .append(header::SET_COOKIE, HeaderValue::from_static("a=b"));
            res
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/own-cookie-only",
        |_req: Request| async move {
            let mut res = Response::new("own".into());
            res.headers_mut()
                .append(header::SET_COOKIE, HeaderValue::from_static("a=b"));
            res
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/fail",
        |req: Request| async move {
            req.session().unwrap().set("v", "failed").unwrap();
            StatusCode::INTERNAL_SERVER_ERROR
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/missing",
        |req: Request| async move {
            req.session().unwrap().set("v", "missing").unwrap();
            StatusCode::NOT_FOUND
        },
    ))
    .unwrap()
    .route(Route::new(
        HttpMethod::Get,
        "/panic",
        |req: Request| async move {
            req.session().unwrap().set("v", "panicked").unwrap();
            if req.path.starts_with('/') {
                panic!("the handler panicked");
            }
            "unreachable"
        },
    ))
    .unwrap()
}

impl Harness {
    fn new(kind: Kind) -> Self {
        Self::with_settings(kind, SessionSettings::default())
    }

    fn with_settings(kind: Kind, settings: SessionSettings) -> Self {
        let clock = ManualClock::new();
        let (backend, store) = backend_for(kind, &settings, &clock);
        let filter = SessionFilter::new(backend, settings)
            .unwrap()
            .with_clock(clock.clone());
        Self {
            app: routes(App::new().filter(filter)),
            clock,
            store,
            jar: Mutex::new(None),
        }
    }

    fn with_store(store: Arc<CountingStore>, clock: Arc<ManualClock>) -> Self {
        let filter = SessionFilter::new(
            SessionBackend::Server(store.clone()),
            SessionSettings::default(),
        )
        .unwrap()
        .with_clock(clock.clone());
        Self {
            app: routes(App::new().filter(filter)),
            clock,
            store: Some(store),
            jar: Mutex::new(None),
        }
    }

    /// Sends a GET with the jar's cookie, if any, and stores what the response sets.
    async fn get(&self, path: &str) -> Response {
        let mut req = Request::new(HttpMethod::Get, path);
        if let Some(value) = self.jar.lock().unwrap().clone() {
            req.headers
                .insert(header::COOKIE, format!("{NAME}={value}").parse().unwrap());
        }
        self.send(req).await
    }

    /// Sends `req` as it is and updates the jar from the response.
    async fn send(&self, req: Request) -> Response {
        let res = self.app.handle(req).await;
        if let Some((value, max_age)) = session_cookie(&res) {
            *self.jar.lock().unwrap() = if max_age == Some(0) {
                None
            } else {
                Some(value)
            };
        }
        res
    }

    fn cookie(&self) -> Option<String> {
        self.jar.lock().unwrap().clone()
    }

    fn set_cookie(&self, value: Option<String>) {
        *self.jar.lock().unwrap() = value;
    }
}

/// All `Set-Cookie` values of a response, as text.
fn set_cookies(res: &Response) -> Vec<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

/// The session cookie of a response: its value and its `Max-Age`.
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

fn body(res: &Response) -> String {
    String::from_utf8(res.body().to_vec()).unwrap()
}

// --- shared behaviour, run against both backends ---

#[tokio::test]
async fn no_cookie_and_no_use_sets_no_cookie() {
    for kind in KINDS {
        let h = Harness::new(kind);
        for path in ["/none", "/read"] {
            let res = h.get(path).await;
            assert_eq!(res.status(), 200, "{kind:?} {path}");
            assert!(set_cookies(&res).is_empty(), "{kind:?} {path}");
            assert!(res.headers().get(header::CACHE_CONTROL).is_none());
        }
        if let Some(store) = &h.store {
            assert_eq!(store.saves(), 0);
            assert!(store.inner.is_empty());
        }
    }
}

#[tokio::test]
async fn write_sets_the_cookie_once_with_exact_attributes() {
    for kind in KINDS {
        let h = Harness::new(kind);
        let res = h.get("/write/hello").await;
        assert_eq!(res.status(), 200);
        let cookies = set_cookies(&res);
        assert_eq!(cookies.len(), 1, "{kind:?}: {} cookies", cookies.len());
        let value = h.cookie().expect("a session cookie");
        assert!(!value.is_empty());
        assert_eq!(
            cookies[0],
            format!("{NAME}={value}; Path=/; Max-Age=1800; HttpOnly; SameSite=Lax"),
            "{kind:?}"
        );
        assert!(
            res.headers()[header::SET_COOKIE].is_sensitive(),
            "{kind:?}: Set-Cookie is sensitive"
        );
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        if kind == Kind::Server {
            assert_eq!(value.len(), 43, "a 256-bit id in base64url");
        }
    }
}

#[tokio::test]
async fn second_request_with_the_cookie_sees_the_data() {
    for kind in KINDS {
        let h = Harness::new(kind);
        h.get("/write/hello").await;
        h.clock.advance(Duration::from_secs(5));
        let res = h.get("/read").await;
        assert_eq!(body(&res), "hello", "{kind:?}");
        assert!(
            set_cookies(&res).is_empty(),
            "{kind:?}: a read within a minute writes nothing"
        );
    }
}

#[tokio::test]
async fn invalidate_sends_max_age_zero_and_next_request_is_empty() {
    for kind in KINDS {
        let h = Harness::new(kind);
        h.get("/write/hello").await;
        let old = h.cookie().unwrap();
        let res = h.get("/invalidate").await;
        let cookies = set_cookies(&res);
        assert_eq!(
            cookies,
            [format!(
                "{NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"
            )],
            "{kind:?}"
        );
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(h.cookie(), None, "the jar dropped the cookie");

        let res = h.get("/read").await;
        assert_eq!(body(&res), "", "{kind:?}");
        assert!(set_cookies(&res).is_empty());

        if kind == Kind::Server {
            // The stored session is gone, so even the old cookie sees nothing.
            h.set_cookie(Some(old));
            assert_eq!(body(&h.get("/read").await), "");
            assert!(h.store.as_ref().unwrap().inner.is_empty());
        }
    }
}

#[tokio::test]
async fn cache_control_no_store_is_added_with_the_cookie_and_does_not_override() {
    for kind in KINDS {
        let h = Harness::new(kind);
        let res = h.get("/cached").await;
        assert!(session_cookie(&res).is_some(), "{kind:?}");
        let values: Vec<_> = res
            .headers()
            .get_all(header::CACHE_CONTROL)
            .iter()
            .collect();
        assert_eq!(values, ["max-age=60"], "{kind:?}");

        // Without a session cookie, no header is added.
        let res = h.get("/own-cookie-only").await;
        assert!(
            res.headers().get(header::CACHE_CONTROL).is_none(),
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn existing_set_cookie_headers_are_kept() {
    for kind in KINDS {
        let h = Harness::new(kind);
        let res = h.get("/own-cookie").await;
        let cookies = set_cookies(&res);
        assert_eq!(cookies.len(), 2, "{kind:?}");
        assert_eq!(cookies[0], "a=b");
        assert!(cookies[1].starts_with(&format!("{NAME}=")));
        assert!(
            res.headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .all(HeaderValue::is_sensitive)
        );

        let res = h.get("/own-cookie-only").await;
        assert_eq!(set_cookies(&res), ["a=b"], "{kind:?}");
    }
}

fn secure_flag(res: &Response) -> bool {
    set_cookies(res)
        .iter()
        .any(|c| c.starts_with(NAME) && c.split("; ").any(|p| p == "Secure"))
}

fn https_request(path: &str) -> Request {
    let mut req = Request::new(HttpMethod::Get, path);
    req.extensions_mut().insert(ClientInfo {
        addr: "192.0.2.1:40000".parse().unwrap(),
        scheme: Scheme::Https,
        host: None,
    });
    req
}

#[tokio::test]
async fn secure_follows_the_request() {
    for kind in KINDS {
        let h = Harness::new(kind);
        assert!(
            !secure_flag(&h.get("/write/a").await),
            "{kind:?}: plain HTTP"
        );
        assert!(
            secure_flag(&h.send(https_request("/write/b")).await),
            "{kind:?}: direct HTTPS"
        );

        // HTTPS reported by a trusted proxy.
        let clock = ManualClock::new();
        let (backend, _store) = backend_for(kind, &SessionSettings::default(), &clock);
        let filter = SessionFilter::new(backend, SessionSettings::default())
            .unwrap()
            .with_clock(clock);
        let app = routes(App::new().filter(filter)).forwarded(ForwardedConfig {
            strategy: ForwardStrategy::Framework,
            trusted: TrustedProxies::loopback(),
        });
        let mut req = Request::new(HttpMethod::Get, "/write/c");
        req.headers
            .insert("x-forwarded-proto", HeaderValue::from_static("https"));
        let res = app.handle_from(req, "127.0.0.1:50000".parse().ok()).await;
        assert!(secure_flag(&res), "{kind:?}: forwarded HTTPS");

        let always =
            Harness::with_settings(kind, SessionSettings::default().secure(SecureMode::Always));
        assert!(
            secure_flag(&always.get("/write/d").await),
            "{kind:?}: always"
        );

        let never =
            Harness::with_settings(kind, SessionSettings::default().secure(SecureMode::Never));
        assert!(
            !secure_flag(&never.send(https_request("/write/e")).await),
            "{kind:?}: never"
        );
    }
}

#[tokio::test]
async fn several_cookie_headers_and_the_name_twice() {
    for kind in KINDS {
        let h = Harness::new(kind);
        h.get("/write/first").await;
        let first = h.cookie().unwrap();
        h.set_cookie(None);
        h.get("/write/second").await;
        let second = h.cookie().unwrap();

        // A garbage value, then the first session; the second session in a later header.
        let mut req = Request::new(HttpMethod::Get, "/read");
        req.headers.append(
            header::COOKIE,
            format!("{NAME}=garbage; other=1; {NAME}={first}")
                .parse()
                .unwrap(),
        );
        req.headers
            .append(header::COOKIE, format!("{NAME}={second}").parse().unwrap());
        let res = h.app.handle(req).await;
        assert_eq!(
            body(&res),
            "first",
            "{kind:?}: the first value that decodes wins"
        );

        // A header with 10 000 cookies is over the length limit: a fresh session.
        let many: String = (0..10_000).map(|i| format!("c{i}=v; ")).collect();
        let mut req = Request::new(HttpMethod::Get, "/read");
        req.headers.append(
            header::COOKIE,
            format!("{many}{NAME}={first}").parse().unwrap(),
        );
        let res = h.app.handle(req).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(&res), "", "{kind:?}");

        // An over-length header holding only the session cookie: a fresh session.
        let mut req = Request::new(HttpMethod::Get, "/read");
        req.headers.append(
            header::COOKIE,
            format!("{NAME}={first}; pad={}", "x".repeat(9000))
                .parse()
                .unwrap(),
        );
        let res = h.app.handle(req).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(&res), "", "{kind:?}");
    }
}

#[tokio::test]
async fn invalidate_then_set_and_rotate_then_invalidate() {
    for kind in KINDS {
        let h = Harness::new(kind);
        h.get("/write/old").await;
        let old = h.cookie().unwrap();

        // Invalidate, then set: a new session that holds only the new value.
        let res = h.get("/invalidate-then-set").await;
        let (new, max_age) = session_cookie(&res).expect("a new cookie");
        assert_ne!(new, old, "{kind:?}");
        assert_eq!(max_age, Some(1800));
        assert_eq!(body(&h.get("/fresh").await), "1", "{kind:?}");
        assert_eq!(
            body(&h.get("/read").await),
            "",
            "{kind:?}: the old value is gone"
        );
        if kind == Kind::Server {
            h.set_cookie(Some(old.clone()));
            assert_eq!(body(&h.get("/read").await), "", "the old id was deleted");
            h.set_cookie(Some(new));
        }

        // Rotate, then invalidate: the cookie is cleared and the stored session deleted.
        let res = h.get("/rotate-then-invalidate").await;
        assert_eq!(session_cookie(&res).unwrap().1, Some(0), "{kind:?}");
        assert_eq!(h.cookie(), None);
        if let Some(store) = &h.store {
            assert!(store.inner.is_empty(), "every stored session was deleted");
        }
    }
}

#[tokio::test]
async fn error_and_404_responses_still_commit_the_session() {
    for kind in KINDS {
        let h = Harness::new(kind);
        let res = h.get("/fail").await;
        assert_eq!(res.status(), 500);
        assert!(session_cookie(&res).is_some(), "{kind:?}: 500 commits");
        assert_eq!(body(&h.get("/read").await), "failed", "{kind:?}");

        h.set_cookie(None);
        let res = h.get("/missing").await;
        assert_eq!(res.status(), 404);
        assert!(session_cookie(&res).is_some(), "{kind:?}: 404 commits");
        assert_eq!(body(&h.get("/read").await), "missing", "{kind:?}");
    }
}

#[tokio::test]
async fn a_panicking_handler_does_not_commit() {
    for kind in KINDS {
        let h = Harness::new(kind);
        // The app's error handling filter, outside the session filter, turns the panic into
        // a 500; the session filter never reaches its commit.
        let res = tokio::time::timeout(Duration::from_secs(5), h.get("/panic"))
            .await
            .expect("no hang");
        assert_eq!(res.status(), 500, "{kind:?}");
        assert!(session_cookie(&res).is_none(), "{kind:?}");
        if let Some(store) = &h.store {
            assert_eq!(store.saves(), 0);
        }
    }
}

#[tokio::test]
async fn untouched_session_does_not_write_within_a_minute() {
    for kind in KINDS {
        let h = Harness::new(kind);
        h.get("/write/v").await;
        let saves = || h.store.as_ref().map(|s| s.saves());
        let after_write = saves();

        // Two reads 10 s apart: no write.
        for _ in 0..2 {
            h.clock.advance(Duration::from_secs(10));
            let res = h.get("/read").await;
            assert_eq!(body(&res), "v");
            assert!(session_cookie(&res).is_none(), "{kind:?}");
        }
        assert_eq!(saves(), after_write, "{kind:?}");

        // Two reads 90 s apart: the second extends the idle timeout, once.
        h.clock.advance(Duration::from_secs(40));
        let res = h.get("/read").await;
        assert!(
            session_cookie(&res).is_none(),
            "{kind:?}: the last touch is 60 s old"
        );
        h.clock.advance(Duration::from_secs(90));
        let res = h.get("/read").await;
        assert_eq!(body(&res), "v");
        let (_, max_age) = session_cookie(&res).expect("a refreshed cookie");
        assert_eq!(max_age, Some(1800));
        if let Some(store) = &h.store {
            assert_eq!(store.saves(), after_write.unwrap() + 1, "one write");
        }

        // The refresh moved the idle deadline: 25 + 25 minutes later the session lives.
        h.clock.advance(Duration::from_secs(25 * 60));
        assert_eq!(body(&h.get("/read").await), "v", "{kind:?}");
        h.clock.advance(Duration::from_secs(25 * 60));
        assert_eq!(body(&h.get("/read").await), "v", "{kind:?}");
    }
}

// --- server backend only ---

#[tokio::test]
async fn unknown_client_supplied_id_is_not_adopted() {
    let h = Harness::new(Kind::Server);
    let planted = "A".repeat(43);
    h.set_cookie(Some(planted.clone()));
    let res = h.get("/write/x").await;
    let (value, _) = session_cookie(&res).expect("a minted cookie");
    assert_ne!(value, planted);
    let store = h.store.as_ref().unwrap();
    assert!(store.inner.load(&planted).await.unwrap().is_none());
    assert!(store.inner.load(&value).await.unwrap().is_some());

    // Without a write, a planted id sets nothing at all.
    h.set_cookie(Some(planted));
    assert!(session_cookie(&h.get("/read").await).is_none());
}

#[tokio::test]
async fn rotate_id_changes_the_cookie_and_old_id_stops_working() {
    let h = Harness::new(Kind::Server);
    h.get("/write/kept").await;
    let old = h.cookie().unwrap();
    let res = h.get("/rotate").await;
    let (new, max_age) = session_cookie(&res).expect("a new cookie");
    assert_ne!(new, old);
    assert_eq!(max_age, Some(1800));
    assert_eq!(body(&h.get("/read").await), "kept");

    h.set_cookie(Some(old.clone()));
    assert_eq!(body(&h.get("/read").await), "");
    let store = h.store.as_ref().unwrap();
    assert!(store.inner.load(&old).await.unwrap().is_none());
    assert_eq!(store.inner.len(), 1);
}

#[tokio::test]
async fn maintain_runs_at_most_once_a_minute() {
    let clock = ManualClock::new();
    let store = Arc::new(CountingStore::new(
        &SessionSettings::default(),
        clock.clone(),
    ));
    let h = Harness::with_store(store.clone(), clock);
    h.get("/none").await;
    h.get("/none").await;
    assert_eq!(store.maintains.load(Ordering::SeqCst), 1);
    h.clock.advance(Duration::from_secs(30));
    h.get("/none").await;
    assert_eq!(store.maintains.load(Ordering::SeqCst), 1);
    h.clock.advance(Duration::from_secs(31));
    h.get("/none").await;
    assert_eq!(store.maintains.load(Ordering::SeqCst), 2);
}

/// Holds a request inside its handler until the test releases it.
#[derive(Default)]
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

/// What the held request does with its session once released.
#[derive(Clone, Copy, Debug)]
enum Late {
    Write,
    Read,
    Rotate,
    RestartRotate,
}

/// A server-backend harness with a `/held/{write|read|rotate|restart-rotate}` route that waits
/// at `gate`.
fn race_harness() -> (Harness, Arc<Gate>) {
    let clock = ManualClock::new();
    let store = Arc::new(CountingStore::new(
        &SessionSettings::default(),
        clock.clone(),
    ));
    let mut h = Harness::with_store(store, clock);
    let gate = Arc::new(Gate::default());
    for (path, late) in [
        ("/held/write", Late::Write),
        ("/held/read", Late::Read),
        ("/held/rotate", Late::Rotate),
        ("/held/restart-rotate", Late::RestartRotate),
    ] {
        let gate = gate.clone();
        h.app = h
            .app
            .route(Route::new(HttpMethod::Get, path, move |req: Request| {
                let gate = gate.clone();
                async move {
                    let session = req.session().unwrap();
                    gate.entered.notify_one();
                    gate.release.notified().await;
                    match late {
                        Late::Write => session.set("v", "late").unwrap(),
                        Late::Read => drop(session.get::<String>("v")),
                        Late::Rotate => session.rotate_id(),
                        Late::RestartRotate => {
                            session.invalidate();
                            session.set("fresh", &1u32).unwrap();
                            session.rotate_id();
                        }
                    }
                    "late"
                }
            }))
            .unwrap();
    }
    (h, gate)
}

/// Runs request A on `path` with cookie `s`, holds it in its handler while request B on the
/// same cookie logs out, then lets A finish. Returns A's response.
async fn logout_during(h: &Harness, gate: &Gate, path: &str, s: &str) -> Response {
    let cookie = format!("{NAME}={s}");
    let mut a = Request::new(HttpMethod::Get, path);
    a.headers.insert(header::COOKIE, cookie.parse().unwrap());
    let mut b = Request::new(HttpMethod::Get, "/invalidate");
    b.headers.insert(header::COOKIE, cookie.parse().unwrap());
    let logout = async {
        gate.entered.notified().await;
        let res = h.app.handle(b).await;
        assert_eq!(session_cookie(&res).unwrap().1, Some(0), "B logged out");
        gate.release.notify_one();
    };
    let (res, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(h.app.handle(a), logout)
    })
    .await
    .expect("no hang");
    res
}

// The race tests apply to the server backend only: the cookie backend keeps no server state,
// and its documented limit is that a copy of an old cookie stays valid until it expires.

#[tokio::test]
async fn a_late_commit_does_not_resurrect_a_logged_out_session() {
    for (path, wait) in [("/held/write", 0), ("/held/read", 90)] {
        let (h, gate) = race_harness();
        h.get("/write/before").await;
        let s = h.cookie().unwrap();
        // For the plain read, make its 60 s touch due so that it would write.
        h.clock.advance(Duration::from_secs(wait));

        let res = logout_during(&h, &gate, path, &s).await;
        let store = h.store.as_ref().unwrap();
        assert!(store.inner.is_empty(), "{path}: S stays deleted");
        assert!(store.inner.load(&s).await.unwrap().is_none(), "{path}");
        assert_eq!(
            session_cookie(&res),
            None,
            "{path}: A leaves the cookie alone instead of setting S or clearing a newer one"
        );

        h.set_cookie(Some(s));
        assert_eq!(body(&h.get("/read").await), "", "{path}: S is empty");
        assert!(store.inner.is_empty(), "{path}");
    }
}

#[tokio::test]
async fn a_late_rotate_does_not_store_data_under_a_new_id() {
    let (h, gate) = race_harness();
    h.get("/write/before").await;
    let s = h.cookie().unwrap();

    let res = logout_during(&h, &gate, "/held/rotate", &s).await;
    let store = h.store.as_ref().unwrap();
    assert!(store.inner.is_empty(), "nothing is stored under any id");
    assert_eq!(
        session_cookie(&res),
        None,
        "a lost rotate leaves the cookie alone"
    );
}

#[tokio::test]
async fn a_restarted_then_rotated_session_survives_a_concurrent_logout() {
    let (h, gate) = race_harness();
    h.get("/write/before").await;
    let s = h.cookie().unwrap();

    // A invalidates S, starts a new session, and rotates it, while B deletes S first.
    let res = logout_during(&h, &gate, "/held/restart-rotate", &s).await;
    let (new, max_age) = session_cookie(&res).expect("the restarted session sets its cookie");
    assert_ne!(new, s);
    assert_eq!(max_age, Some(1800));
    let store = h.store.as_ref().unwrap();
    assert!(
        store.inner.load(&s).await.unwrap().is_none(),
        "S stays deleted"
    );
    let saved = store
        .inner
        .load(&new)
        .await
        .unwrap()
        .expect("the new session was saved");
    assert_eq!(saved.values().get("fresh"), Some(&serde_json::json!(1)));
    assert!(saved.values().get("v").is_none(), "nothing of S came back");
    assert_eq!(store.inner.len(), 1);

    h.set_cookie(Some(new));
    assert_eq!(body(&h.get("/fresh").await), "1");
}

#[tokio::test]
async fn a_restarted_then_rotated_session_deletes_the_old_id_and_saves_the_new_one() {
    let h = Harness::new(Kind::Server);
    h.get("/write/old").await;
    let old = h.cookie().unwrap();
    let res = h.get("/restart-rotate").await;
    let (new, _) = session_cookie(&res).expect("a new cookie");
    assert_ne!(new, old);
    let store = h.store.as_ref().unwrap();
    assert!(
        store.inner.load(&old).await.unwrap().is_none(),
        "the old id was deleted"
    );
    assert_eq!(store.inner.len(), 1);
    assert_eq!(body(&h.get("/fresh").await), "1");
    assert_eq!(body(&h.get("/read").await), "", "the old value is gone");
}

// --- the absolute timeout with a store that knows only TTLs ---

/// A store that ignores the timestamps inside the session and keeps a session only as long as
/// the TTL it was given, like a plain Redis `SET .. EX`. With `honour_ttl` off it never
/// expires anything, like the `MapStore` in the trait documentation. It records the latest
/// expiry any `save` or `update` asked for.
struct TtlOnlyStore {
    clock: Arc<ManualClock>,
    honour_ttl: bool,
    map: Mutex<std::collections::HashMap<String, (SessionData, SystemTime)>>,
    latest_expiry: Mutex<Option<SystemTime>>,
}

impl TtlOnlyStore {
    fn new(clock: Arc<ManualClock>, honour_ttl: bool) -> Arc<Self> {
        Arc::new(Self {
            clock,
            honour_ttl,
            map: Mutex::default(),
            latest_expiry: Mutex::default(),
        })
    }

    fn live(&self, expires: SystemTime) -> bool {
        !self.honour_ttl || self.clock.now() < expires
    }

    fn record(&self, ttl: Duration) -> SystemTime {
        let expires = self.clock.now() + ttl;
        let mut latest = self.latest_expiry.lock().unwrap();
        *latest = Some(latest.map_or(expires, |l| l.max(expires)));
        expires
    }
}

#[async_trait]
impl SessionStore for TtlOnlyStore {
    async fn load(&self, id: &str) -> Result<Option<SessionData>, SessionError> {
        let map = self.map.lock().unwrap();
        Ok(map
            .get(id)
            .filter(|(_, expires)| self.live(*expires))
            .map(|(data, _)| data.clone()))
    }

    async fn save(&self, id: &str, data: &SessionData, ttl: Duration) -> Result<(), SessionError> {
        let expires = self.record(ttl);
        self.map
            .lock()
            .unwrap()
            .insert(id.to_owned(), (data.clone(), expires));
        Ok(())
    }

    async fn update(
        &self,
        id: &str,
        data: &SessionData,
        ttl: Duration,
    ) -> Result<bool, SessionError> {
        let expires = self.record(ttl);
        let mut map = self.map.lock().unwrap();
        match map.get_mut(id) {
            Some(entry) if self.live(entry.1) => {
                *entry = (data.clone(), expires);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn delete(&self, id: &str) -> Result<(), SessionError> {
        self.map.lock().unwrap().remove(id);
        Ok(())
    }

    async fn rotate(&self, old: &str, new: &str) -> Result<bool, SessionError> {
        let mut map = self.map.lock().unwrap();
        match map.remove(old) {
            Some(entry) if self.live(entry.1) => {
                map.insert(new.to_owned(), entry);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// A harness over `store` with a 100 s idle timeout and a 1000 s absolute timeout.
fn ttl_harness(store: Arc<TtlOnlyStore>) -> Harness {
    let settings = SessionSettings::default()
        .idle_timeout(Duration::from_secs(100))
        .absolute_timeout(Duration::from_secs(1000));
    let clock = store.clock.clone();
    let filter = SessionFilter::new(SessionBackend::Server(store), settings)
        .unwrap()
        .with_clock(clock.clone());
    Harness {
        app: routes(App::new().filter(filter)),
        clock,
        store: None,
        jar: Mutex::new(None),
    }
}

/// Keeps a session busy with a read every 90 s, so that each read refreshes it, until shortly
/// before its absolute deadline at 1000 s. Returns the cookie and the creation time.
async fn keep_busy_until_the_deadline(h: &Harness) -> (String, SystemTime) {
    let created = h.clock.now();
    h.get("/write/v").await;
    let s = h.cookie().unwrap();
    for _ in 0..11 {
        h.clock.advance(Duration::from_secs(90));
        assert_eq!(
            body(&h.get("/read").await),
            "v",
            "alive before the deadline"
        );
    }
    // Now 990 s after creation, refreshed by the last read.
    assert_eq!(h.clock.now(), created + Duration::from_secs(990));
    (s, created)
}

#[tokio::test]
async fn a_ttl_only_store_is_never_given_a_ttl_past_the_absolute_deadline() {
    let clock = ManualClock::new();
    let store = TtlOnlyStore::new(clock, true);
    let h = ttl_harness(store.clone());
    let (s, created) = keep_busy_until_the_deadline(&h).await;
    let deadline = created + Duration::from_secs(1000);
    let latest = store
        .latest_expiry
        .lock()
        .unwrap()
        .expect("the store was written");
    assert!(
        latest <= deadline,
        "the TTL given to the store ran {:?} past the absolute deadline",
        latest.duration_since(deadline).unwrap_or_default()
    );

    // Past the deadline the session is gone, also when the client replays the id.
    h.clock.advance(Duration::from_secs(20));
    h.set_cookie(Some(s));
    assert_eq!(
        body(&h.get("/read").await),
        "",
        "not served past the deadline"
    );
}

#[tokio::test]
async fn a_session_past_its_absolute_deadline_is_not_served_from_a_store_without_expiry() {
    let clock = ManualClock::new();
    let store = TtlOnlyStore::new(clock, false);
    let h = ttl_harness(store.clone());
    let (s, _) = keep_busy_until_the_deadline(&h).await;

    h.clock.advance(Duration::from_secs(20));
    h.set_cookie(Some(s.clone()));
    let res = h.get("/read").await;
    assert_eq!(
        body(&res),
        "",
        "a stored session past the deadline counts as unknown"
    );
    assert!(
        session_cookie(&res).is_none(),
        "nothing to set for an empty session"
    );

    // A write starts a new session under a new id instead of reviving the old one.
    h.set_cookie(Some(s.clone()));
    let res = h.get("/write/new").await;
    let (new, _) = session_cookie(&res).expect("a new session");
    assert_ne!(new, s);
}

// --- store failures ---

/// Captures log output of the current thread while the guard lives.
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

impl LogBuffer {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

fn failing(
    fail_load: bool,
    fail_save: bool,
) -> (Harness, LogBuffer, tracing::subscriber::DefaultGuard) {
    let clock = ManualClock::new();
    let mut store = CountingStore::new(&SessionSettings::default(), clock.clone());
    store.fail_load = fail_load;
    store.fail_save = fail_save;
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (Harness::with_store(Arc::new(store), clock), logs, guard)
}

#[tokio::test]
async fn store_failure_answers_503() {
    let (h, _logs, _guard) = failing(true, false);
    h.set_cookie(Some("B".repeat(43)));
    let res = h.get("/read").await;
    assert_eq!(res.status(), 503, "a failed load");
    assert!(session_cookie(&res).is_none());

    let (h, _logs, _guard) = failing(false, true);
    let res = h.get("/write/x").await;
    assert_eq!(res.status(), 503, "a failed save");
    assert!(session_cookie(&res).is_none());

    // A request that leaves the session alone never touches the failing save.
    let res = h.get("/none").await;
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn the_503_body_and_logs_do_not_leak_the_store_error() {
    for (fail_load, fail_save) in [(true, false), (false, true)] {
        let (h, logs, _guard) = failing(fail_load, fail_save);
        let planted = "C".repeat(43);
        h.set_cookie(Some(planted.clone()));
        let res = h.get("/write-hidden").await;
        assert_eq!(res.status(), 503);
        let text = format!("{}\n{}", body(&res), logs.text());
        let store_error = SessionError::Store.to_string();
        assert!(
            !text.contains(&store_error),
            "the store error leaked: {text}"
        );
        assert!(!text.contains(&planted), "the cookie leaked: {text}");
        assert!(!text.contains(HIDDEN), "a value leaked: {text}");
        assert!(
            logs.text().contains("ERROR"),
            "a fixed error line is logged: {}",
            logs.text()
        );
    }
}

// --- construction, Debug and the extension ---

#[test]
fn settings_validation_is_applied_by_the_constructor() {
    let clock = ManualClock::new();
    let bad = [
        SessionSettings::default().cookie_name("bad name"),
        SessionSettings::default().same_site(SameSite::None),
        SessionSettings::default().path("no-slash"),
        SessionSettings::default().idle_timeout(Duration::ZERO),
    ];
    for settings in bad {
        // `InMemorySessionStore::new` does not validate; the filter does.
        let store = InMemorySessionStore::new(&settings, clock.clone());
        let result = SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings);
        assert_eq!(result.unwrap_err(), SessionError::InvalidSettings);
    }
    let store = InMemorySessionStore::new(&SessionSettings::default(), clock);
    let filter = SessionFilter::new(
        SessionBackend::Server(Arc::new(store)),
        SessionSettings::default(),
    )
    .unwrap();
    assert_eq!(filter.order(), SESSION_ORDER);
    assert_eq!(SESSION_ORDER, oxbloom_web::order::SECURITY - 1);
}

#[test]
fn debug_of_filter_and_backend_shows_no_secret() {
    let clock = ManualClock::new();
    for kind in KINDS {
        let (backend, _) = backend_for(kind, &SessionSettings::default(), &clock);
        let backend_text = format!("{backend:?}");
        let filter = SessionFilter::new(backend, SessionSettings::default()).unwrap();
        let filter_text = format!("{filter:?}");
        for text in [&backend_text, &filter_text] {
            assert!(!text.contains(SECRET), "{text}");
            assert!(!text.contains("a-session-secret"), "{text}");
        }
        assert!(filter_text.starts_with("SessionFilter"), "{filter_text}");
    }
}

#[tokio::test]
async fn request_ext_is_none_without_the_filter() {
    let app = App::new()
        .route(Route::new(
            HttpMethod::Get,
            "/",
            |req: Request| async move {
                if req.session().is_none() {
                    "none"
                } else {
                    "some"
                }
            },
        ))
        .unwrap();
    let res = app.handle(Request::new(HttpMethod::Get, "/")).await;
    assert_eq!(body(&res), "none");
    assert!(Request::new(HttpMethod::Get, "/").session().is_none());
}

#[tokio::test]
async fn the_cookie_request_header_is_marked_sensitive() {
    let clock = ManualClock::new();
    let (backend, _) = backend_for(Kind::Server, &SessionSettings::default(), &clock);
    let filter = SessionFilter::new(backend, SessionSettings::default()).unwrap();
    let app = App::new()
        .filter(filter)
        .route(Route::new(
            HttpMethod::Get,
            "/",
            |req: Request| async move {
                let all = req
                    .headers
                    .get_all(header::COOKIE)
                    .iter()
                    .all(HeaderValue::is_sensitive);
                if all { "sensitive" } else { "plain" }
            },
        ))
        .unwrap();
    let mut req = Request::new(HttpMethod::Get, "/");
    req.headers
        .append(header::COOKIE, HeaderValue::from_static("a=1"));
    req.headers
        .append(header::COOKIE, HeaderValue::from_static("b=2"));
    assert_eq!(body(&app.handle(req).await), "sensitive");
}
