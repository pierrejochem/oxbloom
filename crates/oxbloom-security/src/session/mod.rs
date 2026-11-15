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
mod cookie;
mod cookie_backend;
mod error;
mod filter;
mod handle;
mod memory;
mod settings;
mod store;

pub use backend::SessionBackend;
pub use cookie_backend::CookieSessionBackend;
pub use error::SessionError;
pub use filter::{RequestSessionExt, SESSION_ORDER, SessionFilter};
pub use handle::Session;
pub use memory::InMemorySessionStore;
pub use settings::{SameSite, SecureMode, SessionSettings};
pub use store::{SessionData, SessionStore};
