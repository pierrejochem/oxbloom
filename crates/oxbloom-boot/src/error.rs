use std::error::Error;
use std::fmt;

use crate::failure::FailureAnalysis;

type BoxError = Box<dyn Error + Send + Sync>;

/// The phase of the run in which startup failed.
///
/// `Display` prints the lowercase phase name.
///
/// ```
/// use oxbloom_boot::StartupPhase;
///
/// assert_eq!(StartupPhase::Container.to_string(), "container");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartupPhase {
    /// Loading configuration.
    Config,
    /// Building the dependency-injection container.
    Container,
    /// Running an `ApplicationRunner` or `CommandLineRunner`.
    Runner,
    /// Starting a `Service`.
    Service,
}

impl fmt::Display for StartupPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Config => "config",
            Self::Container => "container",
            Self::Runner => "runner",
            Self::Service => "service",
        })
    }
}

/// An Oxbloom application failed to start.
///
/// `Display` is `startup failed in the {phase} phase: {source}`. `source()`
/// returns the underlying error itself (a `ConfigError`, `DiError`,
/// `RunnerError` or `ServiceError` when produced by a run), so it can be
/// downcast. [`find_cause`](Self::find_cause) searches the whole chain, and
/// [`analysis`](Self::analysis) holds the [`FailureAnalysis`] that boot
/// attached, if an analyzer recognised the failure.
#[non_exhaustive]
pub struct StartupError {
    phase: StartupPhase,
    source: BoxError,
    /// Boxed so that `Result<_, StartupError>` stays small.
    analysis: Option<Box<FailureAnalysis>>,
}

impl StartupError {
    /// Creates an error for `phase` caused by `source`.
    ///
    /// ```
    /// use oxbloom_boot::{StartupError, StartupPhase};
    ///
    /// let e = StartupError::new(StartupPhase::Config, "missing file");
    /// assert_eq!(e.to_string(), "startup failed in the config phase: missing file");
    /// ```
    pub fn new(phase: StartupPhase, source: impl Into<BoxError>) -> Self {
        Self {
            phase,
            source: source.into(),
            analysis: None,
        }
    }

    /// The phase in which startup failed.
    ///
    /// ```
    /// use oxbloom_boot::{StartupError, StartupPhase};
    ///
    /// let e = StartupError::new(StartupPhase::Service, "x");
    /// assert_eq!(e.phase(), StartupPhase::Service);
    /// ```
    pub fn phase(&self) -> StartupPhase {
        self.phase
    }

    /// Returns the first error of type `E` in the cause chain: the source
    /// itself, then its `source()`, and so on. The walk stops after 64
    /// levels, so a cyclic chain cannot loop.
    ///
    /// ```
    /// use oxbloom_boot::{RunnerError, StartupError, StartupPhase};
    ///
    /// let e = StartupError::new(
    ///     StartupPhase::Runner,
    ///     RunnerError::new(std::io::Error::other("disk full")),
    /// );
    /// assert!(e.find_cause::<RunnerError>().is_some());
    /// assert_eq!(e.find_cause::<std::io::Error>().unwrap().to_string(), "disk full");
    /// assert!(e.find_cause::<std::fmt::Error>().is_none());
    /// ```
    pub fn find_cause<E: Error + 'static>(&self) -> Option<&E> {
        const MAX_DEPTH: usize = 64;
        let mut current: Option<&(dyn Error + 'static)> = Some(&*self.source);
        for _ in 0..MAX_DEPTH {
            let error = current?;
            if let Some(found) = error.downcast_ref::<E>() {
                return Some(found);
            }
            current = error.source();
        }
        None
    }

    /// The [`FailureAnalysis`] that boot attached before the `failed`
    /// listeners ran, or `None` when no analyzer recognised the failure or
    /// the error was not produced by a run.
    ///
    /// ```
    /// use oxbloom_boot::{FailureAnalysis, FailureAnalyzer, OxbloomApplication, StartupError};
    /// use oxbloom_config::Config;
    ///
    /// struct Always;
    /// impl FailureAnalyzer for Always {
    ///     fn analyze(&self, _: &StartupError) -> Option<FailureAnalysis> {
    ///         Some(FailureAnalysis::with_no_action("Something is misconfigured."))
    ///     }
    /// }
    ///
    /// # tokio::runtime::Builder::new_current_thread().build()?.block_on(async {
    /// // `Config` is registered by boot; registering it again fails.
    /// let err = OxbloomApplication::builder()
    ///     .with_config(Config::from_str("")?)
    ///     .configure(|b| b.instance(Config::from_str("").unwrap()))
    ///     .failure_analyzer(Always)
    ///     .output(std::io::sink())
    ///     .run()
    ///     .await
    ///     .unwrap_err();
    /// assert_eq!(err.analysis().unwrap().description(), "Something is misconfigured.");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # })?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn analysis(&self) -> Option<&FailureAnalysis> {
        self.analysis.as_deref()
    }

    pub(crate) fn set_analysis(&mut self, analysis: FailureAnalysis) {
        self.analysis = Some(Box::new(analysis));
    }
}

