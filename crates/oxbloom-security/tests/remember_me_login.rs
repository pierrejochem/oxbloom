//! Issuing and clearing the remember-me cookie at form login and logout, and the checkbox of the
//! default login page, through `App::handle` over both session backends.
//!
//! The CSRF check is on (the default), so every login and logout first fetches the token of its
//! session from `/token`. A small cookie jar keeps the session cookie; the remember-me cookie is
//! read from each response.

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::Clock;
use oxbloom_security::{
    Argon2PasswordEncoder, Authentication, AuthenticationError, AuthenticationProvider,
    AuthenticationRequest, CookieSessionBackend, DaoAuthenticationProvider, HttpSecurity,
    InMemorySessionStore, InMemoryUserDetailsService, PasswordEncoder, RememberMe,
    RequestSecurityExt, RequestSessionExt, SecureMode, SessionBackend, SessionFilter,
    SessionSettings, UserDetails, UserDetailsError, UserDetailsService,
};
use oxbloom_web::{App, ClientInfo, HttpMethod, Request, Response, Route, Scheme};
use tracing_subscriber::fmt::MakeWriter;

const SESSION: &str = "OXBLOOM_SESSION";
const REMEMBER: &str = "OXBLOOM_REMEMBER_ME";
const SESSION_SECRET: &str = "a-session-secret-of-at-least-32-bytes!";
const REMEMBER_SECRET: &str = "a-remember-me-secret-of-32-bytes";
const HOST: &str = "example.com";
const FORM: &str = "application/x-www-form-urlencoded";
const FOURTEEN_DAYS: u64 = 14 * 24 * 3600;
const USER_LOG: &str = "the remember-me cookie was not issued: the user could not be loaded";
const TOKEN_LOG: &str = "the remember-me cookie was not issued: the token would be too long";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Server,
    Cookie,
}

const KINDS: [Kind; 2] = [Kind::Server, Kind::Cookie];

/// A clock that moves only when the test says so.
struct ManualClock(Mutex<SystemTime>);

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

fn epoch_secs(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

/// A username so long that its token would pass 1 KiB.
fn long_name() -> String {
    "l".repeat(800)
}

fn users() -> Arc<InMemoryUserDetailsService> {
    let encoder = Argon2PasswordEncoder::new(8, 1, 1).unwrap();
    Arc::new(
        InMemoryUserDetailsService::new(vec![
            UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
            UserDetails::new(long_name(), encoder.encode("long-pass"), []).roles(&["USER"]),
        ])
        .unwrap(),
    )
}

fn dao(users: Arc<InMemoryUserDetailsService>) -> Arc<DaoAuthenticationProvider> {
    let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1).unwrap());
    Arc::new(DaoAuthenticationProvider::new(users, encoder))
}

/// A user service that always fails.
struct Failing;

#[async_trait]
impl UserDetailsService for Failing {
    async fn load_user_by_username(
        &self,
        _: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        Err(UserDetailsError::new("the user store is down"))
    }
}

/// A password provider that answers every login with an authentication already carrying the
/// remembered marker, which a password login must not store.
struct MarkedProvider;

#[async_trait]
impl AuthenticationProvider for MarkedProvider {
    fn supports(&self, req: &AuthenticationRequest) -> bool {
        matches!(req, AuthenticationRequest::UsernamePassword { .. })
    }

    async fn authenticate(
        &self,
        _: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        Ok(Authentication::new("alice", []).remembered())
    }
}

/// The remember-me settings of most tests: the shared users, the test clock.
fn remember(
    users: Arc<InMemoryUserDetailsService>,
    clock: Arc<ManualClock>,
) -> impl FnOnce(RememberMe) -> RememberMe {
    move |r| {
        r.secret(Secret::new(REMEMBER_SECRET.to_owned()))
            .user_details(users)
            .clock(clock)
    }
}

struct Env {
    app: App,
    clock: Arc<ManualClock>,
}

