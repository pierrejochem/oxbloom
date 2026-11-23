//! The registration of an OAuth2 client at a provider, and the provider's endpoints.

use oxbloom_config::Secret;

/// The literal template used when `redirect-uri` is not set.
pub(crate) const DEFAULT_REDIRECT_URI: &str = "{base-url}/login/oauth2/code/{id}";

/// How the client proves its identity at the token endpoint.
///
/// # Examples
///
/// ```
/// use oxbloom_security::ClientAuthentication;
///
/// assert_eq!(ClientAuthentication::default(), ClientAuthentication::Basic);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClientAuthentication {
    /// HTTP Basic with the client id and secret. The default.
    #[default]
    Basic,
    /// The client id and secret as form fields of the token request.
    Post,
    /// No secret: a public client that relies on PKCE.
    None,
}

/// The endpoints and options of one provider. Every field is optional on its own; the
/// settings reader checks that a provider names enough of them.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::OAuth2Settings;
///
/// let config = Config::from_str(
///     "[security.oauth2.client.provider.idp]\nissuer-uri = \"https://idp.example\"\n",
/// )?;
/// let settings = OAuth2Settings::from_config(&config)?;
/// let idp = &settings.providers["idp"];
/// assert_eq!(idp.issuer_uri.as_deref(), Some("https://idp.example"));
/// assert_eq!(idp.token_uri, None);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderConfig {
    /// The OpenID Connect issuer, whose discovery document gives the other endpoints.
    pub issuer_uri: Option<String>,
    /// The authorization endpoint.
    pub authorization_uri: Option<String>,
    /// The token endpoint.
    pub token_uri: Option<String>,
    /// The user-info endpoint.
    pub user_info_uri: Option<String>,
    /// The key set that signs id tokens.
    pub jwk_set_uri: Option<String>,
    /// The attribute of the user info that names the user.
    pub user_name_attribute: Option<String>,
}

/// One client registration. `Debug` never shows the secret.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_security::OAuth2Settings;
///
/// let config = Config::from_str(
///     r#"
/// [security.oauth2.client.registration.idp]
/// client-id = "app"
/// client-secret = "hunter2-hunter2"
///
/// [security.oauth2.client.provider.idp]
/// issuer-uri = "https://idp.example"
/// "#,
/// )?;
/// let registration = &OAuth2Settings::from_config(&config)?.registrations[0];
/// assert_eq!(
///     registration.resolved_redirect_uri("https://app.example"),
///     "https://app.example/login/oauth2/code/idp"
/// );
/// assert!(!format!("{registration:?}").contains("hunter2"));
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Debug, Clone)]
pub struct ClientRegistration {
    /// The registration id, `[a-z0-9-]+`; it appears in the login and callback URLs.
    pub id: String,
    /// The client id issued by the provider.
    pub client_id: String,
    /// The client secret; absent only for [`ClientAuthentication::None`].
    pub client_secret: Option<Secret<String>>,
    /// The scopes to ask for. The flow adds `openid` itself for OIDC providers.
    pub scopes: Vec<String>,
    /// The redirect URI, or the template `{base-url}/login/oauth2/code/{id}`.
    pub redirect_uri: String,
    /// How the client authenticates at the token endpoint.
    pub client_authentication: ClientAuthentication,
    /// The key of the provider in [`OAuth2Settings::providers`](crate::OAuth2Settings).
    pub provider: String,
}

impl ClientRegistration {
    /// Substitutes `{base-url}` (without a trailing slash) and `{id}` in the redirect URI.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_security::OAuth2Settings;
    ///
    /// let config = Config::from_str(
    ///     r#"
    /// [security.oauth2.client.registration.idp]
    /// client-id = "app"
    /// client-authentication = "none"
    ///
    /// [security.oauth2.client.provider.idp]
    /// issuer-uri = "https://idp.example"
    /// "#,
    /// )?;
    /// let r = &OAuth2Settings::from_config(&config)?.registrations[0];
    /// assert_eq!(
    ///     r.resolved_redirect_uri("https://app.example/"),
    ///     "https://app.example/login/oauth2/code/idp"
    /// );
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn resolved_redirect_uri(&self, base_url: &str) -> String {
        self.redirect_uri
            .replace("{base-url}", base_url.trim_end_matches('/'))
            .replace("{id}", &self.id)
    }
}
