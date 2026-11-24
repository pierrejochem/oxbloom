//! The OAuth2 login part of `SecurityAutoConfiguration` (feature `oauth2`): the
//! `security.oauth2` keys, their checks, and the startup failures that name a key.

use std::sync::Arc;

use oxbloom_config::{Config, ConfigError};
use oxbloom_core::Clock;
use oxbloom_di::DiError;
use oxbloom_http_client::HttpClient;
use oxbloom_security::{
    HttpSecurity, OAuth2Login, OAuth2Settings, SecurityBuildError, is_safe_redirect_target,
};
use serde::Deserialize;
use serde::de::IgnoredAny;

/// The external base URL of the application, the `{base-url}` of every redirect URI.
pub(crate) const BASE_URL: &str = "security.oauth2.base-url";
/// Where a failed OAuth2 login goes; the failure URL of form login when unset.
pub(crate) const FAILURE_URL: &str = "security.oauth2.failure-url";
const SECTION: &str = "security.oauth2";
pub(crate) const REGISTRATIONS: &str = "security.oauth2.client.registration";
/// The fixed path prefixes of the two OAuth2 login endpoints.
const START_PREFIX: &str = "/oauth2/authorization/";
const CALLBACK_PREFIX: &str = "/login/oauth2/code/";

/// The keys of `security.oauth2` itself; `client` is read by `OAuth2Settings::from_config`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSection {
    #[serde(rename = "base-url")]
    base_url: Option<String>,
    #[serde(rename = "failure-url")]
    failure_url: Option<String>,
    // Read only for its unknown-key check.
    #[allow(dead_code)]
    client: Option<RawClient>,
}

/// The two tables under `security.oauth2.client`; anything else is an unknown key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClient {
    #[allow(dead_code)]
    registration: Option<IgnoredAny>,
    #[allow(dead_code)]
    provider: Option<IgnoredAny>,
}

/// The checked `security.oauth2` section: the settings of the registrations and providers,
/// and the two boot keys as written.
///
/// It is parsed once per application: `SecurityAutoConfiguration` registers it as a lazy bean
/// (see [`oauth2_properties`]), and both the OAuth2 login of the chain and the OAuth2 client of
/// `OAuth2ClientAutoConfiguration` read that bean, so they see the same registrations.
pub(crate) struct OAuth2Properties {
    settings: OAuth2Settings,
    base_url: Option<String>,
    failure_url: Option<String>,
}

impl OAuth2Properties {
    /// Reads `security.oauth2`. Unknown keys and every `security.oauth2.client` error fail
    /// whether or not the login is used; a set `base-url` or `failure-url` must have the right
    /// shape. The errors name the key and never hold a value.
    pub(crate) fn from_config(config: &Config) -> Result<Self, ConfigError> {
        let raw = match config.get::<RawSection>(SECTION) {
            Ok(raw) => Some(raw),
            Err(ConfigError::Missing { key }) if key == SECTION => None,
            Err(error) => return Err(error),
        };
        let settings = OAuth2Settings::from_config(config)?;
        let (base_url, failure_url) = raw.map_or((None, None), |r| (r.base_url, r.failure_url));
        if let Some(url) = &base_url {
            check_base_url(url)?;
        }
        if let Some(url) = &failure_url {
            check_failure_url(url)?;
        }
        Ok(Self {
            settings,
            base_url,
            failure_url,
        })
    }

    /// Whether the configuration has at least one registration.
    pub(crate) fn has_registrations(&self) -> bool {
        !self.settings.registrations.is_empty()
    }

    /// The registrations and providers of `security.oauth2.client`.
    pub(crate) fn settings(&self) -> &OAuth2Settings {
        &self.settings
    }