impl Env {
    /// Form login with the CSRF check on, and remember-me when `remember_me` says so.
    fn new(kind: Kind, remember_me: bool) -> Self {
        Self::build(kind, SessionSettings::default(), move |s, users, clock| {
            if remember_me {
                s.remember_me(remember(users, clock))
            } else {
                s
            }
        })
    }

    fn build(
        kind: Kind,
        settings: SessionSettings,
        configure: impl FnOnce(
            HttpSecurity,
            Arc<InMemoryUserDetailsService>,
            Arc<ManualClock>,
        ) -> HttpSecurity,
    ) -> Self {
        let users = users();
        let clock = clock();
        let backend = match kind {
            Kind::Server => SessionBackend::Server(Arc::new(InMemorySessionStore::new(
                &settings,
                clock.clone(),
            ))),
            Kind::Cookie => SessionBackend::Cookie(Arc::new(
                CookieSessionBackend::new(
                    &Secret::new(SESSION_SECRET.to_owned()),
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
        let security = HttpSecurity::new()
            .password_provider(dao(users.clone()))
            .form_login(|f| f)
            .authorize(|a| {
                a.get("/token")
                    .permit_all()
                    .get("/whoami")
                    .permit_all()
                    .any_request()
                    .authenticated()
            });
        let chain = configure(security, users, clock.clone()).build().unwrap();
        logs();
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
                        .map(|a| format!("{}|{}", a.principal(), a.is_remembered()))
                        .unwrap_or_else(|| "anonymous".to_owned())
                },
            ))
            .unwrap();
        Self { app, clock }
    }

    fn browser(&self) -> Browser<'_> {
        Browser {
            env: self,
            jar: Mutex::new(None),
            https: false,
        }
    }

    fn https_browser(&self) -> Browser<'_> {
        Browser {
            env: self,
            jar: Mutex::new(None),
            https: true,
        }
    }
}

/// One browser with its own session cookie.
struct Browser<'a> {
    env: &'a Env,
    jar: Mutex<Option<String>>,
    https: bool,
}

impl Browser<'_> {
    fn request(&self, method: HttpMethod, path: &str) -> Request {
        let mut req = Request::new(method, path);
        req.headers.insert(header::HOST, HOST.parse().unwrap());
        if self.https {
            req.extensions_mut().insert(ClientInfo {
                addr: "192.0.2.1:40000".parse().unwrap(),
                scheme: Scheme::Https,
                host: None,
            });
        }
        if let Some(value) = self.jar.lock().unwrap().clone() {
            req.headers.insert(
                header::COOKIE,
                format!("{SESSION}={value}").parse().unwrap(),
            );
        }
        req
    }

    async fn send(&self, req: Request) -> Response {
        let res = self.env.app.handle(req).await;
        for value in res.headers().get_all(header::SET_COOKIE) {
            let text = value.to_str().unwrap();
            if let Some(rest) = text.strip_prefix(&format!("{SESSION}=")) {
                let mut parts = rest.split("; ");
                let id = parts.next().unwrap().to_owned();
                let expired = parts.any(|p| p == "Max-Age=0");
                *self.jar.lock().unwrap() = (!expired).then_some(id);
            }
        }
        res
    }

    async fn get(&self, path: &str) -> Response {
        self.send(self.request(HttpMethod::Get, path)).await
    }

    async fn text(&self, path: &str) -> String {
        String::from_utf8_lossy(self.get(path).await.body()).into_owned()
    }

    async fn token(&self) -> String {
        self.text("/token").await
    }

    fn form(&self, path: &str, body: &str) -> Request {
        let mut req = self.request(HttpMethod::Post, path);
        req.headers
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(FORM));
        req.body = body.as_bytes().to_vec().into();
        req
    }

    /// Logs in with the session's token and `extra` appended to the form body.
    async fn login(&self, user: &str, pass: &str, extra: &str) -> Response {
        let token = self.token().await;
        let body = format!("username={user}&password={pass}&_csrf={token}{extra}");
        self.send(self.form("/login", &body)).await
    }

    /// Like [`login`](Self::login), with a remember-me cookie of `value` in the request.
    async fn login_carrying(&self, user: &str, pass: &str, extra: &str, value: &str) -> Response {
        let token = self.token().await;
        let body = format!("username={user}&password={pass}&_csrf={token}{extra}");
        let mut req = self.form("/login", &body);
        let jar = self.jar.lock().unwrap().clone().unwrap();
        req.headers.insert(
            header::COOKIE,
            format!("{SESSION}={jar}; {REMEMBER}={value}")
                .parse()
                .unwrap(),
        );
        self.send(req).await
    }

    async fn logout(&self) -> Response {
        let token = self.token().await;
        self.send(self.form("/logout", &format!("_csrf={token}")))
            .await
    }

    async fn who(&self) -> String {
        self.text("/whoami").await
    }
}

