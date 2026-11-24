//! The security analyzers: security build errors, the session management and remember-me
//! startup failures and invalid `security.jwt` keys.

use oxbloom_config::ConfigError;
use oxbloom_security::SecurityBuildError;

use crate::autoconfigure::{RememberMeError, SessionManagementError};
use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises a `SecurityBuildError` anywhere in the cause chain.
pub(crate) struct SecurityAnalyzer;

impl FailureAnalyzer for SecurityAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        // Exhaustive on purpose: a new variant must get its own text here.
        let (description, action) = match failure.find_cause::<SecurityBuildError>()? {
            SecurityBuildError::NoAuthorizeSection => (
                "The security configuration has no authorize section.",
                "Call `authorize` and add at least one rule.",
            ),
            SecurityBuildError::MatcherAfterAnyRequest => (
                "A security rule follows `any_request()`, where it can never match.",
                "Put `any_request()` last.",
            ),
            SecurityBuildError::ResourceServerWithoutDecoder => (
                "`oauth2_resource_server` is configured without a JWT decoder.",
                "Configure a decoder (static keys or JWKS).",
            ),
            SecurityBuildError::DuplicateAuthorizeSection => (
                "`authorize` was called more than once.",
                "Put all rules in one `authorize` call.",
            ),
            SecurityBuildError::InvalidPattern => (
                "A security matcher pattern is not valid. Patterns use Ant syntax, and `*` or \
                 `**` is allowed only as a whole path segment; `{id}`, `?` and partial \
                 wildcards such as `*.pdf` are not supported.",
                "Use `/users/*` or `/users/**` and match the rest in a handler.",
            ),
            SecurityBuildError::HttpBasicWithoutProvider => (
                "`http_basic` is configured without a password provider.",
                "Add one with `password_provider`, for example a `DaoAuthenticationProvider`.",
            ),
            SecurityBuildError::InvalidRealm => (
                "The HTTP Basic realm contains a double quote, a backslash or a control character.",
                "Use a realm of plain printable text, such as `Oxbloom`.",
            ),
            SecurityBuildError::FormLoginWithoutProvider => (
                "`form_login` is configured without a password provider.",
                "Add one with `password_provider`, for example a `DaoAuthenticationProvider`.",
            ),
            SecurityBuildError::InvalidFormLoginUrl => (
                "A form login URL is not a safe local path, two form login URLs collide, or a form \
                 field name is not made of ASCII letters, digits, `_` and `-`.",
                "Use paths that start with a single `/`, a logout URL that differs from the login \
                 URLs, and plain field names such as `username`.",
            ),
            SecurityBuildError::CsrfWithoutFormLogin => (
                "CSRF protection is configured but form login is not.",
                "Configure `form_login`, or turn CSRF protection off.",
            ),
            SecurityBuildError::InvalidCsrfConfig => (
                "A CSRF field or header name is not made of ASCII letters, digits, `_` and `-`, or \
                 the token endpoint is not a safe local path or collides with a form login URL.",
                "Use plain names such as `_csrf` and `X-CSRF-TOKEN`, and a token endpoint that \
                 starts with a single `/` and differs from the form login URLs.",
            ),
            SecurityBuildError::InvalidHeadersConfig => (
                "A security header value is empty or has a character outside visible ASCII, or the \
                 HSTS max age is zero.",
                "Use plain printable header values and an HSTS max age of at least one second.",
            ),
            SecurityBuildError::SessionLimitWithoutFormLogin => (
                "A session limit is configured but form login is not.",
                "Configure `form_login`, or remove the session limit.",
            ),
            SecurityBuildError::InvalidSessionLimit => (
                "The session limit is zero.",
                "Allow at least one session per user, or remove the session limit.",
            ),
            SecurityBuildError::RevalidateWithoutFormLogin => (
                "Session revalidation is configured but form login is not.",
                "Configure `form_login`, or remove the revalidation.",
            ),
            SecurityBuildError::InvalidRevalidateInterval => (
                "The session revalidation interval is zero.",
                "Use an interval of at least one second, or leave revalidation out to turn it off.",
            ),
            SecurityBuildError::RememberMeWithoutFormLogin => (
                "Remember-me is configured but form login is not.",
                "Configure `form_login`, or remove remember-me.",
            ),
            SecurityBuildError::InvalidRememberMe => (
                "A remember-me setting is missing or out of range: a secret shorter than 32 bytes, \
                 a missing secret or user service, a lifetime outside 1 minute to 365 days, a \
                 cookie name that is not an HTTP token, or a field name not made of ASCII \
                 letters, digits, `_` and `-`.",
                "Set secrets of at least 32 bytes and a `UserDetailsService`, a lifetime between \
                 1 minute and 365 days, and plain names such as `OXBLOOM_REMEMBER_ME` and \
                 `remember-me`.",
            ),
            SecurityBuildError::OAuth2LoginWithoutFormLogin => (
                "OAuth2 login is configured but form login is not.",
                "Configure `form_login` as well (in boot, set `security.form-login.enabled = \
                 true`); it gives OAuth2 login the session, the logout, the saved request and \
                 the success URL, and needs no password provider then.",
            ),
            SecurityBuildError::OAuth2LoginWithoutRegistration => (
                "OAuth2 login has no usable registration: none was added, an id has a character \
                 outside `a` to `z`, `0` to `9` and `-`, or a registration names a provider that \
                 was not added.",
                "Add at least one registration, use ids such as `github`, and add the provider \
                 that each registration names.",
            ),
            SecurityBuildError::OAuth2LoginWithoutBaseUrl => (
                "OAuth2 login has no valid base URL for its redirect URI.",
                "Set the external base URL of the application, an absolute `https` URL (`http` \
                 only for a loopback host) without a query or a fragment, such as \
                 `https://app.example`.",
            ),
            SecurityBuildError::OAuth2LoginWithoutClient => (
                "OAuth2 login has no HTTP client for the provider requests.",
                "Pass an `HttpClient` to `OAuth2Login::new` or `OAuth2Login::client`.",
            ),
            SecurityBuildError::InvalidOAuth2Login => (
                "An OAuth2 login setting is out of range: a redirect URI is not \
                 `{base-url}/login/oauth2/code/{id}` of its own registration, or the pending \
                 login max age is zero.",
                "Leave `redirect-uri` at its default or point it at \
                 `{base-url}/login/oauth2/code/{id}`, and use a pending login max age above zero.",
            ),
        };
        Some(FailureAnalysis::new(description, action))
    }
}

