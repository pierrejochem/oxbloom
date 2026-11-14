//! The SSL analyzer: bundles that cannot be loaded and an unusable `server.ssl.bundle`.

use oxbloom_config::ConfigError;
use oxbloom_ssl::SslError;

use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// The key whose `ConfigError::Invalid` this analyzer explains, next to every `SslError`.
const SERVER_SSL_BUNDLE: &str = "server.ssl.bundle";

/// The valid keys of a bundle, for the action of an invalid `ssl.*` key.
const VALID_KEYS: &str = "`ssl.reload-interval` and `ssl.expiry-warning` take a duration such \
                          as `30s`; a `ssl.bundle.pem.<name>` section takes `certificate`, \
                          `private-key` and `trust`; a `ssl.bundle.pkcs12.<name>` section \
                          takes `path` and `password`.";

/// Recognises an `SslError` anywhere in the cause chain, and a `ConfigError::Invalid` on
/// `server.ssl.bundle`. The report names the bundle and the key to fix with a fixed text. It
/// never shows a private key, a password, file content or a file path: an `SslError` carries
/// none, and the message of an invalid `ssl.*` key is left out because it could echo a value.
/// The message of `server.ssl.bundle` is shown, because the web server writes it with bundle
/// names only.
pub(crate) struct SslAnalyzer;

impl FailureAnalyzer for SslAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        if let Some(error) = failure.find_cause::<SslError>() {
            return Some(ssl_error(error));
        }
        let ConfigError::Invalid { key, message, .. } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        if key != SERVER_SSL_BUNDLE {
            return None;
        }
        Some(FailureAnalysis::new(
            format!("The web server's `{SERVER_SSL_BUNDLE}` is not usable: it {message}."),
            format!(
                "Set `{SERVER_SSL_BUNDLE}` to the name of a bundle with a certificate and a \
                 private key, defined under `ssl.bundle.pem.<name>` or \
                 `ssl.bundle.pkcs12.<name>`, or remove it to serve plain HTTP."
            ),
        ))
    }
}

