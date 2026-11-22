//! The `HttpSecurity` builder.

use std::fmt;
use std::sync::Arc;

use oxbloom_core::HttpMethod;

use crate::chain::{
    ChallengeEntryPoint, Challenges, DefaultAccessDeniedHandler, DefaultEntryPoint,
};
use crate::form_login::{FormLoginEntryPoint, FormLoginHandlers};
use crate::{
    AccessDeniedHandler, AuthenticationEntryPoint, AuthenticationProvider, AuthorizationManager,
    AuthorizationRule, CsrfConfig, FormLogin, JwtAuthenticationConverter,
    JwtAuthenticationProvider, JwtDecoder, ProviderManager, RequestMatcher, SecurityFilterChain,
};

/// Why [`HttpSecurity::build`] refused to build a chain.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{HttpSecurity, SecurityBuildError};
///
/// let err = HttpSecurity::new().build().err().unwrap();
/// assert_eq!(err, SecurityBuildError::NoAuthorizeSection);
/// assert_eq!(err.to_string(), "no authorize section configured");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityBuildError {
    /// `authorize` was never called, so the chain would have no rules.
    NoAuthorizeSection,
    /// A rule was added after `any_request()`, where it could never match.
    MatcherAfterAnyRequest,
    /// `oauth2_resource_server` was called without configuring a JWT decoder.
    ResourceServerWithoutDecoder,
    /// `authorize` was called more than once. Sections are never merged; configure every rule
    /// in one `authorize` call.
    DuplicateAuthorizeSection,
    /// A matcher pattern has a segment with `{`, `}` or `?`, or a `*` that is not a whole `*` or
    /// `**` segment (for example `{id}` or `*.pdf`). Such a pattern would never match.
    InvalidPattern,
    /// `http_basic` was called but no `password_provider` was added, so no login could succeed.
    HttpBasicWithoutProvider,
    /// The Basic realm contains a double quote, a backslash or a control character.
    InvalidRealm,
    /// `form_login` was configured but no `password_provider` was added, so no login could
    /// succeed.
    FormLoginWithoutProvider,
    /// A form login URL is not a safe redirect target, two URLs collide, or a form field name is
    /// empty or has a character outside ASCII letters, digits, `_` and `-`.
    InvalidFormLoginUrl,
    /// CSRF protection is on but form login is not configured, so there is no session flow to
    /// protect.
    CsrfWithoutFormLogin,
    /// A CSRF field or header name is empty or has a character outside ASCII letters, digits,
    /// `_` and `-`, or the token endpoint is not a safe path or collides with a form login URL.
    InvalidCsrfConfig,
}

impl fmt::Display for SecurityBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoAuthorizeSection => "no authorize section configured",
            Self::MatcherAfterAnyRequest => "matcher added after any_request()",
            Self::ResourceServerWithoutDecoder => {
                "oauth2_resource_server configured without a jwt decoder"
            }
            Self::DuplicateAuthorizeSection => "authorize called more than once",
            Self::InvalidPattern => "invalid matcher pattern",
            Self::HttpBasicWithoutProvider => "http_basic configured without a password provider",
            Self::InvalidRealm => "invalid http basic realm",
            Self::FormLoginWithoutProvider => "form_login configured without a password provider",
            Self::InvalidFormLoginUrl => "invalid form login url or field name",
            Self::CsrfWithoutFormLogin => "csrf protection configured without form login",
            Self::InvalidCsrfConfig => "invalid csrf field name, header name or token endpoint",
        })
    }
}

impl std::error::Error for SecurityBuildError {}

/// Builder for a [`SecurityFilterChain`].
///
/// # Examples
///
/// ```
/// use oxbloom_security::HttpSecurity;
///
/// let chain = HttpSecurity::new()
///     .authorize(|a| a.get("/health").permit_all().any_request().authenticated())
///     .build()?;
/// # let _ = chain;
/// # Ok::<(), oxbloom_security::SecurityBuildError>(())
/// ```
pub struct HttpSecurity {
    authorize: Option<AuthorizeBuilder>,
    duplicate_authorize: bool,
    providers: Vec<Arc<dyn AuthenticationProvider>>,
    entry_point: Arc<dyn AuthenticationEntryPoint>,
    access_denied: Arc<dyn AccessDeniedHandler>,
    resource_server_without_decoder: bool,
    basic: Option<HttpBasic>,
    password_provider: bool,
    bearer: bool,
    custom_entry_point: bool,
    form_login: Option<FormLogin>,
    csrf: Option<CsrfConfig>,
}