fn set_cookies(res: &Response) -> Vec<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

/// The whole `Set-Cookie` text of the cookie `name`, if the response sets it.
fn cookie_named(res: &Response, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    set_cookies(res)
        .into_iter()
        .find(|c| c.starts_with(&prefix))
}

/// The value of the cookie `name`, if the response sets it.
fn value_of(res: &Response, name: &str) -> Option<String> {
    let text = cookie_named(res, name)?;
    let rest = &text[name.len() + 1..];
    Some(rest.split("; ").next().unwrap().to_owned())
}

fn assert_success(res: &Response) {
    assert_eq!(res.status(), StatusCode::FOUND, "{res:?}");
    assert_eq!(res.headers()[header::LOCATION], "/");
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

// --- issuing ---

#[tokio::test]
async fn a_login_without_the_field_sets_no_cookie() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b.login("alice", "s3cret", "").await;
        assert_success(&res);
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
        assert_eq!(b.who().await, "alice|false");
    }
}

#[tokio::test]
async fn each_accepted_value_issues_the_cookie_with_its_attributes() {
    for kind in KINDS {
        for value in ["on", "true", "YES", "1", "On", "TRUE"] {
            let env = Env::new(kind, true);
            let b = env.browser();
            let res = b
                .login("alice", "s3cret", &format!("&remember-me={value}"))
                .await;
            assert_success(&res);
            let text = cookie_named(&res, REMEMBER).expect("the cookie is set");
            let token = value_of(&res, REMEMBER).unwrap();
            assert_eq!(
                text,
                format!(
                    "{REMEMBER}={token}; Path=/; Max-Age={FOURTEEN_DAYS}; HttpOnly; SameSite=Lax"
                ),
                "{kind:?} {value}"
            );
            let parts: Vec<&str> = token.split('.').collect();
            assert_eq!(parts.len(), 4, "{token}");
            assert_eq!(parts[0], "v1");
            assert_eq!(parts[1], URL_SAFE_NO_PAD.encode("alice"));
            let expires = epoch_secs(env.clock.now()) + FOURTEEN_DAYS;
            assert_eq!(parts[2], expires.to_string());
            assert_eq!(URL_SAFE_NO_PAD.decode(parts[3]).unwrap().len(), 32);
            // The password login stores an authentication without the marker.
            assert_eq!(b.who().await, "alice|false", "{kind:?} {value}");
        }
    }
}

#[tokio::test]
async fn the_lifetime_sets_max_age_and_the_expiry() {
    for kind in KINDS {
        let env = Env::build(kind, SessionSettings::default(), |s, users, clock| {
            s.remember_me(move |r| remember(users, clock)(r).lifetime(Duration::from_secs(3600)))
        });
        let b = env.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        let text = cookie_named(&res, REMEMBER).unwrap();
        assert!(text.contains("; Max-Age=3600;"), "{text}");
        let token = value_of(&res, REMEMBER).unwrap();
        let expires = epoch_secs(env.clock.now()) + 3600;
        assert_eq!(token.split('.').nth(2), Some(expires.to_string().as_str()));
    }
}

