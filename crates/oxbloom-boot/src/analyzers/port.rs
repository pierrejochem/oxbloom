//! The port-in-use analyzer.

use std::io::ErrorKind;

use crate::error::StartupError;
use crate::failure::{FailureAnalysis, FailureAnalyzer};
use crate::web::BindError;

/// Recognises a `BindError` of kind `AddrInUse` anywhere in the cause chain.
pub(crate) struct PortInUseAnalyzer;

impl FailureAnalyzer for PortInUseAnalyzer {
    fn analyze(&self, failure: &StartupError) -> Option<FailureAnalysis> {
        let bind = failure.find_cause::<BindError>()?;
        if bind.kind() != ErrorKind::AddrInUse {
            return None;
        }
        Some(FailureAnalysis::new(
            format!(
                "The web server could not listen on `{}` because the port is already in use.",
                bind.address()
            ),
            "Stop the process that uses the port, or set `server.port` to a free port (0 picks one).",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ServiceError;
    use crate::error::StartupPhase;
    use std::io;

    fn analyze(kind: ErrorKind) -> Option<FailureAnalysis> {
        let address = "127.0.0.1:8080".parse().unwrap();
        let bind = BindError::new(address, io::Error::from(kind));
        PortInUseAnalyzer.analyze(&StartupError::new(
            StartupPhase::Service,
            ServiceError::new(bind),
        ))
    }

    #[test]
    fn addr_in_use_text_is_exact() {
        let a = analyze(ErrorKind::AddrInUse).unwrap();
        assert_eq!(
            a.description(),
            "The web server could not listen on `127.0.0.1:8080` because the port is already in use."
        );
        assert_eq!(
            a.action(),
            Some(
                "Stop the process that uses the port, or set `server.port` to a free port (0 picks one)."
            )
        );
    }

    #[test]
    fn other_kinds_return_none() {
        assert!(analyze(ErrorKind::PermissionDenied).is_none());
        assert!(analyze(ErrorKind::AddrNotAvailable).is_none());
    }

    #[test]
    fn bind_error_display_has_no_address() {
        let address = "127.0.0.1:8080".parse().unwrap();
        let e = BindError::new(address, io::Error::from(ErrorKind::AddrInUse));
        for text in [e.to_string(), format!("{e:?}")] {
            assert!(!text.contains("8080"), "{text}");
            assert!(!text.contains("127.0.0.1"), "{text}");
        }
        assert!(
            e.to_string()
                .starts_with("the web server cannot bind its listener: ")
        );
    }
}