    /// Turns on `oauth2_login` with the registrations, the providers, the base URL, the failure
    /// URL, the client and the clock bean. Needs a registration (see
    /// [`has_registrations`](Self::has_registrations)), `base-url` and the client.
    pub(crate) fn apply(
        &self,
        http: HttpSecurity,
        client: Option<Arc<dyn HttpClient>>,
        clock: Option<Arc<dyn Clock>>,
    ) -> Result<HttpSecurity, OAuth2Failure> {
        let base_url = self.base_url.clone().ok_or_else(|| {
            OAuth2Failure::Config(ConfigError::Missing {
                key: BASE_URL.to_owned(),
            })
        })?;
        let client = client.ok_or(OAuth2Failure::Login(OAuth2LoginError::WithoutHttpClient))?;
        // The callback path is fixed, so a redirect URI must be exactly the callback URL of its
        // own registration. Checked here to name the registration's key.
        let base = base_url.trim_end_matches('/');
        for registration in &self.settings.registrations {
            let expected = format!("{base}{CALLBACK_PREFIX}{}", registration.id);
            if registration.resolved_redirect_uri(base) != expected {
                return Err(OAuth2Failure::Config(ConfigError::invalid(
                    format!("{REGISTRATIONS}.{}.redirect-uri", registration.id),
                    "must be {base-url}/login/oauth2/code/{id}, because the callback path is \
                     fixed; leave it unset to get that value",
                )));
            }
        }
        let settings = self.settings.clone();
        let failure_url = self.failure_url.clone();
        Ok(http.oauth2_login(move |_| {
            let mut login = OAuth2Login::new(client).base_url(&base_url);
            for registration in settings.registrations {
                login = login.registration(registration);
            }
            for (name, provider) in settings.providers {
                login = login.provider(&name, provider);
            }
            if let Some(url) = failure_url {
                login = login.failure_url(&url);
            }
            if let Some(clock) = clock {
                login = login.clock(clock);
            }
            login
        }))
    }
}

/// The bean factory of the shared [`OAuth2Properties`], and the parse that
/// `OAuth2ClientAutoConfiguration` falls back to when `SecurityAutoConfiguration` is excluded.
/// A bad section fails with a `DiError::Creation` for `OAuth2Settings` whose source is the
/// `ConfigError` naming the key, never the value or the secret.
pub(crate) fn oauth2_properties(config: Arc<Config>) -> Result<OAuth2Properties, DiError> {
    OAuth2Properties::from_config(&config).map_err(DiError::creation_from::<OAuth2Settings>)
}

/// A startup failure of the OAuth2 part: a configuration error that names its key, or one of
/// the [`OAuth2LoginError`]s.
#[derive(Debug)]
pub(crate) enum OAuth2Failure {
    Config(ConfigError),
    Login(OAuth2LoginError),
}

/// Maps the build errors that, when boot configured OAuth2 login, can only come from the
/// configuration: no form login and a base URL the builder refuses. Every other error stays.
pub(crate) fn config_failure(error: &SecurityBuildError) -> Option<OAuth2Failure> {
    match error {
        SecurityBuildError::OAuth2LoginWithoutFormLogin => {
            Some(OAuth2Failure::Login(OAuth2LoginError::WithoutFormLogin))
        }
        SecurityBuildError::OAuth2LoginWithoutBaseUrl => {
            Some(OAuth2Failure::Config(invalid_base_url()))
        }
        _ => None,
    }
}

fn invalid_base_url() -> ConfigError {
    ConfigError::invalid(
        BASE_URL,
        "must be an absolute https URL (http only for localhost, a 127.0.0.0/8 address or ::1) \
         without credentials, a path, a query or a fragment",
    )
}

/// The shape that boot adds to the builder's own base URL check: a scheme, and nothing after
/// the authority but an optional `/`. The callback path is fixed, so a path prefix could
/// never be reached.
fn check_base_url(url: &str) -> Result<(), ConfigError> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(invalid_base_url());
    };
    let scheme_ok = matches!(scheme.to_ascii_lowercase().as_str(), "https" | "http");
    let tail = rest.find(['/', '?', '#']).map_or("", |i| &rest[i..]);
    if !scheme_ok || rest.is_empty() || !(tail.is_empty() || tail == "/") {
        return Err(invalid_base_url());
    }
    Ok(())
}

/// A safe local redirect target of printable ASCII that is not one of the two OAuth2
/// endpoints, which would loop.
fn check_failure_url(url: &str) -> Result<(), ConfigError> {
    let path = url.split('?').next().unwrap_or(url);
    let printable = url.bytes().all(|b| b.is_ascii_graphic() || b == b' ');
    if !is_safe_redirect_target(url)
        || !printable
        || path.starts_with(START_PREFIX)
        || path.starts_with(CALLBACK_PREFIX)
    {
        return Err(ConfigError::invalid(
            FAILURE_URL,
            "must be a local path such as /login?error, outside /oauth2/authorization/ and \
             /login/oauth2/code/",
        ));
    }
    Ok(())
}