/// Recognises a `SessionManagementError` of `SecurityAutoConfiguration` anywhere in the cause
/// chain.
pub(crate) struct SessionManagementAnalyzer;

impl FailureAnalyzer for SessionManagementAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        // Exhaustive on purpose: a new variant must get its own text here.
        let (description, action) = match failure.find_cause::<SessionManagementError>()? {
            SessionManagementError::LimitWithoutIndex => (
                "`security.session.max-sessions-per-user` is set, but the session store cannot \
                 list the sessions of a user.",
                "Use the in-memory store (`security.session.store = \"memory\"`) or a store \
                 that implements `PrincipalSessions`, or remove \
                 `security.session.max-sessions-per-user`.",
            ),
            SessionManagementError::RevalidateWithoutUserService => (
                "`security.session.revalidate-interval` is set, but there is no \
                 `UserDetailsService` bean to load the user again.",
                "Add users under `security.users` or register a `dyn UserDetailsService` bean, or \
                 set `security.session.revalidate-interval = \"0s\"` to turn revalidation off.",
            ),
            SessionManagementError::RegistryWithoutIndex => (
                "The `SessionRegistry` bean was requested, but the session store cannot list the \
                 sessions of a user.",
                "Use the in-memory store (`security.session.store = \"memory\"`) or a store \
                 that implements `PrincipalSessions`, or do not inject `SessionRegistry`.",
            ),
        };
        Some(FailureAnalysis::new(description, action))
    }
}