impl Default for HttpSecurity {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpSecurity {
    /// An empty builder with the default failure handlers and no bearer extraction.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// assert!(HttpSecurity::new().build().is_err());
    /// ```
    pub fn new() -> Self {
        Self {
            authorize: None,
            duplicate_authorize: false,
            providers: Vec::new(),
            entry_point: Arc::new(DefaultEntryPoint),
            access_denied: Arc::new(DefaultAccessDeniedHandler),
            resource_server_without_decoder: false,
            basic: None,
            password_provider: false,
            bearer: false,
            custom_entry_point: false,
            form_login: None,
            csrf: None,
        }
    }

    /// Configures the ordered authorization rules. The first matching rule decides, and a
    /// request that matches no rule is denied. Call it once: a second call makes
    /// [`build`](Self::build) fail with [`SecurityBuildError::DuplicateAuthorizeSection`]
    /// (sections are never merged or replaced).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{HttpSecurity, SecurityBuildError};
    ///
    /// let chain = HttpSecurity::new()
    ///     .authorize(|a| a.any_request().deny_all())
    ///     .build();
    /// assert!(chain.is_ok());
    ///
    /// let err = HttpSecurity::new()
    ///     .authorize(|a| a.path("/admin/**").has_role("ADMIN").any_request().authenticated())
    ///     .authorize(|a| a.any_request().permit_all())
    ///     .build()
    ///     .err()
    ///     .unwrap();
    /// assert_eq!(err, SecurityBuildError::DuplicateAuthorizeSection);
    /// ```
    pub fn authorize(mut self, f: impl FnOnce(AuthorizeBuilder) -> AuthorizeBuilder) -> Self {
        self.duplicate_authorize |= self.authorize.is_some();
        self.authorize = Some(f(AuthorizeBuilder::new()));
        self
    }

    /// Enables bearer-token extraction and registers a provider for it. May be called more than
    /// once; the first provider that supports the request is used. Without any provider the
    /// `Authorization` header is ignored and every request is anonymous.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{
    ///     Authentication, AuthenticationError, AuthenticationProvider, AuthenticationRequest,
    ///     HttpSecurity,
    /// };
    ///
    /// struct Everyone;
    ///
    /// #[async_trait::async_trait]
    /// impl AuthenticationProvider for Everyone {
    ///     fn supports(&self, _req: &AuthenticationRequest) -> bool { true }
    ///     async fn authenticate(
    ///         &self,
    ///         _req: &AuthenticationRequest,
    ///     ) -> Result<Authentication, AuthenticationError> {
    ///         Ok(Authentication::new("anyone", []))
    ///     }
    /// }
    ///
    /// let chain = HttpSecurity::new()
    ///     .bearer_provider(Arc::new(Everyone))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn bearer_provider(mut self, provider: Arc<dyn AuthenticationProvider>) -> Self {
        self.providers.push(provider);
        self.bearer = true;
        self
    }

    /// Registers a provider for `UsernamePassword` requests, such as a
    /// [`DaoAuthenticationProvider`](crate::DaoAuthenticationProvider). It is only used once
    /// [`http_basic`](Self::http_basic) is on. Providers are tried in the order they were added.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{
    ///     Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity,
    ///     InMemoryUserDetailsService,
    /// };
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
    /// let dao = DaoAuthenticationProvider::new(users, Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?));
    /// let chain = HttpSecurity::new()
    ///     .http_basic(|b| b)
    ///     .password_provider(Arc::new(dao))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build();
    /// assert!(chain.is_ok());
    /// # Ok(())
    /// # }
    /// ```
    pub fn password_provider(mut self, provider: Arc<dyn AuthenticationProvider>) -> Self {
        self.providers.push(provider);
        self.password_provider = true;
        self
    }

    /// Turns on HTTP Basic: `Authorization: Basic <base64>` is decoded and handed to the
    /// password providers. Without this call a `Basic` header leaves the request anonymous.
    /// An anonymous request gets `WWW-Authenticate: Basic realm="Oxbloom", charset="UTF-8"` (see
    /// [`HttpBasic::realm`]); when a bearer provider exists too, that 401 carries the Basic
    /// challenge first and the Bearer challenge second. A failed Basic login gets the Basic
    /// challenge only, and a failed bearer token keeps its Bearer value. A malformed Basic
    /// header is a 401, never a 400. A custom [`entry_point`](Self::entry_point) replaces the
    /// default answers. [`build`](Self::build) fails with
    /// [`SecurityBuildError::HttpBasicWithoutProvider`] when no
    /// [`password_provider`](Self::password_provider) was added.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{HttpSecurity, SecurityBuildError};
    ///
    /// let err = HttpSecurity::new()
    ///     .http_basic(|b| b.realm("Admin area"))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build()
    ///     .err()
    ///     .unwrap();
    /// assert_eq!(err, SecurityBuildError::HttpBasicWithoutProvider);
    /// ```
    pub fn http_basic(mut self, f: impl FnOnce(HttpBasic) -> HttpBasic) -> Self {
        self.basic = Some(f(HttpBasic::new()));
        self
    }

