//! The HTTP client analyzer: an invalid `http.client` key.

use oxbloom_config::ConfigError;

use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises a `ConfigError::Invalid` for a key in the `http.client` section anywhere in the
/// cause chain. The report names the key with a fixed text and never the configuration
/// error's message, so no value can reach it.
pub(crate) struct HttpClientConfigAnalyzer;

impl FailureAnalyzer for HttpClientConfigAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let ConfigError::Invalid { key, .. } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        if key != "http.client" && !key.starts_with("http.client.") {
            return None;
        }
        Some(FailureAnalysis::new(
            format!("The HTTP client configuration key `{key}` is not valid."),
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
