//! Slots of the filter chain. Lower values run first; equal values keep registration order.
//!
//! # Examples
//!
//! ```
//! use oxbloom_web::order;
//!
//! assert!(order::REQUEST_LOG < order::CORS && order::CORS < order::ERROR_HANDLING);
//! assert!(order::BODY_LIMIT < order::COMPRESSION && order::COMPRESSION < order::SECURITY);
//! assert!(order::SECURITY < order::DEFAULT);
//! ```

/// Slot of the request log filter, outermost so it sees every answer.
///
/// # Examples
///
/// ```
/// assert_eq!(oxbloom_web::order::REQUEST_LOG, -400);
/// ```
pub const REQUEST_LOG: i32 = -400;

/// Slot of the CORS filter, outside error handling so error answers carry CORS headers and a
/// preflight never reaches security.
///
/// # Examples
///
/// ```
/// use oxbloom_web::order;
///
/// assert_eq!(order::CORS, -350);
/// assert!(order::REQUEST_LOG < order::CORS && order::CORS < order::ERROR_HANDLING);
/// ```
pub const CORS: i32 = -350;

/// Slot of the filter that turns errors into rendered responses.
///
/// # Examples
///
/// ```
/// assert_eq!(oxbloom_web::order::ERROR_HANDLING, -300);
/// ```
pub const ERROR_HANDLING: i32 = -300;

/// Slot of the JSON content negotiation filter, inside error handling and outside the body limit.
///
/// # Examples
///
/// ```
/// use oxbloom_web::order;
///
/// assert_eq!(order::NEGOTIATION, -250);
/// assert!(order::ERROR_HANDLING < order::NEGOTIATION && order::NEGOTIATION < order::BODY_LIMIT);
/// ```
pub const NEGOTIATION: i32 = -250;

/// Slot of the request body limit filter.
///
/// # Examples
///
/// ```
/// assert_eq!(oxbloom_web::order::BODY_LIMIT, -200);
/// ```
pub const BODY_LIMIT: i32 = -200;

/// Slot of the response compression filter, inside the body limit and outside security, so it
/// compresses the answers of handlers and of security filters.
///
/// # Examples
///
/// ```
/// use oxbloom_web::order;
///
/// assert_eq!(order::COMPRESSION, -150);
/// assert!(order::BODY_LIMIT < order::COMPRESSION && order::COMPRESSION < order::SECURITY);
/// ```
pub const COMPRESSION: i32 = -150;

/// Slot of security filters.
///
/// # Examples
///
/// ```
/// assert_eq!(oxbloom_web::order::SECURITY, -100);
/// ```
pub const SECURITY: i32 = -100;

/// Slot of filters that declare no order.
///
/// # Examples
///
/// ```
/// assert_eq!(oxbloom_web::order::DEFAULT, 0);
/// ```
pub const DEFAULT: i32 = 0;
