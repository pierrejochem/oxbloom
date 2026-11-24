//! The token endpoint: the authorization code exchange and the token response.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use oxbloom_config::Secret;
use oxbloom_http_client::{Body, HttpClient, Request, Response};
use serde::Deserialize;
use serde_json::Value;

use super::error::allowed_code;
use super::{
    ClientAuthentication, ClientRegistration, MAX_BODY_BYTES, OAuth2Error, ProviderMetadata,
};

/// A successful answer of the token endpoint. `Debug` shows the token type, the expiry and
/// the scope, never a token.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Secret;
/// use oxbloom_security::__test_support::TokenResponse;
///
/// let tokens = TokenResponse {
///     access_token: Secret::new("at-123".to_owned()),
///     token_type: "Bearer".into(),
///     expires_in: Some(3600),
///     refresh_token: None,
///     id_token: None,
///     scope: Some("openid email".into()),
/// };
/// assert!(!format!("{tokens:?}").contains("at-123"));
/// ```
#[derive(Clone)]
pub struct TokenResponse {
    /// The access token.
    pub access_token: Secret<String>,
    /// The token type, `Bearer` in any case.
    pub token_type: String,
    /// The lifetime of the access token in seconds, when the provider sent it, capped at ten
    /// years so that adding it to a time cannot overflow.
    pub expires_in: Option<u64>,
    /// The refresh token, when the provider sent one.
    pub refresh_token: Option<Secret<String>>,
    /// The id token of an OpenID Connect provider.
    pub id_token: Option<Secret<String>>,
    /// The granted scopes, space-separated, when the provider sent them.
    pub scope: Option<String>,
}

impl TokenResponse {
    /// When the access token expires, counted from `now`: `None` when the provider sent no
    /// lifetime, or when the result would not fit in a `SystemTime`. It never panics, however
    /// large `expires_in` is.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::{Duration, SystemTime};
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::__test_support::TokenResponse;
    ///
    /// let tokens = TokenResponse {
    ///     access_token: Secret::new("at".to_owned()),
    ///     token_type: "Bearer".into(),
    ///     expires_in: Some(3600),
    ///     refresh_token: None,
    ///     id_token: None,
    ///     scope: None,
    /// };
    /// let now = SystemTime::now();
    /// assert_eq!(tokens.expires_at(now), Some(now + Duration::from_secs(3600)));
    /// ```
    #[must_use]
    pub fn expires_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.expires_in
            .and_then(|secs| now.checked_add(Duration::from_secs(secs.min(MAX_EXPIRES_IN))))
    }
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"[REDACTED]")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[REDACTED]"))
            .field("scope", &self.scope)
            .finish()
    }
}

/// The largest accepted `expires_in`: ten years, in seconds.
const MAX_EXPIRES_IN: u64 = 10 * 365 * 24 * 3600;

#[derive(Deserialize)]
struct RawTokenResponse {
    access_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<Value>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
}

#[derive(Deserialize)]
struct RawError {
    error: String,
}

/// Calls a provider's token endpoint through an [`HttpClient`].
///
/// # Examples
///
/// ```
/// use std::time::SystemTime;
/// use oxbloom_config::Secret;
/// use oxbloom_security::{ClientAuthentication, ClientRegistration, ProviderConfig};
/// use oxbloom_security::__test_support::{ProviderMetadata, TokenEndpoint};
/// use oxbloom_test::{FakeProvider, FakeUser};
///
/// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
/// let fake = FakeProvider::new("https://idp.example.test");
/// let provider = ProviderConfig {
///     issuer_uri: Some("https://idp.example.test".into()),
///     ..ProviderConfig::default()
/// };
/// let metadata = ProviderMetadata::resolve(&provider, fake.client().as_ref(), SystemTime::now()).await?;
/// let registration = ClientRegistration {
///     id: "idp".into(),
///     client_id: "app".into(),
///     client_secret: Some(Secret::new("app-secret".into())),
///     scopes: vec!["openid".into()],
///     redirect_uri: "https://app.example/login/oauth2/code/idp".into(),
///     client_authentication: ClientAuthentication::Basic,
///     provider: "idp".into(),
/// };
/// let code = fake.issue_code(FakeUser::new("alice"));
///
/// let tokens = TokenEndpoint::new(fake.client())
///     .exchange_code(
///         &registration,
///         &metadata,
///         &code,
///         "https://app.example/login/oauth2/code/idp",
///         "the-pkce-verifier-of-the-pending-login-0123456",
///     )
///     .await?;
/// assert_eq!(tokens.token_type, "Bearer");
/// # Ok::<(), oxbloom_security::__test_support::OAuth2Error>(())
/// # })?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone)]
pub struct TokenEndpoint {
    client: Arc<dyn HttpClient>,
}

