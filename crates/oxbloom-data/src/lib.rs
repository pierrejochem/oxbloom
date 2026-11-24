//! The data core of Oxbloom, built on [`sqlx`].
//!
//! The crate re-exports `sqlx` as [`sqlx`], so an application uses the same version as the
//! framework. The cargo features `postgres`, `mysql` and `sqlite` (all off by default) turn
//! on the matching `sqlx` driver; the TLS stack is rustls with `aws-lc-rs` and the platform
//! root certificates, never `ring`. The cargo feature `retry` (off by default) is reserved
//! for retrying connection attempts.
//!
//! # Examples
//!
//! ```
//! // The re-export names the same `sqlx` the framework uses.
//! use oxbloom_data::sqlx::Error;
//!
//! let error = Error::PoolClosed;
//! assert_eq!(error.to_string(), "attempted to acquire a connection on a closed pool");
//! ```

mod error;
mod pool;
mod settings;
mod sources;

pub use error::DataError;
pub use pool::TlsMaterial;
pub use settings::{DataSettings, Driver, PROPERTIES, SourceSettings, SslMode};
pub use sources::DataSources;
pub use sqlx;
