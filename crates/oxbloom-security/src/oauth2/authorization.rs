//! The authorization request URL.

use super::ClientRegistration;

/// Builds the URL the browser is sent to at the provider's authorization endpoint. Every
/// parameter is percent-encoded; an existing query on the endpoint is kept.
pub(crate) fn authorization_url(
    authorization_endpoint: &str,
    registration: &ClientRegistration,
    redirect_uri: &str,
    scopes: &[String],
    state: &str,
    nonce: Option<&str>,
    challenge: &str,
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("response_type", "code".to_owned()),
        ("client_id", registration.client_id.clone()),
        ("redirect_uri", redirect_uri.to_owned()),
        ("scope", scopes.join(" ")),
        ("state", state.to_owned()),
        ("code_challenge", challenge.to_owned()),
        ("code_challenge_method", "S256".to_owned()),
    ];
    if let Some(nonce) = nonce {
        params.push(("nonce", nonce.to_owned()));
    }
    let mut url = authorization_endpoint.to_owned();
    url.push(if authorization_endpoint.contains('?') {
        '&'
    } else {
        '?'
    });
    for (i, (name, value)) in params.iter().enumerate() {
        if i > 0 {
            url.push('&');
        }
        url.push_str(name);
        url.push('=');
        url.push_str(&percent_encode(value));
    }
    url
}

/// Percent-encodes everything outside the RFC 3986 unreserved set, byte by byte.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientAuthentication;

    fn registration() -> ClientRegistration {
        ClientRegistration {
            id: "idp".into(),
            client_id: "my client&id".into(),
            client_secret: None,
            scopes: vec![],
            redirect_uri: String::new(),
            client_authentication: ClientAuthentication::None,
            provider: "idp".into(),
        }
    }

    #[test]
    fn authorization_url_encodes_every_parameter() {
        let url = authorization_url(
            "https://idp.example/authorize",
            &registration(),
            "https://app.example/cb?a=1&b=2",
            &["openid".to_owned(), "read all".to_owned()],
            "st+ate",
            Some("no nce"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        );
        assert_eq!(
            url,
            "https://idp.example/authorize?response_type=code&client_id=my%20client%26id\
             &redirect_uri=https%3A%2F%2Fapp.example%2Fcb%3Fa%3D1%26b%3D2\
             &scope=openid%20read%20all&state=st%2Bate\
             &code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM\
             &code_challenge_method=S256&nonce=no%20nce"
        );
    }

    #[test]
    fn nonce_is_omitted_when_absent() {
        let url = authorization_url(
            "https://idp.example/a",
            &registration(),
            "https://x/cb",
            &[],
            "s",
            None,
            "c",
        );
        assert!(!url.contains("nonce"));
    }

    #[test]
    fn an_endpoint_with_a_query_gets_an_ampersand() {
        let url = authorization_url(
            "https://idp.example/a?tenant=1",
            &registration(),
            "https://x/cb",
            &[],
            "s",
            None,
            "c",
        );
        assert!(url.starts_with("https://idp.example/a?tenant=1&response_type=code&"));
    }

    #[test]
    fn non_ascii_is_encoded_as_utf8_bytes() {
        let url = authorization_url(
            "https://idp.example/a",
            &registration(),
            "https://x/cb",
            &["é".to_owned()],
            "s",
            None,
            "c",
        );
        assert!(url.contains("scope=%C3%A9"));
    }
}