    /// Turns on form login: `POST` to the processing URL logs in with the password providers,
    /// `POST` to the logout URL logs out, an anonymous browser `GET` is sent to the login page
    /// and the request it wanted is saved in the session, and the default login page is served
    /// when the application has no handler for it. See [`FormLogin`] for the settings and their
    /// defaults.
    ///
    /// The chain needs a [`SessionFilter`](crate::SessionFilter) in the same application to
    /// keep the login: without one every login fails. The login and logout posts pass a
    /// same-origin check (`Origin`, else `Referer`); a mismatch is a 403. Form login also turns
    /// on the CSRF check with its defaults; [`csrf`](Self::csrf) tunes or disables it. Session
    /// keys under the prefix `oxbloom.security.` are reserved for Oxbloom.
    ///
    /// [`build`](Self::build) fails with [`SecurityBuildError::FormLoginWithoutProvider`] when
    /// no [`password_provider`](Self::password_provider) was added and with
    /// [`SecurityBuildError::InvalidFormLoginUrl`] when [`FormLogin::validate`] fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{
    ///     Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity,
    ///     InMemoryUserDetailsService, PasswordEncoder, SessionBackend, SessionFilter,
    ///     SessionSettings, InMemorySessionStore, UserDetails,
    /// };
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
    /// let users = InMemoryUserDetailsService::new(vec![
    ///     UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
    /// ])?;
    /// let chain = HttpSecurity::new()
    ///     .password_provider(Arc::new(DaoAuthenticationProvider::new(Arc::new(users), encoder)))
    ///     .form_login(|f| f.default_success_url("/home"))
    ///     // The CSRF check is shown at `csrf`; it is off here to keep the example short.
    ///     .csrf(|c| c.enabled(false))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build()?;
    /// let settings = SessionSettings::default();
    /// let store = InMemorySessionStore::new(&settings, Arc::new(oxbloom_core::SystemClock));
    /// let sessions = SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings)?;
    /// let app = App::new()
    ///     .filter(sessions)
    ///     .filter(chain)
    ///     .route(Route::new(HttpMethod::Get, "/home", |_| async { "home" }))?;
    ///
    /// // The default login page is served at `/login`.
    /// assert_eq!(app.handle(Request::new(HttpMethod::Get, "/login")).await.status(), 200);
    ///
    /// // A good login answers 302 to the success URL and sets the session cookie.
    /// let mut login = Request::new(HttpMethod::Post, "/login");
    /// login.headers.insert("content-type", "application/x-www-form-urlencoded".parse()?);
    /// login.body = "username=alice&password=s3cret".into();
    /// let res = app.handle(login).await;
    /// assert_eq!(res.status(), 302);
    /// assert_eq!(res.headers()["location"], "/home");
    /// assert!(res.headers().contains_key("set-cookie"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn form_login(mut self, f: impl FnOnce(FormLogin) -> FormLogin) -> Self {
        self.form_login = Some(f(FormLogin::default()));
        self
    }

    /// Tunes or turns off the CSRF check of form login. See [`CsrfConfig`] for the settings and
    /// their defaults. With [`form_login`](Self::form_login) on and this method never called,
    /// the check is on with the defaults.
    ///
    /// The check covers `POST`, `PUT`, `PATCH` and `DELETE` requests that the session
    /// authenticates, and the login and logout posts. Such a request must send the token of its
    /// session in the header named `header_name` or, in an urlencoded form, in the field named
    /// `field_name`; read it with
    /// [`RequestSessionExt::csrf_token`](crate::RequestSessionExt::csrf_token). Requests
    /// authenticated by `Basic` or a bearer token, anonymous requests without a session and safe
    /// methods are not checked. A failed check is a 403 with an empty body and changes nothing.
    /// A successful login replaces the token.
    ///
    /// [`build`](Self::build) fails with [`SecurityBuildError::CsrfWithoutFormLogin`] when this
    /// method was called without `form_login`, and with
    /// [`SecurityBuildError::InvalidCsrfConfig`] when [`CsrfConfig::validate`] fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{
    ///     Argon2PasswordEncoder, DaoAuthenticationProvider, HttpSecurity, InMemorySessionStore,
    ///     InMemoryUserDetailsService, PasswordEncoder, RequestSessionExt, SessionBackend,
    ///     SessionFilter, SessionSettings, UserDetails,
    /// };
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
    /// let users = InMemoryUserDetailsService::new(vec![
    ///     UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
    /// ])?;
    /// let chain = HttpSecurity::new()
    ///     .password_provider(Arc::new(DaoAuthenticationProvider::new(Arc::new(users), encoder)))
    ///     .form_login(|f| f)
    ///     .csrf(|c| c.header_name("X-XSRF-TOKEN"))
    ///     .authorize(|a| a.get("/token").permit_all().any_request().authenticated())
    ///     .build()?;
    /// let settings = SessionSettings::default();
    /// let store = InMemorySessionStore::new(&settings, Arc::new(oxbloom_core::SystemClock));
    /// let sessions = SessionFilter::new(SessionBackend::Server(Arc::new(store)), settings)?;
    /// let app = App::new()
    ///     .filter(sessions)
    ///     .filter(chain)
    ///     .route(Route::new(HttpMethod::Get, "/token", |req: Request| async move {
    ///         req.csrf_token().map(|t| t.as_str().to_owned()).unwrap_or_default()
    ///     }))?;
    ///
    /// // A login post without the token is refused before the password is checked.
    /// let mut login = Request::new(HttpMethod::Post, "/login");
    /// login.headers.insert("content-type", "application/x-www-form-urlencoded".parse()?);
    /// login.body = "username=alice&password=s3cret".into();
    /// assert_eq!(app.handle(login).await.status(), 403);
    ///
    /// // The token of the session, sent back in the form, lets the login through.
    /// let res = app.handle(Request::new(HttpMethod::Get, "/token")).await;
    /// let cookie = res.headers()["set-cookie"].to_str()?.split(';').next().unwrap().to_owned();
    /// let token = String::from_utf8(res.body().to_vec())?;
    /// let mut login = Request::new(HttpMethod::Post, "/login");
    /// login.headers.insert("content-type", "application/x-www-form-urlencoded".parse()?);
    /// login.headers.insert("cookie", cookie.parse()?);
    /// login.body = format!("username=alice&password=s3cret&_csrf={token}").into();
    /// assert_eq!(app.handle(login).await.status(), 302);
    /// # Ok(())
    /// # }
    /// ```
    pub fn csrf(mut self, f: impl FnOnce(CsrfConfig) -> CsrfConfig) -> Self {
        self.csrf = Some(f(CsrfConfig::default()));
        self
    }

    /// Configures a JWT resource server: installs a [`JwtAuthenticationProvider`] built from the
    /// [`ResourceServer`]'s decoder and converter as a bearer provider (see
    /// [`bearer_provider`](Self::bearer_provider)). Without a decoder, [`build`](Self::build)
    /// fails with [`SecurityBuildError::ResourceServerWithoutDecoder`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use jsonwebtoken::{EncodingKey, Header, encode};
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, HttpSecurity, JwtDecoderBuilder};
    /// use oxbloom_web::{App, HttpMethod, Request, Route};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let secret = b"an-example-secret-of-32-bytes!!!";
    /// let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec()))
    ///     .build()?;
    /// let chain = HttpSecurity::new()
    ///     .oauth2_resource_server(|r| r.jwt(Arc::new(decoder)))
    ///     .authorize(|a| a.any_request().has_authority("SCOPE_read"))
    ///     .build()?;
    /// let app = App::new()
    ///     .filter(chain)
    ///     .route(Route::new(HttpMethod::Get, "/data", |_| async { "data" }))?;
    ///
    /// let exp = jsonwebtoken::get_current_timestamp() + 300;
    /// let claims = serde_json::json!({ "sub": "alice", "exp": exp, "scope": "read" });
    /// let token = encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret))?;
    /// let mut req = Request::new(HttpMethod::Get, "/data");
    /// req.headers.insert("authorization", format!("Bearer {token}").parse()?);
    /// assert_eq!(app.handle(req).await.status(), 200);
    /// assert_eq!(app.handle(Request::new(HttpMethod::Get, "/data")).await.status(), 401);
    /// # Ok(())
    /// # }
    /// ```
    pub fn oauth2_resource_server(
        mut self,
        f: impl FnOnce(ResourceServer) -> ResourceServer,
    ) -> Self {
        let server = f(ResourceServer::new());
        match server.decoder {
            Some(decoder) => {
                self.providers.push(Arc::new(JwtAuthenticationProvider::new(
                    decoder,
                    server.converter,
                )));
                self.bearer = true;
            }
            None => self.resource_server_without_decoder = true,
        }
        self
    }

    /// Replaces the handler that answers anonymous or failed-authentication requests.
    ///
    /// The one exception is a malformed `Authorization` header (several headers, an empty token,
    /// or a token with whitespace): the chain answers that with its built-in 401
    /// (`error="invalid_request"`, or the Basic challenge for a malformed Basic header) and
    /// never calls this entry point. With [`http_basic`](Self::http_basic) on, a custom entry
    /// point also replaces the Basic challenge.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{DefaultEntryPoint, HttpSecurity};
    ///
    /// let chain = HttpSecurity::new()
    ///     .entry_point(Arc::new(DefaultEntryPoint))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn entry_point(mut self, entry_point: Arc<dyn AuthenticationEntryPoint>) -> Self {
        self.entry_point = entry_point;
        self.custom_entry_point = true;
        self
    }

    /// Replaces the handler that answers authenticated but unauthorized requests.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{DefaultAccessDeniedHandler, HttpSecurity};
    ///
    /// let chain = HttpSecurity::new()
    ///     .access_denied_handler(Arc::new(DefaultAccessDeniedHandler))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn access_denied_handler(mut self, handler: Arc<dyn AccessDeniedHandler>) -> Self {
        self.access_denied = handler;
        self
    }

    /// Builds the chain.
    ///
    /// # Errors
    ///
    /// [`SecurityBuildError::NoAuthorizeSection`] without an `authorize` call,
    /// [`SecurityBuildError::DuplicateAuthorizeSection`] when `authorize` was called more than
    /// once, [`SecurityBuildError::MatcherAfterAnyRequest`] when a rule follows `any_request()`,
    /// [`SecurityBuildError::InvalidPattern`] when a matcher pattern is not valid Ant syntax
    /// (see [`RequestMatcher::is_valid`]), and
    /// [`SecurityBuildError::ResourceServerWithoutDecoder`] when `oauth2_resource_server` has no
    /// decoder, [`SecurityBuildError::HttpBasicWithoutProvider`] when `http_basic` has no
    /// `password_provider`, [`SecurityBuildError::InvalidRealm`] for a realm with `"`, `\` or
    /// a control character, [`SecurityBuildError::InvalidFormLoginUrl`] when the form login
    /// settings fail [`FormLogin::validate`],
    /// [`SecurityBuildError::FormLoginWithoutProvider`] when `form_login` has no
    /// `password_provider`, [`SecurityBuildError::CsrfWithoutFormLogin`] when `csrf` was called
    /// without `form_login`, and [`SecurityBuildError::InvalidCsrfConfig`] when the CSRF
    /// settings fail [`CsrfConfig::validate`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{HttpSecurity, SecurityBuildError};
    ///
    /// let err = HttpSecurity::new()
    ///     .authorize(|a| a.any_request().permit_all().path("/x").deny_all())
    ///     .build()
    ///     .err()
    ///     .unwrap();
    /// assert_eq!(err, SecurityBuildError::MatcherAfterAnyRequest);
    ///
    /// // `{id}` is router syntax, not a matcher wildcard: use `*`.
    /// let err = HttpSecurity::new()
    ///     .authorize(|a| a.path("/users/{id}").authenticated())
    ///     .build()
    ///     .err()
    ///     .unwrap();
    /// assert_eq!(err, SecurityBuildError::InvalidPattern);
    /// ```
    pub fn build(self) -> Result<SecurityFilterChain, SecurityBuildError> {
        let authorize = self
            .authorize
            .ok_or(SecurityBuildError::NoAuthorizeSection)?;
        if self.duplicate_authorize {
            return Err(SecurityBuildError::DuplicateAuthorizeSection);
        }
        if authorize.after_any_request {
            return Err(SecurityBuildError::MatcherAfterAnyRequest);
        }
        if authorize
            .rules
            .iter()
            .any(|(matcher, _)| !matcher.is_valid())
        {
            return Err(SecurityBuildError::InvalidPattern);
        }
        if self.resource_server_without_decoder {
            return Err(SecurityBuildError::ResourceServerWithoutDecoder);
        }
        if let Some(basic) = &self.basic {
            if basic
                .realm
                .chars()
                .any(|c| c == '"' || c == '\\' || c.is_control())
            {
                return Err(SecurityBuildError::InvalidRealm);
            }
            if !self.password_provider {
                return Err(SecurityBuildError::HttpBasicWithoutProvider);
            }
        }
        if let Some(form) = &self.form_login {
            form.validate()?;
            if !self.password_provider {
                return Err(SecurityBuildError::FormLoginWithoutProvider);
            }
        }
        // With form login on and `csrf` never called, the check is on with the defaults.
        let csrf = match (&self.form_login, self.csrf) {
            (None, Some(_)) => return Err(SecurityBuildError::CsrfWithoutFormLogin),
            (None, None) => None,
            (Some(form), csrf) => {
                let csrf = csrf.unwrap_or_default();
                csrf.validate(&[
                    &form.login_page,
                    &form.login_processing_url,
                    &form.logout_url,
                    &form.default_success_url,
                    &form.failure_url,
                    &form.logout_success_url,
                ])?;
                csrf.enabled.then_some(csrf)
            }
        };
        let challenges = Challenges {
            basic_realm: self.basic.map(|b| Arc::from(b.realm)),
            bearer: self.bearer,
        };
        let entry_point: Arc<dyn AuthenticationEntryPoint> =
            if challenges.basic_realm.is_some() && !self.custom_entry_point {
                Arc::new(ChallengeEntryPoint(challenges.clone()))
            } else {
                self.entry_point
            };
        let form_login = self.form_login.map(Arc::new);
        let entry_point: Arc<dyn AuthenticationEntryPoint> = match &form_login {
            Some(settings) if !self.custom_entry_point => {
                Arc::new(FormLoginEntryPoint::new(settings.clone(), entry_point))
            }
            _ => entry_point,
        };
        let manager = (!self.providers.is_empty()).then(|| ProviderManager::new(self.providers));
        Ok(SecurityFilterChain::new(
            manager,
            authorize.rules,
            entry_point,
            self.access_denied,
            challenges,
            form_login.map(|settings| FormLoginHandlers::new(settings, csrf)),
        ))
    }
}