impl TokenEndpoint {
    /// A token endpoint caller that sends its requests through `client`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_security::__test_support::TokenEndpoint;
    /// use oxbloom_test::MockHttpClient;
    ///
    /// let endpoint = TokenEndpoint::new(Arc::new(MockHttpClient::new()));
    /// # let _ = endpoint;
    /// ```
    #[must_use]
    pub fn new(client: Arc<dyn HttpClient>) -> TokenEndpoint {
        TokenEndpoint { client }
    }

    /// Exchanges an authorization code for tokens.
    ///
    /// Sends `POST` to the token endpoint with the form fields `grant_type=authorization_code`,
    /// `code`, `redirect_uri` and `code_verifier`, and `Accept: application/json`. The client
    /// authenticates as its registration says: [`ClientAuthentication::Basic`] sends
    /// `Authorization: Basic` with the form-URL-encoded id and secret (RFC 6749 section
    /// 2.3.1), [`ClientAuthentication::Post`] sends `client_id` and `client_secret` as form
    /// fields, and [`ClientAuthentication::None`] sends `client_id` only.
    ///
    /// # Errors
    ///
    /// [`OAuth2Error::Http`] when the request fails; [`OAuth2Error::Provider`] for a non-2xx
    /// answer with a JSON `error`, holding the code when it is one of the RFC 6749 codes and
    /// `unknown` otherwise (the `error_description` is never read);
    /// [`OAuth2Error::Token`] for any other non-2xx answer, a body over 1 MiB, a body without
    /// an access token, or a token type other than `Bearer`; [`OAuth2Error::Config`] when the
    /// registration needs a secret and has none.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::SystemTime;
    /// use oxbloom_config::Secret;
    /// use oxbloom_security::{ClientAuthentication, ClientRegistration, ProviderConfig};
    /// use oxbloom_security::__test_support::{OAuth2Error, ProviderMetadata, TokenEndpoint};
    /// use oxbloom_test::FakeProvider;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
    /// let fake = FakeProvider::new("https://idp.example.test");
    /// let provider = ProviderConfig {
    ///     issuer_uri: Some("https://idp.example.test".into()),
    ///     ..ProviderConfig::default()
    /// };
    /// let metadata = ProviderMetadata::resolve(&provider, fake.client().as_ref(), SystemTime::now()).await?;
    /// let registration = ClientRegistration {
    ///     id: "idp".into(),
    ///     client_id: "spa".into(),
    ///     client_secret: None,
    ///     scopes: vec!["openid".into()],
    ///     redirect_uri: "https://app.example/cb".into(),
    ///     client_authentication: ClientAuthentication::None,
    ///     provider: "idp".into(),
    /// };
    /// // A code the provider never issued.
    /// let err = TokenEndpoint::new(fake.client())
    ///     .exchange_code(&registration, &metadata, "made-up", "https://app.example/cb", "v")
    ///     .await
    ///     .unwrap_err();
    /// assert_eq!(err, OAuth2Error::Provider("invalid_grant"));
    /// # Ok::<(), OAuth2Error>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn exchange_code(
        &self,
        registration: &ClientRegistration,
        metadata: &ProviderMetadata,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
    ) -> Result<TokenResponse, OAuth2Error> {
        let form = vec![
            ("grant_type".to_owned(), "authorization_code".to_owned()),
            ("code".to_owned(), code.to_owned()),
            ("redirect_uri".to_owned(), redirect_uri.to_owned()),
            ("code_verifier".to_owned(), verifier.to_owned()),
        ];
        self.request(registration, &metadata.token_endpoint, form)
            .await
    }

    /// Sends a token request with `form` plus the client authentication, and reads the answer.
    async fn request(
        &self,
        registration: &ClientRegistration,
        token_endpoint: &str,
        mut form: Vec<(String, String)>,
    ) -> Result<TokenResponse, OAuth2Error> {
        let mut request = Request::post(token_endpoint).header("accept", "application/json");
        match registration.client_authentication {
            ClientAuthentication::Basic => {
                let secret = registration
                    .client_secret
                    .as_ref()
                    .ok_or(OAuth2Error::Config)?;
                let credentials = format!(
                    "{}:{}",
                    form_urlencode(&registration.client_id),
                    form_urlencode(secret.expose())
                );
                let value = format!("Basic {}", STANDARD.encode(credentials));
                request = request.header("authorization", &value);
            }
            ClientAuthentication::Post => {
                let secret = registration
                    .client_secret
                    .as_ref()
                    .ok_or(OAuth2Error::Config)?;
                form.push(("client_id".to_owned(), registration.client_id.clone()));
                form.push(("client_secret".to_owned(), secret.expose().clone()));
            }
            ClientAuthentication::None => {
                form.push(("client_id".to_owned(), registration.client_id.clone()));
            }
        }
        let response = self
            .client
            .execute(request.body(Body::Form(form)))
            .await
            .map_err(|_| OAuth2Error::Http)?;
        read_token_response(&response)
    }
}

