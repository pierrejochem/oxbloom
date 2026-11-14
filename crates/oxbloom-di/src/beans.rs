//! Bean factories: the [`Inject`] shapes a factory parameter can have, the arity-generic
//! [`BeanFn`] and [`TryBeanFn`] traits behind `ContainerBuilder::bean` and `try_bean`, and the
//! [`Configuration`] trait that groups registrations.
//!
//! The impls for 0 through 12 parameters come from one `macro_rules!`. Their `call` bodies sit
//! on the recursion path of a deep build (provider -> `call` -> `inject` -> `Resolver::get` ->
//! next provider), so they resolve each parameter with an early-return `match` instead of `?`:
//! in debug builds every temporary of `?` gets its own stack slot.

use std::any::type_name;
use std::sync::Arc;

use crate::entry::{Provider, erase};
use crate::{ContainerBuilder, DiError, ErrorSource, Resolver};

/// A bean-factory parameter the container can supply: `Arc<T>` (also `Arc<dyn Trait>`),
/// `Option<Arc<T>>` or `Vec<Arc<T>>`, resolved as [`Resolver::get`], [`Resolver::get_opt`] and
/// [`Resolver::get_all`].
///
/// The set of shapes is closed: the trait is sealed and cannot be implemented outside
/// `oxbloom-di`. Its `inject` method can be called, e.g. from a `factory`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError, Inject, Resolver};
///
/// struct Clock;
/// struct Audit { clock: Option<Arc<Clock>> }
///
/// let c = ContainerBuilder::new()
///     .factory(|r: &Resolver| Ok(Audit { clock: Inject::inject(r)? }))
///     .build()?;
/// assert!(c.get::<Audit>()?.clock.is_none());
/// # Ok::<(), DiError>(())
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be injected into a bean",
    label = "not an injectable shape",
    note = "bean parameters must be `Arc<T>`, `Arc<dyn Trait>`, `Option<Arc<..>>` or `Vec<Arc<..>>`; register a value with `.instance(..)` and take it as `Arc<..>`"
)]
pub trait Inject: Sized + sealed::Sealed {
    /// Resolves the value from `r`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{ContainerBuilder, DiError, Inject, Resolver};
    ///
    /// struct Port(u16);
    /// struct Server { port: Arc<Port> }
    ///
    /// let c = ContainerBuilder::new()
    ///     .instance(Port(8080))
    ///     .factory(|r: &Resolver| Ok(Server { port: <Arc<Port>>::inject(r)? }))
    ///     .build()?;
    /// assert_eq!(c.get::<Server>()?.port.0, 8080);
    /// # Ok::<(), DiError>(())
    /// ```
    fn inject(r: &Resolver<'_>) -> Result<Self, DiError>;
}

/// Seals [`Inject`]: only the shapes below implement it.
mod sealed {
    use std::sync::Arc;

    #[diagnostic::on_unimplemented(
        message = "`Inject` is sealed: `{Self}` cannot implement it",
        label = "not an injectable shape",
        note = "the injectable shapes are `Arc<T>`, `Arc<dyn Trait>`, `Option<Arc<..>>` and `Vec<Arc<..>>`; register `{Self}` and inject it as `Arc<..>`"
    )]
    pub trait Sealed {}

    impl<T: ?Sized + Send + Sync + 'static> Sealed for Arc<T> {}
    impl<T: ?Sized + Send + Sync + 'static> Sealed for Option<Arc<T>> {}
    impl<T: ?Sized + Send + Sync + 'static> Sealed for Vec<Arc<T>> {}

    /// Seals [`BeanFn`](super::BeanFn): implemented by `bean_fn!` with the same bounds.
    pub trait Bean<Args, T> {}

    /// Seals [`TryBeanFn`](super::TryBeanFn): implemented by `bean_fn!` with the same bounds.
    pub trait TryBean<Args, T> {}
}

impl<T: ?Sized + Send + Sync + 'static> Inject for Arc<T> {
    #[inline(always)]
    fn inject(r: &Resolver<'_>) -> Result<Self, DiError> {
        r.get::<T>()
    }
}

impl<T: ?Sized + Send + Sync + 'static> Inject for Option<Arc<T>> {
    #[inline(always)]
    fn inject(r: &Resolver<'_>) -> Result<Self, DiError> {
        r.get_opt::<T>()
    }
}

impl<T: ?Sized + Send + Sync + 'static> Inject for Vec<Arc<T>> {
    #[inline(always)]
    fn inject(r: &Resolver<'_>) -> Result<Self, DiError> {
        r.get_all::<T>()
    }
}

