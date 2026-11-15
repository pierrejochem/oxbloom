//! The filter that loads the session before the request and writes it back after, and the
//! request extension that hands the session to handlers.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use http::{HeaderValue, StatusCode, header};
use oxbloom_core::{Clock, SystemClock};
use oxbloom_web::{Filter, Next, Request, Response, Scheme};

use super::backend::{CookieAction, SessionBackend};
use super::cookie::{build_set_cookie, parse_cookie_header};
use super::error::SessionError;
use super::handle::Session;
use super::settings::{SecureMode, SessionSettings};

/// The slot of the [`SessionFilter`]: one step before the security chain, so authentication
/// can read the session.
///
/// # Examples
///
/// ```
/// use oxbloom_security::SESSION_ORDER;
///
/// assert_eq!(SESSION_ORDER, oxbloom_web::order::SECURITY - 1);
/// ```
pub const SESSION_ORDER: i32 = oxbloom_web::order::SECURITY - 1;

/// The store housekeeping runs at most this often.
const MAINTAIN_INTERVAL: Duration = Duration::from_secs(60);

/// Gives every request a [`Session`] and writes it back once, after the rest of the chain.
///
/// The flow of one request:
///
/// 1. Every `Cookie` header is marked sensitive, and the session cookie is read from all of
///    them. The first value that decodes is used: for the server backend the first id the store
///    knows, for the cookie backend the first value that opens. A missing, unknown, expired or
///    undecryptable cookie gives a fresh empty session.
/// 2. The session goes into the request extensions, where [`RequestSessionExt::session`] finds
///    it, and the rest of the chain runs.
/// 3. The session is committed, whatever the response status: a handler that wrote to the
///    session and then answered 404 or 500 still gets its cookie. A dirty session, or a new one
///    that holds values, is saved and its cookie set; an invalidated one is deleted and its
///    cookie expired with `Max-Age=0`. An untouched session is written back to extend its idle
///    timeout only when its last touch is more than a minute old. An empty, untouched session
///    is never stored and sets no cookie.
///
///    A session loaded from the store is written back only with the conditional
///    [`SessionStore::update`](crate::SessionStore::update) and moved only with
///    [`SessionStore::rotate`](crate::SessionStore::rotate). When the stored session is gone by
///    then, because a concurrent request logged out or it expired, nothing is stored and the
///    cookie is cleared: a slow request in flight during a logout cannot bring the session
///    back, under the old id or a new one. A session past its absolute timeout is cleared
///    without a write as well.
/// 4. The session `Set-Cookie` is appended, never replacing a `Set-Cookie` of the handler, and
///    every `Set-Cookie` is marked sensitive. `Cache-Control: no-store` is added with it unless
///    the response already has a `Cache-Control` header.
///
/// `Secure` follows [`SecureMode`]: `Auto` sets it when [`Request::scheme`] is HTTPS, directly
/// or as reported by a trusted proxy; `Always` and `Never` override.
///
/// A [`SessionError`] while loading or committing, such as a failed store or a session over
/// the cookie size limit, answers 503 with an empty body and logs a fixed line at `error`
/// without an id, a value or the error text. The filter never starts an empty session in
/// silence instead.
///
/// With the server backend the filter calls [`SessionStore::maintain`](crate::SessionStore::maintain)
/// at most once a minute by its clock, inline on the request that finds it due.
///
/// # Panics
///
/// The filter does not catch panics. A panic in the handler, or in the session code itself
/// (minting an id or a nonce panics when the system random number generator fails), unwinds
/// through the filter without a commit; the app's error handling filter, which runs outside
/// this one, turns it into a 500 without a session cookie.
///
/// `Debug` prints the kind of backend and the settings, never an id, a value or a key.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_core::SystemClock;
/// use oxbloom_security::{
///     InMemorySessionStore, RequestSessionExt, SessionBackend, SessionFilter, SessionSettings,
/// };
/// use oxbloom_web::{App, HttpMethod, Request, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let settings = SessionSettings::default();
/// let store = InMemorySessionStore::new(&settings, Arc::new(SystemClock));
/// let filter = SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings)?;
///
/// let app = App::new()
///     .filter(filter)
///     .route(Route::new(HttpMethod::Get, "/visit", |req: Request| async move {
///         let session = req.session().expect("the session filter ran");
///         let visits = session.get::<u32>("visits").ok().flatten().unwrap_or(0) + 1;
///         session.set("visits", &visits).ok();
///         visits.to_string()
///     }))?;
///
/// let res = app.handle(Request::new(HttpMethod::Get, "/visit")).await;
/// let cookie = res.headers()["set-cookie"].to_str()?;
/// assert!(cookie.starts_with("OXBLOOM_SESSION="));
/// assert!(cookie.ends_with("; Path=/; Max-Age=1800; HttpOnly; SameSite=Lax"));
/// assert_eq!(res.headers()["cache-control"], "no-store");
/// # Ok(())
/// # }
/// ```
pub struct SessionFilter {
    backend: SessionBackend,
    settings: SessionSettings,
    clock: Arc<dyn Clock>,
    last_maintain: Mutex<Option<SystemTime>>,
}

