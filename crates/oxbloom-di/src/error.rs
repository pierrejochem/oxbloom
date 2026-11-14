use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// The underlying error of a [`DiError::Creation`], if any, shared behind an [`Arc`].
///
/// An `ErrorSource` either holds an error ([`ErrorSource::new`]) or is empty
/// ([`ErrorSource::none`], also the `Default`). Cloning it (and so a cached `DiError`) shares
/// the error instead of copying it. `Debug` prints `ErrorSource(..)` in both states, never the
/// error's content, and any two sources compare equal, held or empty, so a source never changes
/// the equality, `Debug` or `Display` of a `DiError`. Read the error with
/// [`ErrorSource::as_error`] or through [`std::error::Error::source`] on the `DiError`.
///
/// # Examples
///
/// ```
/// use oxbloom_di::ErrorSource;
///
/// let source = ErrorSource::new(std::fmt::Error);
/// assert!(source.as_error().and_then(|e| e.downcast_ref::<std::fmt::Error>()).is_some());
/// assert_eq!(format!("{source:?}"), "ErrorSource(..)");
/// assert_eq!(source, ErrorSource::none());
/// ```
#[derive(Clone, Default)]
pub struct ErrorSource(
    // `Arc<Box<..>>` keeps this a thin pointer, so `DiError` (and every `Result<_, DiError>`
    // frame on the recursive creation path) does not grow.
    Option<Arc<Box<dyn Error + Send + Sync>>>,
);

impl ErrorSource {
    /// Wraps `error` as the source of a `Creation` error.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{DiError, ErrorSource};
    ///
    /// struct Db;
    /// let err = DiError::Creation {
    ///     type_name: std::any::type_name::<Db>().to_owned(),
    ///     message: "cannot connect".to_owned(),
    ///     source: ErrorSource::new(std::io::Error::other("refused")),
    /// };
    /// assert!(std::error::Error::source(&err).is_some());
    /// ```
    pub fn new(error: impl Error + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(Box::new(error))))
    }

    /// An empty source, for a `Creation` error without a typed cause.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{DiError, ErrorSource};
    ///
    /// let err = DiError::Creation {
    ///     type_name: "app::Db".to_owned(),
    ///     message: "cannot connect".to_owned(),
    ///     source: ErrorSource::none(),
    /// };
    /// assert!(std::error::Error::source(&err).is_none());
    /// assert!(!ErrorSource::default().is_some());
    /// ```
    pub fn none() -> Self {
        Self(None)
    }

    /// Whether this source holds an error.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ErrorSource;
    ///
    /// assert!(ErrorSource::new(std::fmt::Error).is_some());
    /// assert!(!ErrorSource::none().is_some());
    /// ```
    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }

    /// The wrapped error, if any, for `downcast_ref` or walking its own `source()` chain.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::ErrorSource;
    ///
    /// let source = ErrorSource::new(std::io::Error::other("refused"));
    /// let io = source.as_error().and_then(|e| e.downcast_ref::<std::io::Error>());
    /// assert_eq!(io.map(|e| e.kind()), Some(std::io::ErrorKind::Other));
    /// assert!(ErrorSource::none().as_error().is_none());
    /// ```
    pub fn as_error(&self) -> Option<&(dyn Error + 'static)> {
        match &self.0 {
            Some(error) => Some(&***error),
            None => None,
        }
    }
}

/// Prints `ErrorSource(..)`, held or empty, never the wrapped error.
///
/// ```
/// use oxbloom_di::ErrorSource;
///
/// assert_eq!(format!("{:?}", ErrorSource::new(std::fmt::Error)), "ErrorSource(..)");
/// assert_eq!(format!("{:?}", ErrorSource::none()), "ErrorSource(..)");
/// ```
impl fmt::Debug for ErrorSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ErrorSource(..)")
    }
}