impl fmt::Debug for TokenEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenEndpoint").finish_non_exhaustive()
    }
}

/// Reads a token endpoint answer: a token response, an allow-listed error code, or a failure.
fn read_token_response(response: &Response) -> Result<TokenResponse, OAuth2Error> {
    if response.bytes().len() > MAX_BODY_BYTES {
        return Err(OAuth2Error::Token);
    }
    if !response.status().is_success() {
        return match serde_json::from_slice::<RawError>(response.bytes()) {
            Ok(error) => Err(OAuth2Error::Provider(allowed_code(&error.error))),
            Err(_) => Err(OAuth2Error::Token),
        };
    }
    let raw: RawTokenResponse =
        serde_json::from_slice(response.bytes()).map_err(|_| OAuth2Error::Token)?;
    let access_token = raw
        .access_token
        .filter(|token| !token.is_empty())
        .ok_or(OAuth2Error::Token)?;
    let token_type = raw
        .token_type
        .filter(|t| t.eq_ignore_ascii_case("bearer"))
        .ok_or(OAuth2Error::Token)?;
    // Some providers send the lifetime as a string; anything else that is not a whole number
    // of seconds is ignored.
    let expires_in = match raw.expires_in {
        Some(Value::Number(n)) => n.as_u64(),
        Some(Value::String(s)) => s.parse().ok(),
        _ => None,
    }
    .map(|secs: u64| secs.min(MAX_EXPIRES_IN));
    Ok(TokenResponse {
        access_token: Secret::new(access_token),
        token_type,
        expires_in,
        refresh_token: raw.refresh_token.map(Secret::new),
        id_token: raw.id_token.map(Secret::new),
        scope: raw.scope,
    })
}

/// Encodes `value` as `application/x-www-form-urlencoded`: ASCII letters, digits and `*-._`
/// stay, a space becomes `+`, every other byte becomes `%XX`.
fn form_urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_urlencoding_follows_the_whatwg_serializer() {
        assert_eq!(form_urlencode("my client:1"), "my+client%3A1");
        assert_eq!(form_urlencode("a*b-c.d_e~"), "a*b-c.d_e%7E");
        assert_eq!(form_urlencode("ä"), "%C3%A4");
        assert_eq!(form_urlencode("%+&="), "%25%2B%26%3D");
    }
}