/// A function whose parameters are all [`Inject`] shapes and whose result is a bean `T`:
/// implemented for every `Fn(A1, .., An) -> T + Send + Sync + 'static` with 0 to 12
/// parameters, closures and `fn` items alike. `Args` is the parameter tuple; it only tells the
/// impls apart and is inferred. Register one with [`ContainerBuilder::bean`].
///
/// The trait is sealed: it cannot be implemented outside `oxbloom-di`, but it can be named in
/// bounds and its `call` method can be called.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{BeanFn, ContainerBuilder, DiError};
///
/// struct Host(&'static str);
/// struct Url(String);
///
/// fn url(host: Arc<Host>) -> Url { Url(format!("https://{}", host.0)) }
///
/// fn takes_bean<Args, F: BeanFn<Args, Url>>(f: F) -> ContainerBuilder {
///     ContainerBuilder::new().instance(Host("example.org")).bean(f)
/// }
/// assert_eq!(takes_bean(url).build()?.get::<Url>()?.0, "https://example.org");
/// # Ok::<(), DiError>(())
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be registered as a bean",
    label = "not a bean factory",
    note = "a bean factory is a `Fn(..) -> T + Send + Sync + 'static` with at most 12 parameters, each `Arc<T>`, `Arc<dyn Trait>`, `Option<Arc<..>>` or `Vec<Arc<..>>`; use `try_bean` for `Fn(..) -> Result<T, DiError>`"
)]
pub trait BeanFn<Args, T>: Send + Sync + 'static + sealed::Bean<Args, T> {
    /// Resolves the parameters from `r` and calls the function; the first parameter that cannot
    /// be resolved is the error.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_di::{BeanFn, ContainerBuilder, DiError, Resolver};
    ///
    /// struct Name(&'static str);
    /// struct Greeting(String);
    ///
    /// let greet = |n: Arc<Name>| Greeting(format!("hello {}", n.0));
    /// let c = ContainerBuilder::new()
    ///     .instance(Name("Ada"))
    ///     .factory(move |r: &Resolver| greet.call(r))
    ///     .build()?;
    /// assert_eq!(c.get::<Greeting>()?.0, "hello Ada");
    /// # Ok::<(), DiError>(())
    /// ```
    fn call(&self, r: &Resolver<'_>) -> Result<T, DiError>;
}

/// A fallible [`BeanFn`]: `Fn(A1, .., An) -> Result<T, DiError> + Send + Sync + 'static` with 0
/// to 12 [`Inject`] parameters. Register one with [`ContainerBuilder::try_bean`].
///
/// The trait is sealed like [`BeanFn`]: usable in bounds and callable, not implementable
/// outside `oxbloom-di`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{ContainerBuilder, DiError, TryBeanFn};
///
/// struct Port(u16);
/// struct Listener(u16);
///
/// fn listen(port: Arc<Port>) -> Result<Listener, DiError> {
///     if port.0 == 0 {
///         return Err(DiError::creation::<Listener>("port 0"));
///     }
///     Ok(Listener(port.0))
/// }
///
/// fn register<Args, F: TryBeanFn<Args, Listener>>(port: u16, f: F) -> ContainerBuilder {
///     ContainerBuilder::new().instance(Port(port)).try_bean(f)
/// }
/// assert_eq!(register(80, listen).build()?.get::<Listener>()?.0, 80);
/// assert!(matches!(register(0, listen).build(), Err(DiError::Creation { .. })));
/// # Ok::<(), DiError>(())
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be registered as a fallible bean",
    label = "not a fallible bean factory",
    note = "a fallible bean factory is a `Fn(..) -> Result<T, DiError> + Send + Sync + 'static` with at most 12 parameters, each `Arc<T>`, `Arc<dyn Trait>`, `Option<Arc<..>>` or `Vec<Arc<..>>`"
)]
pub trait TryBeanFn<Args, T>: Send + Sync + 'static + sealed::TryBean<Args, T> {
    /// Resolves the parameters from `r` and calls the function. A dependency error is returned
    /// as is; an error of the function itself becomes [`DiError::Creation`] for `T` (a
    /// `Creation` keeps its message and source, any other variant becomes the fixed text
    /// `bean failed: {error}`).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{ContainerBuilder, DiError, Resolver, TryBeanFn};
    ///
    /// struct Token(String);
    ///
    /// let issue = || -> Result<Token, DiError> { Ok(Token("t-1".into())) };
    /// let c = ContainerBuilder::new()
    ///     .factory(move |r: &Resolver| issue.call(r))
    ///     .build()?;
    /// assert_eq!(c.get::<Token>()?.0, "t-1");
    /// # Ok::<(), DiError>(())
    /// ```
    fn call(&self, r: &Resolver<'_>) -> Result<T, DiError>;
}

/// Resolves one parameter, returning its error from the enclosing `call`.
macro_rules! inject {
    ($r:ident, $a:ident) => {
        match <$a as Inject>::inject($r) {
            Ok(value) => value,
            Err(e) => return Err(e),
        }
    };
}