/// The report of one `SslError`.
fn ssl_error(error: &SslError) -> FailureAnalysis {
    match error {
        SslError::Io { bundle, key, kind } => FailureAnalysis::new(
            format!("SSL bundle `{bundle}` cannot read the file of `{key}` ({kind})."),
            format!("Fix `{key}`: check that the file exists and that the process may read it."),
        ),
        SslError::Pem { bundle, key } => FailureAnalysis::new(
            format!("`{key}` of SSL bundle `{bundle}` holds no usable PEM data."),
            format!(
                "Fix `{key}`: give a file path or inline PEM text with an unencrypted \
                 certificate chain, or with an unencrypted PKCS#8, PKCS#1 or SEC1 private key \
                 for `private-key`."
            ),
        ),
        SslError::NoIdentity { bundle } => FailureAnalysis::new(
            format!("SSL bundle `{bundle}` has no certificate and private key."),
            format!(
                "Set `ssl.bundle.pem.{bundle}.certificate` and \
                 `ssl.bundle.pem.{bundle}.private-key`, or use a PKCS#12 file with a key under \
                 `ssl.bundle.pkcs12.{bundle}.path`."
            ),
        ),
        SslError::KeyMismatch { bundle } => FailureAnalysis::new(
            format!("The private key of SSL bundle `{bundle}` does not match its certificate."),
            format!(
                "Fix `ssl.bundle.pem.{bundle}.private-key` or \
                 `ssl.bundle.pem.{bundle}.certificate` (or the file of \
                 `ssl.bundle.pkcs12.{bundle}.path`) so that the key belongs to the first \
                 certificate of the chain."
            ),
        ),
        SslError::Pkcs12 { bundle } => FailureAnalysis::new(
            format!("The PKCS#12 file of SSL bundle `{bundle}` cannot be decoded."),
            format!(
                "Fix `ssl.bundle.pkcs12.{bundle}.path` or `ssl.bundle.pkcs12.{bundle}.password`: \
                 the password is wrong, the file is damaged or its format is not supported."
            ),
        ),
        SslError::EmptyBundle { bundle } => FailureAnalysis::new(
            format!("SSL bundle `{bundle}` has neither an identity nor a trust store."),
            format!(
                "Set `certificate` and `private-key`, `trust`, or all three under \
                 `ssl.bundle.pem.{bundle}`, or `path` under `ssl.bundle.pkcs12.{bundle}`."
            ),
        ),
        SslError::DuplicateName { name } => FailureAnalysis::new(
            format!(
                "SSL bundle name `{name}` is used under both `ssl.bundle.pem` and \
                 `ssl.bundle.pkcs12`."
            ),
            format!("Rename `ssl.bundle.pem.{name}` or `ssl.bundle.pkcs12.{name}`."),
        ),
        SslError::UnknownBundle { name, available } => {
            let list = if available.is_empty() {
                "none".to_owned()
            } else {
                available.join(", ")
            };
            FailureAnalysis::new(
                format!("No SSL bundle is named `{name}`; available bundles: {list}."),
                "Use one of the available names, or add the bundle under \
                 `ssl.bundle.pem.<name>` or `ssl.bundle.pkcs12.<name>`.",
            )
        }
        SslError::Config(ConfigError::Missing { key }) => FailureAnalysis::new(
            format!("The SSL configuration key `{key}` is not set."),
            format!(
                "Set `{key}`: `certificate` and `private-key` of a PEM bundle go together, so \
                 set both or neither. {VALID_KEYS}"
            ),
        ),
        SslError::Config(ConfigError::Invalid { key, .. }) => FailureAnalysis::new(
            format!("The SSL configuration key `{key}` is not valid."),
            format!("Fix `{key}`: {VALID_KEYS}"),
        ),
        SslError::Config(_) => FailureAnalysis::new(
            "The `ssl` configuration cannot be read.",
            format!("Fix the `ssl` section: {VALID_KEYS}"),
        ),
        // A variant added later: its `Display` text is fixed and carries no secret.
        other => FailureAnalysis::with_no_action(format!("{other}.")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;

    use oxbloom_di::DiError;

    use super::*;
    use crate::error::StartupPhase;

    fn analyze(source: impl std::error::Error + Send + Sync + 'static) -> Option<FailureAnalysis> {
        SslAnalyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    /// The `SslError` behind a `DiError::Creation`, as the bean factory reports it.
    fn created(error: SslError) -> DiError {
        DiError::creation_from::<oxbloom_ssl::SslBundles>(error)
    }

    #[test]
    fn io_error_names_the_key_and_the_kind() {
        let a = analyze(created(SslError::Io {
            bundle: "web".into(),
            key: "ssl.bundle.pem.web.certificate".into(),
            kind: ErrorKind::NotFound,
        }))
        .unwrap();
        assert_eq!(
            a.description(),
            "SSL bundle `web` cannot read the file of `ssl.bundle.pem.web.certificate` \
             (entity not found)."
        );
        assert!(
            a.action()
                .unwrap()
                .starts_with("Fix `ssl.bundle.pem.web.certificate`")
        );
    }

    #[test]
    fn pem_error_names_the_key() {
        let a = analyze(SslError::Pem {
            bundle: "web".into(),
            key: "ssl.bundle.pem.web.private-key".into(),
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "`ssl.bundle.pem.web.private-key` of SSL bundle `web` holds no usable PEM data."
        );
        assert!(
            a.to_string()
                .contains("Fix `ssl.bundle.pem.web.private-key`")
        );
    }

    #[test]
    fn every_bundle_error_names_a_key_of_the_bundle() {
        for error in [
            SslError::NoIdentity { bundle: "b".into() },
            SslError::KeyMismatch { bundle: "b".into() },
            SslError::Pkcs12 { bundle: "b".into() },
            SslError::EmptyBundle { bundle: "b".into() },
            SslError::DuplicateName { name: "b".into() },
        ] {
            let text = analyze(error).unwrap().to_string();
            assert!(
                text.contains("ssl.bundle.pem.b") || text.contains("ssl.bundle.pkcs12.b"),
                "{text}"
            );
        }
    }

    #[test]
    fn unknown_bundle_lists_the_available_names() {
        let a = analyze(SslError::UnknownBundle {
            name: "api".into(),
            available: vec!["client".into(), "web".into()],
        })
        .unwrap();
        assert_eq!(
            a.description(),
            "No SSL bundle is named `api`; available bundles: client, web."
        );
        let none = analyze(SslError::UnknownBundle {
            name: "api".into(),
            available: Vec::new(),
        })
        .unwrap();
        assert!(none.description().ends_with("available bundles: none."));
    }

    #[test]
    fn invalid_ssl_key_never_shows_the_message() {
        let error = SslError::Config(ConfigError::invalid(
            "ssl.bundle.pem.web.private-key",
            "hunter2",
        ));
        let a = analyze(created(error)).unwrap();
        assert_eq!(
            a.description(),
            "The SSL configuration key `ssl.bundle.pem.web.private-key` is not valid."
        );
        assert!(
            a.action()
                .unwrap()
                .contains("`certificate`, `private-key` and `trust`")
        );
        assert!(!a.to_string().contains("hunter2"));
    }

    #[test]
    fn missing_half_of_an_identity_names_the_key() {
        let error = SslError::Config(ConfigError::Missing {
            key: "ssl.bundle.pem.web.private-key".into(),
        });
        let a = analyze(error).unwrap();
        assert_eq!(
            a.description(),
            "The SSL configuration key `ssl.bundle.pem.web.private-key` is not set."
        );
        assert!(a.action().unwrap().contains("set both or neither"));
    }

    #[test]
    fn server_ssl_bundle_shows_the_names_of_its_message() {
        let di: DiError = ConfigError::invalid(
            "server.ssl.bundle",
            "names an SSL bundle that does not exist; available bundles: web",
        )
        .into();
        let a = analyze(di).unwrap();
        assert_eq!(
            a.description(),
            "The web server's `server.ssl.bundle` is not usable: it names an SSL bundle that \
             does not exist; available bundles: web."
        );
        assert!(
            a.action()
                .unwrap()
                .contains("remove it to serve plain HTTP")
        );
    }

    #[test]
    fn other_errors_are_none() {
        assert!(analyze(ConfigError::invalid("server.port", "bad")).is_none());
        assert!(analyze(ConfigError::invalid("server.ssl", "bad")).is_none());
        assert!(
            analyze(ConfigError::Missing {
                key: "server.ssl.bundle".into()
            })
            .is_none()
        );
        assert!(analyze(std::io::Error::other("x")).is_none());
    }
}