/// HTTP Basic settings inside [`HttpSecurity::http_basic`].
///
/// # Examples
///
/// ```
/// use oxbloom_security::HttpSecurity;
///
/// let _security = HttpSecurity::new().http_basic(|b| b.realm("Admin area"));
/// ```
#[derive(Debug, Clone)]
pub struct HttpBasic {
    realm: String,
}

impl HttpBasic {
    fn new() -> Self {
        Self {
            realm: "Oxbloom".to_string(),
        }
    }

    /// Sets the realm of the `WWW-Authenticate` challenge. The default is `Oxbloom`. A realm
    /// with `"`, `\` or a control character (also a C1 control such as U+0085) makes [`HttpSecurity::build`] fail with
    /// [`SecurityBuildError::InvalidRealm`].
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{HttpSecurity, SecurityBuildError};
    ///
    /// let err = HttpSecurity::new()
    ///     .http_basic(|b| b.realm("bad\"realm"))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build()
    ///     .err();
    /// assert_eq!(err, Some(SecurityBuildError::InvalidRealm));
    ///
    /// // A C1 control character (U+0085) is rejected too.
    /// let err = HttpSecurity::new()
    ///     .http_basic(|b| b.realm("bad\u{85}realm"))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build()
    ///     .err();
    /// assert_eq!(err, Some(SecurityBuildError::InvalidRealm));
    /// ```
    pub fn realm(mut self, realm: impl Into<String>) -> Self {
        self.realm = realm.into();
        self
    }
}

