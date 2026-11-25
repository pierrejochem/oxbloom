//! The data analyzers: an invalid `data` key or a source that cannot be built, and a source
//! that is not reachable at start.

use oxbloom_config::ConfigError;
use oxbloom_data::DataError;

use crate::autoconfigure::{DataUnreachable, SourceBuildError};
use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};

/// Recognises a `ConfigError::Invalid` for a key under `data` (`data.sources.` included)
/// anywhere in the cause chain, and a source the registry could not build. The report names
/// the key with a fixed text and leaves out the configuration error's message, so no value can
/// reach it. The one exception is an `ssl-bundle` key, whose messages are fixed texts that
/// list bundle names at most, so the report shows them. A source refused for
/// `ssl-mode = verify-ca` with a bundle that has a trust store is reported on its `ssl-mode`
/// key with the fixed text of the error.
pub(crate) struct DataConfigAnalyzer;

impl FailureAnalyzer for DataConfigAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        if let Some(build) = failure.find_cause::<SourceBuildError>() {
            return Some(build_failure(build));
        }
        let ConfigError::Invalid { key, message, .. } = failure.find_cause::<ConfigError>()? else {
            return None;
        };
        if key != "data" && !key.starts_with("data.") {
            return None;
        }
        if key.ends_with(".ssl-bundle") {
            return Some(FailureAnalysis::new(
                format!("The data source key `{key}` is not usable ({message})."),
                "Set it to the name of a bundle defined under `ssl.bundle.pem.<name>` or \
                 `ssl.bundle.pkcs12.<name>` (this needs the `ssl` feature), for a Postgres or \
                 MySQL source whose `ssl-mode` is not `disable`, or remove it.",
            ));
        }
        Some(FailureAnalysis::new(
            format!("The data source configuration key `{key}` is not valid."),
            "Fix the `data` section: `url` is required and starts with `postgres://`, \
             `postgresql://`, `mysql://`, `mariadb://` or `sqlite:`; `max-connections` is at \
             least 1 (1 for an in-memory SQLite URL without `cache=shared`) and not below \
             `min-connections`; `acquire-timeout`, `idle-timeout` and `max-lifetime` are \
             durations such as `5s` above zero; `ssl-mode` is `disable`, `prefer`, `require`, \
             `verify-ca` or `verify-full`, and `ssl-mode` and `ssl-bundle` are for Postgres and \
             MySQL only; a name under `data.sources` uses lowercase letters, digits and `-` \
             and is not `default`.",
        ))
    }
}

/// The report of a source the registry could not build: the source, its key and what to do,
/// by the kind of error.
fn build_failure(build: &SourceBuildError) -> FailureAnalysis {
    let name = build.error.source_name();
    let prefix = build.error.config_prefix();
    let key = format!("{prefix}.url");
    if build.error.error() == DataError::VerifyCaWithTrust {
        return FailureAnalysis::new(
            format!(
                "The data source key `{prefix}.ssl-mode` is not usable: {}.",
                build.error.error()
            ),
            format!(
                "Set `{prefix}.ssl-mode` to `verify-full`, or name in `{prefix}.ssl-bundle` a \
                 bundle without a trust store (for example one with only a client \
                 certificate)."
            ),
        );
    }
    let description = format!(
        "The data source `{name}` cannot be built: {}.",
        build.error.error()
    );
    let action = match build.error.error() {
        DataError::Unsupported => match build.driver {
            // The boot feature of a driver is `data-` and the driver's name.
            Some(driver) => format!(
                "`{key}` names the {driver} driver: enable the `data-{driver}` feature of \
                 `oxbloom-boot`, or point the source at a database of an enabled driver."
            ),
            None => format!(
                "Enable the `data-*` feature of `oxbloom-boot` for the driver that `{key}` \
                 names."
            ),
        },
        DataError::Tls => format!(
            "Check `{prefix}.ssl-mode` and `{prefix}.ssl-bundle`: the bundle needs a usable \
             trust store or client certificate."
        ),
        DataError::Pool => "Build the application on a Tokio runtime: the pools are created \
                            while the container is built and need the runtime."
            .to_owned(),
        _ => format!(
            "Check the options of `{key}`: the driver refused one of them. The URL is not shown \
             because it can contain a password."
        ),
    };
    FailureAnalysis::new(description, action)
}

/// Recognises a source that failed the start-up check. [`DataError::Connect`] and
/// [`DataError::Pool`] get the same report: sqlx retries a refused connection until the
/// acquire timeout, so an unreachable server can end in either. The report names the source
/// and its key and says to check the URL, the network and the credentials; it never shows the
/// URL or the password.
pub(crate) struct DataUnreachableAnalyzer;

