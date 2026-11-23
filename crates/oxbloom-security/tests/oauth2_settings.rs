#![cfg(feature = "oauth2")]
//! The OAuth2 client registration model and its configuration keys.

use oxbloom_config::{Config, ConfigError};
use oxbloom_security::{ClientAuthentication, OAuth2Settings};

const REG: &str = "security.oauth2.client.registration";
const PROV: &str = "security.oauth2.client.provider";

fn read(toml: &str) -> Result<OAuth2Settings, ConfigError> {
    OAuth2Settings::from_config(&Config::from_str(toml).unwrap())
}

fn invalid_key(toml: &str) -> String {
    match read(toml) {
        Err(ConfigError::Invalid { key, .. }) => key,
        Err(other) => panic!("unexpected error: {other}"),
        Ok(_) => panic!("accepted"),
    }
}

const GOOGLE: &str = r#"
[security.oauth2.client.registration.google]
client-id = "cid"
client-secret = "s3cr3t-value"
scope = ["openid", "email"]
redirect-uri = "https://app.example/cb"
client-authentication = "post"

[security.oauth2.client.provider.google]
issuer-uri = "https://accounts.example.com"
user-name-attribute = "email"
"#;

#[test]
fn reads_a_full_oidc_registration_and_provider() {
    let s = read(GOOGLE).unwrap();
    assert_eq!(s.registrations.len(), 1);
    let r = &s.registrations[0];
    assert_eq!(r.id, "google");
    assert_eq!(r.client_id, "cid");
    assert_eq!(r.client_secret.as_ref().unwrap().expose(), "s3cr3t-value");
    assert_eq!(r.scopes, ["openid", "email"]);
    assert_eq!(r.redirect_uri, "https://app.example/cb");
    assert_eq!(r.client_authentication, ClientAuthentication::Post);
    assert_eq!(r.provider, "google");
    let p = &s.providers["google"];
    assert_eq!(
        p.issuer_uri.as_deref(),
        Some("https://accounts.example.com")
    );
    assert_eq!(p.user_name_attribute.as_deref(), Some("email"));
    assert_eq!(p.token_uri, None);
}

#[test]
fn reads_a_plain_oauth2_registration() {
    let s = read(
        r#"
[security.oauth2.client.registration.gh]
client-id = "cid"
client-secret = "x"
provider = "github"

[security.oauth2.client.provider.github]
authorization-uri = "https://github.example/authorize"
token-uri = "https://github.example/token"
user-info-uri = "https://api.github.example/user"
user-name-attribute = "login"
"#,
    )
    .unwrap();
    let r = &s.registrations[0];
    assert_eq!(r.provider, "github");
    assert_eq!(r.client_authentication, ClientAuthentication::Basic);
    assert_eq!(r.redirect_uri, "{base-url}/login/oauth2/code/{id}");
    assert!(r.scopes.is_empty());
    assert!(s.providers["github"].jwk_set_uri.is_none());
}

#[test]
fn the_provider_defaults_to_the_registration_id() {
    let s = read(GOOGLE).unwrap();
    assert_eq!(s.registrations[0].provider, "google");
    assert_eq!(ClientAuthentication::default(), ClientAuthentication::Basic);
}

#[test]
fn no_registration_section_gives_empty_settings() {
    let s = read("[server]\nport = 1\n").unwrap();
    assert!(s.registrations.is_empty());
    assert!(s.providers.is_empty());
    let s = read(&format!(
        "[{PROV}.only]\nissuer-uri = \"https://idp.example\"\n"
    ))
    .unwrap();
    assert!(s.registrations.is_empty());
    assert_eq!(s.providers.len(), 1);
}

#[test]
fn a_missing_client_id_is_refused() {
    let toml = GOOGLE.replace("client-id = \"cid\"\n", "");
    assert_eq!(invalid_key(&toml), format!("{REG}.google.client-id"));
}

#[test]
fn a_missing_client_secret_is_refused_unless_authentication_is_none() {
    let toml = GOOGLE.replace("client-secret = \"s3cr3t-value\"\n", "");
    assert_eq!(invalid_key(&toml), format!("{REG}.google.client-secret"));
    let ok = toml.replace("\"post\"", "\"none\"");
    let s = read(&ok).unwrap();
    assert_eq!(
        s.registrations[0].client_authentication,
        ClientAuthentication::None
    );
    assert!(s.registrations[0].client_secret.is_none());
}

#[test]
fn an_unknown_client_authentication_is_refused() {
    let toml = GOOGLE.replace("\"post\"", "\"digest\"");
    assert_eq!(
        invalid_key(&toml),
        format!("{REG}.google.client-authentication")
    );
}

#[test]
fn an_unknown_provider_is_refused() {
    let toml = GOOGLE.replace(
        "client-id = \"cid\"",
        "client-id = \"cid\"\nprovider = \"nope\"",
    );
    assert_eq!(invalid_key(&toml), format!("{REG}.google.provider"));
    // No provider table at all: the default (the id) is unknown too.
    let toml =
        "[security.oauth2.client.registration.a]\nclient-id = \"c\"\nclient-secret = \"s\"\n";
    assert_eq!(invalid_key(toml), format!("{REG}.a.provider"));
}

