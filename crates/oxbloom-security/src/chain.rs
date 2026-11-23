//! The security filter chain, bearer extraction and default failure handlers.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::HttpMethod;
use oxbloom_web::{Filter, Next, Request, Response};

use crate::csrf::hide_token_header;
use crate::form_login::FormLoginHandlers;
use crate::headers::apply_headers;
use crate::remember_me::AutoLogin;
use crate::{
    AuthenticationError, AuthenticationRequest, AuthorizationRule, HeadersConfig, ProviderManager,
    RequestMatcher, RequestSecurityExt, RequestSessionExt, SecurityContext,
};

/// Produces the response for a request that needs authentication, or whose credentials failed.
///
/// `error` is `None` for an anonymous request and `Some` when a supplied token failed.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{AuthenticationEntryPoint, AuthenticationError};
/// use oxbloom_web::{HttpMethod, Request, Response};
///
/// struct Teapot;
///
/// impl AuthenticationEntryPoint for Teapot {
///     fn commence(&self, _req: &Request, _error: Option<&AuthenticationError>) -> Response {
///         let mut res = Response::new(Default::default());
///         *res.status_mut() = http::StatusCode::IM_A_TEAPOT;
///         res
///     }
/// }
///
/// let res = Teapot.commence(&Request::new(HttpMethod::Get, "/"), None);
/// assert_eq!(res.status(), 418);
/// ```
pub trait AuthenticationEntryPoint: Send + Sync {
    /// Builds the failure response.
    fn commence(&self, req: &Request, error: Option<&AuthenticationError>) -> Response;
}

/// Produces the response for an authenticated request that is not authorized.
///
/// # Examples
///
/// ```
/// use oxbloom_security::AccessDeniedHandler;
/// use oxbloom_web::{HttpMethod, Request, Response};
///
/// struct Gone;
///
/// impl AccessDeniedHandler for Gone {
///     fn handle(&self, _req: &Request) -> Response {
///         let mut res = Response::new(Default::default());
///         *res.status_mut() = http::StatusCode::GONE;
///         res
///     }
/// }
///
/// assert_eq!(Gone.handle(&Request::new(HttpMethod::Get, "/")).status(), 410);
/// ```
pub trait AccessDeniedHandler: Send + Sync {
    /// Builds the 403-style response.
    fn handle(&self, req: &Request) -> Response;
}

fn empty(status: StatusCode) -> Response {
    let mut res = Response::new(Default::default());
    *res.status_mut() = status;
    res
}

fn unauthorized(error: Option<&'static str>) -> Response {
    let mut res = empty(StatusCode::UNAUTHORIZED);
    let value = match error {
        None => HeaderValue::from_static("Bearer"),
        Some("invalid_request") => HeaderValue::from_static("Bearer error=\"invalid_request\""),
        Some(_) => HeaderValue::from_static("Bearer error=\"invalid_token\""),
    };
    res.headers_mut().insert(header::WWW_AUTHENTICATE, value);
    res
}

/// Which challenges a chain advertises: the Basic realm (when `http_basic` is on) and whether a
/// bearer provider exists.
#[derive(Debug, Clone)]
pub(crate) struct Challenges {
    pub(crate) basic_realm: Option<Arc<str>>,
    pub(crate) bearer: bool,
}

impl Challenges {
    fn basic_value(&self) -> Option<HeaderValue> {
        let realm = self.basic_realm.as_ref()?;
        HeaderValue::from_str(&format!("Basic realm=\"{realm}\", charset=\"UTF-8\"")).ok()
    }

    fn response(&self, values: impl IntoIterator<Item = HeaderValue>) -> Response {
        let mut res = empty(StatusCode::UNAUTHORIZED);
        for value in values {
            res.headers_mut().append(header::WWW_AUTHENTICATE, value);
        }
        res
    }

    /// The challenge for an anonymous request: Basic first, then Bearer.
    fn anonymous(&self) -> Response {
        let mut values = Vec::new();
        values.extend(self.basic_value());
        if self.bearer || values.is_empty() {
            values.push(HeaderValue::from_static("Bearer"));
        }
        self.response(values)
    }

