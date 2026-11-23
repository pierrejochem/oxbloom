//! Reads `security.oauth2.client.*` into an [`OAuth2Settings`] and validates it.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

use oxbloom_config::{Config, ConfigError, Secret};
use serde::Deserialize;

use super::registration::DEFAULT_REDIRECT_URI;
use super::{ClientAuthentication, ClientRegistration, ProviderConfig};

const REGISTRATIONS: &str = "security.oauth2.client.registration";
const PROVIDERS: &str = "security.oauth2.client.provider";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRegistration {
    #[serde(rename = "client-id")]
    client_id: Option<String>,
    #[serde(rename = "client-secret")]
    client_secret: Option<Secret<String>>,
    scope: Option<Vec<String>>,
    #[serde(rename = "redirect-uri")]
    redirect_uri: Option<String>,
    #[serde(rename = "client-authentication")]
    client_authentication: Option<String>,
    provider: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvider {
    #[serde(rename = "issuer-uri")]
    issuer_uri: Option<String>,
    #[serde(rename = "authorization-uri")]
    authorization_uri: Option<String>,
    #[serde(rename = "token-uri")]
    token_uri: Option<String>,
    #[serde(rename = "user-info-uri")]
    user_info_uri: Option<String>,
    #[serde(rename = "jwk-set-uri")]
    jwk_set_uri: Option<String>,
    #[serde(rename = "user-name-attribute")]
    user_name_attribute: Option<String>,
}

/// The OAuth2 client settings: every registration and every provider.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::{ClientAuthentication, OAuth2Settings};
///
/// let config = Config::from_str(
///     r#"
/// [security.oauth2.client.registration.idp]
/// client-id = "app"
/// client-secret = "hunter2-hunter2"
/// scope = ["openid", "email"]
///
/// [security.oauth2.client.provider.idp]
/// issuer-uri = "https://idp.example"
/// "#,
/// )?;
/// let settings = OAuth2Settings::from_config(&config)?;
/// assert_eq!(settings.registrations[0].client_authentication, ClientAuthentication::Basic);
/// assert_eq!(settings.registrations[0].provider, "idp");
///
/// // Without the section the settings are empty.
/// let none = OAuth2Settings::from_config(&Config::from_str("")?)?;
/// assert!(none.registrations.is_empty());
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Default)]
pub struct OAuth2Settings {
    /// The registrations, ordered by id.
    pub registrations: Vec<ClientRegistration>,
    /// The providers by id.
    pub providers: BTreeMap<String, ProviderConfig>,
}

fn table<T: serde::de::DeserializeOwned>(
    config: &Config,
    prefix: &str,
) -> Result<BTreeMap<String, T>, ConfigError> {
    match config.get::<BTreeMap<String, T>>(prefix) {
        Ok(map) => Ok(map),
        Err(ConfigError::Missing { key }) if key == prefix => Ok(BTreeMap::new()),
        Err(error) => Err(error),
    }
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    if let Ok(v4) = host.parse::<Ipv4Addr>() {
        return v4.octets()[0] == 127;
    }
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .and_then(|h| h.parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_ipv6() && ip.is_loopback())
}

/// An absolute `https` URL with a host and no credentials, or `http` for a loopback host.
fn is_acceptable_endpoint(value: &str) -> bool {
    let Ok(uri) = value.parse::<http::Uri>() else {
        return false;
    };
    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        return false;
    };
    let host = authority.host();
    if host.is_empty() || authority.as_str().contains('@') {
        return false;
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => is_loopback_host(host),
        _ => false,
    }
}

fn check_endpoint(key: &str, value: &Option<String>) -> Result<(), ConfigError> {
    match value {
        Some(v) if !is_acceptable_endpoint(v) => Err(ConfigError::invalid(
            key,
            "must be an absolute https URL without credentials (http only for localhost, a 127.0.0.0/8 address or ::1)",
        )),
        _ => Ok(()),
    }
}

fn read_provider(id: &str, raw: RawProvider) -> Result<ProviderConfig, ConfigError> {
    let key = |name: &str| format!("{PROVIDERS}.{id}.{name}");
    let provider = ProviderConfig {
        issuer_uri: raw.issuer_uri,
        authorization_uri: raw.authorization_uri,
        token_uri: raw.token_uri,
        user_info_uri: raw.user_info_uri,
        jwk_set_uri: raw.jwk_set_uri,
        user_name_attribute: raw.user_name_attribute,
    };
    check_endpoint(&key("issuer-uri"), &provider.issuer_uri)?;
    check_endpoint(&key("authorization-uri"), &provider.authorization_uri)?;
    check_endpoint(&key("token-uri"), &provider.token_uri)?;
    check_endpoint(&key("user-info-uri"), &provider.user_info_uri)?;
    check_endpoint(&key("jwk-set-uri"), &provider.jwk_set_uri)?;
    if provider.issuer_uri.is_none() {
        if provider.authorization_uri.is_none() && provider.token_uri.is_none() {
            return Err(ConfigError::invalid(
                key("issuer-uri"),
                "a provider needs an issuer-uri, or an authorization-uri and a token-uri",
            ));
        }
        if provider.authorization_uri.is_none() {
            return Err(ConfigError::invalid(
                key("authorization-uri"),
                "a provider without an issuer-uri needs an authorization-uri",
            ));
        }
        if provider.token_uri.is_none() {
            return Err(ConfigError::invalid(
                key("token-uri"),
                "a provider without an issuer-uri needs a token-uri",
            ));
        }
        if provider.jwk_set_uri.is_none()
            && provider.user_info_uri.is_none()
            && provider.user_name_attribute.is_none()
        {
            return Err(ConfigError::invalid(
                key("user-info-uri"),
                "a plain OAuth2 provider needs a user-info-uri or a user-name-attribute",
            ));
        }
    }
    Ok(provider)
}

