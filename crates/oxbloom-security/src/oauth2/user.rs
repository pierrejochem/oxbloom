//! The user info endpoint and the mapping of user attributes to an [`Authentication`].

use oxbloom_config::Secret;
use oxbloom_http_client::{HttpClient, Request};
use serde_json::{Map, Value};

use super::{ClientRegistration, MAX_BODY_BYTES, OAuth2Error, ProviderConfig};
use crate::{Authentication, GrantedAuthority};

/// The `details` key that holds the registration id of an OAuth2 login.
pub(crate) const REGISTRATION_KEY: &str = "oauth2.registration";
/// The `details` key that marks an OAuth2 principal; revalidation skips such principals.
pub(crate) const PRINCIPAL_KEY: &str = "oauth2.principal";

/// Loads the user attributes from a provider's user-info endpoint.
///
/// # Examples
///
/// ```
/// use http::Method;
/// use oxbloom_config::Secret;
/// use oxbloom_security::__test_support::UserInfo;
/// use oxbloom_test::{MockHttpClient, MockResponse};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let mock = MockHttpClient::new();
/// mock.on(
///     Method::GET,
///     "https://api.example.test/user",
///     MockResponse::status(200).json(&serde_json::json!({"login": "alice"})),
/// );
/// let attributes = UserInfo
///     .fetch("https://api.example.test/user", &Secret::new("access-token".into()), &mock)
///     .await?;
/// assert_eq!(attributes["login"], "alice");
/// # Ok::<(), oxbloom_security::__test_support::OAuth2Error>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct UserInfo;

impl UserInfo {
    /// Sends `GET endpoint` with `Authorization: Bearer <access token>` and
    /// `Accept: application/json`, and returns the JSON object of the answer.
    ///
    /// # Errors
    ///
    /// [`OAuth2Error::Http`] when the request fails; [`OAuth2Error::UserInfo`] for a non-2xx
    /// answer, a body over 1 MiB, or a body that is not a JSON object.
    ///
    /// # Examples
    ///
    /// ```
    /// use http::Method;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::__test_support::{OAuth2Error, UserInfo};
    /// use oxbloom_test::{MockHttpClient, MockResponse};
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let mock = MockHttpClient::new();
    /// mock.on(Method::GET, "https://api.example.test/user", MockResponse::status(401));
    /// let err = UserInfo
    ///     .fetch("https://api.example.test/user", &Secret::new("expired".into()), &mock)
    ///     .await
    ///     .unwrap_err();
    /// assert_eq!(err, OAuth2Error::UserInfo);
    /// # })
    /// # ;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn fetch(
        &self,
        endpoint: &str,
        access_token: &Secret<String>,
        client: &dyn HttpClient,
    ) -> Result<Map<String, Value>, OAuth2Error> {
        let request = Request::get(endpoint)
            .header("accept", "application/json")
            .header(
                "authorization",
                &format!("Bearer {}", access_token.expose()),
            );
        let response = client
            .execute(request)
            .await
            .map_err(|_| OAuth2Error::Http)?;
        if !response.status().is_success() || response.bytes().len() > MAX_BODY_BYTES {
            return Err(OAuth2Error::UserInfo);
        }
        match serde_json::from_slice(response.bytes()) {
            Ok(Value::Object(attributes)) => Ok(attributes),
            _ => Err(OAuth2Error::UserInfo),
        }
    }
}

/// Maps the attributes of a logged-in user to an [`Authentication`].
///
/// The principal is the attribute named by the provider's `user-name-attribute`; for an
/// OpenID Connect login (`oidc`) it defaults to `sub`, for a plain OAuth2 login it is
/// required. The authorities are `OIDC_USER` or `OAUTH2_USER`, plus `SCOPE_<scope>` for each
/// granted scope. The `details` are the attributes, plus `oauth2.registration` with the
/// registration id and `oauth2.principal = true`, which overwrite attributes of the same
/// names; revalidation skips a principal with that marker.
///
/// # Errors
///
/// [`OAuth2Error::Config`] for a plain OAuth2 provider without `user-name-attribute`;
/// [`OAuth2Error::UserInfo`] when the attribute is missing, not a string, or empty.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Secret;
/// use oxbloom_security::{ClientAuthentication, ClientRegistration, ProviderConfig};
/// use oxbloom_security::__test_support::authentication_from;
///
/// let registration = ClientRegistration {
///     id: "github".into(),
///     client_id: "app".into(),
///     client_secret: Some(Secret::new("app-secret".into())),
///     scopes: vec!["read:user".into()],
///     redirect_uri: "https://app.example/login/oauth2/code/github".into(),
///     client_authentication: ClientAuthentication::Basic,
///     provider: "github".into(),
/// };
/// let provider = ProviderConfig {
///     authorization_uri: Some("https://github.example.test/login/oauth/authorize".into()),
///     token_uri: Some("https://github.example.test/login/oauth/access_token".into()),
///     user_info_uri: Some("https://api.github.example.test/user".into()),
///     user_name_attribute: Some("login".into()),
///     ..ProviderConfig::default()
/// };
/// let serde_json::Value::Object(attributes) = serde_json::json!({"login": "alice", "id": 7}) else {
///     unreachable!()
/// };
/// let auth = authentication_from(&registration, &provider, false, attributes, &["read:user".into()])?;
/// assert_eq!(auth.principal(), "alice");
/// assert!(auth.has_authority("OAUTH2_USER"));
/// assert!(auth.has_authority("SCOPE_read:user"));
/// assert_eq!(auth.details()["oauth2.registration"], "github");
/// # Ok::<(), oxbloom_security::__test_support::OAuth2Error>(())
/// ```
pub fn authentication_from(
    registration: &ClientRegistration,
    provider: &ProviderConfig,
    oidc: bool,
    attributes: Map<String, Value>,
    granted_scopes: &[String],
) -> Result<Authentication, OAuth2Error> {
    let attribute = match (&provider.user_name_attribute, oidc) {
        (Some(name), _) => name.as_str(),
        (None, true) => "sub",
        (None, false) => return Err(OAuth2Error::Config),
    };
    let principal = attributes
        .get(attribute)
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or(OAuth2Error::UserInfo)?
        .to_owned();
    let kind = if oidc { "OIDC_USER" } else { "OAUTH2_USER" };
    let authorities = std::iter::once(GrantedAuthority::new(kind)).chain(
        granted_scopes
            .iter()
            .map(|scope| GrantedAuthority::new(format!("SCOPE_{scope}"))),
    );
    let mut details = attributes;
    details.insert(
        REGISTRATION_KEY.to_owned(),
        Value::String(registration.id.clone()),
    );
    details.insert(PRINCIPAL_KEY.to_owned(), Value::Bool(true));
    Ok(Authentication::new(principal, authorities).with_details(details))
}