    /// The challenge for a malformed header. `basic` tells whether the scheme was Basic.
    fn malformed(&self, scheme: Option<Scheme>) -> Response {
        let invalid_request = HeaderValue::from_static("Bearer error=\"invalid_request\"");
        let mut values = Vec::new();
        match scheme {
            Some(Scheme::Basic) => values.extend(self.basic_value()),
            Some(Scheme::Bearer) => values.push(invalid_request),
            None => {
                values.extend(self.basic_value());
                if self.bearer || values.is_empty() {
                    values.push(invalid_request);
                }
            }
        }
        self.response(values)
    }

    /// The challenge for a failed login, by the scheme the request used.
    fn failed(&self, req: &Request) -> Response {
        if let (true, Some(basic)) = (is_basic(req), self.basic_value()) {
            return self.response([basic]);
        }
        unauthorized(Some("invalid_token"))
    }
}

#[derive(Debug, Clone, Copy)]
enum Scheme {
    Basic,
    Bearer,
}

fn is_basic(req: &Request) -> bool {
    req.headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|t| t.split(' ').next())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("basic"))
}

/// The entry point `build` installs when `http_basic` is on and no custom one was set.
pub(crate) struct ChallengeEntryPoint(pub(crate) Challenges);

impl AuthenticationEntryPoint for ChallengeEntryPoint {
    fn commence(&self, req: &Request, error: Option<&AuthenticationError>) -> Response {
        match error {
            None => self.0.anonymous(),
            Some(AuthenticationError::ProviderUnavailable) => {
                empty(StatusCode::SERVICE_UNAVAILABLE)
            }
            Some(_) => self.0.failed(req),
        }
    }
}

/// The default [`AuthenticationEntryPoint`].
///
/// Anonymous requests get 401 with `WWW-Authenticate: Bearer`. A failed token gets 401 with
/// `error="invalid_token"`. `ProviderUnavailable` gets 503 without a challenge. Bodies are empty
/// and never contain token text; in an `oxbloom_web::App` the error filter renders the empty
/// answer as `application/problem+json` and keeps the headers.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{AuthenticationEntryPoint, AuthenticationError, DefaultEntryPoint};
/// use oxbloom_web::{HttpMethod, Request};
///
/// let req = Request::new(HttpMethod::Get, "/");
/// let res = DefaultEntryPoint.commence(&req, Some(&AuthenticationError::InvalidToken));
/// assert_eq!(res.status(), 401);
/// assert_eq!(res.headers()["www-authenticate"], "Bearer error=\"invalid_token\"");
/// let res = DefaultEntryPoint.commence(&req, Some(&AuthenticationError::ProviderUnavailable));
/// assert_eq!(res.status(), 503);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultEntryPoint;

impl AuthenticationEntryPoint for DefaultEntryPoint {
    fn commence(&self, _req: &Request, error: Option<&AuthenticationError>) -> Response {
        match error {
            None => unauthorized(None),
            Some(AuthenticationError::ProviderUnavailable) => {
                empty(StatusCode::SERVICE_UNAVAILABLE)
            }
            Some(_) => unauthorized(Some("invalid_token")),
        }
    }
}

/// The default [`AccessDeniedHandler`]: 403 with an empty body.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{AccessDeniedHandler, DefaultAccessDeniedHandler};
/// use oxbloom_web::{HttpMethod, Request};
///
/// let res = DefaultAccessDeniedHandler.handle(&Request::new(HttpMethod::Get, "/"));
/// assert_eq!(res.status(), 403);
/// assert!(res.body().is_empty());
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultAccessDeniedHandler;

impl AccessDeniedHandler for DefaultAccessDeniedHandler {
    fn handle(&self, _req: &Request) -> Response {
        empty(StatusCode::FORBIDDEN)
    }
}

enum Credentials {
    Anonymous,
    Malformed(Option<Scheme>),
    Token(String),
    Basic { username: String, password: String },
}

