//! Authentication, authorization and JWT resource-server support for Oxbloom.
//!
//! A [`SecurityFilterChain`] is a `oxbloom-web` filter. It runs before routing: it reads
//! `Authorization: Bearer <token>`, authenticates it through the registered providers, stores a
//! [`SecurityContext`] in the request extensions and then authorizes the request against ordered
//! rules. **Requests that match no rule are denied.** There is one chain per application.
//! An unknown path is denied (401/403) unless a rule permits it, so anonymous clients cannot
//! probe which routes exist.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use jsonwebtoken::{EncodingKey, Header, encode};
//! use oxbloom_config::Secret;
//! use oxbloom_security::{
//!     Algorithm, HttpSecurity, JwtAuthenticationConverter, JwtDecoderBuilder, RequestSecurityExt,
//! };
//! use oxbloom_web::{App, HttpMethod, Request, Route};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let secret = b"an-example-secret-of-32-bytes!!!";
//! let decoder = JwtDecoderBuilder::with_secret(Algorithm::HS256, Secret::new(secret.to_vec()))
//!     .build()?;
//!
//! let chain = HttpSecurity::new()
//!     .authorize(|a| a
//!         .get("/public/**").permit_all()
//!         .path("/admin/**").has_role("ADMIN")
//!         .any_request().authenticated())
//!     .oauth2_resource_server(|r| r
//!         .jwt(Arc::new(decoder))
//!         .converter(JwtAuthenticationConverter::default().authorities_claim("roles", "ROLE_")))
//!     .build()?;
//! let app = App::new()
//!     .filter(chain)
//!     .route(Route::new(HttpMethod::Get, "/me", |req: Request| async move {
//!         req.authentication().map(|a| a.principal().to_owned()).unwrap_or_default()
//!     }))?;
//!
//! // Anonymous: 401 with a Bearer challenge. An unknown path is denied too, not 404.
//! assert_eq!(app.handle(Request::new(HttpMethod::Get, "/me")).await.status(), 401);
//! assert_eq!(app.handle(Request::new(HttpMethod::Get, "/nope")).await.status(), 401);
//!
//! // A signed token reaches the handler, which sees the principal.
//! let exp = jsonwebtoken::get_current_timestamp() + 300;
//! let claims = serde_json::json!({ "sub": "alice", "exp": exp, "roles": ["USER"] });
//! let token = encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(secret))?;
//! let mut req = Request::new(HttpMethod::Get, "/me");
//! req.headers.insert("authorization", format!("Bearer {token}").parse()?);
//! let res = app.handle(req).await;
//! assert_eq!(res.status(), 200);
//! assert_eq!(res.body(), "alice");
//! # Ok(())
//! # }
//! ```
//!
//! # Configuration
//!
//! [`jwt_decoder_from_config`] builds a decoder from `security.jwt`. Register [`SECRET_KEY`] when
//! you build the `Config`, so the HMAC secret cannot come from a command-line argument; the
//! decoder also refuses a command-line secret on its own.
//!
//! ```
//! use oxbloom_config::Config;
//! use oxbloom_security::{SECRET_KEY, jwt_decoder_from_config};
//! # let dir = tempfile::tempdir()?;
//!
//! let config = Config::builder()
//!     .dir(dir.path())
//!     .env_from([
//!         ("APP_SECURITY__JWT__ALGORITHMS".to_string(), "HS256".to_string()),
//!         ("APP_SECURITY__JWT__SECRET".to_string(), "an-example-secret-of-32-bytes!!!".to_string()),
//!     ])
//!     .secret_key(SECRET_KEY)
//!     .build()?;
//! let _decoder = jwt_decoder_from_config(&config, None)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Features
//!
//! - `jwks`: [`ReqwestJwksFetcher`] (reqwest with rustls), with `ReqwestJwksFetcher::with_tls`
//!   for a custom rustls client configuration such as one from an SSL bundle. Needs a C
//!   compiler to build.
//! - `di`: [`SecurityContainerExt`] registers a built chain in a `oxbloom-di` container.

mod authentication;
mod authorization;
mod chain;
mod config;
mod dao;
#[cfg(feature = "di")]
mod di;
mod http_security;
mod jwks;
mod jwt;
mod matcher;
mod password;
mod password_config;
mod provider;
#[cfg(feature = "jwks")]
mod reqwest_fetcher;
mod user;

pub use authentication::{
    Authentication, AuthenticationError, AuthenticationRequest, GrantedAuthority,
    RequestSecurityExt, SecurityContext,
};
pub use authorization::{AuthorizationManager, AuthorizationRule};
pub use chain::{
    AccessDeniedHandler, AuthenticationEntryPoint, DefaultAccessDeniedHandler, DefaultEntryPoint,
    SecurityFilterChain,
};
pub use config::{
    SECRET_KEY, jwks_ssl_bundle, jwt_decoder_from_config, jwt_decoder_from_config_with_clock,
};
pub use dao::DaoAuthenticationProvider;
#[cfg(feature = "di")]
pub use di::SecurityContainerExt;
pub use http_security::{
    AuthorizeBuilder, HttpBasic, HttpSecurity, ResourceServer, RuleBuilder, SecurityBuildError,
};
pub use jsonwebtoken::{Algorithm, Header as JwtHeader};
pub use jwks::{FetchError, JwksError, JwksFetcher, JwksSource, JwksSourceBuilder, KeyLookupError};
pub use jwt::{
    DefaultJwtDecoder, FailureReason, Jwt, JwtAuthenticationConverter, JwtAuthenticationProvider,
    JwtConfigError, JwtDecoder, JwtDecoderBuilder,
};
pub use matcher::RequestMatcher;
pub use password::{
    ARGON2_ID, Argon2PasswordEncoder, DelegatingPasswordEncoder, PasswordEncoder,
    PasswordEncoderError,
};
pub use password_config::{Argon2Settings, HttpBasicSettings, PasswordSettings, users_from_config};
pub use provider::{AuthenticationProvider, ProviderManager};
#[cfg(feature = "jwks")]
pub use reqwest_fetcher::ReqwestJwksFetcher;
pub use user::{
    InMemoryUserDetailsService, UserDetails, UserDetailsError, UserDetailsService, UserStoreError,
};
