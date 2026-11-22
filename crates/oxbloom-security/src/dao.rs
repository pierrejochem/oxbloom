//! Username and password authentication against a [`UserDetailsService`].

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    Authentication, AuthenticationError, AuthenticationProvider, AuthenticationRequest,
    PasswordEncoder, UserDetails, UserDetailsService,
};

/// The longest username or password, in bytes, that is checked at all.
const MAX_CREDENTIAL_BYTES: usize = 1024;

/// The fixed text hashed once at construction to make the dummy hash.
const DUMMY_PASSWORD: &str = "oxbloom-dummy-password";

/// Authenticates a username and password against a [`UserDetailsService`].
///
/// Credentials longer than 1024 bytes fail before any hashing. For an unknown user the provider
/// still verifies the password against a dummy hash made by the same encoder, so the work does
/// not reveal whether the username exists. A disabled or locked account fails only after the
/// password was correct. Every other failure is [`AuthenticationError::BadCredentials`], and a
/// failing service is [`AuthenticationError::ProviderUnavailable`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{
///     Argon2PasswordEncoder, AuthenticationProvider, AuthenticationRequest,
///     DaoAuthenticationProvider, InMemoryUserDetailsService, PasswordEncoder, UserDetails,
/// };
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let encoder = Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?);
/// let users = InMemoryUserDetailsService::new(vec![
///     UserDetails::new("alice", encoder.encode("s3cret"), []).roles(&["USER"]),
/// ])?;
/// let provider = DaoAuthenticationProvider::new(Arc::new(users), encoder);
///
/// let request = AuthenticationRequest::UsernamePassword {
///     username: "alice".to_string(),
///     password: Secret::new("s3cret".to_string()),
/// };
/// assert!(provider.supports(&request));
/// let auth = provider.authenticate(&request).await?;
/// assert_eq!(auth.principal(), "alice");
/// assert!(auth.has_role("USER"));
/// # Ok(())
/// # }
/// ```
pub struct DaoAuthenticationProvider {
    users: Arc<dyn UserDetailsService>,
    encoder: Arc<dyn PasswordEncoder>,
    dummy_hash: Arc<str>,
}

impl DaoAuthenticationProvider {
    /// Creates a provider. This hashes a fixed string once with `encoder` to make the dummy hash.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::{
    ///     Argon2PasswordEncoder, DaoAuthenticationProvider, InMemoryUserDetailsService,
    /// };
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let users = Arc::new(InMemoryUserDetailsService::new(vec![])?);
    /// let _provider = DaoAuthenticationProvider::new(users, Arc::new(Argon2PasswordEncoder::new(8, 1, 1)?));
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(users: Arc<dyn UserDetailsService>, encoder: Arc<dyn PasswordEncoder>) -> Self {
        let dummy_hash: Arc<str> = encoder.encode(DUMMY_PASSWORD).into();
        Self {
            users,
            encoder,
            dummy_hash,
        }
    }

    /// Runs `matches` on the blocking pool. A join error is `ProviderUnavailable`.
    async fn verify(&self, raw: &str, encoded: &str) -> Result<bool, AuthenticationError> {
        let encoder = Arc::clone(&self.encoder);
        let raw = raw.to_owned();
        let encoded = encoded.to_owned();
        tokio::task::spawn_blocking(move || encoder.matches(&raw, &encoded))
            .await
            .map_err(|_| AuthenticationError::ProviderUnavailable)
    }
}

impl fmt::Debug for DaoAuthenticationProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DaoAuthenticationProvider")
            .finish_non_exhaustive()
    }
}

fn fail(error: AuthenticationError, username_len: usize) -> AuthenticationError {
    tracing::debug!(
        target: "oxbloom::security",
        error = ?error,
        username_len,
        "username and password authentication failed"
    );
    error
}

#[async_trait]
impl AuthenticationProvider for DaoAuthenticationProvider {
    fn supports(&self, req: &AuthenticationRequest) -> bool {
        matches!(req, AuthenticationRequest::UsernamePassword { .. })
    }

    async fn authenticate(
        &self,
        req: &AuthenticationRequest,
    ) -> Result<Authentication, AuthenticationError> {
        let AuthenticationRequest::UsernamePassword { username, password } = req else {
            return Err(AuthenticationError::BadCredentials);
        };
        let len = username.len();
        let raw = password.expose();
        if len > MAX_CREDENTIAL_BYTES || raw.len() > MAX_CREDENTIAL_BYTES || username.is_empty() {
            return Err(fail(AuthenticationError::BadCredentials, len));
        }
        let loaded = self
            .users
            .load_user_by_username(username)
            .await
            .map_err(|_| fail(AuthenticationError::ProviderUnavailable, len))?;
        let Some(user) = loaded else {
            self.verify(raw, &self.dummy_hash)
                .await
                .map_err(|e| fail(e, len))?;
            return Err(fail(AuthenticationError::BadCredentials, len));
        };
        let matched = self
            .verify(raw, user.password())
            .await
            .map_err(|e| fail(e, len))?;
        if !matched || raw.is_empty() {
            return Err(fail(AuthenticationError::BadCredentials, len));
        }
        if !user.is_enabled() {
            return Err(fail(AuthenticationError::AccountDisabled, len));
        }
        if user.is_locked() {
            return Err(fail(AuthenticationError::AccountLocked, len));
        }
        Ok(authentication_of(&user))
    }
}

/// The authentication of a loaded user: the username as principal and the user's authorities,
/// without details. Revalidation uses the same mapping.
pub(crate) fn authentication_of(user: &UserDetails) -> Authentication {
    Authentication::new(user.username(), user.authorities().iter().cloned())
}
