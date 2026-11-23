//! The HTTP client analyzer: an invalid `http.client` key.

use oxbloom_config::ConfigError;

use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// The key whose message the report shows: the auto-configuration writes it with bundle
/// names only.
const SSL_BUNDLE: &str = "http.client.ssl-bundle";

/// Recognises a `ConfigError::Invalid` for a key in the `http.client` section anywhere in the
/// cause chain. The report names the key with a fixed text and leaves out the configuration
/// error's message, so no value can reach it. The one exception is `http.client.ssl-bundle`,
/// whose message `HttpClientAutoConfiguration` writes itself with the available bundle names
/// and never a value, a path or file content, so the report shows it.
pub(crate) struct HttpClientConfigAnalyzer;

impl FailureAnalyzer for HttpClientConfigAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let ConfigError::Invalid { key, message, .. } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        if key != "http.client" && !key.starts_with("http.client.") {
            return None;
        }
        if key == SSL_BUNDLE {
            return Some(FailureAnalysis::new(
                format!("The HTTP client's `{SSL_BUNDLE}` is not usable: it {message}."),
                format!(
                    "Set `{SSL_BUNDLE}` to the name of a bundle defined under \
                     `ssl.bundle.pem.<name>` or `ssl.bundle.pkcs12.<name>` (this needs the \
                     `ssl` feature), or remove it to use the platform certificate verifier."
                ),
            ));
        }
        let description = if key == "http.client" {
            "The HTTP client cannot be built from the `http.client` section.".to_owned()
        } else {
            format!("The HTTP client configuration key `{key}` is not valid.")
        };
        Some(FailureAnalysis::new(
            description,
            "Fix the `http.client` section: `timeout` and `connect-timeout` are durations such \
             as `5s` above zero; `max-body-bytes` is at least 1; `follow-redirects` is a count; \
             `user-agent` is a non-empty header value; `ssl-bundle` names a bundle of \
             `ssl.bundle.*` and needs the `ssl` feature; `retry.max-attempts` is at least 1, \
             `retry.backoff` is fixed or exponential, `retry.initial` and `retry.max-delay` are \
             durations with `initial` not above `max-delay`, `retry.multiplier` is at least 1.0 \
             and `retry.jitter` is none, full or equal.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StartupPhase;
    use oxbloom_di::DiError;

    fn analyze(source: impl std::error::Error + Send + Sync + 'static) -> Option<FailureAnalysis> {
        HttpClientConfigAnalyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    #[test]
    fn an_invalid_http_client_key_is_named_and_the_message_is_not() {
        let di: DiError = ConfigError::invalid("http.client.timeout", "hunter2").into();
        let a = analyze(di).unwrap();
        assert_eq!(
            a.description(),
            "The HTTP client configuration key `http.client.timeout` is not valid."
        );
        assert!(
            a.action()
                .unwrap()
                .contains("`retry.max-attempts` is at least 1")
        );
        assert!(!a.to_string().contains("hunter2"));
    }

    #[test]
    fn an_unknown_ssl_bundle_shows_the_message_with_the_available_bundles() {
        let di: DiError = ConfigError::invalid(
            "http.client.ssl-bundle",
            "names an SSL bundle that does not exist; available bundles: api, idp",
        )
        .into();
        let a = analyze(di).unwrap();
        assert_eq!(
            a.description(),
            "The HTTP client's `http.client.ssl-bundle` is not usable: it names an SSL bundle \
             that does not exist; available bundles: api, idp."
        );
        assert!(a.action().unwrap().contains("ssl.bundle.pem.<name>"));
    }

    #[test]
    fn a_build_failure_names_the_section() {
        let di: DiError =
            ConfigError::invalid("http.client", "the HTTP client cannot be built").into();
        let a = analyze(di).unwrap();
        assert_eq!(
            a.description(),
            "The HTTP client cannot be built from the `http.client` section."
        );
    }

    #[test]
    fn other_keys_and_errors_are_none() {
        assert!(analyze(ConfigError::invalid("http.clientx", "bad")).is_none());
        assert!(analyze(ConfigError::invalid("http.server.port", "bad")).is_none());
        assert!(
            analyze(ConfigError::Missing {
                key: "http.client.timeout".into()
            })
            .is_none()
        );
    }
}