/// Implements [`BeanFn`] and [`TryBeanFn`] for functions of the listed parameter types.
macro_rules! bean_fn {
    ($($a:ident)*) => {
        impl<F, T, $($a,)*> sealed::Bean<($($a,)*), T> for F
        where
            F: Fn($($a),*) -> T + Send + Sync + 'static,
            $($a: Inject,)*
        {
        }

        impl<F, T, $($a,)*> sealed::TryBean<($($a,)*), T> for F
        where
            F: Fn($($a),*) -> Result<T, DiError> + Send + Sync + 'static,
            $($a: Inject,)*
        {
        }

        impl<F, T, $($a,)*> BeanFn<($($a,)*), T> for F
        where
            F: Fn($($a),*) -> T + Send + Sync + 'static,
            $($a: Inject,)*
        {
            #[allow(non_snake_case, unused_variables)]
            fn call(&self, r: &Resolver<'_>) -> Result<T, DiError> {
                Ok(self($(inject!(r, $a)),*))
            }
        }

        impl<F, T, $($a,)*> TryBeanFn<($($a,)*), T> for F
        where
            F: Fn($($a),*) -> Result<T, DiError> + Send + Sync + 'static,
            $($a: Inject,)*
        {
            #[allow(non_snake_case, unused_variables)]
            fn call(&self, r: &Resolver<'_>) -> Result<T, DiError> {
                match self($(inject!(r, $a)),*) {
                    Ok(value) => Ok(value),
                    Err(e) => Err(as_creation::<T>(e)),
                }
            }
        }
    };
}

bean_fn!();
bean_fn!(A1);
bean_fn!(A1 A2);
bean_fn!(A1 A2 A3);
bean_fn!(A1 A2 A3 A4);
bean_fn!(A1 A2 A3 A4 A5);
bean_fn!(A1 A2 A3 A4 A5 A6);
bean_fn!(A1 A2 A3 A4 A5 A6 A7);
bean_fn!(A1 A2 A3 A4 A5 A6 A7 A8);
bean_fn!(A1 A2 A3 A4 A5 A6 A7 A8 A9);
bean_fn!(A1 A2 A3 A4 A5 A6 A7 A8 A9 A10);
bean_fn!(A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11);
bean_fn!(A1 A2 A3 A4 A5 A6 A7 A8 A9 A10 A11 A12);

/// An error returned by a `try_bean` function as `Creation` for `T`. A `Creation` keeps its
/// message and source; any other variant becomes the fixed text `bean failed: {error}` (type
/// names and fixed text only, no values) with an empty source. Wiring errors of the parameters
/// never come through here.
#[cold]
#[inline(never)]
pub(crate) fn as_creation<T>(error: DiError) -> DiError {
    let (message, source) = match error {
        DiError::Creation {
            message, source, ..
        } => (message, source),
        other => (format!("bean failed: {other}"), ErrorSource::none()),
    };
    DiError::Creation {
        type_name: type_name::<T>().to_owned(),
        message,
        source,
    }
}

/// The provider of a `bean` entry: calls `f` and stores its result.
pub(crate) fn provider<Args, T, F>(f: F) -> Provider
where
    T: Send + Sync + 'static,
    F: BeanFn<Args, T>,
{
    Box::new(move |r| match f.call(r) {
        Ok(value) => Ok(erase(Arc::new(value))),
        Err(e) => Err(e),
    })
}

/// The provider of a `try_bean` entry: calls `f` and stores its result.
pub(crate) fn try_provider<Args, T, F>(f: F) -> Provider
where
    T: Send + Sync + 'static,
    F: TryBeanFn<Args, T>,
{
    Box::new(move |r| match f.call(r) {
        Ok(value) => Ok(erase(Arc::new(value))),
        Err(e) => Err(e),
    })
}

/// A group of registrations, typically the beans of one module or third-party library; add it
/// with [`ContainerBuilder::configuration`]. `#[configuration]` implements it for an `impl`
/// block of `#[bean]` methods; implement it by hand to group builder calls.
///
/// Modifiers belong inside `register`, next to their registration: one written after
/// `.configuration::<C>()` applies only to the last registration `C` made.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_di::{Configuration, ContainerBuilder, DiError};
///
/// struct DbUrl(&'static str);
/// struct Pool { url: &'static str }
///
/// struct Persistence;
/// impl Configuration for Persistence {
///     fn register(builder: ContainerBuilder) -> ContainerBuilder {
///         builder
///             .instance(DbUrl("postgres://db/app"))
///             .bean(|url: Arc<DbUrl>| Pool { url: url.0 })
///     }
/// }
///
/// let c = ContainerBuilder::new().configuration::<Persistence>().build()?;
/// assert_eq!(c.get::<Pool>()?.url, "postgres://db/app");
/// # Ok::<(), DiError>(())
/// ```
pub trait Configuration {
    /// Adds this configuration's registrations (and their modifiers) to `builder`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_di::{Configuration, ContainerBuilder, DiError};
    ///
    /// struct Limits { max: u32 }
    /// struct Defaults;
    /// impl Configuration for Defaults {
    ///     fn register(builder: ContainerBuilder) -> ContainerBuilder {
    ///         builder.bean(|| Limits { max: 10 })
    ///     }
    /// }
    ///
    /// // `configuration::<C>()` is the usual way; calling `register` directly is the same.
    /// let c = Defaults::register(ContainerBuilder::new()).build()?;
    /// assert_eq!(c.get::<Limits>()?.max, 10);
    /// # Ok::<(), DiError>(())
    /// ```
    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}