#[tokio::test]
async fn other_values_issue_no_cookie() {
    for kind in KINDS {
        for extra in [
            "&remember-me=0",
            "&remember-me=off",
            "&remember-me=",
            "&remember-me",
            "&remember-me=no",
            "&remember-me=onn",
            "&remember-me=%20on",
            "&remember-me=off&remember-me=on",
            "&Remember-Me=on",
        ] {
            let env = Env::new(kind, true);
            let b = env.browser();
            let res = b.login("alice", "s3cret", extra).await;
            assert_success(&res);
            assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?} {extra}");
        }
    }
}

#[tokio::test]
async fn the_first_duplicate_value_wins() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b
            .login("alice", "s3cret", "&remember-me=on&remember-me=off")
            .await;
        assert!(cookie_named(&res, REMEMBER).is_some(), "{kind:?}");
    }
}

#[tokio::test]
async fn a_field_that_pushes_the_body_over_four_kib_fails_the_login() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let pad = format!("&remember-me=on&pad={}", "x".repeat(4096));
        let res = b.login("alice", "s3cret", &pad).await;
        // The CSRF check cannot read the token from such a body either, so it is a 403.
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None);
        assert_eq!(b.who().await, "anonymous");
    }
}

#[tokio::test]
async fn without_remember_me_the_field_does_nothing() {
    for kind in KINDS {
        let env = Env::new(kind, false);
        let b = env.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        assert_success(&res);
        assert!(
            set_cookies(&res).iter().all(|c| c.starts_with(SESSION)),
            "{kind:?} {res:?}"
        );
    }
}

#[tokio::test]
async fn a_failed_login_a_csrf_403_and_an_origin_403_set_no_cookie() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b.login("alice", "wrong", "&remember-me=on").await;
        assert_eq!(res.headers()[header::LOCATION], "/login?error", "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None);

        b.token().await;
        let res = b
            .send(b.form("/login", "username=alice&password=s3cret&remember-me=on"))
            .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None);

        let token = b.token().await;
        let body = format!("username=alice&password=s3cret&remember-me=on&_csrf={token}");
        let mut req = b.form("/login", &body);
        req.headers
            .insert(header::ORIGIN, "http://evil.com".parse().unwrap());
        let res = b.send(req).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None);
        assert_eq!(b.who().await, "anonymous");
    }
}

#[tokio::test]
async fn a_provider_marker_is_not_stored_by_a_password_login() {
    for kind in KINDS {
        let env = Env::build(kind, SessionSettings::default(), |_, users, clock| {
            HttpSecurity::new()
                .password_provider(Arc::new(MarkedProvider))
                .form_login(|f| f)
                .remember_me(remember(users, clock))
                .authorize(|a| a.any_request().permit_all())
        });
        let b = env.browser();
        assert_success(&b.login("alice", "anything", "").await);
        assert_eq!(b.who().await, "alice|false", "{kind:?}");
    }
}

// --- the cookie attributes from the session settings ---

#[tokio::test]
async fn secure_and_path_follow_the_session_settings() {
    for kind in KINDS {
        let always = SessionSettings::default()
            .secure(SecureMode::Always)
            .path("/app");
        let env = Env::build(kind, always, |s, users, clock| {
            s.remember_me(remember(users, clock))
        });
        let b = env.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        let text = cookie_named(&res, REMEMBER).unwrap();
        assert!(
            text.ends_with(&format!(
                "; Path=/app; Max-Age={FOURTEEN_DAYS}; HttpOnly; Secure; SameSite=Lax"
            )),
            "{kind:?} {text}"
        );
        let res = b.logout().await;
        assert_eq!(
            cookie_named(&res, REMEMBER).unwrap(),
            format!("{REMEMBER}=; Path=/app; Max-Age=0; HttpOnly; Secure; SameSite=Lax")
        );

        // `Auto` follows the scheme of the request; `SameSite` stays `Lax` whatever the session uses.
        let auto = SessionSettings::default().same_site(oxbloom_security::SameSite::Strict);
        let env = Env::build(kind, auto, |s, users, clock| {
            s.remember_me(remember(users, clock))
        });
        let plain = env.browser();
        let text = cookie_named(
            &plain.login("alice", "s3cret", "&remember-me=on").await,
            REMEMBER,
        )
        .unwrap();
        assert!(!text.contains("Secure"), "{text}");
        assert!(text.ends_with("; HttpOnly; SameSite=Lax"), "{text}");
        let https = env.https_browser();
        let text = cookie_named(
            &https.login("alice", "s3cret", "&remember-me=on").await,
            REMEMBER,
        )
        .unwrap();
        assert!(text.ends_with("; HttpOnly; Secure; SameSite=Lax"), "{text}");

        let never = SessionSettings::default().secure(SecureMode::Never);
        let env = Env::build(kind, never, |s, users, clock| {
            s.remember_me(remember(users, clock))
        });
        let https = env.https_browser();
        let text = cookie_named(
            &https.login("alice", "s3cret", "&remember-me=on").await,
            REMEMBER,
        )
        .unwrap();
        assert!(!text.contains("Secure"), "{text}");
    }
}

