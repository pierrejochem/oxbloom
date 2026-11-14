//! User records and the services that load them.

use std::collections::HashMap;
use std::fmt;

use async_trait::async_trait;

use crate::GrantedAuthority;

/// A stored user: a username, a password hash, authorities and account flags.
///
/// `Debug` shows the username and the authorities. The password hash prints as `[REDACTED]`.
///
/// # Examples
///
/// ```
/// use oxbloom_security::UserDetails;
///
/// let user = UserDetails::new("alice", "$stored-hash", []).roles(&["USER"]).locked(false);
/// assert_eq!(user.username(), "alice");
/// assert!(user.is_enabled() && !user.is_locked());
/// assert!(user.authorities().iter().any(|a| a.as_str() == "ROLE_USER"));
/// assert!(!format!("{user:?}").contains("$stored-hash"));
/// ```
#[derive(Clone)]
pub struct UserDetails {
    username: String,
    password: String,
    authorities: Vec<GrantedAuthority>,
    enabled: bool,
    locked: bool,
}

impl UserDetails {
    /// Creates an enabled, unlocked user. `password` is the stored hash, not a raw password.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{GrantedAuthority, UserDetails};
    ///
    /// let user = UserDetails::new("bob", "hash", [GrantedAuthority::new("SCOPE_read")]);
    /// assert_eq!(user.authorities().len(), 1);
    /// ```
    pub fn new(
        username: impl Into<String>,
        password: impl Into<String>,
        authorities: impl IntoIterator<Item = GrantedAuthority>,
    ) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
            authorities: authorities.into_iter().collect(),
            enabled: true,
            locked: false,
        }
    }

    /// Adds a `ROLE_<name>` authority for each role.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// let user = UserDetails::new("a", "h", []).roles(&["ADMIN", "USER"]);
    /// let names: Vec<&str> = user.authorities().iter().map(|a| a.as_str()).collect();
    /// assert_eq!(names, ["ROLE_ADMIN", "ROLE_USER"]);
    /// ```
    #[must_use]
    pub fn roles(mut self, roles: &[&str]) -> Self {
        self.authorities.extend(
            roles
                .iter()
                .map(|r| GrantedAuthority::new(format!("ROLE_{r}"))),
        );
        self
    }

    /// Sets whether the account is enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// assert!(!UserDetails::new("a", "h", []).enabled(false).is_enabled());
    /// ```
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets whether the account is locked.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// assert!(UserDetails::new("a", "h", []).locked(true).is_locked());
    /// ```
    #[must_use]
    pub fn locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }

    /// The username.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// assert_eq!(UserDetails::new("alice", "h", []).username(), "alice");
    /// ```
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The stored password hash.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// assert_eq!(UserDetails::new("a", "hash", []).password(), "hash");
    /// ```
    pub fn password(&self) -> &str {
        &self.password
    }

    /// The granted authorities.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// assert!(UserDetails::new("a", "h", []).authorities().is_empty());
    /// ```
    pub fn authorities(&self) -> &[GrantedAuthority] {
        &self.authorities
    }

    /// Whether the account is enabled.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// assert!(UserDetails::new("a", "h", []).is_enabled());
    /// ```
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether the account is locked.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetails;
    ///
    /// assert!(!UserDetails::new("a", "h", []).is_locked());
    /// ```
    pub fn is_locked(&self) -> bool {
        self.locked
    }
}

impl fmt::Debug for UserDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserDetails")
            .field("username", &self.username)
            .field("password", &format_args!("[REDACTED]"))
            .field(
                "authorities",
                &self
                    .authorities
                    .iter()
                    .map(GrantedAuthority::as_str)
                    .collect::<Vec<_>>(),
            )
            .field("enabled", &self.enabled)
            .field("locked", &self.locked)
            .finish()
    }
}

/// A [`UserDetailsService`] backend failed. The message is chosen by the service author.
///
/// # Examples
///
/// ```
/// use oxbloom_security::UserDetailsError;
///
/// let err = UserDetailsError::new("database unreachable");
/// assert_eq!(err.to_string(), "database unreachable");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDetailsError {
    message: String,
}