/// A 503 with an empty body.
fn unavailable() -> Response {
    let mut res = Response::new(Default::default());
    *res.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    res
}

/// Appends the session `Set-Cookie`, marks every `Set-Cookie` sensitive, and adds
/// `Cache-Control: no-store` when the response has no `Cache-Control` yet.
fn add_session_cookie(res: &mut Response, mut cookie: HeaderValue) {
    cookie.set_sensitive(true);
    let headers = res.headers_mut();
    headers.append(header::SET_COOKIE, cookie);
    if let header::Entry::Occupied(mut entry) = headers.entry(header::SET_COOKIE) {
        entry.iter_mut().for_each(|v| v.set_sensitive(true));
    }
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
}

impl SessionFilter {
    /// Creates the filter for `backend` with `settings`, reading the system clock.
    ///
    /// This is where the settings are validated, also for a store such as
    /// [`InMemorySessionStore`](crate::InMemorySessionStore) whose constructor does not
    /// validate: no session cookie is ever built from settings that did not pass.
    ///
    /// # Errors
    ///
    /// [`SessionError::InvalidSettings`] when the settings fail [`SessionSettings::validate`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::SystemClock;
    /// use oxbloom_security::{
    ///     InMemorySessionStore, SameSite, SessionBackend, SessionError, SessionFilter,
    ///     SessionSettings,
    /// };
    ///
    /// // `SameSite=None` needs `SecureMode::Always`.
    /// let bad = SessionSettings::default().same_site(SameSite::None);
    /// let store = InMemorySessionStore::new(&bad, Arc::new(SystemClock));
    /// let result = SessionFilter::new(SessionBackend::Server(Arc::new(store)), bad);
    /// assert_eq!(result.unwrap_err(), SessionError::InvalidSettings);
    /// ```
    pub fn new(backend: SessionBackend, settings: SessionSettings) -> Result<Self, SessionError> {
        settings.validate()?;
        Ok(Self {
            backend,
            settings,
            clock: Arc::new(SystemClock),
            last_maintain: Mutex::new(None),
        })
    }

    /// Replaces the clock that decides the touch interval, the cookie lifetime and the
    /// housekeeping interval. Give the store and the cookie backend the same clock.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_core::{Clock, SystemClock};
    /// use oxbloom_security::{InMemorySessionStore, SessionBackend, SessionFilter, SessionSettings};
    ///
    /// let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    /// let settings = SessionSettings::default();
    /// let store = InMemorySessionStore::new(&settings, clock.clone());
    /// let filter = SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings)
    ///     .unwrap()
    ///     .with_clock(clock);
    /// # let _ = filter;
    /// ```
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Runs the store housekeeping when the last run is a minute old or there was none.
    async fn maintain_if_due(&self, now: SystemTime) {
        let due = {
            let mut last = self
                .last_maintain
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let due = last.is_none_or(|at| {
                now.duration_since(at)
                    .is_ok_and(|gap| gap >= MAINTAIN_INTERVAL)
            });
            if due {
                *last = Some(now);
            }
            due
        };
        if due {
            self.backend.maintain().await;
        }
    }

