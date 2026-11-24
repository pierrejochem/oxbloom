//! The form login handlers the chain runs: login, logout, the login page fallback, the restore of
//! the authentication from the session and the content-negotiated entry point.
//!
//! No username, password, session id, saved path or remember-me cookie reaches a log line. A
//! failed login logs at `debug` the error variant and the length of the username only.

use std::sync::Arc;

use http::{HeaderValue, StatusCode, header};
use oxbloom_core::{Clock, HttpMethod};
use oxbloom_web::{Request, Response};

use super::form::{LoginForm, parse_login_form};
use super::origin::same_origin;
use super::page::login_page;
use super::redirect::path_of;
use super::repository::{
    OAUTH2_TOKENS_PREFIX, load_authentication, save_request, store_authentication, store_principal,
    take_saved_request,
};
use super::revalidate::{Revalidated, Revalidation, revalidate};
use super::settings::FormLogin;
use crate::csrf::{ensure_token, replace_token, request_passes};
use crate::dao::authentication_of;
use crate::remember_me::{
    AutoLogin, Checked, RememberMeRuntime, check_token, clear_cookie, invalid_cookie, issue_cookie,
    no_session_error as no_remember_me_session, remember_me_token, sets_cookie,
};
use crate::session::CookieScope;
use crate::{
    Authentication, AuthenticationEntryPoint, AuthenticationError, AuthenticationRequest,
    CsrfConfig, PrincipalSessions, ProviderManager, RequestSessionExt, Session,
};

/// The log marker of a form that could not be read.
#[derive(Debug)]
struct MalformedForm;

pub(crate) const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// The JSON the token endpoint returns: the header name, the field name and the token, in that
/// order of keys, every value escaped by `serde_json`. The session and the token are created
/// when missing. The answer must never be cached.
pub(crate) fn token_endpoint_response(session: &Session, cfg: &CsrfConfig) -> Response {
    let token = ensure_token(session);
    let json = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned());
    let body = format!(
        "{{\"headerName\":{},\"parameterName\":{},\"token\":{}}}",
        json(&cfg.header_name),
        json(&cfg.field_name),
        json(token.as_str())
    );
    let mut res = Response::new(body.into());
    *res.status_mut() = StatusCode::OK;
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// Logs that form login found no session, which means the application has no session filter.
fn no_session_error() {
    tracing::error!(
        target: "oxbloom::security",
        "form login has no session: add a SessionFilter to the application"
    );
}

fn status(status: StatusCode) -> Response {
    let mut res = Response::new(Default::default());
    *res.status_mut() = status;
    res
}

/// A 302 to `target`. A target that is not a valid header value goes to `fallback`, which the
/// settings validation guarantees is one.
fn redirect(target: &str, fallback: &str) -> Response {
    let value = HeaderValue::from_str(target)
        .or_else(|_| HeaderValue::from_str(fallback))
        .unwrap_or_else(|_| HeaderValue::from_static("/"));
    let mut res = status(StatusCode::FOUND);
    res.headers_mut().insert(header::LOCATION, value);
    res
}

/// Whether the request body is declared as `application/x-www-form-urlencoded`, parameters such
/// as `charset` allowed.
pub(crate) fn is_form(req: &Request) -> bool {
    req.headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case(FORM_CONTENT_TYPE))
}

/// The quality of one `Accept` range: `q` when given, 1 otherwise, `None` when `q` is not a
/// number in `0..=1`.
fn quality(params: &str) -> Option<f32> {
    let mut q = 1.0;
    for param in params.split(';') {
        if let Some((name, value)) = param.split_once('=')
            && name.trim().eq_ignore_ascii_case("q")
        {
            q = value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|q| (0.0..=1.0).contains(q))?;
        }
    }
    Some(q)
}