/// Prints the phase and the source only, never the analysis.
impl fmt::Debug for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartupError")
            .field("phase", &self.phase)
            .field("source", &self.source)
            .finish()
    }
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "startup failed in the {} phase: {}",
            self.phase, self.source
        )
    }
}

impl Error for StartupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}

macro_rules! forwarding_error {
    ($(#[$doc:meta])* $name:ident, $example:literal) => {
        $(#[$doc])*
        #[derive(Debug)]
        #[non_exhaustive]
        pub struct $name {
            source: BoxError,
        }

        impl $name {
            /// Wraps `source`.
            ///
            #[doc = concat!("```\nuse oxbloom_boot::", stringify!($name), ";\n")]
            #[doc = concat!("let e = ", stringify!($name), "::new(\"", $example, "\");")]
            #[doc = concat!("assert_eq!(e.to_string(), \"", $example, "\");\n```")]
            pub fn new(source: impl Into<BoxError>) -> Self {
                Self {
                    source: source.into(),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.source, f)
            }
        }

        impl Error for $name {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&*self.source)
            }
        }
    };
}

forwarding_error!(
    /// An error returned by a `Service` start or stop.
    ///
    /// `Display` forwards to the wrapped error, and `source()` returns the wrapped
    /// error itself, so you can downcast it to your own error type:
    ///
    /// ```
    /// use std::error::Error;
    /// use oxbloom_boot::ServiceError;
    ///
    /// let e = ServiceError::new(std::io::Error::other("port in use"));
    /// assert!(e.source().and_then(|s| s.downcast_ref::<std::io::Error>()).is_some());
    /// ```
    ServiceError,
    "port in use"
);

forwarding_error!(
    /// An error returned by an application or command-line runner.
    ///
    /// `Display` forwards to the wrapped error, and `source()` returns the wrapped
    /// error itself, so you can downcast it to your own error type:
    ///
    /// ```
    /// use std::error::Error;
    /// use oxbloom_boot::RunnerError;
    ///
    /// let e = RunnerError::new(std::io::Error::other("bad input"));
    /// assert!(e.source().and_then(|s| s.downcast_ref::<std::io::Error>()).is_some());
    /// ```
    RunnerError,
    "bad input"
);

/// The process exit status of a finished run.
///
/// ```
/// use oxbloom_boot::ExitStatus;
///
/// assert_eq!(ExitStatus::new(0).code(), 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExitStatus {
    code: i32,
}

impl ExitStatus {
    /// Creates a status with exit `code`.
    ///
    /// ```
    /// use oxbloom_boot::ExitStatus;
    ///
    /// assert_eq!(ExitStatus::new(2), ExitStatus::new(2));
    /// ```
    pub fn new(code: i32) -> Self {
        Self { code }
    }

    /// The exit code.
    ///
    /// ```
    /// use oxbloom_boot::ExitStatus;
    ///
    /// assert_eq!(ExitStatus::new(42).code(), 42);
    /// ```
    pub fn code(&self) -> i32 {
        self.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_phase_and_source() {
        let e = StartupError::new(StartupPhase::Container, "boom");
        assert_eq!(e.to_string(), "startup failed in the container phase: boom");
        for (p, n) in [
            (StartupPhase::Config, "config"),
            (StartupPhase::Runner, "runner"),
            (StartupPhase::Service, "service"),
        ] {
            assert_eq!(p.to_string(), n);
        }
    }

    #[test]
    fn source_returns_inner_error() {
        use std::error::Error;
        let e = StartupError::new(StartupPhase::Config, ServiceError::new("inner"));
        assert_eq!(e.phase(), StartupPhase::Config);
        assert_eq!(e.source().unwrap().to_string(), "inner");
        assert_eq!(RunnerError::new("r").to_string(), "r");
    }

    #[derive(Debug)]
    struct Mine;
    impl fmt::Display for Mine {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("mine")
        }
    }
    impl Error for Mine {}

    #[test]
    fn forwarding_errors_expose_the_wrapped_error_as_source() {
        let r = RunnerError::new(Mine);
        assert!(r.source().unwrap().downcast_ref::<Mine>().is_some());
        let s = ServiceError::new(Mine);
        assert!(s.source().unwrap().downcast_ref::<Mine>().is_some());
        assert_eq!(s.to_string(), "mine");
    }

    #[test]
    fn analysis_is_boxed_and_hidden_from_debug() {
        // Phase, boxed source and boxed analysis: the error stays small.
        assert!(std::mem::size_of::<StartupError>() <= 4 * std::mem::size_of::<usize>());
        let mut e = StartupError::new(StartupPhase::Config, "x");
        let before = format!("{e:?}");
        e.set_analysis(FailureAnalysis::new("analysis-marker", "act"));
        assert_eq!(format!("{e:?}"), before);
        assert!(!before.contains("analysis"));
        assert_eq!(e.to_string(), "startup failed in the config phase: x");
        assert_eq!(e.analysis().unwrap().description(), "analysis-marker");
    }

    #[test]
    fn exit_status_code() {
        assert_eq!(ExitStatus::new(7).code(), 7);
    }
}
