//! The security analyzers: security build errors and invalid `security.jwt` keys.

use oxbloom_config::ConfigError;
use oxbloom_security::SecurityBuildError;

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
    fn none_without_security_error() {
        let other = StartupError::new(StartupPhase::Container, std::fmt::Error);
        assert!(SecurityAnalyzer.analyze(&other).is_none());
    }
}
