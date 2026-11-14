//! Authentication providers and the manager that dispatches to them.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{Authentication, AuthenticationError, AuthenticationRequest};

/// Authenticates the requests it supports.
///
/// Return a definitive [`AuthenticationError`] to reject a request; the
/// [`ProviderManager`] never retries it with another provider.
///
/// # Examples
///
/// ```
/// use async_trait::async_trait;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{
///     Authentication, AuthenticationError, AuthenticationProvider, AuthenticationRequest,
/// };
///
/// struct Anyone;
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
/// assert!(Anyone.supports(&AuthenticationRequest::BearerToken(Secret::new("t".to_string()))));
/// ```
#[async_trait]
pub trait AuthenticationProvider: Send + Sync {
    /// Whether this provider can handle `req`.
    fn supports(&self, req: &AuthenticationRequest) -> bool;

    /// Authenticates `req`; only called when [`supports`](Self::supports) is true.
    async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError>;
}

/// Dispatches a request to the first provider that supports it.
///
/// A provider's error is returned as is and later providers are not tried.
/// When no provider supports the request the result is
/// [`AuthenticationError::ProviderUnavailable`].
///
/// # Examples
///
/// ```
/// use oxbloom_config::Secret;
/// use oxbloom_security::{AuthenticationError, AuthenticationRequest, ProviderManager};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let manager = ProviderManager::new(vec![]);
/// let request = AuthenticationRequest::BearerToken(Secret::new("t".to_string()));
/// assert_eq!(
///     manager.authenticate(&request).await.unwrap_err(),
///     AuthenticationError::ProviderUnavailable
/// );
/// # }
/// ```
pub struct ProviderManager {
    providers: Vec<Arc<dyn AuthenticationProvider>>,
}

impl ProviderManager {
    /// Creates a manager that consults `providers` in order.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::ProviderManager;
    ///
    /// let _manager = ProviderManager::new(vec![]);
    /// ```
    pub fn new(providers: Vec<Arc<dyn AuthenticationProvider>>) -> Self {
        Self { providers }
    }

    /// Authenticates `req` with the first supporting provider.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{AuthenticationRequest, ProviderManager};
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let request = AuthenticationRequest::BearerToken(Secret::new("t".to_string()));
    /// assert!(ProviderManager::new(vec![]).authenticate(&request).await.is_err());
    /// # }
    /// ```
    pub async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        match self.providers.iter().find(|p| p.supports(req)) {
            Some(provider) => provider.authenticate(req).await,
            None => Err(AuthenticationError::ProviderUnavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use oxbloom_config::Secret;

    use super::*;

    struct Fake {
        supports: bool,
        result: Result<Authentication, AuthenticationError>,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(supports: bool, result: Result<Authentication, AuthenticationError>) -> Arc<Self> {
            Arc::new(Self {
                supports,
                result,
                calls: AtomicUsize::new(0),
            })
        }
    }

    #[async_trait]
    impl AuthenticationProvider for Fake {
        fn supports(&self, _req: &AuthenticationRequest) -> bool {
            self.supports
        }

        async fn authenticate(
            &self,
            _req: &AuthenticationRequest,
        ) -> Result<Authentication, AuthenticationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    fn request() -> AuthenticationRequest {
        AuthenticationRequest::BearerToken(Secret::new("t".to_string()))
    }

    fn ok(name: &str) -> Result<Authentication, AuthenticationError> {
        Ok(Authentication::new(name, []))
    }

    #[tokio::test(flavor = "current_thread")]
    async fn first_supporting_provider_is_used() {
        let a = Fake::new(true, ok("a"));
        let b = Fake::new(true, ok("b"));
        let manager = ProviderManager::new(vec![a.clone(), b.clone()]);
        let auth = manager.authenticate(&request()).await.unwrap();
        assert_eq!(auth.principal(), "a");
        assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn definitive_error_does_not_fall_through() {
        let a = Fake::new(true, Err(AuthenticationError::BadCredentials));
        let b = Fake::new(true, ok("b"));
        let manager = ProviderManager::new(vec![a.clone(), b.clone()]);
        let result = manager.authenticate(&request()).await;
        assert_eq!(result.unwrap_err(), AuthenticationError::BadCredentials);
        assert_eq!(a.calls.load(Ordering::SeqCst), 1);
        assert_eq!(b.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unsupported_provider_is_skipped() {
        let a = Fake::new(false, ok("a"));
        let b = Fake::new(true, ok("b"));
        let manager = ProviderManager::new(vec![a.clone(), b]);
        let auth = manager.authenticate(&request()).await.unwrap();
        assert_eq!(auth.principal(), "b");
        assert_eq!(a.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn no_supporting_provider_is_provider_unavailable() {
        let manager = ProviderManager::new(vec![Fake::new(false, ok("a"))]);
        let result = manager.authenticate(&request()).await;
        assert_eq!(
            result.unwrap_err(),
            AuthenticationError::ProviderUnavailable
        );
        let empty = ProviderManager::new(vec![]);
        assert_eq!(
            empty.authenticate(&request()).await.unwrap_err(),
            AuthenticationError::ProviderUnavailable
        );
    }
}