/// JWT resource-server settings inside [`HttpSecurity::oauth2_resource_server`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{
///     Algorithm, HttpSecurity, JwtAuthenticationConverter, JwtDecoderBuilder,
/// };
///
/// let decoder = JwtDecoderBuilder::with_secret(
///     Algorithm::HS256,
///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
/// )
/// .build()?;
/// let chain = HttpSecurity::new()
///     .oauth2_resource_server(|r| {
///         r.jwt(Arc::new(decoder))
///             .converter(JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_"))
///     })
///     .authorize(|a| a.any_request().has_role("ADMIN"))
///     .build();
/// assert!(chain.is_ok());
/// # Ok::<(), oxbloom_security::JwtConfigError>(())
/// ```
pub struct ResourceServer {
    decoder: Option<Arc<dyn JwtDecoder>>,
    converter: JwtAuthenticationConverter,
}

impl ResourceServer {
    fn new() -> Self {
        Self {
            decoder: None,
            converter: JwtAuthenticationConverter::default(),
        }
    }

    /// Sets the decoder that verifies bearer tokens. Required.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{Algorithm, HttpSecurity, JwtDecoderBuilder};
    ///
    /// let decoder = JwtDecoderBuilder::with_secret(
    ///     Algorithm::HS256,
    ///     Secret::new(b"an-example-secret-of-32-bytes!!!".to_vec()),
    /// )
    /// .build()?;
    /// let chain = HttpSecurity::new()
    ///     .oauth2_resource_server(|r| r.jwt(Arc::new(decoder)))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build();
    /// assert!(chain.is_ok());
    /// # Ok::<(), oxbloom_security::JwtConfigError>(())
    /// ```
    pub fn jwt(mut self, decoder: Arc<dyn JwtDecoder>) -> Self {
        self.decoder = Some(decoder);
        self
    }

