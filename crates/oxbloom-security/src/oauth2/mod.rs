//! OAuth2 and OpenID Connect client support (feature `oauth2`).
//!
//! This module holds the registration model and its configuration keys,
//! `security.oauth2.client.registration.<id>.*` and `security.oauth2.client.provider.<id>.*`,
//! and the steps of a login: provider metadata, the token exchange, id token validation, the
//! user info and the mapping to an `Authentication`, and the login endpoints of the security
//! chain.

mod authorization;
mod error;
mod id_token;
mod login;
mod metadata;
mod pending;
mod pkce;
mod registration;
mod settings;
mod stored;
mod token;
mod user;

pub use error::OAuth2Error;
pub use id_token::{IdTokenClaims, IdTokenValidator};
pub use login::OAuth2Login;
pub(crate) use login::OAuth2LoginHandlers;
pub use metadata::{MetadataCache, ProviderMetadata};
pub use registration::{ClientAuthentication, ClientRegistration, ProviderConfig};
pub use settings::OAuth2Settings;
pub use token::{TokenEndpoint, TokenResponse};
pub(crate) use user::{
    PRINCIPAL_KEY as OAUTH2_PRINCIPAL_KEY, REGISTRATION_KEY as OAUTH2_REGISTRATION_KEY,
};
pub use user::{UserInfo, authentication_from};

/// The largest provider answer read: 1 MiB, as for a JWKS.
pub(crate) const MAX_BODY_BYTES: usize = 1_048_576;

/// The timeout of every provider call: discovery, the key set, the token exchange and the user
/// info. It is set on each request, so it caps the timeout of the shared `HttpClient` bean
/// (`http.client.timeout`) for these calls.
pub(crate) const PROVIDER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Compares two secrets (a nonce, a state) in constant time with respect to their content.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    aws_lc_rs::constant_time::verify_slices_are_equal(a.as_bytes(), b.as_bytes()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn constant_time_eq_compares_content_and_length() {
        assert!(constant_time_eq("nonce-1", "nonce-1"));
        assert!(constant_time_eq("", ""));
        assert!(!constant_time_eq("nonce-1", "nonce-2"));
        assert!(!constant_time_eq("nonce", "nonce-1"));
        assert!(!constant_time_eq("", "x"));
    }
}