impl UserDetailsError {
    /// Creates an error. Do not put a username or a password in the message.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::UserDetailsError;
    ///
    /// assert_eq!(UserDetailsError::new("down").to_string(), "down");
    /// ```
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for UserDetailsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UserDetailsError {}

/// Loads users by username.
///
/// Return `Ok(None)` for an unknown user and `Err` only when the backend failed.
///
/// # Examples
///
/// ```
/// use async_trait::async_trait;
/// use oxbloom_security::{UserDetails, UserDetailsError, UserDetailsService};
///
/// struct OnlyAlice;
///
/// #[async_trait]
/// impl UserDetailsService for OnlyAlice {
///     async fn load_user_by_username(
///         &self,
///         username: &str,
///     ) -> Result<Option<UserDetails>, UserDetailsError> {
///         Ok((username == "alice").then(|| UserDetails::new("alice", "hash", [])))
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// assert!(OnlyAlice.load_user_by_username("alice").await.unwrap().is_some());
/// assert!(OnlyAlice.load_user_by_username("bob").await.unwrap().is_none());
/// # }
/// ```
#[async_trait]
pub trait UserDetailsService: Send + Sync {
    /// Loads the user with exactly this username, or `None` when there is none.
    async fn load_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError>;
}

/// An [`InMemoryUserDetailsService`] could not be built.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{InMemoryUserDetailsService, UserDetails, UserStoreError};
///
/// let err = InMemoryUserDetailsService::new(vec![UserDetails::new("", "h", [])]).unwrap_err();
/// assert_eq!(err, UserStoreError::EmptyUsername);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStoreError {
    /// Two users have the same username.
    DuplicateUsername,
    /// A user has an empty username.
    EmptyUsername,
}

impl fmt::Display for UserStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DuplicateUsername => "two users share a username",
            Self::EmptyUsername => "a user has an empty username",
        })
    }
}

impl std::error::Error for UserStoreError {}

/// A fixed set of users held in memory. Usernames are compared exactly, so `Alice` is not `alice`.
///
/// # Examples
///
/// ```
/// use oxbloom_security::{InMemoryUserDetailsService, UserDetails, UserDetailsService};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let users = InMemoryUserDetailsService::new(vec![UserDetails::new("alice", "hash", [])])?;
/// assert!(users.load_user_by_username("alice").await?.is_some());
/// assert!(users.load_user_by_username("Alice").await?.is_none());
/// # Ok(())
/// # }
/// ```
pub struct InMemoryUserDetailsService {
    users: HashMap<String, UserDetails>,
}

impl InMemoryUserDetailsService {
    /// Builds the store, rejecting empty and duplicate usernames.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_security::{InMemoryUserDetailsService, UserDetails, UserStoreError};
    ///
    /// let twice = vec![UserDetails::new("a", "h", []), UserDetails::new("a", "h", [])];
    /// assert_eq!(
    ///     InMemoryUserDetailsService::new(twice).err(),
    ///     Some(UserStoreError::DuplicateUsername)
    /// );
    /// ```
    pub fn new(users: Vec<UserDetails>) -> Result<Self, UserStoreError> {
        let mut map = HashMap::with_capacity(users.len());
        for user in users {
            if user.username().is_empty() {
                return Err(UserStoreError::EmptyUsername);
            }
            if map.insert(user.username().to_owned(), user).is_some() {
                return Err(UserStoreError::DuplicateUsername);
            }
        }
        Ok(Self { users: map })
    }
}

impl fmt::Debug for InMemoryUserDetailsService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryUserDetailsService")
            .field("users", &self.users.len())
            .finish()
    }
}

#[async_trait]
impl UserDetailsService for InMemoryUserDetailsService {
    async fn load_user_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserDetails>, UserDetailsError> {
        Ok(self.users.get(username).cloned())
    }
}