    /// Replaces the default [`JwtAuthenticationConverter`] (`sub` principal, `SCOPE_`
    /// authorities from `scope`/`scp`).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{HttpSecurity, JwtAuthenticationConverter, SecurityBuildError};
    ///
    /// // A converter alone is not enough: the decoder is required.
    /// let err = HttpSecurity::new()
    ///     .oauth2_resource_server(|r| r.converter(JwtAuthenticationConverter::default()))
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build()
    ///     .err()
    ///     .unwrap();
    /// assert_eq!(err, SecurityBuildError::ResourceServerWithoutDecoder);
    /// ```
    pub fn converter(mut self, converter: JwtAuthenticationConverter) -> Self {
        self.converter = converter;
        self
    }
}

/// Collects ordered `(matcher, rule)` pairs inside [`HttpSecurity::authorize`].
///
/// # Examples
///
/// ```
/// use oxbloom_security::HttpSecurity;
///
/// let chain = HttpSecurity::new()
///     .authorize(|a| a.post("/orders").has_role("USER").any_request().deny_all())
///     .build();
/// assert!(chain.is_ok());
/// ```
pub struct AuthorizeBuilder {
    rules: Vec<(RequestMatcher, AuthorizationRule)>,
    seen_any_request: bool,
    after_any_request: bool,
}

