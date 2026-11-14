use oxbloom_di::{ContainerBuilder, DiError};

use crate::{SecurityBuildError, SecurityFilterChain};

/// Registers a [`SecurityFilterChain`] in a `oxbloom-di` container. Requires the `di` feature.
///
/// # Examples
///
/// ```
/// use oxbloom_di::ContainerBuilder;
/// use oxbloom_security::{HttpSecurity, SecurityContainerExt, SecurityFilterChain};
/// use oxbloom_web::{App, HttpMethod, Request, Route};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let chain = HttpSecurity::new()
///     .authorize(|a| a.any_request().authenticated())
///     .build()?;
/// let container = ContainerBuilder::new().security_chain(chain).build()?;
///
/// // The resolved `Arc<SecurityFilterChain>` is accepted by `App::filter` directly.
/// let app = App::new()
///     .filter(container.get::<SecurityFilterChain>()?)
///     .route(Route::new(HttpMethod::Get, "/data", |_| async { "data" }))?;
/// assert_eq!(app.handle(Request::new(HttpMethod::Get, "/data")).await.status(), 401);
/// # Ok(())
/// # }
/// ```
///
/// Authentication providers can be registered as beans too. Each provider is made available as
/// `dyn AuthenticationProvider` with `provides`, and a `bean` that takes
/// `Vec<Arc<dyn AuthenticationProvider>>` receives all of them, sorted by `order` and then by
/// registration order, to build a [`ProviderManager`](crate::ProviderManager):
///
/// ```
/// use std::sync::Arc;
/// use async_trait::async_trait;
/// use oxbloom_config::Secret;
/// use oxbloom_di::ContainerBuilder;
/// use oxbloom_security::{
///     Authentication, AuthenticationError, AuthenticationProvider, AuthenticationRequest,
///     ProviderManager,
/// };
///
/// // Two providers: one for service tokens, one for everything else.
/// struct ServiceTokens;
/// struct Anyone;
///
/// #[async_trait]
/// impl AuthenticationProvider for ServiceTokens {
///     fn supports(&self, req: &AuthenticationRequest) -> bool {
///         match req {
///             AuthenticationRequest::BearerToken(token) => token.expose().starts_with("svc-"),
///         }
///     }
///
///     async fn authenticate(
///         &self,
///         _req: &AuthenticationRequest,
///     ) -> Result<Authentication, AuthenticationError> {
///         Ok(Authentication::new("service", []))
///     }
/// }
///
/// #[async_trait]
/// impl AuthenticationProvider for Anyone {
///     fn supports(&self, _req: &AuthenticationRequest) -> bool {
///         true
///     }
///
///     async fn authenticate(
///         &self,
///         _req: &AuthenticationRequest,
///     ) -> Result<Authentication, AuthenticationError> {
///         Ok(Authentication::new("anyone", []))
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let container = ContainerBuilder::new()
///     .bean(|| Anyone)
///     .provides(|p: Arc<Anyone>| -> Arc<dyn AuthenticationProvider> { p })
///     .order(10) // asked last, although registered first
///     .bean(|| ServiceTokens)
///     .provides(|p: Arc<ServiceTokens>| -> Arc<dyn AuthenticationProvider> { p })
///     .bean(|providers: Vec<Arc<dyn AuthenticationProvider>>| ProviderManager::new(providers))
///     .build()?;
///
/// let manager = container.get::<ProviderManager>()?;
/// let service = AuthenticationRequest::BearerToken(Secret::new("svc-1".to_string()));
/// let user = AuthenticationRequest::BearerToken(Secret::new("user-1".to_string()));
/// assert_eq!(manager.authenticate(&service).await?.principal(), "service");
/// assert_eq!(manager.authenticate(&user).await?.principal(), "anyone");
/// # Ok(())
/// # }
/// ```
pub trait SecurityContainerExt {
    /// Registers `chain` as a singleton, resolvable as `Arc<SecurityFilterChain>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::ContainerBuilder;
    /// use oxbloom_security::{HttpSecurity, SecurityContainerExt, SecurityFilterChain};
    ///
    /// let chain = HttpSecurity::new()
    ///     .authorize(|a| a.any_request().authenticated())
    ///     .build()?;
    /// let container = ContainerBuilder::new().security_chain(chain).build()?;
    /// let a = container.get::<SecurityFilterChain>()?;
    /// let b = container.get::<SecurityFilterChain>()?;
    /// assert!(Arc::ptr_eq(&a, &b));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    fn security_chain(self, chain: SecurityFilterChain) -> Self;
}

impl SecurityContainerExt for ContainerBuilder {
    fn security_chain(self, chain: SecurityFilterChain) -> Self {
        self.instance(chain)
    }
}

/// Lets `?` turn a [`SecurityBuildError`] into a [`DiError`] inside a `try_bean` factory.
/// Requires the `di` feature.
///
/// The result is a `DiError::Creation` whose message is the error's `Display` and whose source is
/// the original [`SecurityBuildError`]. `try_bean` replaces the type name with the bean being
/// built and keeps the source.
///
/// # Examples
///
/// ```
/// use std::error::Error;
/// use oxbloom_di::{ContainerBuilder, DiError};
/// use oxbloom_security::{HttpSecurity, SecurityBuildError, SecurityFilterChain};
///
/// let err = ContainerBuilder::new()
///     .try_bean(|| -> Result<SecurityFilterChain, DiError> { Ok(HttpSecurity::new().build()?) })
///     .build()
///     .err()
///     .unwrap();
/// let source = err.source().and_then(|s| s.downcast_ref::<SecurityBuildError>());
/// assert_eq!(source, Some(&SecurityBuildError::NoAuthorizeSection));
/// ```
impl From<SecurityBuildError> for DiError {
    fn from(err: SecurityBuildError) -> Self {
        DiError::creation_from::<SecurityBuildError>(err)
    }
}
