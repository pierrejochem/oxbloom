//! Support for the code `#[configuration]` generates: one registration per `#[bean]` method.
//! Reached through the hidden `oxbloom_di::__private`; not a public API.

use std::any::type_name;
use std::sync::Arc;

use crate::entry::erase;
use crate::{ContainerBuilder, DiError, Resolver};

/// Registers the bean `T` of the `#[bean]` method `method` of the configuration `C`, created by
/// the generated adapter `f`, with the source label `C::method`.
pub fn bean<C, T, F>(builder: ContainerBuilder, method: &'static str, f: F) -> ContainerBuilder
where
    C: 'static,
    T: Send + Sync + 'static,
    F: Fn(&Resolver<'_>) -> Result<T, DiError> + Send + Sync + 'static,
{
    let label = format!("{}::{method}", type_name::<C>());
    builder.push_labelled::<T>(
        label,
        Box::new(move |r| match f(r) {
            Ok(value) => Ok(erase(Arc::new(value))),
            Err(e) => Err(e),
        }),
    )
}

/// The return type of a `#[bean(fallible)]` method: `Result<T, DiError>`, whose bean is `T`.
#[diagnostic::on_unimplemented(
    message = "a `#[bean(fallible)]` method must return `Result<T, DiError>`, not `{Self}`",
    label = "not `Result<T, DiError>`",
    note = "drop `fallible` to register `{Self}` itself as the bean"
)]
pub trait Fallible {
    /// The bean type `T`.
    type Value;
    /// The method's result as a `Result`.
    fn into_result(self) -> Result<Self::Value, DiError>;
}

impl<T> Fallible for Result<T, DiError> {
    type Value = T;
    #[inline(always)]
    fn into_result(self) -> Result<T, DiError> {
        self
    }
}

/// An error returned by a `#[bean(fallible)]` method, as `try_bean` reports it: `Creation` for
/// `T`.
#[cold]
#[inline(never)]
pub fn bean_failed<T>(error: DiError) -> DiError {
    crate::beans::as_creation::<T>(error)
}
