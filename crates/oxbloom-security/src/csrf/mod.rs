//! CSRF protection: the settings, the token type and the reader for the submitted token.
//!
//! The synchronizer token is a random value kept in the session. A state-changing request must
//! send the same value back in a header or in a form field. The token never appears in a log
//! line, an error text, a redirect, a URL or `Debug` output.

mod reader;
mod settings;
mod token;

#[allow(unused_imports)] // Consumed by the chain check in a later task.
pub(crate) use reader::submitted_token;
pub use settings::CsrfConfig;
pub use token::CsrfToken;
