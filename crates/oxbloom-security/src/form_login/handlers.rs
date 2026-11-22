//! The form login handlers the chain runs: login, logout, the login page fallback, the restore of
//! the authentication from the session and the content-negotiated entry point.
//!
//! No username, password, session id or saved path reaches a log line. A failed login logs at
//! `debug` the error variant and the length of the username only.

use std::sync::Arc;
use std::time::SystemTime;

use http::{HeaderValue, StatusCode, header};
use oxbloom_core::HttpMethod;
use oxbloom_web::{Request, Response};

use super::form::parse_login_form;
use super::origin::same_origin;
use super::page::login_page;
use super::redirect::path_of;
use super::repository::{
    load_authentication, save_request, store_authentication, take_saved_request,
};
use super::settings::FormLogin;
use crate::csrf::{replace_token, request_passes};
use crate::{
    Authentication, AuthenticationEntryPoint, AuthenticationError, AuthenticationRequest,
    CsrfConfig, ProviderManager, RequestSessionExt,
};

/// The log marker of a form that could not be read.
#[derive(Debug)]
struct MalformedForm;

const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

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
fn is_form(req: &Request) -> bool {
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
}

impl FormLoginHandlers {
    /// `csrf` is `Some` only when the CSRF check is on.
    pub(crate) fn new(settings: Arc<FormLogin>, csrf: Option<CsrfConfig>) -> Self {
        Self { settings, csrf }
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
    pub(crate) fn page_fallback(&self, res: Response, flags: (bool, bool)) -> Response {
        if res.status() == StatusCode::NOT_FOUND && self.settings.login_page_enabled {
            login_page(&self.settings, flags.0, flags.1)
        } else {
            res
        }
    }

    /// The authentication kept in a session that already existed before this request, if any.
    /// A new session is never read, so an anonymous request leaves it untouched.
    pub(crate) fn restore(&self, req: &Request) -> Option<Authentication> {
        let session = req.session().filter(|s| !s.is_new())?;
        load_authentication(&session)
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
            )
        };
        let Some((username, password)) = form else {
            return self.failure(&MalformedForm, 0);
        };
        let username_len = username.chars().count();
        let request = AuthenticationRequest::UsernamePassword { username, password };
        let result = match manager {
            Some(manager) => manager.authenticate(&request).await,
            None => Err(AuthenticationError::ProviderUnavailable),
        };
        let authentication = match result {
            Ok(authentication) => authentication,
            Err(error) => return self.failure(&error, username_len),
        };
        // A new id first, so a planted or stolen anonymous id never carries the login.
        session.rotate_id();
        // A new token too, so a token known before the login is dead after it.
        if self.csrf.is_some() {
            replace_token(&session);
        }
        if store_authentication(&session, &authentication, SystemTime::now()).is_err() {
            tracing::error!(
                target: "oxbloom::security",
                "the authentication could not be stored in the session"
            );
            return redirect(&self.settings.failure_url, "/");
        }
        let target = take_saved_request(&session);
        let target = target
            .as_deref()
            .unwrap_or(&self.settings.default_success_url);
        redirect(target, &self.settings.default_success_url)
    }

    fn logout(&self, req: &Request) -> Response {
        if !same_origin(req) || !self.csrf_passes(req) {
            return status(StatusCode::FORBIDDEN);
        }
        if let Some(session) = req.session() {
            session.invalidate();
        }
        redirect(&self.settings.logout_success_url, "/")
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
