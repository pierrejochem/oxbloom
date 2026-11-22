//! Sessions: the per-request [`Session`] handle, the [`SessionStore`] trait with its in-memory
//! default, the stateless [`CookieSessionBackend`], settings, errors and the cookie that carries
//! the session.
//!
//! Session ids, values, keys and cookie values never appear in an error text or in `Debug`
//! output.
//!
//! # Examples
//!
//! ```
//! use std::time::Duration;
//! use oxbloom_security::{SameSite, SecureMode, SessionSettings};
//!
//! let settings = SessionSettings::default()
//!     .secure(SecureMode::Always)
//!     .same_site(SameSite::Strict)
//!     .idle_timeout(Duration::from_secs(600));
//! assert!(settings.validate().is_ok());
//! ```

mod backend;
mod config;
mod cookie;
mod cookie_backend;
mod error;
mod filter;
pub(crate) mod handle;
mod management_properties;
mod memory;
mod principal;
mod settings;
mod store;

pub use backend::SessionBackend;
pub use config::{SESSION_OLD_SECRETS_KEY, SESSION_SECRET_KEY, SessionConfig, StoreKind};
pub(crate) use cookie::{CookieScope, build_set_cookie, parse_cookie_header};
pub use cookie_backend::CookieSessionBackend;
pub use error::SessionError;
pub use filter::{RequestSessionExt, SESSION_ORDER, SessionFilter};
pub use handle::Session;
pub use management_properties::{SessionManagement, SessionManagementProperties};
pub use memory::InMemorySessionStore;
pub(crate) use principal::PRINCIPAL_KEY;
pub use principal::{PrincipalSessions, SessionRegistry, SessionSummary};
pub(crate) use settings::is_valid_cookie_name;
pub use settings::{SameSite, SecureMode, SessionSettings};
pub use store::{SessionData, SessionStore};