// --- clearing ---

#[tokio::test]
async fn logout_clears_the_cookie() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        b.login("alice", "s3cret", "&remember-me=on").await;
        let res = b.logout().await;
        assert_eq!(res.headers()[header::LOCATION], "/login?logout", "{kind:?}");
        assert_eq!(
            cookie_named(&res, REMEMBER).unwrap(),
            format!("{REMEMBER}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax")
        );
        assert_eq!(b.who().await, "anonymous");

        // Logout clears it even when this login asked for no cookie, as another one may have.
        b.login("alice", "s3cret", "").await;
        assert!(cookie_named(&b.logout().await, REMEMBER).is_some());
    }
}

#[tokio::test]
async fn a_login_without_the_box_clears_a_cookie_it_carries() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b.login_carrying("alice", "s3cret", "", "stale").await;
        assert_success(&res);
        assert_eq!(
            cookie_named(&res, REMEMBER).unwrap(),
            format!("{REMEMBER}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"),
            "{kind:?}"
        );
        assert_eq!(b.who().await, "alice|false");
    }
}

#[tokio::test]
async fn a_login_without_the_box_and_without_a_cookie_sets_no_cookie_of_that_name() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b.login("alice", "s3cret", "").await;
        assert_success(&res);
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
    }
}

#[tokio::test]
async fn a_failed_login_with_a_cookie_clears_nothing_and_a_ticked_one_issues_a_fresh_cookie() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b.login_carrying("alice", "wrong", "", "stale").await;
        assert_eq!(res.headers()[header::LOCATION], "/login?error", "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");

        let res = b
            .login_carrying("alice", "s3cret", "&remember-me=on", "stale")
            .await;
        assert_success(&res);
        let fresh = value_of(&res, REMEMBER).unwrap();
        assert_ne!(fresh, "", "{kind:?}");
        assert!(fresh.starts_with("v1."), "{fresh}");
    }
}

#[tokio::test]
async fn a_rejected_logout_and_a_chain_without_remember_me_do_not_clear() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        b.login("alice", "s3cret", "&remember-me=on").await;
        let res = b.send(b.form("/logout", "")).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None);

        let env = Env::new(kind, false);
        let b = env.browser();
        b.login("alice", "s3cret", "").await;
        let res = b.logout().await;
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
    }
}

// --- custom names ---

#[tokio::test]
async fn a_custom_field_name_and_cookie_name_are_used() {
    for kind in KINDS {
        let env = Env::build(kind, SessionSettings::default(), |s, users, clock| {
            s.remember_me(move |r| {
                remember(users, clock)(r)
                    .field_name("stay")
                    .cookie_name("REMEMBER")
            })
        });
        let b = env.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        assert_eq!(cookie_named(&res, "REMEMBER"), None, "{kind:?}");
        assert_eq!(cookie_named(&res, REMEMBER), None);

        let res = b.login("alice", "s3cret", "&stay=yes").await;
        let text = cookie_named(&res, "REMEMBER").expect("the custom cookie");
        assert!(text.ends_with("; HttpOnly; SameSite=Lax"), "{text}");
        assert_eq!(cookie_named(&res, REMEMBER), None);

        let res = b.logout().await;
        assert_eq!(
            cookie_named(&res, "REMEMBER").unwrap(),
            "REMEMBER=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"
        );
    }
}