/// Reads `Authorization: Bearer <token>` and, when `basic` is on, `Authorization: Basic <token>`.
/// A missing header or another scheme is anonymous; so is a scheme that is switched off.
fn extract(req: &Request, basic: bool, bearer: bool) -> Credentials {
    let mut values = req.headers.get_all(header::AUTHORIZATION).iter();
    let Some(first) = values.next() else {
        return Credentials::Anonymous;
    };
    if values.next().is_some() {
        return Credentials::Malformed(None);
    }
    let Ok(text) = first.to_str() else {
        return Credentials::Malformed(None);
    };
    let (scheme, rest) = text.split_once(' ').unwrap_or((text, ""));
    let token = rest.trim_start_matches(' ');
    if bearer && scheme.eq_ignore_ascii_case("bearer") {
        if token.is_empty() || token.contains(char::is_whitespace) {
            Credentials::Malformed(Some(Scheme::Bearer))
        } else {
            Credentials::Token(token.to_owned())
        }
    } else if basic && scheme.eq_ignore_ascii_case("basic") {
        decode_basic(token).unwrap_or(Credentials::Malformed(Some(Scheme::Basic)))
    } else {
        Credentials::Anonymous
    }
}

/// Decodes the Base64 token of a Basic header and splits it at the first colon.
fn decode_basic(token: &str) -> Option<Credentials> {
    if token.is_empty() || token.contains(char::is_whitespace) {
        return None;
    }
    let bytes = STANDARD.decode(token).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (username, password) = text.split_once(':')?;
    Some(Credentials::Basic {
        username: username.to_owned(),
        password: password.to_owned(),
    })
}

/// A [`Filter`] that authenticates a request, authorizes it and either continues or answers
/// 401, 403 or 503 itself.
///
/// Every `Authorization` header value is marked sensitive first, so `Debug` output of the
/// request (and of its headers) shows `Sensitive` instead of the token.
///
/// Build it with [`HttpSecurity`](crate::HttpSecurity). Unmatched requests are denied.
///
/// A `HEAD` request is authorized as `GET`, because the router answers it with the `GET`
/// handler. Rules are tried in declaration order, each with both the `HEAD` and the `GET` view
/// of the request, so a `get(..)` rule also governs `HEAD`, and whichever of an earlier
/// `get(..)` or `head(..)` rule matches first decides. The chain runs before routing: an
/// unknown path is denied (401/403) unless a rule permits it, so anonymous clients cannot
/// probe which routes exist.
///
/// # Examples
///
/// ```
/// use oxbloom_security::HttpSecurity;
/// use oxbloom_web::{App, Filter, HttpMethod, Request, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let chain = HttpSecurity::new()
///     .authorize(|a| a.get("/ping").permit_all())
///     .build()?;
/// assert_eq!(chain.order(), oxbloom_web::order::SECURITY);
/// let app = App::new()
///     .filter(chain)
///     .route(Route::new(HttpMethod::Get, "/ping", |_| async { "pong" }))?;
/// assert_eq!(app.handle(Request::new(HttpMethod::Get, "/ping")).await.status(), 200);
/// assert_eq!(app.handle(Request::new(HttpMethod::Get, "/other")).await.status(), 401);
/// # Ok(())
/// # }
/// ```
///
/// The chain takes the slot [`order::SECURITY`](oxbloom_web::order::SECURITY) (-100): it runs
/// after the request log, error handling and the body limit, and before default filters.
pub struct SecurityFilterChain {
    manager: Option<ProviderManager>,
    rules: Vec<(RequestMatcher, AuthorizationRule)>,
    entry_point: Arc<dyn AuthenticationEntryPoint>,
    access_denied: Arc<dyn AccessDeniedHandler>,
    challenges: Challenges,
    form_login: Option<FormLoginHandlers>,
    headers: HeadersConfig,
}

