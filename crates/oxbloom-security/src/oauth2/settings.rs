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

pub(crate) fn is_valid_id(id: &str) -> bool {
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

/// An absolute `https` URL with a host and no credentials, or `http` for a loopback host,
/// without a fragment and with a valid port. `allow_query` is `false` for an issuer.
pub(crate) fn is_acceptable_endpoint(value: &str, allow_query: bool) -> bool {
    if value.contains('#') {
        return false;
    }
    let Ok(uri) = value.parse::<http::Uri>() else {
        return false;
    };
    if !allow_query && (uri.query().is_some() || value.contains('?')) {
        return false;
    }
    let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
        return false;
    };
    let host = authority.host();
    if host.is_empty() || authority.as_str().contains('@') {
        return false;
    }
    if let Some(rest) = authority
        .as_str()
        .strip_prefix(host)
        .filter(|r| !r.is_empty())
    {
        let valid_port = rest
            .strip_prefix(':')
            .is_some_and(|p| !p.is_empty() && p.parse::<u16>().is_ok());
        if !valid_port {
            return false;
        }
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => is_loopback_host(host),
        _ => false,
    }
}

/// Only `{base-url}` and `{id}` may appear in a redirect template.
fn redirect_template_is_valid(template: &str, id: &str) -> bool {
    let mut rest = template;
    let mut out = String::new();
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(t) = tail.strip_prefix("{base-url}") {
            out.push_str("https://app.example");
            rest = t;
        } else if let Some(t) = tail.strip_prefix("{id}") {
            out.push_str(id);
            rest = t;
        } else {
            return false;
        }
    }
    out.push_str(rest);
    is_acceptable_endpoint(&out, true)
}

/// RFC 6749 section 3.3: `%x21 / %x23-5B / %x5D-7E`.
pub(crate) fn is_scope_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|b| b == 0x21 || (0x23..=0x5B).contains(&b) || (0x5D..=0x7E).contains(&b))
}

/// Which part of the configuration a [`ShapeError`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShapeScope {
    Registration,
    Provider,
}

/// A registration or a provider with the wrong shape: the scope, the id, the key below it
/// (`None` for the id itself) and a fixed message. It never holds a configured value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShapeError {
    pub(crate) scope: ShapeScope,
    pub(crate) id: String,
    pub(crate) field: Option<&'static str>,
    pub(crate) message: &'static str,
}

impl ShapeError {
    fn registration(id: &str, field: Option<&'static str>, message: &'static str) -> Self {
        Self {
            scope: ShapeScope::Registration,
            id: id.to_owned(),
            field,
            message,
        }
    }

    fn provider(id: &str, field: Option<&'static str>, message: &'static str) -> Self {
        Self {
            scope: ShapeScope::Provider,
            id: id.to_owned(),
            field,
            message,
        }
    }

    /// The full configuration key, such as
    /// `security.oauth2.client.provider.github.user-info-uri`.
    pub(crate) fn key(&self) -> String {
        let prefix = match self.scope {
            ShapeScope::Registration => REGISTRATIONS,
            ShapeScope::Provider => PROVIDERS,
        };
        match self.field {
            Some(field) => format!("{prefix}.{}.{field}", self.id),
            None => format!("{prefix}.{}", self.id),
        }
    }
}

impl From<ShapeError> for ConfigError {
    fn from(error: ShapeError) -> Self {
        ConfigError::invalid(error.key(), error.message)
    }
}

const ENDPOINT_MESSAGE: &str = "must be an absolute https URL without credentials or fragment, \
     and for an issuer without a query (http only for localhost, a 127.0.0.0/8 address or ::1)";

/// Checks the shape of a provider, from the configuration or built in code: the id, every
/// endpoint, an `issuer_uri` or both an `authorization_uri` and a `token_uri`, and for a plain
/// OAuth2 provider (no `issuer_uri`, no `jwk_set_uri`) both a `user_info_uri` and a
/// `user_name_attribute`, which its login needs to name the user.
pub(crate) fn validate_provider(id: &str, provider: &ProviderConfig) -> Result<(), ShapeError> {
    if !is_valid_id(id) {
        return Err(ShapeError::provider(
            id,
            None,
            "a provider id may only hold a to z, 0 to 9 and -",
        ));
    }
    let endpoints = [
        ("issuer-uri", &provider.issuer_uri, false),
        ("authorization-uri", &provider.authorization_uri, true),
        ("token-uri", &provider.token_uri, true),
        ("user-info-uri", &provider.user_info_uri, true),
        ("jwk-set-uri", &provider.jwk_set_uri, true),
    ];
    for (field, value, allow_query) in endpoints {
        if let Some(value) = value
            && !is_acceptable_endpoint(value, allow_query)
        {
            return Err(ShapeError::provider(id, Some(field), ENDPOINT_MESSAGE));
        }
    }
    if provider.issuer_uri.is_some() {
        return Ok(());
    }
    if provider.authorization_uri.is_none() && provider.token_uri.is_none() {
        return Err(ShapeError::provider(
            id,
            Some("issuer-uri"),
            "a provider needs an issuer-uri, or an authorization-uri and a token-uri",
        ));
    }
    if provider.authorization_uri.is_none() {
        return Err(ShapeError::provider(
            id,
            Some("authorization-uri"),
            "a provider without an issuer-uri needs an authorization-uri",
        ));
    }
    if provider.token_uri.is_none() {
        return Err(ShapeError::provider(
            id,
            Some("token-uri"),
            "a provider without an issuer-uri needs a token-uri",
        ));
    }
    if provider.jwk_set_uri.is_none() {
        const PLAIN: &str = "a plain OAuth2 provider (no issuer-uri, no jwk-set-uri) needs a \
                             user-info-uri and a user-name-attribute";
        if provider.user_info_uri.is_none() {
            return Err(ShapeError::provider(id, Some("user-info-uri"), PLAIN));
        }
        if provider.user_name_attribute.is_none() {
            return Err(ShapeError::provider(id, Some("user-name-attribute"), PLAIN));
        }
    }
    Ok(())
}

