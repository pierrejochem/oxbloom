//! The security filter chain, bearer extraction and default failure handlers.

use std::sync::Arc;

use async_trait::async_trait;
use http::{HeaderValue, StatusCode, header};
use oxbloom_config::Secret;
use oxbloom_core::HttpMethod;
use oxbloom_web::{Filter, Next, Request, Response};

use crate::{
    AuthenticationError, AuthenticationRequest, AuthorizationRule, ProviderManager, RequestMatcher,
    RequestSecurityExt, SecurityContext,
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
    Malformed,
    Token(String),
}

/// Reads `Authorization: Bearer <token>`. A missing header or another scheme is anonymous.
fn extract(req: &Request) -> Credentials {
    let mut values = req.headers.get_all(header::AUTHORIZATION).iter();
    let Some(first) = values.next() else {
        return Credentials::Anonymous;
    };
    if values.next().is_some() {
        return Credentials::Malformed;
    }
    let Ok(text) = first.to_str() else {
        return Credentials::Malformed;
    };
    let (scheme, rest) = text.split_once(' ').unwrap_or((text, ""));
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Credentials::Anonymous;
    }
    let token = rest.trim_start_matches(' ');
    if token.is_empty() || token.contains(char::is_whitespace) {
        Credentials::Malformed
    } else {
        Credentials::Token(token.to_owned())
    }
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
}

impl SecurityFilterChain {
    pub(crate) fn new(
        manager: Option<ProviderManager>,
        rules: Vec<(RequestMatcher, AuthorizationRule)>,
        entry_point: Arc<dyn AuthenticationEntryPoint>,
        access_denied: Arc<dyn AccessDeniedHandler>,
    ) -> Self {
        Self {
            manager,
            rules,
            entry_point,
            access_denied,
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
    async fn filter(&self, mut req: Request, next: Next<'_>) -> Response {
        // Keep the token out of `Debug` output of the request from here on, also for requests
        // built by hand. Marking a value sensitive does not change its bytes.
        if let header::Entry::Occupied(mut entry) = req.headers.entry(header::AUTHORIZATION) {
            entry.iter_mut().for_each(|v| v.set_sensitive(true));
        }
        let mut context = SecurityContext::anonymous();
        if let Some(manager) = &self.manager {
            match extract(&req) {
                Credentials::Anonymous => {}
                Credentials::Malformed => return unauthorized(Some("invalid_request")),
                Credentials::Token(token) => {
                    let credentials = AuthenticationRequest::BearerToken(Secret::new(token));
                    match manager.authenticate(&credentials).await {
                        Ok(authentication) => {
                            context = SecurityContext::authenticated(authentication);
                        }
                        Err(error) => return self.entry_point.commence(&req, Some(&error)),
                    }
                }
            }
        }
        req.extensions_mut().insert(context);
        if self.authorized(&req) {
            next.run(req).await
        } else if req.authentication().is_none() {
            self.entry_point.commence(&req, None)
        } else {
            self.access_denied.handle(&req)
        }
    }

    fn order(&self) -> i32 {
        oxbloom_web::order::SECURITY
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
            let AuthenticationRequest::BearerToken(token) = req;
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
