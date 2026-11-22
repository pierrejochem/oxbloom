//! CSRF protection: the settings, the token type, the reader for the submitted token, the token
//! in the session and the check.
//!
//! The synchronizer token is a random value kept in the session. A state-changing request must
//! send the same value back in a header or in a form field. The token never appears in a log
//! line, an error text, a redirect, a URL or `Debug` output.

mod check;
mod properties;
mod reader;
mod settings;
mod store;
mod token;

pub(crate) use check::{hide_token_header, request_passes};
pub use properties::CsrfProperties;
pub(crate) use reader::submitted_token;
pub use settings::CsrfConfig;
pub(crate) use store::{ensure_token, replace_token};
pub use token::CsrfToken;
