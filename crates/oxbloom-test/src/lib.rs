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
//! `TestApp::secured` needs `security`, which also adds `with_authentication` to run a secured
//! service as a given caller without HTTP. The feature `tls` turns on `web` and boot's `ssl` and
//! adds `TestClient::connect_tls` and `RunningApp::client_tls`, which send HTTPS to a server
//! whose certificate a given CA signed. The feature `http-client` forwards to boot's feature of
//! the same name and adds `MockHttpClient` and `MockResponse`, a scripted `HttpClient` that
//! records the requests it gets. The feature `oauth2` turns on `http-client` and adds
//! `FakeProvider`, an OpenID Connect provider over `MockHttpClient` that signs id tokens with a
//! key generated at construction, with `FakeUser`, `Fault` and `TokenRequest`.
//! The feature `data` adds `TestDatabase`, a uniquely named in-memory SQLite database with a
//! `[data]` snippet for `ContextRunner`; `data-postgres` and `data-mysql` add
//! `TestDatabase::postgres` and `TestDatabase::mysql`, which create a throwaway database on the
//! server named by `OXBLOOM_TEST_POSTGRES_URL` or `OXBLOOM_TEST_MYSQL_URL` (see `make test-db`).
//! `web`, `security`, `logging`, `tls`, `http-client`, `oauth2` and `data` are off by default.
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
#[cfg(feature = "data")]
mod data;
#[cfg(feature = "oauth2")]
mod fake_provider;
mod lifecycle;
mod log;
#[cfg(feature = "http-client")]
mod mock_http;
mod runner;
#[cfg(feature = "security")]
mod security_context;

#[cfg(feature = "web")]
pub use app::{RunningApp, Stopped, TestApp};
pub use captured::Captured;
#[cfg(feature = "web")]
pub use client::{RequestBuilder, TestClient, TestResponse};
pub use clock::{FixedClock, ManualClock};
pub use config_dir::TempConfigDir;
#[cfg(feature = "data")]
pub use data::{MigrationFiles, TestDatabase};
#[cfg(feature = "oauth2")]
pub use fake_provider::{FakeProvider, FakeUser, Fault, TokenRequest};
pub use log::Log;
#[cfg(feature = "http-client")]
pub use mock_http::{MockHttpClient, MockResponse};
pub use runner::{ContextRunner, ContextView, FailureView};
#[cfg(feature = "security")]
pub use security_context::with_authentication;