// --- the login page ---

#[tokio::test]
async fn the_default_page_shows_the_checkbox_only_with_remember_me() {
    for kind in KINDS {
        let checkbox = "<input type=\"checkbox\" name=\"remember-me\" value=\"on\">";
        let env = Env::new(kind, true);
        let res = env.browser().get("/login").await;
        assert_eq!(res.status(), StatusCode::OK, "{kind:?}");
        let page = String::from_utf8_lossy(res.body()).into_owned();
        assert!(page.contains(checkbox), "{page}");
        let form = &page[page.find("<form").unwrap()..page.find("</form>").unwrap()];
        assert!(form.contains(checkbox));
        assert!(page.contains("Remember me"));
        assert_eq!(
            res.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'"
        );

        let env = Env::new(kind, false);
        let page = env.browser().text("/login").await;
        assert!(!page.contains("checkbox"), "{page}");
        assert!(!page.contains("Remember me"));

        let env = Env::build(kind, SessionSettings::default(), |s, users, clock| {
            s.remember_me(move |r| remember(users, clock)(r).field_name("stay_signed-in"))
        });
        let page = env.browser().text("/login").await;
        assert!(
            page.contains("<input type=\"checkbox\" name=\"stay_signed-in\" value=\"on\">"),
            "{page}"
        );
    }
}

// --- failures and logs ---

#[tokio::test]
async fn a_user_service_failure_still_logs_in_without_a_cookie() {
    for kind in KINDS {
        let env = Env::build(kind, SessionSettings::default(), |s, _, clock| {
            s.remember_me(move |r| {
                r.secret(Secret::new(REMEMBER_SECRET.to_owned()))
                    .user_details(Arc::new(Failing))
                    .clock(clock)
            })
        });
        let b = env.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        assert_success(&res);
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
        assert_eq!(b.who().await, "alice|false");
        assert!(log_text().contains(USER_LOG));

        // A service that does not know the user is treated the same way.
        let env = Env::build(kind, SessionSettings::default(), |s, _, clock| {
            let empty = Arc::new(InMemoryUserDetailsService::new(vec![]).unwrap());
            s.remember_me(move |r| {
                r.secret(Secret::new(REMEMBER_SECRET.to_owned()))
                    .user_details(empty)
                    .clock(clock)
            })
        });
        let b = env.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        assert_success(&res);
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
        assert_eq!(b.who().await, "alice|false");
    }
    let text = log_text();
    assert!(!text.contains("store is down"), "{text}");
}

#[tokio::test]
async fn a_token_over_one_kib_is_not_issued() {
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b.login(&long_name(), "long-pass", "&remember-me=on").await;
        assert_success(&res);
        assert_eq!(cookie_named(&res, REMEMBER), None, "{kind:?}");
        assert!(b.who().await.ends_with("|false"));
    }
    let text = log_text();
    assert!(text.contains(TOKEN_LOG), "{text}");
    assert!(!text.contains(&long_name()));
}

#[tokio::test]
async fn the_cookie_value_and_the_username_reach_no_log_line() {
    let mut values = Vec::new();
    for kind in KINDS {
        let env = Env::new(kind, true);
        let b = env.browser();
        let res = b.login("alice", "s3cret", "&remember-me=on").await;
        let value = value_of(&res, REMEMBER).unwrap();
        assert!(!format!("{res:?}").contains(&value), "{kind:?}");
        values.push(value);
        b.logout().await;
    }
    let text = log_text();
    for value in &values {
        assert!(!text.contains(value.as_str()), "{text}");
        let mac = value.rsplit('.').next().unwrap();
        assert!(!text.contains(mac), "{text}");
    }
    assert!(!text.contains(&URL_SAFE_NO_PAD.encode("alice")), "{text}");
    assert!(!text.contains("\"alice\""), "{text}");
    assert!(!text.contains("username=alice"), "{text}");
}