/// Whether `Accept` prefers HTML. Of the ranges `text/html`, `application/xhtml+xml`,
/// `application/json`, `*/*` and `text/*`, the first with the highest quality is the preferred
/// one. An HTML type preferred is a browser. A wildcard preferred is a browser only when
/// `text/html` or `application/xhtml+xml` is listed with a quality above zero and at least that
/// of `application/json`. `*/*` alone, a JSON preference and a missing header are not.
pub(crate) fn prefers_html(req: &Request) -> bool {
    let mut best: Option<(&str, f32)> = None;
    let (mut html, mut json) = (0.0_f32, 0.0_f32);
    for value in req.headers.get_all(header::ACCEPT) {
        let Ok(text) = value.to_str() else {
            return false;
        };
        for range in text.split(',') {
            let (media, params) = range.split_once(';').unwrap_or((range, ""));
            let media = media.trim();
            let kind = match media.to_ascii_lowercase().as_str() {
                "text/html" | "application/xhtml+xml" => "html",
                "application/json" => "json",
                "*/*" | "text/*" => "wildcard",
                _ => continue,
            };
            let Some(q) = quality(params) else {
                continue;
            };
            match kind {
                "html" => html = html.max(q),
                "json" => json = json.max(q),
                _ => {}
            }
            if best.is_none_or(|(_, b)| q > b) {
                best = Some((kind, q));
            }
        }
    }
    match best {
        Some((_, q)) if q <= 0.0 => false,
        Some(("html", _)) => true,
        Some(("wildcard", _)) => html > 0.0 && html >= json,
        _ => false,
    }
}

/// The form login part of a chain: the settings, the CSRF settings when the check is on, and
/// the handlers that use them.
pub(crate) struct FormLoginHandlers {
    settings: Arc<FormLogin>,
    csrf: Option<CsrfConfig>,
    limit: Option<(Arc<dyn PrincipalSessions>, usize)>,
    revalidation: Option<Revalidation>,
    remember_me: Option<Arc<RememberMeRuntime>>,
    clock: Arc<dyn Clock>,
}

