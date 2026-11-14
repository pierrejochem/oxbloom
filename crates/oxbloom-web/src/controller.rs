use std::sync::Arc;

use crate::Route;

/// A type that can hand out its routes. Implemented by `#[controller]`.
///
/// Implement it by hand only when the macro does not fit. A controller is `Send + Sync`, so it
/// can be registered in a container as `Arc<dyn Controller>`.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use oxbloom_web::{Controller, HttpMethod, Route};
///
/// struct Health;
///
/// impl Controller for Health {
///     fn routes(self: Arc<Self>) -> Vec<Route> {
///         vec![Route::new(HttpMethod::Get, "/health", |_| async { "ok" })]
///     }
/// }
///
/// assert_eq!(Controller::routes(Arc::new(Health)).len(), 1);
/// ```
pub trait Controller: Send + Sync {
    /// Returns every route of this controller. Pass them to `App::routes`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use oxbloom_web::{App, Controller, HttpMethod, Route};
    ///
    /// struct Health;
    /// impl Controller for Health {
    ///     fn routes(self: Arc<Self>) -> Vec<Route> {
    ///         vec![Route::new(HttpMethod::Get, "/health", |_| async { "ok" })]
    ///     }
    /// }
    ///
    /// # fn main() -> Result<(), oxbloom_web::RouteError> {
    /// let app = App::new().routes(Controller::routes(Arc::new(Health)))?;
    /// # let _ = app;
    /// # Ok(())
    /// # }
    /// ```
    fn routes(self: Arc<Self>) -> Vec<Route>;
}
