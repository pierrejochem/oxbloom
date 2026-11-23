//! OAuth2 and OpenID Connect client support (feature `oauth2`).
//!
//! This module holds the registration model and its configuration keys,
//! `security.oauth2.client.registration.<id>.*` and `security.oauth2.client.provider.<id>.*`.

#[allow(dead_code)]
mod authorization;
#[allow(dead_code)]
mod pending;
#[allow(dead_code)]
mod pkce;
mod registration;
mod settings;

pub use registration::{ClientAuthentication, ClientRegistration, ProviderConfig};
pub use settings::OAuth2Settings;