/// Recognises a `RememberMeError` of `SecurityAutoConfiguration` anywhere in the cause chain.
pub(crate) struct RememberMeAnalyzer;

impl FailureAnalyzer for RememberMeAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        // Exhaustive on purpose: a new variant must get its own text here.
        let (description, action) = match failure.find_cause::<RememberMeError>()? {
            RememberMeError::WithoutFormLogin => (
                "`security.remember-me.enabled` is `true`, but form login is off.",
                "Set `security.form-login.enabled = true`, or turn remember-me off.",
            ),
            RememberMeError::WithoutSecret => (
                "`security.remember-me.enabled` is `true`, but `security.remember-me.secret` is \
                 not set.",
                "Set `security.remember-me.secret` to a secret of at least 32 bytes in a file or \
                 an environment variable (`APP_SECURITY__REMEMBER_ME__SECRET`), never on the \
                 command line.",
            ),
            RememberMeError::WithoutUserService => (
                "`security.remember-me.enabled` is `true`, but there is no `UserDetailsService` \
                 bean to load the user named in the cookie.",
                "Add users under `security.users` or register a `dyn UserDetailsService` bean, or \
                 set `security.remember-me.enabled = false`.",
            ),
            RememberMeError::CookieNameClash => (
                "`security.remember-me.cookie-name` is the session cookie name, so the two \
                 cookies would overwrite each other.",
                "Choose a different `security.remember-me.cookie-name`, such as the default \
                 `OXBLOOM_REMEMBER_ME`.",
            ),
        };
        Some(FailureAnalysis::new(description, action))
    }
}

/// Recognises an `OAuth2LoginError` of `SecurityAutoConfiguration`, or a `ConfigError::Invalid`
/// for a key in the `security.oauth2` section, anywhere in the cause chain. The report names
/// the key with a fixed text and leaves out the configuration error's message, so no value
/// (a URL, a secret given with the wrong type) can reach it.
#[cfg(feature = "oauth2")]
pub(crate) struct OAuth2Analyzer;

#[cfg(feature = "oauth2")]
impl FailureAnalyzer for OAuth2Analyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        use crate::autoconfigure::OAuth2LoginError;

        if let Some(error) = failure.find_cause::<OAuth2LoginError>() {
            // Exhaustive on purpose: a new variant must get its own text here.
            let (description, action) = match error {
                OAuth2LoginError::WithoutFormLogin => (
                    "`security.oauth2.client.registration` is set, but form login is off. \
                     OAuth2 login keeps the user in the form login session.",
                    "Set `security.form-login.enabled = true` (OAuth2 login needs no password \
                     provider then), or remove the registrations.",
                ),
                OAuth2LoginError::WithoutHttpClient => (
                    "`security.oauth2.client.registration` is set, but there is no `HttpClient` \
                     bean for the requests to the provider.",
                    "Keep `HttpClientAutoConfiguration` in the auto-configurations (it comes with \
                     the `http-client` feature, which `oauth2` turns on), or register a \
                     `dyn HttpClient` bean.",
                ),
            };
            return Some(FailureAnalysis::new(description, action));
        }
        let ConfigError::Invalid { key, .. } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        if key != "security.oauth2" && !key.starts_with("security.oauth2.") {
            return None;
        }
        let action = if key == crate::autoconfigure::OAUTH2_BASE_URL {
            "Set `security.oauth2.base-url` to the external URL of the application: an absolute \
             `https` URL (`http` only for a loopback host) with no path, query or fragment, such \
             as `https://app.example`. The callback path is fixed at \
             `/login/oauth2/code/{id}`; a proxy that adds a path prefix must strip it."
        } else if key == crate::autoconfigure::OAUTH2_FAILURE_URL {
            "Set `security.oauth2.failure-url` to a local path such as `/login?error`, outside \
             `/oauth2/authorization/` and `/login/oauth2/code/`, or remove it to use the \
             failure URL of form login."
        } else if key.ends_with(".redirect-uri") {
            "The callback path is fixed: leave `redirect-uri` unset, or set it to \
             `{base-url}/login/oauth2/code/{id}`."
        } else if key.ends_with(".client-secret") {
            "Set the client secret of the registration in a file or an environment variable, \
             never on the command line, or set `client-authentication = \"none\"` for a public \
             client."
        } else {
            "Fix the `security.oauth2` section: it holds `base-url`, `failure-url` and \
             `client`; each registration under `client.registration.<id>` needs `client-id`, a \
             `client-secret` (unless `client-authentication` is `none`) and a provider under \
             `client.provider.<id>` with an `issuer-uri`, or an `authorization-uri` and a \
             `token-uri`; endpoints are absolute `https` URLs (`http` only for a loopback \
             host); ids hold only `a` to `z`, `0` to `9` and `-`."
        };
        Some(FailureAnalysis::new(
            format!("The OAuth2 configuration key `{key}` is not valid."),
            action,
        ))
    }
}