#[test]
fn a_provider_without_issuer_or_authorization_and_token_is_refused() {
    let both = "authorization-uri = \"https://x.example/a\"\nuser-name-attribute = \"id\"\n";
    let toml = format!("[{PROV}.p]\n{both}");
    assert_eq!(invalid_key(&toml), format!("{PROV}.p.token-uri"));
    let toml = "[security.oauth2.client.provider.p]\nuser-info-uri = \"https://x.example/u\"\n";
    assert_eq!(invalid_key(toml), format!("{PROV}.p.issuer-uri"));
}

#[test]
fn a_plain_oauth2_provider_needs_user_info_or_a_user_name_attribute() {
    let toml = format!(
        "[{PROV}.p]\nauthorization-uri = \"https://x.example/a\"\ntoken-uri = \"https://x.example/t\"\n"
    );
    assert_eq!(invalid_key(&toml), format!("{PROV}.p.user-info-uri"));
    // A user name attribute alone is enough, and so is a jwk-set-uri (the id token).
    assert!(read(&format!("{toml}user-name-attribute = \"id\"\n")).is_ok());
    assert!(read(&format!("{toml}jwk-set-uri = \"https://x.example/k\"\n")).is_ok());
}

#[test]
fn non_https_is_refused_but_loopback_http_is_allowed() {
    for key in [
        "issuer-uri",
        "authorization-uri",
        "token-uri",
        "user-info-uri",
        "jwk-set-uri",
    ] {
        let base = if key == "issuer-uri" {
            ""
        } else {
            "issuer-uri = \"https://idp.example\"\n"
        };
        let toml = format!("[{PROV}.p]\n{base}{key} = \"http://idp.example/x\"\n");
        assert_eq!(invalid_key(&toml), format!("{PROV}.p.{key}"), "{key}");
    }
    for bad in [
        "/relative",
        "idp.example",
        "ftp://idp.example",
        "https://u:p@idp.example/x",
        "https://",
    ] {
        let toml = format!("[{PROV}.p]\nissuer-uri = \"{bad}\"\n");
        assert_eq!(invalid_key(&toml), format!("{PROV}.p.issuer-uri"), "{bad}");
    }
    for ok in [
        "http://localhost:8080/x",
        "http://127.0.0.1:9000",
        "http://127.9.9.9/x",
        "http://[::1]:9000/x",
        "https://idp.example/x",
    ] {
        let toml = format!("[{PROV}.p]\nissuer-uri = \"{ok}\"\n");
        assert!(read(&toml).is_ok(), "{ok}");
    }
    assert_eq!(
        invalid_key(&format!(
            "[{PROV}.p]\nissuer-uri = \"http://localhost.evil.example\"\n"
        )),
        format!("{PROV}.p.issuer-uri")
    );
}

#[test]
fn a_registration_id_must_be_lowercase_alphanumeric_or_dash() {
    for id in ["Google", "my_idp", "a.b"] {
        let toml = format!(
            "[{REG}.\"{id}\"]\nclient-id = \"c\"\nclient-secret = \"s\"\nprovider = \"p\"\n\n[{PROV}.p]\nissuer-uri = \"https://idp.example\"\n"
        );
        assert_eq!(invalid_key(&toml), format!("{REG}.{id}"), "{id}");
    }
    let toml = format!(
        "[{REG}.my-idp-2]\nclient-id = \"c\"\nclient-secret = \"s\"\nprovider = \"p\"\n\n[{PROV}.p]\nissuer-uri = \"https://idp.example\"\n"
    );
    assert!(read(&toml).is_ok());
}

#[test]
fn redirect_uri_template_is_resolved() {
    let s = read(&GOOGLE.replace("redirect-uri = \"https://app.example/cb\"\n", "")).unwrap();
    let r = &s.registrations[0];
    assert_eq!(
        r.resolved_redirect_uri("https://app.example"),
        "https://app.example/login/oauth2/code/google"
    );
    assert_eq!(
        r.resolved_redirect_uri("https://app.example/"),
        "https://app.example/login/oauth2/code/google"
    );
}

#[test]
fn the_secret_never_appears_in_the_error_or_debug_output() {
    let s = read(GOOGLE).unwrap();
    let debug = format!("{s:?} {:?}", s.registrations[0]);
    assert!(!debug.contains("s3cr3t-value"));
    assert!(debug.contains("[REDACTED]"));
    // Every rule that can fail next to a secret keeps it out of the message.
    for toml in [
        GOOGLE.replace("client-id = \"cid\"\n", ""),
        GOOGLE.replace("\"post\"", "\"digest\""),
        GOOGLE.replace("https://accounts.example.com", "http://idp.example"),
        GOOGLE.replace(
            "client-id = \"cid\"",
            "client-id = \"cid\"\nprovider = \"nope\"",
        ),
    ] {
        let err = read(&toml).unwrap_err();
        assert!(!err.to_string().contains("s3cr3t-value"));
        assert!(!format!("{err:?}").contains("s3cr3t-value"));
    }
    // A wrong type for the secret itself is refused without printing it.
    let err = read(&GOOGLE.replace("\"s3cr3t-value\"", "[\"s3cr3t-value\"]")).unwrap_err();
    assert!(!format!("{err:?}{err}").contains("s3cr3t-value"));
}