fn read_registration(
    id: &str,
    raw: RawRegistration,
    providers: &BTreeMap<String, ProviderConfig>,
) -> Result<ClientRegistration, ConfigError> {
    let key = |name: &str| format!("{REGISTRATIONS}.{id}.{name}");
    if !is_valid_id(id) {
        return Err(ConfigError::invalid(
            format!("{REGISTRATIONS}.{id}"),
            "a registration id may only hold a to z, 0 to 9 and -",
        ));
    }
    let client_id = raw
        .client_id
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ConfigError::invalid(key("client-id"), "a client id is required"))?;
    let client_authentication = match raw.client_authentication.as_deref() {
        None | Some("basic") => ClientAuthentication::Basic,
        Some("post") => ClientAuthentication::Post,
        Some("none") => ClientAuthentication::None,
        Some(_) => {
            return Err(ConfigError::invalid(
                key("client-authentication"),
                "expected one of basic, post, none",
            ));
        }
    };
    let client_secret = raw.client_secret.filter(|s| !s.expose().is_empty());
    if client_secret.is_none() && client_authentication != ClientAuthentication::None {
        return Err(ConfigError::invalid(
            key("client-secret"),
            "a client secret is required unless client-authentication is none",
        ));
    }
    let provider = raw.provider.unwrap_or_else(|| id.to_owned());
    if !providers.contains_key(&provider) {
        return Err(ConfigError::invalid(
            key("provider"),
            format!("no provider is configured under {PROVIDERS}.<provider>"),
        ));
    }
    Ok(ClientRegistration {
        id: id.to_owned(),
        client_id,
        client_secret,
        scopes: raw.scope.unwrap_or_default(),
        redirect_uri: raw
            .redirect_uri
            .unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_owned()),
        client_authentication,
        provider,
    })
}

impl OAuth2Settings {
    /// Reads and validates `security.oauth2.client.registration.<id>.*` and
    /// `security.oauth2.client.provider.<id>.*`. Without either section the settings are
    /// empty.
    ///
    /// An error is a [`ConfigError::Invalid`] that names the full key and never holds a
    /// value, so a secret cannot reach a log through it.
    ///
    /// # Errors
    ///
    /// A missing `client-id`; a missing `client-secret` unless `client-authentication` is
    /// `none`; an unknown `provider`; a provider with neither an `issuer-uri` nor both an
    /// `authorization-uri` and a `token-uri`; a plain OAuth2 provider (no `issuer-uri`, no
    /// `jwk-set-uri`) with neither `user-info-uri` nor `user-name-attribute`; an endpoint
    /// that is not an absolute `https` URL unless its host is loopback; a registration id
    /// outside `[a-z0-9-]+`; an unknown key.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    /// use oxbloom_security::OAuth2Settings;
    ///
    /// let config = Config::from_str(
    ///     r#"
    /// [security.oauth2.client.registration.idp]
    /// client-id = "app"
    ///
    /// [security.oauth2.client.provider.idp]
    /// issuer-uri = "https://idp.example"
    /// "#,
    /// )?;
    /// match OAuth2Settings::from_config(&config) {
    ///     Err(ConfigError::Invalid { key, .. }) => {
    ///         assert_eq!(key, "security.oauth2.client.registration.idp.client-secret");
    ///     }
    ///     other => panic!("unexpected {other:?}"),
    /// }
    /// # Ok::<(), ConfigError>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let raw_providers: BTreeMap<String, RawProvider> = table(config, PROVIDERS)?;
        let mut providers = BTreeMap::new();
        for (id, raw) in raw_providers {
            let provider = read_provider(&id, raw)?;
            providers.insert(id, provider);
        }
        let raw_registrations: BTreeMap<String, RawRegistration> = table(config, REGISTRATIONS)?;
        let mut registrations = Vec::with_capacity(raw_registrations.len());
        for (id, raw) in raw_registrations {
            registrations.push(read_registration(&id, raw, &providers)?);
        }
        Ok(Self {
            registrations,
            providers,
        })
    }
}
