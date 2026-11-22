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

mod form;
mod origin;
mod page;
mod redirect;
mod repository;
mod settings;

// Consumed by the form login filter.
#[allow(unused_imports)]
pub(crate) use form::parse_login_form;
#[allow(unused_imports)]
pub(crate) use origin::same_origin;
#[allow(unused_imports)]
pub(crate) use page::{escape_html, login_page};
// Consumed by the form login filter.
pub use redirect::is_safe_redirect_target;
#[allow(unused_imports)]
pub(crate) use repository::{
    AUTHENTICATION_KEY, SAVED_REQUEST_KEY, load_authentication, save_request, store_authentication,
    take_saved_request,
};
pub use settings::FormLogin;