/// Checks that a registration id may appear in the login and callback paths.
fn validate_registration_id(id: &str) -> Result<(), ShapeError> {
    if is_valid_id(id) {
        Ok(())
    } else {
        Err(ShapeError::registration(
            id,
            None,
            "a registration id may only hold a to z, 0 to 9 and -",
        ))
    }
}

/// Checks the shape of a registration, from the configuration or built in code: the id, a
/// non-empty client id, the scope tokens, and a non-empty client secret unless the client
/// authentication is [`ClientAuthentication::None`]. Whether its provider exists, and its
/// redirect URI, are checked by the callers, which know the providers and the base URL.
pub(crate) fn validate_registration(registration: &ClientRegistration) -> Result<(), ShapeError> {
    let id = registration.id.as_str();
    validate_registration_id(id)?;
    if registration.client_id.is_empty() {
        return Err(ShapeError::registration(
            id,
            Some("client-id"),
            "a client id is required",
        ));
    }
    if !registration.scopes.iter().all(|t| is_scope_token(t)) {
        return Err(ShapeError::registration(
            id,
            Some("scope"),
            "a scope may only hold printable ASCII without space, double quote and backslash, \
             and may not be empty",
        ));
    }
    let has_secret = registration
        .client_secret
        .as_ref()
        .is_some_and(|s| !s.expose().is_empty());
    if !has_secret && registration.client_authentication != ClientAuthentication::None {
        return Err(ShapeError::registration(
            id,
            Some("client-secret"),
            "a client secret is required unless client-authentication is none",
        ));
    }
    Ok(())
}

fn read_provider(id: &str, raw: RawProvider) -> Result<ProviderConfig, ConfigError> {
    let provider = ProviderConfig {
        issuer_uri: raw.issuer_uri,
        authorization_uri: raw.authorization_uri,
        token_uri: raw.token_uri,
        user_info_uri: raw.user_info_uri,
        jwk_set_uri: raw.jwk_set_uri,
        user_name_attribute: raw.user_name_attribute,
    };
    validate_provider(id, &provider)?;
    Ok(provider)
}

fn read_registration(
    id: &str,
    raw: RawRegistration,
    providers: &BTreeMap<String, ProviderConfig>,
) -> Result<ClientRegistration, ConfigError> {
    let key = |name: &str| format!("{REGISTRATIONS}.{id}.{name}");
    validate_registration_id(id)?;
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
    let registration = ClientRegistration {
        id: id.to_owned(),
        client_id: raw.client_id.unwrap_or_default(),
        client_secret: raw.client_secret.filter(|s| !s.expose().is_empty()),
        scopes: raw.scope.unwrap_or_default(),
        redirect_uri: raw
            .redirect_uri
            .unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_owned()),
        client_authentication,
        provider: raw.provider.unwrap_or_else(|| id.to_owned()),
    };
    validate_registration(&registration)?;
    if !redirect_template_is_valid(&registration.redirect_uri, id) {
        return Err(ConfigError::invalid(
            key("redirect-uri"),
            "must be an absolute https URL (http only for a loopback host) without credentials \
             or fragment; only {base-url} and {id} may appear in braces",
        ));
    }
    if !providers.contains_key(&registration.provider) {
        return Err(ConfigError::invalid(
            key("provider"),
            format!("no provider is configured under {PROVIDERS}.<provider>"),
        ));
    }
    Ok(registration)
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
    /// `jwk-set-uri`) without both `user-info-uri` and `user-name-attribute`; an endpoint
    /// that is not an absolute `https` URL unless its host is loopback; a registration id
    /// or provider id outside `[a-z0-9-]+`; an invalid redirect URI or scope; a fragment, a bad
    /// port or (on `issuer-uri`) a query in an endpoint; an unknown key. Relaxed binding
    /// applies: `client_id` and `clientid` normalize to `client-id` and are accepted.
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
