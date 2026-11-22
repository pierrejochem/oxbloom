//! Form login: the settings and the pure helpers (redirect-target rule, same-origin check, form
//! parser and the default login page).
//!
//! No username, password, session id or saved path appears in an error text or in `Debug` output.
//!
//! # Examples
//!
//! ```
//! use oxbloom_security::{FormLogin, is_safe_redirect_target};
//!
//! let settings = FormLogin::default().login_page("/signin").login_processing_url("/signin");
//! assert!(settings.validate().is_ok());
//! assert!(is_safe_redirect_target("/orders?page=2"));
//! assert!(!is_safe_redirect_target("//evil.example"));
//! ```

mod config;
mod form;
mod handlers;
mod origin;
mod page;
mod redirect;
mod repository;
mod settings;

pub use config::FormLoginConfig;
pub(crate) use handlers::{FormLoginEntryPoint, FormLoginHandlers};
pub use redirect::is_safe_redirect_target;
pub use settings::FormLogin;
