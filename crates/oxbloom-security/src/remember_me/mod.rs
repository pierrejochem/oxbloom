//! Remember-me: a signed cookie that logs a user in again after the session is gone.
//!
//! The cookie holds `v1.<base64url(username)>.<expiry seconds>.<base64url(HMAC-SHA256)>`. The
//! MAC covers the username, the expiry and the stored password hash, under a key derived from
//! the remember-me secret alone. No secret, token or username appears in a log line, an error
//! text or `Debug` output.

mod cookie;
mod settings;
mod token;

pub(crate) use cookie::{clear_cookie, issue_cookie};
pub use settings::RememberMe;
pub(crate) use token::TokenCodec;

use crate::http_security::SecurityBuildError;

/// What a built chain keeps of remember-me: the validated settings and the codec of their keys.
/// `Debug` shows neither a secret nor a key.
#[derive(Debug)]
pub(crate) struct RememberMeRuntime {
    pub(crate) settings: RememberMe,
    pub(crate) codec: TokenCodec,
}

impl RememberMeRuntime {
    /// Validates `settings` and derives their keys.
    ///
    /// # Errors
    ///
    /// [`SecurityBuildError::InvalidRememberMe`] when a setting is out of range.
    pub(crate) fn new(settings: RememberMe) -> Result<Self, SecurityBuildError> {
        settings.validate()?;
        let codec = TokenCodec::new(&settings)?;
        Ok(Self { settings, codec })
    }
}
