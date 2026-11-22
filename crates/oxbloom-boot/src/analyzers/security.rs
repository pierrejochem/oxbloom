//! The security analyzers: security build errors, the session management startup failures and
//! invalid `security.jwt` keys.

use oxbloom_config::ConfigError;
use oxbloom_security::SecurityBuildError;

use crate::autoconfigure::SessionManagementError;
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

    #[test]
    fn none_without_security_error() {
        let other = StartupError::new(StartupPhase::Container, std::fmt::Error);
        assert!(SecurityAnalyzer.analyze(&other).is_none());
    }
}