    fn secure(&self, req: &Request) -> bool {
        match self.settings.secure {
            SecureMode::Always => true,
            SecureMode::Never => false,
            SecureMode::Auto => req.scheme() == Scheme::Https,
        }
    }
}

#[async_trait]
impl Filter for SessionFilter {
    async fn filter(&self, mut req: Request, next: Next<'_>) -> Response {
        // Keep the cookies out of `Debug` output of the request from here on. Marking a value
        // sensitive does not change its bytes.
        if let header::Entry::Occupied(mut entry) = req.headers.entry(header::COOKIE) {
            entry.iter_mut().for_each(|v| v.set_sensitive(true));
        }
        let candidates = {
            let values: Vec<&HeaderValue> = req.headers.get_all(header::COOKIE).iter().collect();
            parse_cookie_header(&values, &self.settings.cookie_name)
        };
        let now = self.clock.now();
        self.maintain_if_due(now).await;
        let Ok(state) = self.backend.load(&candidates, now).await else {
            tracing::error!(
                target: "oxbloom::security",
                "the session could not be loaded; answering 503"
            );
            return unavailable();
        };
        let secure = self.secure(&req);
        let session = Session::from_state(state);
        req.extensions_mut().insert(session.clone());

        let mut res = next.run(req).await;

        let state = session.take_state();
        let action = match self
            .backend
            .commit(state, self.clock.now(), &self.settings)
            .await
        {
            Ok(action) => action,
            Err(_) => {
                tracing::error!(
                    target: "oxbloom::security",
                    "the session could not be saved; answering 503"
                );
                return unavailable();
            }
        };
        let cookie = match action {
            CookieAction::None => return res,
            CookieAction::Set { value, max_age } => {
                build_set_cookie(&self.settings, &value, Some(max_age), secure)
            }
            CookieAction::Clear => {
                build_set_cookie(&self.settings, "", Some(Duration::ZERO), secure)
            }
        };
        add_session_cookie(&mut res, cookie);
        res
    }

    fn order(&self) -> i32 {
        SESSION_ORDER
    }
}

impl fmt::Debug for SessionFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionFilter")
            .field("backend", &self.backend)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

/// Reads the [`Session`] of a request.
///
/// # Examples
///
/// ```
/// use oxbloom_security::RequestSessionExt;
/// use oxbloom_web::{HttpMethod, Request};
///
/// // Without a `SessionFilter` there is no session.
/// assert!(Request::new(HttpMethod::Get, "/").session().is_none());
/// ```
pub trait RequestSessionExt {
    /// The session the [`SessionFilter`] stored, or `None` when no session filter ran.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{RequestSessionExt, SessionError};
    /// use oxbloom_web::Request;
    ///
    /// fn user(req: &Request) -> Result<Option<String>, SessionError> {
    ///     match req.session() {
    ///         Some(session) => session.get("user"),
    ///         None => Ok(None),
    ///     }
    /// }
    /// ```
    fn session(&self) -> Option<Session>;
}

impl RequestSessionExt for Request {
    fn session(&self) -> Option<Session> {
        self.extensions().get::<Session>().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_session_cookie_appends_and_keeps_cache_control() {
        let mut res = Response::new(Default::default());
        res.headers_mut()
            .append(header::SET_COOKIE, HeaderValue::from_static("a=b"));
        res.headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("private"));
        add_session_cookie(&mut res, HeaderValue::from_static("S=v"));
        let cookies: Vec<_> = res.headers().get_all(header::SET_COOKIE).iter().collect();
        assert_eq!(cookies, ["a=b", "S=v"]);
        assert!(cookies.iter().all(|v| v.is_sensitive()));
        assert_eq!(res.headers()[header::CACHE_CONTROL], "private");

        let mut res = Response::new(Default::default());
        add_session_cookie(&mut res, HeaderValue::from_static("S=v"));
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    }
}
