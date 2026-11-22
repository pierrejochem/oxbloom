//! Remember-me: a signed cookie that logs a user in again after the session is gone.
//!
//! The cookie holds `v1.<base64url(username)>.<expiry seconds>.<base64url(HMAC-SHA256)>`. The
//! MAC covers the username, the expiry and the stored password hash, under a key derived from
//! the remember-me secret alone. No secret, token or username appears in a log line, an error
//! text or `Debug` output.

mod settings;
mod token;

pub use settings::RememberMe;
#[allow(
    unused_imports,
    reason = "used by the cookie and the auto login of the chain"
)]
pub(crate) use token::{ParsedToken, TokenCodec};