impl SecurityFilterChain {
    pub(crate) fn new(
        manager: Option<ProviderManager>,
        rules: Vec<(RequestMatcher, AuthorizationRule)>,
        entry_point: Arc<dyn AuthenticationEntryPoint>,
        access_denied: Arc<dyn AccessDeniedHandler>,
        challenges: Challenges,
        form_login: Option<FormLoginHandlers>,
        headers: HeadersConfig,
    ) -> Self {
        Self {
            manager,
            rules,
            entry_point,
            access_denied,
            challenges,
            form_login,
            headers,
        }
    }

    /// Whether the request is authorized. `HEAD` is evaluated as `GET` too, because the router
    /// runs the `GET` handler for it.
    fn authorized(&self, req: &Request) -> bool {
        let get_view = (req.method == HttpMethod::Head)
            .then(|| Request::new(HttpMethod::Get, req.path.clone()));
        let rule = self.rules.iter().find(|(matcher, _)| {
            matcher.matches(req) || get_view.as_ref().is_some_and(|g| matcher.matches(g))
        });
        match rule {
            Some((_, rule)) => rule.grants(req.authentication(), req),
            None => false,
        }
    }
}

#[async_trait]
impl Filter for SecurityFilterChain {
    async fn filter(&self, req: Request, next: Next<'_>) -> Response {
        let scheme = req.scheme();
        let (mut res, authenticated, clearing) = self.run(req, next).await;
        if let (Some(cookie), Some(form)) = (clearing, &self.form_login) {
            form.add_clearing_cookie(&mut res, cookie);
        }
        apply_headers(&self.headers, authenticated, scheme, &mut res);
        res
    }

    fn order(&self) -> i32 {
        oxbloom_web::order::SECURITY
    }
}