/// Always equal, held or empty: a source never changes the equality of a `DiError`.
///
/// ```
/// use oxbloom_di::ErrorSource;
///
/// assert_eq!(ErrorSource::new(std::fmt::Error), ErrorSource::new(std::io::Error::other("x")));
/// assert_eq!(ErrorSource::new(std::fmt::Error), ErrorSource::none());
/// ```
impl PartialEq for ErrorSource {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for ErrorSource {}

/// Wiring or construction failure reported by the container.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError, component};
///
/// #[component]
/// struct A { b: Arc<B> }
/// #[component]
/// struct B { a: Arc<A> }
///
/// let err = ContainerBuilder::new().component::<A>().component::<B>().build().err().unwrap();
/// assert!(matches!(err, DiError::Cycle { .. }));
///
/// let err = ContainerBuilder::new().instance(1u8).instance(2u8).build().err().unwrap();
/// assert!(matches!(err, DiError::Duplicate { .. }));
/// ```
///
/// The enum is `#[non_exhaustive]`: a `match` outside this crate needs a wildcard arm.
///
/// ```
/// use oxbloom_di::{ContainerBuilder, DiError};
///
/// let err = ContainerBuilder::new().build().unwrap().get::<u8>().err().unwrap();
/// let kind = match err {
///     DiError::Missing { .. } => "missing",
///     DiError::Cycle { .. } => "cycle",
///     DiError::Duplicate { .. } => "duplicate",
///     DiError::Creation { .. } => "creation",
///     _ => "other", // required: more variants may be added
/// };
/// assert_eq!(kind, "missing");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiError {
    /// `requested` is not registered; `required_by` is the build stack, outermost first.
    Missing {
        requested: String,
        required_by: Vec<String>,
    },
    /// Several providers match a single-value request and none is preferred.
    Ambiguous {
        requested: String,
        candidates: Vec<String>,
    },
    /// Circular dependency; `path` starts and ends with the repeated type.
    Cycle { path: Vec<String> },
    /// The same type was registered more than once; `sources` labels the clashing registrations.
    Duplicate {
        type_name: String,
        sources: Vec<String>,
    },
    /// A user factory, `Component::create` or lifecycle hook failed.
    ///
    /// `source` holds the typed underlying error when one was given (see
    /// [`DiError::creation_from`]) and is [`ErrorSource::none`] otherwise; the error is returned
    /// by [`std::error::Error::source`], and the field (held or empty) is ignored by equality and
    /// never printed by `Display` or `Debug`.
    Creation {
        type_name: String,
        message: String,
        source: ErrorSource,
    },
    /// A registration or request is malformed.
    Invalid { message: String },
    /// The container was shut down.
    ShutDown,
    /// The dependency chain exceeded `limit`; `path` is its truncated beginning.
    TooDeep { limit: usize, path: Vec<String> },
}

impl DiError {
    /// Builds a `Creation` error for `T` from a failure message.
    ///
    /// Return it from a factory or `Component::create` when construction fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Db;
    /// let result = ContainerBuilder::new()
    ///     .factory(|_| -> Result<Db, DiError> { Err(DiError::creation::<Db>("cannot connect")) })
    ///     .build();
    /// assert!(matches!(result, Err(DiError::Creation { .. })));
    /// ```
    pub fn creation<T: ?Sized>(message: impl Into<String>) -> Self {
        Self::Creation {
            type_name: std::any::type_name::<T>().to_owned(),
            message: message.into(),
            source: ErrorSource::none(),
        }
    }

    /// Builds a `Creation` error for `T` that keeps `source` as its typed cause.
    ///
    /// The message is `source.to_string()`: the source's `Display` becomes the message, so do not
    /// pass an error whose `Display` contains configuration values, argument values or secrets.
    /// The source is reachable through [`std::error::Error::source`], so a caller can
    /// `downcast_ref` it to the original type.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::error::Error;
    /// use oxbloom_di::{ContainerBuilder, DiError};
    ///
    /// struct Db;
    /// let err = ContainerBuilder::new()
    ///     .try_bean(|| -> Result<Db, DiError> {
    ///         Err(DiError::creation_from::<Db>(std::io::Error::other("refused")))
    ///     })
    ///     .build()
    ///     .err()
    ///     .unwrap();
    /// assert_eq!(err.to_string(), format!("failed to create {}: refused", std::any::type_name::<Db>()));
    /// let io = err.source().and_then(|s| s.downcast_ref::<std::io::Error>());
    /// assert!(io.is_some());
    /// ```
    pub fn creation_from<T: ?Sized>(source: impl Error + Send + Sync + 'static) -> Self {
        Self::Creation {
            type_name: std::any::type_name::<T>().to_owned(),
            message: source.to_string(),
            source: ErrorSource::new(source),
        }
    }
}

