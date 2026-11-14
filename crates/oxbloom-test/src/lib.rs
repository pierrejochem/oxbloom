//! Test support for Oxbloom applications.
//!
//! Add this crate as a dev-dependency. It offers a [`ContextRunner`] that runs an application
//! context through the real boot lifecycle, in-memory capture helpers, a temporary
//! configuration directory and controllable clocks. With the feature `web`, a [`TestClient`]
//! sends requests to an `oxbloom_web::App` in memory or over real HTTP/1.1, and answers with a
//! [`TestResponse`]; the feature `json` adds its JSON helpers. Also with `web`, a [`TestApp`]
//! starts a whole boot application with a real web server on port 0 and hands a
//! [`RunningApp`] with a connected client.
//!
//! The features `security` and `logging` forward to the same features of `oxbloom-boot`;
//! `TestApp::secured` needs `security`. The feature `tls` turns on `web` and boot's `ssl` and
//! adds `TestClient::connect_tls` and `RunningApp::client_tls`, which send HTTPS to a server
//! whose certificate a given CA signed. `web`, `security`, `logging` and `tls` are off by
//! default.
//!
//! # Examples
//!
//! ```
//! use oxbloom_test::Captured;
//!
//! let captured = Captured::new();
//! assert!(captured.contents().is_empty());
//! ```

#[cfg(feature = "web")]
mod app;
mod captured;
#[cfg(feature = "web")]
mod client;
mod clock;
mod config_dir;
mod lifecycle;
mod log;
mod runner;

#[cfg(feature = "web")]
pub use app::{RunningApp, Stopped, TestApp};
pub use captured::Captured;
#[cfg(feature = "web")]
pub use client::{RequestBuilder, TestClient, TestResponse};
pub use clock::{FixedClock, ManualClock};
pub use config_dir::TempConfigDir;
pub use log::Log;
pub use runner::{ContextRunner, ContextView, FailureView};