/// The startup failures of `security.oauth2.client.registration` that are not a bad value.
/// The texts are fixed and name the key or the bean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OAuth2LoginError {
    /// Registrations without form login.
    WithoutFormLogin,
    /// Registrations without a `dyn HttpClient` bean.
    WithoutHttpClient,
}

impl std::fmt::Display for OAuth2LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WithoutFormLogin => {
                "security.oauth2.client.registration needs form login: set \
                 security.form-login.enabled to true"
            }
            Self::WithoutHttpClient => {
                "security.oauth2.client.registration needs an `HttpClient` bean for the provider \
                 requests, such as the one of `HttpClientAutoConfiguration`"
            }
        })
    }
}

impl std::error::Error for OAuth2LoginError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(toml: &str) -> Result<OAuth2Properties, ConfigError> {
        OAuth2Properties::from_config(&Config::from_str(toml).unwrap())
    }

    fn key_of(result: Result<OAuth2Properties, ConfigError>) -> String {
        match result {
            Err(ConfigError::Invalid { key, .. }) => key,
            Err(other) => panic!("unexpected {other:?}"),
            Ok(_) => panic!("accepted"),
        }
    }

    #[test]
    fn the_section_is_optional() {
        let props = read("").unwrap();
        assert!(!props.has_registrations());
        assert!(props.base_url.is_none());
        let props = read("[security.oauth2]\nbase-url = \"https://app.example\"\n").unwrap();
        assert!(!props.has_registrations());
        assert_eq!(props.base_url.as_deref(), Some("https://app.example"));
    }

    #[test]
    fn base_urls_without_a_path_are_accepted() {
        for url in [
            "https://app.example",
            "https://app.example/",
            "https://app.example:8443",
            "http://localhost:8080",
            "HTTPS://app.example",
        ] {
            assert!(check_base_url(url).is_ok(), "{url}");
        }
    }

    #[test]
    fn base_urls_with_a_path_a_query_or_a_fragment_are_refused() {
        for url in [
            "https://app.example/app",
            "https://app.example//",
            "https://app.example?x=1",
            "https://app.example/?x=1",
            "https://app.example#f",
            "ftp://app.example",
            "app.example",
            "https://",
            "",
        ] {
            let error = check_base_url(url).unwrap_err();
            assert!(matches!(&error, ConfigError::Invalid { key, .. } if key == BASE_URL));
            if !url.is_empty() {
                assert!(!error.to_string().contains(url), "{error}");
            }
        }
    }

    #[test]
    fn failure_urls_must_be_local_and_off_the_endpoints() {
        assert!(check_failure_url("/login?error").is_ok());
        assert!(check_failure_url("/signin?failed=1").is_ok());
        for url in [
            "//evil.example/x",
            "https://evil.example/",
            "login",
            "/oauth2/authorization/idp",
            "/login/oauth2/code/idp?x",
            "/caf\u{e9}",
        ] {
            assert!(check_failure_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn unknown_keys_are_refused_with_the_section_key() {
        let key = key_of(read("[security.oauth2]\nbase-path = \"/x\"\n"));
        assert!(key.starts_with(SECTION), "{key}");
        let key = key_of(read(
            "[security.oauth2.client.registrations.idp]\nclient-id = \"a\"\n",
        ));
        assert!(key.starts_with(SECTION), "{key}");
        let key = key_of(read("[security.oauth2]\nfailure-url = \"//evil\"\n"));
        assert_eq!(key, FAILURE_URL);
    }

    #[test]
    fn the_error_texts_name_the_key() {
        assert!(
            OAuth2LoginError::WithoutFormLogin
                .to_string()
                .contains("security.form-login.enabled")
        );
        assert!(
            OAuth2LoginError::WithoutHttpClient
                .to_string()
                .contains("`HttpClient` bean")
        );
        assert!(matches!(
            config_failure(&SecurityBuildError::OAuth2LoginWithoutBaseUrl),
            Some(OAuth2Failure::Config(ConfigError::Invalid { key, .. })) if key == BASE_URL
        ));
        assert!(matches!(
            config_failure(&SecurityBuildError::OAuth2LoginWithoutFormLogin),
            Some(OAuth2Failure::Login(OAuth2LoginError::WithoutFormLogin))
        ));
        assert!(config_failure(&SecurityBuildError::NoAuthorizeSection).is_none());
    }
}