macro_rules! method_matchers {
    ($($name:ident => $variant:ident, $example:literal, $note:literal;)*) => {$(
        #[doc = concat!("Starts a rule for `", stringify!($variant), "` requests matching an Ant-style `pattern`.")]
        ///
        #[doc = $note]
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::HttpSecurity;
        ///
        #[doc = concat!("let chain = HttpSecurity::new().authorize(|a| a.", $example, ".permit_all()).build();")]
        /// assert!(chain.is_ok());
        /// ```
        pub fn $name(self, pattern: &str) -> RuleBuilder {
            self.rule(Some(HttpMethod::$variant), pattern)
        }
    )*};
}

impl AuthorizeBuilder {
    fn new() -> Self {
        Self {
            rules: Vec::new(),
            seen_any_request: false,
            after_any_request: false,
        }
    }

    fn rule(self, method: Option<HttpMethod>, pattern: &str) -> RuleBuilder {
        RuleBuilder {
            parent: self,
            matcher: RequestMatcher::new(method, pattern),
            any_request: false,
        }
    }

    method_matchers! {
        get => Get, r#"get("/x/**")"#, "A `HEAD` request is authorized as `GET`, so this rule also governs `HEAD`. Rules are tried in declaration order, each with both views, so an earlier `get(..)` rule also shadows a later `head(..)` rule.";
        post => Post, r#"post("/x/**")"#, "Matching is on the raw, undecoded request path.";
        put => Put, r#"put("/x/**")"#, "Matching is on the raw, undecoded request path.";
        patch => Patch, r#"patch("/x/**")"#, "Matching is on the raw, undecoded request path.";
        delete => Delete, r#"delete("/x/**")"#, "Matching is on the raw, undecoded request path.";
        head => Head, r#"head("/x/**")"#, "Rules are tried in declaration order and each is tried with both the `HEAD` and the `GET` view of a `HEAD` request, so `head(..)` rules get no priority: an earlier `get(..)` rule also shadows a later `head(..)` rule, and an earlier `head(..)` rule shadows a later `get(..)` rule.";
        options => Options, r#"options("/x/**")"#, "Matching is on the raw, undecoded request path.";
    }

