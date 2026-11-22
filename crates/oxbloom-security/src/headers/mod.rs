//! Security response headers: the settings and the function that adds them to a response.
//!
//! Every response that leaves the security chain gets `X-Content-Type-Options`,
//! `Referrer-Policy` and `X-Frame-Options`; an authenticated response also gets
//! `Cache-Control: no-store`, and a response to an HTTPS request gets
//! `Strict-Transport-Security`. A header the handler already set is never changed.

mod apply;
mod config;

pub(crate) use apply::apply_headers;
pub use config::{HeadersConfig, HstsConfig};