impl FormLoginHandlers {
    /// `csrf` is `Some` only when the CSRF check is on. `limit` is the per-user index and the
    /// positive maximum of sessions per user, when a limit is configured. `revalidation` is
    /// `Some` when revalidation is on. `remember_me` is `Some` when remember-me is on. `clock`
    /// gives the login time and the revalidation time.
    pub(crate) fn new(
        settings: Arc<FormLogin>,
        csrf: Option<CsrfConfig>,
        limit: Option<(Arc<dyn PrincipalSessions>, usize)>,
        revalidation: Option<Revalidation>,
        remember_me: Option<Arc<RememberMeRuntime>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            settings,
            csrf,
            limit,
            revalidation,
            remember_me,
            clock,
        }
    }

    /// The form login settings: the OAuth2 login shares the default success URL.
    #[cfg(feature = "oauth2")]
    pub(crate) fn settings(&self) -> &FormLogin {
        &self.settings
    }

    /// The `Set-Cookie` that clears the remember-me cookie of `req`, when remember-me is on and
    /// the request carries one. A login that issues no cookie, such as an OAuth2 login, sends it
    /// so a cookie left from an earlier user cannot log that user in again.
    #[cfg(feature = "oauth2")]
    pub(crate) fn stale_remember_me_cookie(&self, req: &Request) -> Option<HeaderValue> {
        let runtime = self.remember_me.as_ref()?;
        remember_me_token(runtime, req)?;
        Some(clear_cookie(&runtime.settings, &CookieScope::of(req)))
    }

    /// The CSRF settings, when the check is on.
    pub(crate) fn csrf(&self) -> Option<&CsrfConfig> {
        self.csrf.as_ref()
    }

    /// Whether the request passes the CSRF check: always when the check is off or the method
    /// is safe.
    pub(crate) fn csrf_passes(&self, req: &Request) -> bool {
        request_passes(req, self.csrf.as_ref())
    }

    fn is_login_post(&self, req: &Request) -> bool {
        req.method == HttpMethod::Post && req.path == path_of(&self.settings.login_processing_url)
    }

    fn is_logout_post(&self, req: &Request) -> bool {
        req.method == HttpMethod::Post && req.path == path_of(&self.settings.logout_url)
    }

    /// Answers `POST` to the processing URL and to the logout URL, or `None` for any other
    /// request.
    pub(crate) async fn handle_post(
        &self,
        req: &Request,
        manager: Option<&ProviderManager>,
    ) -> Option<Response> {
        if self.is_login_post(req) {
            Some(self.login(req, manager).await)
        } else if self.is_logout_post(req) {
            Some(self.logout(req))
        } else {
            None
        }
    }

    /// For a `GET` or `HEAD` of the login page, whether `?error` and `?logout` are present;
    /// `None` for any other request.
    pub(crate) fn login_page_request(&self, req: &Request) -> Option<(bool, bool)> {
        let get = matches!(req.method, HttpMethod::Get | HttpMethod::Head);
        if !get || req.path != path_of(&self.settings.login_page) {
            return None;
        }
        let has = |name: &str| {
            req.query.as_deref().is_some_and(|q| {
                q.split('&')
                    .any(|pair| pair.split('=').next() == Some(name))
            })
        };
        Some((has("error"), has("logout")))
    }

    /// Replaces the router's 404 for the login page with the default page, when it is on.
    ///
    /// `session` is the session of the request, cloned before the application ran because this
    /// function does not see the request. When the CSRF check is on, the page creates the token
    /// in that session and carries it in a hidden field. Without a session the page has no
    /// field and the missing session filter is logged.
    pub(crate) fn page_fallback(
        &self,
        res: Response,
        flags: (bool, bool),
        session: Option<Session>,
    ) -> Response {
        if res.status() == StatusCode::NOT_FOUND && self.settings.login_page_enabled {
            let token = self.csrf.as_ref().and_then(|cfg| match &session {
                Some(session) => Some((cfg.field_name.as_str(), ensure_token(session))),
                None => {
                    no_session_error();
                    None
                }
            });
            let field = token.as_ref().map(|(name, token)| (*name, token.as_str()));
            let remember_me = self
                .remember_me
                .as_ref()
                .map(|r| r.settings.field_name.as_str());
            login_page(&self.settings, flags.0, flags.1, field, remember_me)
        } else {
            res
        }
    }

    /// Whether the request is a `GET` of the token endpoint, when there is one. `HEAD`, other
    /// methods and other paths are not.
    pub(crate) fn token_endpoint_request(&self, req: &Request) -> bool {
        self.csrf
            .as_ref()
            .and_then(|cfg| cfg.token_endpoint.as_deref())
            .is_some_and(|path| req.method == HttpMethod::Get && req.path == path_of(path))
    }

    /// Answers the token endpoint. Without a session the application has no session filter: log
    /// that and return `None`, so the request goes on to the application as any other.
    pub(crate) fn token_endpoint(&self, req: &Request) -> Option<Response> {
        let cfg = self.csrf.as_ref()?;
        match req.session() {
            Some(session) => Some(token_endpoint_response(&session, cfg)),
            None => {
                no_session_error();
                None
            }
        }
    }

    /// The authentication kept in a session that already existed before this request, if any.
    /// A new session is never read, so an anonymous request leaves it untouched.
    pub(crate) fn restore(&self, req: &Request) -> Option<Authentication> {
        let session = req.session().filter(|s| !s.is_new())?;
        load_authentication(&session)
    }

    /// Revalidates `authentication`, just restored from the session of `req`, when revalidation
    /// is on. Returns the authentication the request goes on with, or `None` when the session
    /// was ended and the request is anonymous.
    pub(crate) async fn revalidate(
        &self,
        req: &Request,
        authentication: Authentication,
    ) -> Option<Authentication> {
        let (Some(revalidation), Some(session)) = (&self.revalidation, req.session()) else {
            return Some(authentication);
        };
        match revalidate(revalidation, &session, &authentication, self.clock.now()).await {
            Revalidated::Unchanged | Revalidated::Skipped => Some(authentication),
            Revalidated::Updated(updated) => Some(updated),
            Revalidated::Ended => None,
        }
    }

    fn failure(&self, error: &dyn std::fmt::Debug, username_len: usize) -> Response {
        tracing::debug!(
            target: "oxbloom::security",
            error = ?error,
            username_len,
            "form login failed"
        );
        redirect(&self.settings.failure_url, "/")
    }

    async fn login(&self, req: &Request, manager: Option<&ProviderManager>) -> Response {
        // The origin first, the token second, both before the form is read or the provider is
        // asked, and both with the same answer.
        if !same_origin(req) {
            return status(StatusCode::FORBIDDEN);
        }
        if !self.csrf_passes(req) {
            // The answer stays the same 403; only the log names the missing filter.
            if req.session().is_none() {
                no_session_error();
            }
            return status(StatusCode::FORBIDDEN);
        }
        if !is_form(req) {
            return status(StatusCode::BAD_REQUEST);
        }
        let Some(session) = req.session() else {
            no_session_error();
            return redirect(&self.settings.failure_url, "/");
        };
        let form = if req.body_too_large() {
            None
        } else {
            parse_login_form(
                &req.body,
                &self.settings.username_field,
                &self.settings.password_field,
                self.remember_me
                    .as_ref()
                    .map(|r| r.settings.field_name.as_str()),
            )
        };
        let Some(LoginForm {
            username,
            password,
            remember_me,
        }) = form
        else {
            return self.failure(&MalformedForm, 0);
        };
        let username_len = username.chars().count();
        let request = AuthenticationRequest::UsernamePassword { username, password };
        let result = match manager {
            Some(manager) => manager.authenticate(&request).await,
            None => Err(AuthenticationError::ProviderUnavailable),
        };
        // A password login is never remembered, whatever the provider put in the details.
        let authentication = match result {
            Ok(authentication) => authentication.not_remembered(),
            Err(error) => return self.failure(&error, username_len),
        };
        let principal = authentication.principal().to_string();
        if self
            .establish(&session, &authentication, None)
            .await
            .is_err()
        {
            return redirect(&self.settings.failure_url, "/");
        }
        let cookie = match (&self.remember_me, remember_me) {
            (Some(runtime), true) => self.remember_cookie(runtime, req, &principal).await,
            // A login without the box ends the remember-me of the browser: a cookie left from an
            // earlier user would otherwise log that user in again when this session idles out.
            (Some(runtime), false) if remember_me_token(runtime, req).is_some() => {
                Some(clear_cookie(&runtime.settings, &CookieScope::of(req)))
            }
            _ => None,
        };
        let target = take_saved_request(&session);
        let target = target
            .as_deref()
            .unwrap_or(&self.settings.default_success_url);
        let mut res = redirect(target, &self.settings.default_success_url);
        if let Some(cookie) = cookie {
            res.headers_mut().append(header::SET_COOKIE, cookie);
        }
        res
    }

    /// The remember-me cookie for `principal`, who just logged in with a password. The user is
    /// loaded once more, through the remember-me user service, for the stored password hash the
    /// token is bound to. A failed or empty load, or a token that would be too long, logs one
    /// fixed text and gives `None`: the login goes on without a cookie.
    async fn remember_cookie(
        &self,
        runtime: &RememberMeRuntime,
        req: &Request,
        principal: &str,
    ) -> Option<HeaderValue> {
        let settings = &runtime.settings;
        let users = settings.user_details.as_ref()?;
        let Ok(Some(user)) = users.load_user_by_username(principal).await else {
            tracing::warn!(
                target: "oxbloom::security",
                "the remember-me cookie was not issued: the user could not be loaded"
            );
            return None;
        };
        let now = settings.clock.now();
        let cookie = issue_cookie(settings, &runtime.codec, &user, now, &CookieScope::of(req));
        if cookie.is_none() {
            tracing::warn!(
                target: "oxbloom::security",
                "the remember-me cookie was not issued: the token would be too long"
            );
        }
        cookie
    }

    /// Logs `authentication` in to `session`, for a password login, an auto login and an OAuth2
    /// login alike: a new session id, a new CSRF token when the check is on, the
    /// authentication, the principal key and the revalidation time from one read of the clock,
    /// then the session limit. A failed write logs one fixed text, invalidates the session and
    /// gives `Err`.
    ///
    /// `oauth2_registration` is the registration of an OAuth2 login and `None` for every other
    /// login. It decides the principal key, which the per-user session index and the session
    /// limit use: the bare principal for a password or remember-me login, and
    /// `oauth2:<registration>:<principal>` for an OAuth2 login, so an OAuth2 user never shares
    /// the sessions of a password user with the same name. A login that is not an OAuth2 login
    /// also removes the OAuth2 tokens of an earlier login in the session.
    pub(crate) async fn establish(
        &self,
        session: &Session,
        authentication: &Authentication,
        oauth2_registration: Option<&str>,
    ) -> Result<(), ()> {
        // The id of the session the login replaces, read before the rotation removes it.
        let replaced_id = session.id();
        // A new id first, so a planted or stolen anonymous id never carries the login.
        session.rotate_id();
        // A new token too, so a token known before the login is dead after it.
        if self.csrf.is_some() {
            replace_token(session);
        }
        // One read of the clock, so the login time and the revalidation time are the same.
        let now = self.clock.now();
        let principal = match oauth2_registration {
            Some(registration) => format!("oauth2:{registration}:{}", authentication.principal()),
            None => {
                session.remove_reserved_prefix(OAUTH2_TOKENS_PREFIX);
                authentication.principal().to_string()
            }
        };
        if store_authentication(session, authentication, now).is_err()
            || store_principal(session, &principal, now).is_err()
        {
            tracing::error!(
                target: "oxbloom::security",
                "the authentication could not be stored in the session"
            );
            // A half-written login must not stay: without the principal key the session would
            // escape the limit and the registry.
            session.invalidate();
            return Err(());
        }
        self.apply_limit(&principal, &replaced_id).await;
        Ok(())
    }

    /// Logs the request in from its remember-me cookie, when remember-me is on. The chain calls
    /// this only for a request without a header credential whose session gave no
    /// authentication. A valid cookie logs in like a password login, through
    /// [`establish`](Self::establish), with the authentication marked remembered; the cookie is
    /// not issued again, so its lifetime stays fixed.
    pub(crate) async fn auto_login(&self, req: &Request) -> AutoLogin {
        let Some(runtime) = &self.remember_me else {
            return AutoLogin::Absent;
        };
        let Some(token) = remember_me_token(runtime, req) else {
            return AutoLogin::Absent;
        };
        let Some(session) = req.session() else {
            no_remember_me_session();
            return AutoLogin::Absent;
        };
        let user = match check_token(runtime, &token).await {
            Checked::Valid(user) => user,
            Checked::Invalid => {
                invalid_cookie();
                return AutoLogin::Invalid;
            }
            Checked::Unavailable => return AutoLogin::Unavailable,
        };
        // The same authentication a password login of this user builds, then the marker.
        let authentication = authentication_of(&user).remembered();
        match self.establish(&session, &authentication, None).await {
            Ok(()) => AutoLogin::LoggedIn(authentication),
            Err(()) => AutoLogin::Unavailable,
        }
    }

    /// The `Set-Cookie` that clears the remember-me cookie in the answer to `req`, when
    /// remember-me is on. Built before the request goes on, because the cookie scope is read
    /// from the request.
    pub(crate) fn clearing_cookie(&self, req: &Request) -> Option<HeaderValue> {
        let runtime = self.remember_me.as_ref()?;
        Some(clear_cookie(&runtime.settings, &CookieScope::of(req)))
    }

    /// Appends `cookie`, from [`clearing_cookie`](Self::clearing_cookie), to `res`, unless the
    /// response already sets the remember-me cookie itself.
    pub(crate) fn add_clearing_cookie(&self, res: &mut Response, cookie: HeaderValue) {
        if let Some(runtime) = &self.remember_me
            && !sets_cookie(res, &runtime.settings.cookie_name)
        {
            res.headers_mut().append(header::SET_COOKIE, cookie);
        }
    }

    /// Evicts the oldest sessions of `principal` until `max - 1` others remain. The session being
    /// replaced is never counted. A failure is logged with a fixed text and never fails the login.
    async fn apply_limit(&self, principal: &str, replaced_id: &str) {
        let Some((sessions, max)) = &self.limit else {
            return;
        };
        if sessions
            .evict_oldest(principal, max - 1, Some(replaced_id))
            .await
            .is_err()
        {
            tracing::error!(
                target: "oxbloom::security",
                "the session limit could not be applied"
            );
        }
    }

    fn logout(&self, req: &Request) -> Response {
        if !same_origin(req) {
            return status(StatusCode::FORBIDDEN);
        }
        if !self.csrf_passes(req) {
            // The answer stays the same 403; only the log names the missing filter.
            if req.session().is_none() {
                no_session_error();
            }
            return status(StatusCode::FORBIDDEN);
        }
        if let Some(session) = req.session() {
            session.invalidate();
        }
        let mut res = redirect(&self.settings.logout_success_url, "/");
        if let Some(runtime) = &self.remember_me {
            let cookie = clear_cookie(&runtime.settings, &CookieScope::of(req));
            res.headers_mut().append(header::SET_COOKIE, cookie);
        }
        res
    }
}