impl SecurityFilterChain {
    /// The chain proper. Returns the response, whether the request was authenticated, which
    /// the wrapper needs to decide on `Cache-Control`, and the `Set-Cookie` that clears an
    /// invalid remember-me cookie, which the wrapper appends.
    async fn run(&self, mut req: Request, next: Next<'_>) -> (Response, bool, Option<HeaderValue>) {
        // Keep the token out of `Debug` output of the request from here on, also for requests
        // built by hand. Marking a value sensitive does not change its bytes.
        if let header::Entry::Occupied(mut entry) = req.headers.entry(header::AUTHORIZATION) {
            entry.iter_mut().for_each(|v| v.set_sensitive(true));
        }
        if let Some(cfg) = self.form_login.as_ref().and_then(|f| f.csrf()) {
            hide_token_header(&mut req, cfg);
        }
        // Form login answers its two posts itself and serves its page only at the configured
        // paths, before the authorization rules.
        if let Some(form) = &self.form_login {
            if let Some(res) = form.handle_post(&req, self.manager.as_ref()).await {
                return (res, false, None);
            }
            if form.token_endpoint_request(&req)
                && let Some(res) = form.token_endpoint(&req)
            {
                return (res, false, None);
            }
            if let Some(flags) = form.login_page_request(&req) {
                req.extensions_mut().insert(SecurityContext::anonymous());
                let session = req.session();
                let res = next.run(req).await;
                return (form.page_fallback(res, flags, session), false, None);
            }
        }
        let mut context = SecurityContext::anonymous();
        if let Some(manager) = &self.manager {
            let credentials = match extract(
                &req,
                self.challenges.basic_realm.is_some(),
                self.challenges.bearer,
            ) {
                Credentials::Anonymous => None,
                Credentials::Malformed(scheme) => {
                    return (self.challenges.malformed(scheme), false, None);
                }
                Credentials::Token(token) => {
                    Some(AuthenticationRequest::BearerToken(Secret::new(token)))
                }
                Credentials::Basic { username, password } => {
                    Some(AuthenticationRequest::UsernamePassword {
                        username,
                        password: Secret::new(password),
                    })
                }
            };
            if let Some(credentials) = credentials {
                match manager.authenticate(&credentials).await {
                    Ok(authentication) => {
                        context = SecurityContext::authenticated(authentication);
                    }
                    Err(error) => {
                        return (self.entry_point.commence(&req, Some(&error)), false, None);
                    }
                }
            }
        }
        // Credentials in the header win; the session is the fallback, and the remember-me
        // cookie the last resort. The user of a session is revalidated first, so an ended
        // session is anonymous to the CSRF check and to the rules. A request with any
        // `Authorization` header never logs in from the cookie. A request the session
        // authenticates, also one just logged in from the cookie, must carry the CSRF token when
        // it changes state.
        let mut clearing = None;
        if context.authentication().is_none()
            && let Some(form) = &self.form_login
        {
            let restored = match form.restore(&req) {
                Some(authentication) => form.revalidate(&req, authentication).await,
                None => None,
            };
            let authentication = match restored {
                Some(authentication) => Some(authentication),
                None if req.headers.contains_key(header::AUTHORIZATION) => None,
                None => match form.auto_login(&req).await {
                    AutoLogin::LoggedIn(authentication) => Some(authentication),
                    AutoLogin::Invalid => {
                        clearing = form.clearing_cookie(&req);
                        None
                    }
                    AutoLogin::Unavailable | AutoLogin::Absent => None,
                },
            };
            if let Some(authentication) = authentication {
                if !form.csrf_passes(&req) {
                    return (empty(StatusCode::FORBIDDEN), true, None);
                }
                context = SecurityContext::authenticated(authentication);
            }
        }
        req.extensions_mut().insert(context.clone());
        let authenticated = req.authentication().is_some();
        let res = if self.authorized(&req) {
            SecurityContext::scope(context, next.run(req)).await
        } else if authenticated {
            self.access_denied.handle(&req)
        } else {
            self.entry_point.commence(&req, None)
        };
        (res, authenticated, clearing)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use http::{HeaderValue, StatusCode, header};
    use oxbloom_web::{App, Filter, HttpMethod, Request, Response, Route};

    use crate::{
        AccessDeniedHandler, Authentication, AuthenticationEntryPoint, AuthenticationError,
        AuthenticationProvider, AuthenticationRequest, GrantedAuthority, HttpSecurity,
        RequestSecurityExt, SecurityBuildError, SecurityFilterChain,
    };

    struct Fake;

    #[async_trait]
    impl AuthenticationProvider for Fake {
        fn supports(&self, _req: &AuthenticationRequest) -> bool {
            true
        }

        async fn authenticate(
            &self,
            req: &AuthenticationRequest,
        ) -> Result<Authentication, AuthenticationError> {
            let AuthenticationRequest::BearerToken(token) = req else {
                return Err(AuthenticationError::BadCredentials);
            };
            match token.expose().as_str() {
                "good" => Ok(Authentication::new(
                    "alice",
                    [GrantedAuthority::new("ROLE_USER")],
                )),
                "admin" => Ok(Authentication::new(
                    "root",
                    [GrantedAuthority::new("ROLE_ADMIN")],
                )),
                "down" => Err(AuthenticationError::ProviderUnavailable),
                _ => Err(AuthenticationError::InvalidToken),
            }
        }
    }

    fn base() -> HttpSecurity {
        HttpSecurity::new().bearer_provider(Arc::new(Fake))
    }

    fn standard() -> SecurityFilterChain {
        base()
            .authorize(|a| {
                a.get("/public")
                    .permit_all()
                    .get("/admin")
                    .has_role("ADMIN")
                    .path("/deny")
                    .deny_all()
                    .any_request()
                    .authenticated()
            })
            .build()
            .unwrap()
    }

    fn app(chain: SecurityFilterChain) -> App {
        let ok = |path: &'static str| {
            Route::new(HttpMethod::Get, path, |req: Request| async move {
                req.authentication()
                    .map(|a| a.principal().to_string())
                    .unwrap_or_else(|| "anonymous".to_string())
            })
        };
        App::new()
            .filter(chain)
            .route(ok("/public"))
            .unwrap()
            .route(ok("/me"))
            .unwrap()
            .route(ok("/admin"))
            .unwrap()
            .route(ok("/deny"))
            .unwrap()
            .route(ok("/x/a"))
            .unwrap()
    }