    /// Starts a rule for any method matching `pattern`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let chain = HttpSecurity::new().authorize(|a| a.path("/static/**").permit_all()).build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn path(self, pattern: &str) -> RuleBuilder {
        self.rule(None, pattern)
    }

    /// Starts a rule for every request. Nothing may follow it.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let chain = HttpSecurity::new().authorize(|a| a.any_request().authenticated()).build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn any_request(self) -> RuleBuilder {
        let mut rule = self.rule(None, "/**");
        rule.any_request = true;
        rule
    }
}

/// Picks the [`AuthorizationRule`] for a matcher started on [`AuthorizeBuilder`].
///
/// # Examples
///
/// ```
/// use oxbloom_security::HttpSecurity;
///
/// let chain = HttpSecurity::new()
///     .authorize(|a| a.get("/me").authenticated())
///     .build();
/// assert!(chain.is_ok());
/// ```
pub struct RuleBuilder {
    parent: AuthorizeBuilder,
    matcher: RequestMatcher,
    any_request: bool,
}

macro_rules! simple_rules {
    ($($name:ident => $rule:expr, $doc:literal;)*) => {$(
        #[doc = $doc]
        ///
        /// # Examples
        ///
        /// ```
        /// use oxbloom_security::HttpSecurity;
        ///
        #[doc = concat!("let chain = HttpSecurity::new().authorize(|a| a.path(\"/x\").", stringify!($name), "()).build();")]
        /// assert!(chain.is_ok());
        /// ```
        pub fn $name(self) -> AuthorizeBuilder {
            self.finish($rule)
        }
    )*};
}

impl RuleBuilder {
    fn finish(self, rule: AuthorizationRule) -> AuthorizeBuilder {
        let mut parent = self.parent;
        if parent.seen_any_request {
            parent.after_any_request = true;
        }
        if self.any_request {
            parent.seen_any_request = true;
        }
        parent.rules.push((self.matcher, rule));
        parent
    }

    simple_rules! {
        permit_all => AuthorizationRule::PermitAll, "Allows everyone, including anonymous requests.";
        deny_all => AuthorizationRule::DenyAll, "Denies everyone: 401 when anonymous, 403 otherwise.";
        authenticated => AuthorizationRule::Authenticated, "Allows any authenticated request.";
    }

    /// Requires the authority `ROLE_<role>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let chain = HttpSecurity::new().authorize(|a| a.path("/admin/**").has_role("ADMIN")).build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn has_role(self, role: &str) -> AuthorizeBuilder {
        self.finish(AuthorizationRule::HasRole(role.to_owned()))
    }

    /// Requires at least one of the roles.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let chain = HttpSecurity::new()
    ///     .authorize(|a| a.path("/ops/**").has_any_role(&["ADMIN", "OPS"]))
    ///     .build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn has_any_role(self, roles: &[&str]) -> AuthorizeBuilder {
        let roles = roles.iter().map(|r| (*r).to_owned()).collect();
        self.finish(AuthorizationRule::HasAnyRole(roles))
    }

    /// Requires the exact authority.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let chain = HttpSecurity::new()
    ///     .authorize(|a| a.get("/data").has_authority("SCOPE_read"))
    ///     .build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn has_authority(self, authority: &str) -> AuthorizeBuilder {
        self.finish(AuthorizationRule::HasAuthority(authority.to_owned()))
    }

    /// Requires at least one of the authorities.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::HttpSecurity;
    ///
    /// let chain = HttpSecurity::new()
    ///     .authorize(|a| a.get("/data").has_any_authority(&["SCOPE_read", "SCOPE_admin"]))
    ///     .build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn has_any_authority(self, authorities: &[&str]) -> AuthorizeBuilder {
        let authorities = authorities.iter().map(|a| (*a).to_owned()).collect();
        self.finish(AuthorizationRule::HasAnyAuthority(authorities))
    }

    /// Delegates the decision to a custom [`AuthorizationManager`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{Authentication, AuthorizationManager, HttpSecurity};
    /// use oxbloom_web::Request;
    ///
    /// struct Always;
    /// impl AuthorizationManager for Always {
    ///     fn check(&self, _auth: Option<&Authentication>, _req: &Request) -> bool { true }
    /// }
    ///
    /// let chain = HttpSecurity::new()
    ///     .authorize(|a| a.path("/x").access(Arc::new(Always)))
    ///     .build();
    /// assert!(chain.is_ok());
    /// ```
    pub fn access(self, manager: Arc<dyn AuthorizationManager>) -> AuthorizeBuilder {
        self.finish(AuthorizationRule::Custom(manager))
    }
}