/// The entry point with form login on and no custom entry point: an anonymous browser `GET` or
/// `HEAD` is sent to the login page and its target saved in the session; every other request
/// gets the answer of the entry point it wraps.
pub(crate) struct FormLoginEntryPoint {
    settings: Arc<FormLogin>,
    fallback: Arc<dyn AuthenticationEntryPoint>,
}

impl FormLoginEntryPoint {
    pub(crate) fn new(
        settings: Arc<FormLogin>,
        fallback: Arc<dyn AuthenticationEntryPoint>,
    ) -> Self {
        Self { settings, fallback }
    }

    /// Whether `path` is one of the configured form login paths, which are never saved.
    fn is_form_login_path(&self, path: &str) -> bool {
        let s = &self.settings;
        [
            &s.login_page,
            &s.login_processing_url,
            &s.logout_url,
            &s.default_success_url,
            &s.failure_url,
            &s.logout_success_url,
        ]
        .iter()
        .any(|url| path_of(url) == path)
    }
}

impl AuthenticationEntryPoint for FormLoginEntryPoint {
    fn commence(&self, req: &Request, error: Option<&AuthenticationError>) -> Response {
        let browser_get = matches!(req.method, HttpMethod::Get | HttpMethod::Head);
        if error.is_some() || !browser_get || !prefers_html(req) {
            return self.fallback.commence(req, error);
        }
        // The one place an anonymous request writes a session: the session filter creates it
        // lazily when the saved request is set.
        if let Some(session) = req.session()
            && !self.is_form_login_path(&req.path)
        {
            let target = match &req.query {
                Some(query) => format!("{}?{query}", req.path),
                None => req.path.clone(),
            };
            save_request(&session, &target);
        }
        redirect(&self.settings.login_page, "/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accept(value: Option<&str>) -> bool {
        let mut req = Request::new(HttpMethod::Get, "/");
        if let Some(value) = value {
            req.headers.insert(header::ACCEPT, value.parse().unwrap());
        }
        prefers_html(&req)
    }

    #[test]
    fn browsers_prefer_html() {
        assert!(accept(Some(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        )));
        assert!(accept(Some("text/html")));
        assert!(accept(Some("application/xhtml+xml")));
        assert!(accept(Some("TEXT/HTML; charset=utf-8")));
        assert!(accept(Some("text/html, application/json")));
        assert!(accept(Some("*/*, text/html")));
    }

    #[test]
    fn api_clients_do_not() {
        assert!(!accept(None));
        assert!(!accept(Some("*/*")));
        assert!(!accept(Some("text/*")));
        assert!(!accept(Some("application/json")));
        assert!(!accept(Some("application/json, text/html")));
        assert!(!accept(Some("text/html;q=0.5, application/json")));
        assert!(!accept(Some("text/html;q=0")));
        assert!(!accept(Some("*/*, application/json;q=1, text/html;q=0.5")));
        assert!(!accept(Some("image/png")));
    }

    #[test]
    fn a_bad_quality_is_ignored() {
        assert!(!accept(Some("text/html;q=2")));
        assert!(!accept(Some("text/html;q=x")));
        assert!(accept(Some("text/html;q=x, text/html")));
    }

    #[test]
    fn form_content_type_allows_parameters() {
        let mut req = Request::new(HttpMethod::Post, "/login");
        assert!(!is_form(&req));
        req.headers.insert(
            header::CONTENT_TYPE,
            "Application/X-WWW-Form-Urlencoded; charset=UTF-8"
                .parse()
                .unwrap(),
        );
        assert!(is_form(&req));
        req.headers
            .insert(header::CONTENT_TYPE, "multipart/form-data".parse().unwrap());
        assert!(!is_form(&req));
    }

    #[test]
    fn a_target_that_is_no_header_value_falls_back() {
        let res = redirect("/a\u{7f}b", "/home");
        assert_eq!(res.headers()[header::LOCATION], "/home");
        assert_eq!(
            redirect("/a?b=c", "/").headers()[header::LOCATION],
            "/a?b=c"
        );
    }
}
