//! Server-side sessions: settings, errors and the cookie that carries the session id.
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

mod cookie;
mod error;
mod settings;

pub use error::SessionError;
pub use settings::{SameSite, SecureMode, SessionSettings};