/// Recognises a `ConfigError::Invalid` for a key in the `security.jwt` section anywhere in the
/// cause chain, as `jwt_decoder_from_config` returns it. Other invalid keys get no analysis:
/// only this section's messages are known never to carry a value.
pub(crate) struct JwtConfigAnalyzer;

impl FailureAnalyzer for JwtConfigAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let ConfigError::Invalid { key, message, .. } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        if key != "security.jwt" && !key.starts_with("security.jwt.") {
            return None;
        }
        Some(FailureAnalysis::new(
            format!("The JWT configuration key `{key}` is not valid: {message}."),
            "Fix the `security.jwt` section: `algorithms` and exactly one key source \
             (`secret` for HMAC, `public-key-pem` or `jwk-set-uri` for RSA and EC). Set \
             `security.jwt.secret` in a file or an environment variable, never on the command \
             line.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StartupPhase;
    use oxbloom_di::DiError;

    fn analyze_jwt(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Option<FailureAnalysis> {
        JwtConfigAnalyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    #[test]
    fn invalid_jwt_key_text() {
        let di: DiError =
            ConfigError::invalid("security.jwt.secret", "required with HMAC algorithms").into();
        let a = analyze_jwt(di).unwrap();
        assert_eq!(
            a.description(),
            "The JWT configuration key `security.jwt.secret` is not valid: required with HMAC algorithms."
        );
        assert!(a.action().unwrap().contains("never on the command line"));
        assert!(analyze_jwt(ConfigError::invalid("security.jwt", "unknown field `x`")).is_some());
    }

    #[test]
    fn other_keys_and_variants_are_none() {
        assert!(analyze_jwt(ConfigError::invalid("db.port", "bad")).is_none());
        assert!(analyze_jwt(ConfigError::invalid("security.jwtx", "bad")).is_none());
        assert!(
            analyze_jwt(ConfigError::Missing {
                key: "security.jwt.algorithms".into()
            })
            .is_none()
        );
        assert!(analyze_jwt(std::fmt::Error).is_none());
    }

    fn analyze(e: SecurityBuildError) -> Option<FailureAnalysis> {
        SecurityAnalyzer.analyze(&StartupError::new(StartupPhase::Container, e))
    }

    fn check(e: SecurityBuildError, description: &str, action: &str) {
        let a = analyze(e).unwrap();
        assert_eq!(a.description(), description);
        assert_eq!(a.action(), Some(action));
    }

    #[test]
    fn no_authorize_section_text() {
        check(
            SecurityBuildError::NoAuthorizeSection,
            "The security configuration has no authorize section.",
            "Call `authorize` and add at least one rule.",
        );
    }

    #[test]
    fn matcher_after_any_request_text() {
        check(
            SecurityBuildError::MatcherAfterAnyRequest,
            "A security rule follows `any_request()`, where it can never match.",
            "Put `any_request()` last.",
        );
    }

    #[test]
    fn resource_server_without_decoder_text() {
        check(
            SecurityBuildError::ResourceServerWithoutDecoder,
            "`oauth2_resource_server` is configured without a JWT decoder.",
            "Configure a decoder (static keys or JWKS).",
        );
    }

    #[test]
    fn duplicate_authorize_section_text() {
        check(
            SecurityBuildError::DuplicateAuthorizeSection,
            "`authorize` was called more than once.",
            "Put all rules in one `authorize` call.",
        );
    }

    #[test]
    fn invalid_pattern_text() {
        check(
            SecurityBuildError::InvalidPattern,
            "A security matcher pattern is not valid. Patterns use Ant syntax, and `*` or `**` is allowed only as a whole path segment; `{id}`, `?` and partial wildcards such as `*.pdf` are not supported.",
            "Use `/users/*` or `/users/**` and match the rest in a handler.",
        );
    }

    #[test]
    fn http_basic_without_provider_text() {
        check(
            SecurityBuildError::HttpBasicWithoutProvider,
            "`http_basic` is configured without a password provider.",
            "Add one with `password_provider`, for example a `DaoAuthenticationProvider`.",
        );
    }

    #[test]
    fn invalid_realm_text() {
        check(
            SecurityBuildError::InvalidRealm,
            "The HTTP Basic realm contains a double quote, a backslash or a control character.",
            "Use a realm of plain printable text, such as `Oxbloom`.",
        );
    }

    #[test]
    fn form_login_without_provider_text() {
        check(
            SecurityBuildError::FormLoginWithoutProvider,
            "`form_login` is configured without a password provider.",
            "Add one with `password_provider`, for example a `DaoAuthenticationProvider`.",
        );
    }

    #[test]
    fn invalid_form_login_url_text() {
        check(
            SecurityBuildError::InvalidFormLoginUrl,
            "A form login URL is not a safe local path, two form login URLs collide, or a form field name is not made of ASCII letters, digits, `_` and `-`.",
            "Use paths that start with a single `/`, a logout URL that differs from the login URLs, and plain field names such as `username`.",
        );
    }

    #[test]
    fn csrf_without_form_login_text() {
        check(
            SecurityBuildError::CsrfWithoutFormLogin,
            "CSRF protection is configured but form login is not.",
            "Configure `form_login`, or turn CSRF protection off.",
        );
    }

    #[test]
    fn invalid_csrf_config_text() {
        check(
            SecurityBuildError::InvalidCsrfConfig,
            "A CSRF field or header name is not made of ASCII letters, digits, `_` and `-`, or the token endpoint is not a safe local path or collides with a form login URL.",
            "Use plain names such as `_csrf` and `X-CSRF-TOKEN`, and a token endpoint that starts with a single `/` and differs from the form login URLs.",
        );
    }

    #[test]
    fn invalid_headers_config_text() {
        check(
            SecurityBuildError::InvalidHeadersConfig,
            "A security header value is empty or has a character outside visible ASCII, or the HSTS max age is zero.",
            "Use plain printable header values and an HSTS max age of at least one second.",
        );
    }

    #[test]
    fn session_limit_without_form_login_text() {
        check(
            SecurityBuildError::SessionLimitWithoutFormLogin,
            "A session limit is configured but form login is not.",
            "Configure `form_login`, or remove the session limit.",
        );
    }

    #[test]
    fn invalid_session_limit_text() {
        check(
            SecurityBuildError::InvalidSessionLimit,
            "The session limit is zero.",
            "Allow at least one session per user, or remove the session limit.",
        );
    }

    #[test]
    fn revalidate_without_form_login_text() {
        check(
            SecurityBuildError::RevalidateWithoutFormLogin,
            "Session revalidation is configured but form login is not.",
            "Configure `form_login`, or remove the revalidation.",
        );
    }

    #[test]
    fn invalid_revalidate_interval_text() {
        check(
            SecurityBuildError::InvalidRevalidateInterval,
            "The session revalidation interval is zero.",
            "Use an interval of at least one second, or leave revalidation out to turn it off.",
        );
    }

    #[test]
    fn remember_me_without_form_login_text() {
        check(
            SecurityBuildError::RememberMeWithoutFormLogin,
            "Remember-me is configured but form login is not.",
            "Configure `form_login`, or remove remember-me.",
        );
    }

    #[test]
    fn invalid_remember_me_text() {
        check(
            SecurityBuildError::InvalidRememberMe,
            "A remember-me setting is missing or out of range: a secret shorter than 32 bytes, a missing secret or user service, a lifetime outside 1 minute to 365 days, a cookie name that is not an HTTP token, or a field name not made of ASCII letters, digits, `_` and `-`.",
            "Set secrets of at least 32 bytes and a `UserDetailsService`, a lifetime between 1 minute and 365 days, and plain names such as `OXBLOOM_REMEMBER_ME` and `remember-me`.",
        );
    }

    #[test]
    fn oauth2_login_texts() {
        check(
            SecurityBuildError::OAuth2LoginWithoutFormLogin,
            "OAuth2 login is configured but form login is not.",
            "Configure `form_login` as well (in boot, set `security.form-login.enabled = true`); it gives OAuth2 login the session, the logout, the saved request and the success URL, and needs no password provider then.",
        );
        check(
            SecurityBuildError::OAuth2LoginWithoutRegistration,
            "OAuth2 login has no usable registration: none was added, an id has a character outside `a` to `z`, `0` to `9` and `-`, or a registration names a provider that was not added.",
            "Add at least one registration, use ids such as `github`, and add the provider that each registration names.",
        );
        check(
            SecurityBuildError::OAuth2LoginWithoutBaseUrl,
            "OAuth2 login has no valid base URL for its redirect URI.",
            "Set the external base URL of the application, an absolute `https` URL (`http` only for a loopback host) without a query or a fragment, such as `https://app.example`.",
        );
        check(
            SecurityBuildError::OAuth2LoginWithoutClient,
            "OAuth2 login has no HTTP client for the provider requests.",
            "Pass an `HttpClient` to `OAuth2Login::new` or `OAuth2Login::client`.",
        );
        check(
            SecurityBuildError::InvalidOAuth2Login,
            "An OAuth2 login setting is out of range: a redirect URI is not `{base-url}/login/oauth2/code/{id}` of its own registration, or the pending login max age is zero.",
            "Leave `redirect-uri` at its default or point it at `{base-url}/login/oauth2/code/{id}`, and use a pending login max age above zero.",
        );
    }

    fn analyze_session(e: SessionManagementError) -> FailureAnalysis {
        let di = DiError::creation_from::<u8>(e);
        SessionManagementAnalyzer
            .analyze(&StartupError::new(StartupPhase::Container, di))
            .expect("an analysis")
    }

    #[test]
    fn session_management_texts_name_the_key() {
        let a = analyze_session(SessionManagementError::LimitWithoutIndex);
        assert_eq!(
            a.description(),
            "`security.session.max-sessions-per-user` is set, but the session store cannot list the sessions of a user."
        );
        assert_eq!(
            a.action(),
            Some(
                "Use the in-memory store (`security.session.store = \"memory\"`) or a store that implements `PrincipalSessions`, or remove `security.session.max-sessions-per-user`."
            )
        );
        let a = analyze_session(SessionManagementError::RevalidateWithoutUserService);
        assert_eq!(
            a.description(),
            "`security.session.revalidate-interval` is set, but there is no `UserDetailsService` bean to load the user again."
        );
        assert_eq!(
            a.action(),
            Some(
                "Add users under `security.users` or register a `dyn UserDetailsService` bean, or set `security.session.revalidate-interval = \"0s\"` to turn revalidation off."
            )
        );
        let a = analyze_session(SessionManagementError::RegistryWithoutIndex);
        assert!(a.description().contains("`SessionRegistry`"));
        assert!(a.action().unwrap().contains("PrincipalSessions"));
        assert!(
            SessionManagementAnalyzer
                .analyze(&StartupError::new(StartupPhase::Container, std::fmt::Error))
                .is_none()
        );
    }

    fn analyze_remember_me(e: RememberMeError) -> FailureAnalysis {
        let di = DiError::creation_from::<u8>(e);
        RememberMeAnalyzer
            .analyze(&StartupError::new(StartupPhase::Container, di))
            .expect("an analysis")
    }

    #[test]
    fn remember_me_texts_name_the_key() {
        let a = analyze_remember_me(RememberMeError::WithoutFormLogin);
        assert_eq!(
            a.description(),
            "`security.remember-me.enabled` is `true`, but form login is off."
        );
        assert_eq!(
            a.action(),
            Some("Set `security.form-login.enabled = true`, or turn remember-me off.")
        );
        let a = analyze_remember_me(RememberMeError::WithoutSecret);
        assert!(a.description().contains("`security.remember-me.secret`"));
        assert!(a.action().unwrap().contains("never on the command line"));
        let a = analyze_remember_me(RememberMeError::WithoutUserService);
        assert!(a.description().contains("`security.remember-me.enabled`"));
        assert!(
            a.action()
                .unwrap()
                .contains("`dyn UserDetailsService` bean")
        );
        let a = analyze_remember_me(RememberMeError::CookieNameClash);
        assert!(
            a.description()
                .contains("`security.remember-me.cookie-name`")
        );
        assert!(a.action().unwrap().contains("OXBLOOM_REMEMBER_ME"));
        assert!(
            RememberMeAnalyzer
                .analyze(&StartupError::new(StartupPhase::Container, std::fmt::Error))
                .is_none()
        );
    }

    #[cfg(feature = "oauth2")]
    fn analyze_oauth2(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Option<FailureAnalysis> {
        OAuth2Analyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    #[cfg(feature = "oauth2")]
    #[test]
    fn oauth2_texts_name_the_key_and_never_the_message() {
        use crate::autoconfigure::OAuth2LoginError;

        let di = DiError::creation_from::<u8>(OAuth2LoginError::WithoutFormLogin);
        let a = analyze_oauth2(di).unwrap();
        assert!(a.description().contains("form login is off"));
        assert_eq!(
            a.action(),
            Some(
                "Set `security.form-login.enabled = true` (OAuth2 login needs no password provider then), or remove the registrations."
            )
        );
        let di = DiError::creation_from::<u8>(OAuth2LoginError::WithoutHttpClient);
        let a = analyze_oauth2(di).unwrap();
        assert!(a.description().contains("`HttpClient`"));
        assert!(a.action().unwrap().contains("HttpClientAutoConfiguration"));

        for (key, expected) in [
            ("security.oauth2.base-url", "no path, query or fragment"),
            ("security.oauth2.failure-url", "local path"),
            (
                "security.oauth2.client.registration.idp.redirect-uri",
                "`{base-url}/login/oauth2/code/{id}`",
            ),
            (
                "security.oauth2.client.registration.idp.client-secret",
                "never on the command line",
            ),
            (
                "security.oauth2.client.provider.idp.issuer-uri",
                "issuer-uri",
            ),
            ("security.oauth2", "base-url"),
        ] {
            let di: DiError = ConfigError::invalid(key, "hunter2-value").into();
            let a = analyze_oauth2(di).unwrap();
            assert_eq!(
                a.description(),
                format!("The OAuth2 configuration key `{key}` is not valid.")
            );
            assert!(a.action().unwrap().contains(expected), "{key}: {a}");
            assert!(!a.to_string().contains("hunter2"), "{a}");
        }
        assert!(analyze_oauth2(ConfigError::invalid("security.oauth2x", "bad")).is_none());
        assert!(analyze_oauth2(ConfigError::invalid("security.jwt.secret", "bad")).is_none());
        assert!(
            analyze_oauth2(ConfigError::Missing {
                key: "security.oauth2.base-url".into()
            })
            .is_none()
        );
        assert!(analyze_oauth2(std::fmt::Error).is_none());
    }

    #[test]
    fn none_without_security_error() {
        let other = StartupError::new(StartupPhase::Container, std::fmt::Error);
        assert!(SecurityAnalyzer.analyze(&other).is_none());
    }
}