/// Human-readable message that names the types involved; a cycle prints its full path.
///
/// ```
/// use oxbloom_di::DiError;
///
/// let err = DiError::Duplicate { type_name: "app::Repo".into(), sources: Vec::new() };
/// assert!(err.to_string().contains("app::Repo"));
/// ```
impl fmt::Display for DiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing {
                requested,
                required_by,
            } if required_by.is_empty() => {
                write!(f, "no component registered for {requested}")
            }
            Self::Missing {
                requested,
                required_by,
            } => write!(
                f,
                "no component registered for {requested} (required by {})",
                required_by.join(" -> ")
            ),
            Self::Cycle { path } => write!(f, "dependency cycle: {}", path.join(" -> ")),
            Self::Ambiguous {
                requested,
                candidates,
            } => write!(
                f,
                "more than one candidate for {requested}: {}",
                candidates.join(", ")
            ),
            Self::Duplicate { type_name, .. } => {
                write!(f, "{type_name} is registered more than once")
            }
            Self::Creation {
                type_name, message, ..
            } => {
                write!(f, "failed to create {type_name}: {message}")
            }
            Self::Invalid { message } => write!(f, "invalid registration: {message}"),
            Self::ShutDown => write!(f, "container is shut down"),
            Self::TooDeep { limit, path } => write!(
                f,
                "dependency chain deeper than {limit}: {}",
                path.join(" -> ")
            ),
        }
    }
}

/// `source()` returns the typed cause of a `Creation` error, when it has one.
///
/// ```
/// use std::error::Error;
/// use oxbloom_di::DiError;
///
/// let err = DiError::creation_from::<u8>(std::fmt::Error);
/// assert!(err.source().unwrap().downcast_ref::<std::fmt::Error>().is_some());
/// assert!(DiError::creation::<u8>("boom").source().is_none());
/// ```
impl Error for DiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Creation { source, .. } => source.as_error(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_uses_type_name() {
        assert_eq!(
            DiError::creation::<String>("boom"),
            DiError::Creation {
                type_name: "alloc::string::String".into(),
                message: "boom".into(),
                source: ErrorSource::none(),
            }
        );
    }

    #[test]
    fn display_missing_lists_chain() {
        let e = DiError::Missing {
            requested: "B".into(),
            required_by: vec!["A".into(), "C".into()],
        };
        let s = e.to_string();
        assert!(s.contains('B') && s.contains('A') && s.contains('C'), "{s}");
        assert!(s.find('A').unwrap() < s.find('C').unwrap(), "{s}");
    }

    #[test]
    fn display_cycle_joins_with_arrows() {
        let e = DiError::Cycle {
            path: vec!["A".into(), "B".into(), "A".into()],
        };
        assert!(e.to_string().contains("A -> B -> A"));
    }

    #[test]
    fn display_duplicate_and_creation() {
        let d = DiError::Duplicate {
            type_name: "Foo".into(),
            sources: vec!["component Foo".into(), "instance Foo".into()],
        };
        assert!(d.to_string().contains("Foo"));
        let c = DiError::Creation {
            type_name: "Bar".into(),
            message: "oops".into(),
            source: ErrorSource::none(),
        };
        let s = c.to_string();
        assert!(s.contains("Bar") && s.contains("oops"), "{s}");
    }

    #[test]
    fn display_texts_are_fixed_and_value_free() {
        let ambiguous = DiError::Ambiguous {
            requested: "dyn T".into(),
            candidates: vec!["A".into(), "B".into()],
        };
        assert_eq!(
            ambiguous.to_string(),
            "more than one candidate for dyn T: A, B"
        );
        let dup = DiError::Duplicate {
            type_name: "Foo".into(),
            sources: vec!["component Foo".into(), "instance Foo".into()],
        };
        assert_eq!(dup.to_string(), "Foo is registered more than once");
        assert_eq!(
            DiError::Invalid {
                message: "bad".into()
            }
            .to_string(),
            "invalid registration: bad"
        );
        assert_eq!(DiError::ShutDown.to_string(), "container is shut down");
        let deep = DiError::TooDeep {
            limit: 256,
            path: vec!["A".into(), "B".into()],
        };
        assert_eq!(deep.to_string(), "dependency chain deeper than 256: A -> B");
    }

    #[test]
    fn error_is_non_exhaustive_and_send_sync_static() {
        fn assert_bounds<T: std::error::Error + Clone + Eq + Send + Sync + 'static>() {}
        assert_bounds::<DiError>();
        // Inside the defining crate a match may be exhaustive; `#[non_exhaustive]` is pinned by
        // the wildcard doc example on `DiError` (compiled as an external crate).
        let e = DiError::ShutDown;
        assert!(matches!(e, DiError::ShutDown));
    }
}