    async fn call(app: &App, method: HttpMethod, path: &str, auth: &[&str]) -> Response {
        let mut req = Request::new(method, path);
        for v in auth {
            req.headers
                .append(header::AUTHORIZATION, HeaderValue::from_str(v).unwrap());
        }
        app.handle(req).await
    }

    async fn get(app: &App, path: &str, auth: &[&str]) -> Response {
        call(app, HttpMethod::Get, path, auth).await
    }

    fn challenge(res: &Response) -> &str {
        res.headers()[header::WWW_AUTHENTICATE].to_str().unwrap()
    }

    /// The app's error pipeline renders the chain's bodiless answer as problem+json.
    fn assert_problem(res: &Response) {
        assert_eq!(
            res.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body = String::from_utf8_lossy(res.body());
        let status = format!("\"status\":{}", res.status().as_u16());
        assert!(body.contains(&status), "{body}");
    }

    #[test]
    fn build_without_authorize_fails() {
        let err = base().build().err().unwrap();
        assert_eq!(err, SecurityBuildError::NoAuthorizeSection);
        assert!(!err.to_string().is_empty());
        fn check<T: std::error::Error + Send + Sync + 'static>() {}
        check::<SecurityBuildError>();
    }

    #[test]
    fn matcher_after_any_request_fails() {
        let err = base()
            .authorize(|a| a.any_request().permit_all().get("/x").permit_all())
            .build()
            .err()
            .unwrap();
        assert_eq!(err, SecurityBuildError::MatcherAfterAnyRequest);
    }

    #[test]
    fn authorize_called_twice_fails_to_build() {
        let err = base()
            .authorize(|a| {
                a.path("/admin/**")
                    .has_role("ADMIN")
                    .any_request()
                    .authenticated()
            })
            .authorize(|a| a.any_request().permit_all())
            .build()
            .err();
        assert_eq!(err, Some(SecurityBuildError::DuplicateAuthorizeSection));
        assert_eq!(err.unwrap().to_string(), "authorize called more than once");
        assert!(
            base()
                .authorize(|a| a.any_request().permit_all())
                .build()
                .is_ok()
        );
    }

    #[test]
    fn invalid_patterns_fail_to_build() {
        for pattern in [
            "/users/{id}/secret",
            "/files/*.pdf",
            "a*",
            "***",
            "{id}",
            "?",
            "/a/*x/b",
            "/a/b?",
            "/a/}",
            "/**/x*",
        ] {
            let err = base()
                .authorize(|a| a.path(pattern).has_role("ADMIN").any_request().permit_all())
                .build()
                .err();
            assert_eq!(err, Some(SecurityBuildError::InvalidPattern), "{pattern:?}");
            assert_eq!(err.unwrap().to_string(), "invalid matcher pattern");
        }
        for pattern in ["/a/*/b", "/a/**", "/**", "*", "/users/*/secret", "/", ""] {
            let chain = base()
                .authorize(|a| a.get(pattern).permit_all().any_request().authenticated())
                .build();
            assert!(chain.is_ok(), "{pattern:?}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chain_marks_authorization_header_sensitive_and_still_authenticates() {
        let debug = Route::new(HttpMethod::Get, "/debug", |req: Request| async move {
            let principal = req
                .authentication()
                .map(|a| a.principal().to_string())
                .unwrap_or_default();
            format!("{principal} {:?} {req:?}", req.headers)
        });
        let chain = base()
            .authorize(|a| a.any_request().authenticated())
            .build()
            .unwrap();
        let app = App::new().filter(chain).route(debug).unwrap();
        let res = get(&app, "/debug", &["Bearer good"]).await;
        assert_eq!(res.status(), StatusCode::OK);
        let text = String::from_utf8_lossy(res.body()).into_owned();
        assert!(text.starts_with("alice "), "{text}");
        assert!(!text.contains("Bearer good"), "{text}");
        assert!(text.contains("Sensitive"), "{text}");
    }

    #[test]
    fn chain_declares_the_security_order() {
        assert_eq!(standard().order(), oxbloom_web::order::SECURITY);
    }

    #[test]
    fn chain_is_send_sync_static() {
        fn check<T: Send + Sync + 'static>() {}
        check::<SecurityFilterChain>();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anonymous_to_permit_all_passes_to_handler() {
        let app = app(standard());
        let res = get(&app, "/public", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.body(), "anonymous");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn anonymous_to_authenticated_is_401_with_challenge() {
        let app = app(standard());
        let res = get(&app, "/me", &[]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenge(&res), "Bearer");
        assert_problem(&res);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unknown_path_under_authenticated_catch_all_is_401_not_404() {
        let app = app(standard());
        let res = get(&app, "/nope", &[]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let res = get(&app, "/nope", &["Bearer good"]).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bad_token_is_401_invalid_token_rendered_as_problem_json() {
        let app = app(standard());
        let res = get(&app, "/public", &["Bearer nonsense"]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenge(&res), "Bearer error=\"invalid_token\"");
        assert_problem(&res);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn provider_unavailable_is_503_without_challenge() {
        let app = app(standard());
        let res = get(&app, "/public", &["Bearer down"]).await;
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(res.headers().get(header::WWW_AUTHENTICATE).is_none());
        assert_problem(&res);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn valid_token_wrong_role_is_403_rendered_as_problem_json() {
        let app = app(standard());
        let res = get(&app, "/admin", &["Bearer good"]).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert_problem(&res);
        assert!(res.headers().get(header::WWW_AUTHENTICATE).is_none());
        let res = get(&app, "/admin", &[]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn valid_token_right_role_reaches_handler_and_handler_sees_authentication() {
        let app = app(standard());
        let res = get(&app, "/admin", &["Bearer admin"]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.body(), "root");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_matching_rule_wins() {
        let permit_first = base()
            .authorize(|a| a.path("/x/**").permit_all().path("/x/**").deny_all())
            .build()
            .unwrap();
        assert_eq!(
            get(&app(permit_first), "/x/a", &[]).await.status(),
            StatusCode::OK
        );
        let deny_first = base()
            .authorize(|a| a.path("/x/**").deny_all().path("/x/**").permit_all())
            .build()
            .unwrap();
        assert_eq!(
            get(&app(deny_first), "/x/a", &[]).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_matching_rule_denies() {
        let chain = base()
            .authorize(|a| a.get("/public").permit_all())
            .build()
            .unwrap();
        let app = app(chain);
        assert_eq!(get(&app, "/me", &[]).await.status(), 401);
        assert_eq!(get(&app, "/me", &["Bearer good"]).await.status(), 403);
        // Method is part of the match: POST is not covered by the GET rule.
        let res = call(&app, HttpMethod::Post, "/public", &[]).await;
        assert_eq!(res.status(), 401);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn deny_all_anonymous_is_401_authenticated_is_403() {
        let app = app(standard());
        assert_eq!(get(&app, "/deny", &[]).await.status(), 401);
        assert_eq!(get(&app, "/deny", &["Bearer admin"]).await.status(), 403);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bearer_scheme_is_case_insensitive() {
        let app = app(standard());
        for v in ["bearer good", "BEARER good", "BeArEr good"] {
            let res = get(&app, "/me", &[v]).await;
            assert_eq!(res.status(), StatusCode::OK, "{v}");
            assert_eq!(res.body(), "alice");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_bearer_token_is_invalid_request() {
        let app = app(standard());
        for v in ["Bearer ", "Bearer", "Bearer  ", "Bearer a b"] {
            let res = get(&app, "/public", &[v]).await;
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{v:?}");
            assert_eq!(challenge(&res), "Bearer error=\"invalid_request\"", "{v:?}");
            assert_problem(&res);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn two_authorization_headers_is_invalid_request() {
        let app = app(standard());
        let res = get(&app, "/public", &["Bearer good", "Bearer good"]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenge(&res), "Bearer error=\"invalid_request\"");
        assert_problem(&res);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn basic_scheme_is_treated_as_anonymous() {
        let app = app(standard());
        let res = get(&app, "/public", &["Basic Zm9vOmJhcg=="]).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.body(), "anonymous");
        let res = get(&app, "/me", &["Basic Zm9vOmJhcg=="]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenge(&res), "Bearer");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn token_text_never_in_response_headers_or_body() {
        let app = app(standard());
        let secret = "s3cr3t-token-text";
        let value = format!("Bearer {secret}");
        for (path, auth) in [
            ("/public", value.as_str()),
            ("/me", value.as_str()),
            ("/public", "Bearer down"),
            ("/admin", "Bearer good"),
        ] {
            let res = get(&app, path, &[auth]).await;
            let mut text = String::from_utf8_lossy(res.body()).into_owned();
            for (k, v) in res.headers() {
                text.push_str(k.as_str());
                text.push_str(&String::from_utf8_lossy(v.as_bytes()));
            }
            assert!(!text.contains(secret) && !text.contains("down") && !text.contains("good"));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn custom_entry_point_is_used() {
        struct Teapot;
        impl AuthenticationEntryPoint for Teapot {
            fn commence(&self, _req: &Request, _error: Option<&AuthenticationError>) -> Response {
                let mut res = Response::new(Default::default());
                *res.status_mut() = StatusCode::IM_A_TEAPOT;
                res
            }
        }
        struct Gone;
        impl AccessDeniedHandler for Gone {
            fn handle(&self, _req: &Request) -> Response {
                let mut res = Response::new(Default::default());
                *res.status_mut() = StatusCode::GONE;
                res
            }
        }
        let chain = base()
            .authorize(|a| a.any_request().has_role("ADMIN"))
            .entry_point(Arc::new(Teapot))
            .access_denied_handler(Arc::new(Gone))
            .build()
            .unwrap();
        let app = app(chain);
        assert_eq!(get(&app, "/me", &[]).await.status(), 418);
        assert_eq!(get(&app, "/me", &["Bearer bad"]).await.status(), 418);
        assert_eq!(get(&app, "/me", &["Bearer good"]).await.status(), 410);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn chain_without_bearer_provider_ignores_authorization_header() {
        let chain = HttpSecurity::new()
            .authorize(|a| a.get("/public").permit_all().any_request().authenticated())
            .build()
            .unwrap();
        let app = app(chain);
        for v in ["Bearer good", "Bearer ", "Bearer nonsense"] {
            let res = get(&app, "/public", &[v]).await;
            assert_eq!(res.status(), StatusCode::OK, "{v:?}");
            assert_eq!(res.body(), "anonymous");
            let res = get(&app, "/me", &[v]).await;
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(challenge(&res), "Bearer");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn head_is_authorized_as_get_permit() {
        let chain = base()
            .authorize(|a| a.get("/public").permit_all())
            .build()
            .unwrap();
        let app = app(chain);
        let res = call(&app, HttpMethod::Head, "/public", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
        // Other paths stay denied for HEAD.
        let res = call(&app, HttpMethod::Head, "/me", &[]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn earlier_get_rule_shadows_later_head_rule() {
        let chain = base()
            .authorize(|a| a.get("/public").permit_all().head("/public").deny_all())
            .build()
            .unwrap();
        let app = app(chain);
        let res = call(&app, HttpMethod::Head, "/public", &[]).await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn head_cannot_bypass_get_protection_via_later_catch_all() {
        let chain = base()
            .authorize(|a| a.get("/admin").has_role("ADMIN").any_request().permit_all())
            .build()
            .unwrap();
        let app = app(chain);
        let res = call(&app, HttpMethod::Head, "/admin", &[]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        let res = call(&app, HttpMethod::Head, "/admin", &["Bearer good"]).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let res = call(&app, HttpMethod::Head, "/admin", &["Bearer admin"]).await;
        assert_eq!(res.status(), StatusCode::OK);
    }
}