fn reg_with(field: &str) -> String {
    format!(
        "[security.oauth2.client.registration.idp]\nclient-id = \"app\"\nclient-secret = \"hunter2-hunter2\"\n{field}\n\n[security.oauth2.client.provider.idp]\nissuer-uri = \"https://idp.example\"\n"
    )
}

#[test]
fn the_redirect_uri_is_validated() {
    let key = format!("{REG}.idp.redirect-uri");
    for bad in [
        "javascript:x",
        "http://evil.example/cb",
        "https://app.example/cb#frag",
        "{base-url}/cb/{host}",
        "{base-url}/cb/{id",
        "{base-url}/cb/id}",
        "https://user:pw@app.example/cb",
    ] {
        match read(&reg_with(&format!("redirect-uri = \"{bad}\""))) {
            Err(ConfigError::Invalid {
                key: k, message, ..
            }) => {
                assert_eq!(k, key, "{bad}");
                assert!(!message.contains(bad), "{bad} leaked");
            }
            other => panic!("{bad}: {other:?}"),
        }
    }
    for good in [
        "https://app.example/cb/{id}",
        "http://localhost:8080/cb",
        "{base-url}/login/oauth2/code/{id}",
    ] {
        read(&reg_with(&format!("redirect-uri = \"{good}\"")))
            .unwrap_or_else(|e| panic!("{good}: {e}"));
    }
    read(&reg_with("")).unwrap();
}

#[test]
fn scope_tokens_are_validated() {
    let key = format!("{REG}.idp.scope");
    for bad in [
        r#"["a b"]"#,
        r#"["\n"]"#,
        r#"[""]"#,
        r#"["a\"b"]"#,
        r#"["a\\b"]"#,
        r#"["openid", "\t"]"#,
    ] {
        assert_eq!(
            invalid_key(&reg_with(&format!("scope = {bad}"))),
            key,
            "{bad}"
        );
    }
    let ok = read(&reg_with(
        r#"scope = ["openid", "profile", "https://example.com/auth/read"]"#,
    ))
    .unwrap();
    assert_eq!(ok.registrations[0].scopes.len(), 3);
}

fn provider_with(line: &str) -> String {
    format!(
        "[security.oauth2.client.registration.idp]\nclient-id = \"app\"\nclient-secret = \"hunter2-hunter2\"\n\n[security.oauth2.client.provider.idp]\n{line}\n"
    )
}

#[test]
fn endpoints_refuse_fragments_issuer_queries_and_bad_ports() {
    let issuer = format!("{PROV}.idp.issuer-uri");
    assert_eq!(
        invalid_key(&provider_with(r#"issuer-uri = "https://i.example/#f""#)),
        issuer
    );
    assert_eq!(
        invalid_key(&provider_with(r#"issuer-uri = "https://i.example/?a=b""#)),
        issuer
    );
    assert_eq!(
        invalid_key(&provider_with(
            r#"issuer-uri = "https://good.example:99999/""#
        )),
        issuer
    );
    assert_eq!(
        invalid_key(&provider_with(
            "authorization-uri = \"https://i.example/a\"\ntoken-uri = \"https://i.example/t#f\"\nuser-name-attribute = \"sub\""
        )),
        format!("{PROV}.idp.token-uri")
    );
    read(&provider_with(
        "authorization-uri = \"https://i.example/a?prompt=consent\"\ntoken-uri = \"https://i.example:8443/t\"\nuser-name-attribute = \"sub\"",
    ))
    .unwrap();
}

#[test]
fn a_provider_id_must_be_lowercase_alphanumeric_or_dash() {
    let toml = "[security.oauth2.client.provider.Bad_Id]\nissuer-uri = \"https://i.example\"\n";
    assert_eq!(invalid_key(toml), format!("{PROV}.Bad_Id"));
}

#[test]
fn a_key_typo_in_a_registration_is_an_error_naming_it() {
    let toml = reg_with("client-idd = \"x\"");
    let err = read(&toml).unwrap_err().to_string();
    assert!(err.contains("client-idd"), "{err}");
}

#[test]
fn relaxed_binding_accepts_underscore_and_joined_keys() {
    for name in ["client_id", "clientid"] {
        let toml = format!(
            "[security.oauth2.client.registration.idp]\n{name} = \"app\"\nclient-secret = \"hunter2-hunter2\"\n\n[security.oauth2.client.provider.idp]\nissuer-uri = \"https://i.example\"\n"
        );
        assert_eq!(
            read(&toml).unwrap().registrations[0].client_id,
            "app",
            "{name}"
        );
    }
}