impl FailureAnalyzer for DataUnreachableAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let unreachable = &failure.find_cause::<DataUnreachable>()?.0;
        let name = unreachable.source_name();
        let description = match unreachable.error() {
            DataError::Connect | DataError::Pool => format!(
                "The data source `{name}` is not reachable: no connection could be made during \
                 the start-up check."
            ),
            other => format!("The data source `{name}` failed the start-up check: {other}."),
        };
        Some(FailureAnalysis::new(
            description,
            format!(
                "Check the URL in `{}.url` (host, port, database name or file path), the \
                 network path to the database and the credentials. Set \
                 `data.connect-on-startup = false` to start without connecting.",
                unreachable.config_prefix()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{ServiceError, StartupPhase};
    use oxbloom_data::SourceError;
    use oxbloom_di::DiError;

    fn analyze(
        analyzer: &dyn FailureAnalyzer,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Option<FailureAnalysis> {
        analyzer.analyze(&StartupError::new(StartupPhase::Container, source))
    }

    #[test]
    fn an_invalid_data_key_is_named_and_the_message_is_not() {
        let di: DiError = ConfigError::invalid("data.sources.cache.url", "hunter2").into();
        let a = analyze(&DataConfigAnalyzer, di).unwrap();
        assert_eq!(
            a.description(),
            "The data source configuration key `data.sources.cache.url` is not valid."
        );
        assert!(!a.to_string().contains("hunter2"));
    }

    #[test]
    fn an_ssl_bundle_key_shows_the_message() {
        let di: DiError = ConfigError::invalid(
            "data.ssl-bundle",
            "names an SSL bundle that does not exist; available bundles: api, idp",
        )
        .into();
        let a = analyze(&DataConfigAnalyzer, di).unwrap();
        assert_eq!(
            a.description(),
            "The data source key `data.ssl-bundle` is not usable (names an SSL bundle that \
             does not exist; available bundles: api, idp)."
        );
    }

    #[test]
    fn verify_ca_with_a_trust_store_is_reported_on_the_ssl_mode_key() {
        let build = SourceBuildError {
            error: SourceError::new("orders", DataError::VerifyCaWithTrust),
            driver: Some(oxbloom_data::Driver::Postgres),
        };
        let di = DiError::creation_from::<oxbloom_data::DataSources>(build);
        let a = analyze(&DataConfigAnalyzer, di).unwrap();
        assert_eq!(
            a.description(),
            "The data source key `data.sources.orders.ssl-mode` is not usable: ssl-mode \
             verify-ca cannot be combined with an ssl-bundle that has a trust store because the \
             database drivers add the bundle's trust to the platform roots; use verify-full."
        );
        let action = a.action().unwrap();
        assert!(
            action.contains("`data.sources.orders.ssl-mode`"),
            "{action}"
        );
        assert!(action.contains("`verify-full`"), "{action}");
    }

    #[test]
    fn other_keys_and_errors_are_none() {
        assert!(
            analyze(
                &DataConfigAnalyzer,
                ConfigError::invalid("database.url", "x")
            )
            .is_none()
        );
        assert!(
            analyze(
                &DataConfigAnalyzer,
                ConfigError::invalid("http.client", "x")
            )
            .is_none()
        );
        assert!(
            analyze(
                &DataConfigAnalyzer,
                ConfigError::Missing {
                    key: "data.url".into()
                }
            )
            .is_none()
        );
        assert!(analyze(&DataUnreachableAnalyzer, DataError::Connect).is_none());
    }

    #[test]
    fn an_unreachable_source_is_named_with_its_key() {
        // A refused connection and a pool timeout get the same report.
        for error in [DataError::Connect, DataError::Pool] {
            let error = ServiceError::new(DataUnreachable(SourceError::new("orders", error)));
            let a = analyze(&DataUnreachableAnalyzer, error).unwrap();
            assert_eq!(
                a.description(),
                "The data source `orders` is not reachable: no connection could be made during \
                 the start-up check."
            );
            assert!(a.action().unwrap().contains("`data.sources.orders.url`"));
            assert!(a.action().unwrap().contains("credentials"));
        }
        let tls = ServiceError::new(DataUnreachable(SourceError::new("default", DataError::Tls)));
        let a = analyze(&DataUnreachableAnalyzer, tls).unwrap();
        assert_eq!(
            a.description(),
            "The data source `default` failed the start-up check: the TLS configuration of the \
             database is unusable."
        );
    }
}
