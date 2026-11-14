//! Routing and request/response types for Oxbloom.
//!
//! - Routing: [`Router`], [`Route`] and [`Controller`]; [`App`] adds filters and the Hyper server.
//! - Filters: a [`Filter`] wraps routing. Filters run in the order of [`Filter::order`] (lower
//!   first, ties in registration order); the constants live in the [`order`] module.
//!   [`App::filter_ordered`] sets a slot from code and [`Ordered`] wraps a filter with one.
//!   [`RequestLogFilter`] logs one event per request.
//! - CORS: [`App::cors`] registers a [`CorsFilter`] built from a [`CorsConfig`] at
//!   [`order::CORS`], outside error handling and security, so error answers carry CORS headers
//!   and an allowed preflight never reaches security. Invalid settings give a [`CorsError`].
//! - Compression: [`App::compression`] registers a [`CompressionFilter`] built from a
//!   [`CompressionConfig`] at [`order::COMPRESSION`]. The features `gzip`, `brotli` and `zstd`
//!   (all in `compression`, on by default) enable the codecs; [`codecs`] lists those compiled in.
//! - Static resources: [`StaticResources`] is a [`Controller`] that serves `GET` and `HEAD` below
//!   a prefix from a [`ResourceSource`]: [`DiskResources`] (a directory, kept under its canonical
//!   root) or [`EmbeddedResources`] (files compiled into the binary). It sends `ETag`,
//!   `Last-Modified` and `Cache-Control` and answers conditional requests with 304.
//! - Features: `json` (on by default) adds `Json`, `Request::json` and the `Accept` check;
//!   `gzip`, `brotli` and `zstd` each enable one codec of the compression filter and `compression`
//!   (on by default) enables all three; `di` adds `App::controller`. CORS and static resources
//!   need no feature.
//! - Errors: handlers return an [`HttpError`] (or any `Result<T, E>` of [`IntoResponse`] types).
//!   Every error answer, including 401 and 403 from the security chain, 404, 405, 413 and a
//!   panic as 500, is rendered by one [`ErrorRenderer`]; the default [`ProblemRenderer`] writes
//!   RFC 9457 `application/problem+json`. [`App::error_renderer`] replaces it. A response that
//!   already has a body is never rewritten.
//! - JSON (feature `json`, on by default): `Json` is a JSON response, `Request::json` parses a
//!   request body (415 for a wrong `Content-Type`, 400 for an invalid body), and a built-in filter at
//!   [`order::NEGOTIATION`] answers 406 when `Accept` excludes a `Json` response.
//! - Server options: [`App::max_body_bytes`], [`App::header_read_timeout`],
//!   [`App::shutdown_timeout`] and [`App::forwarded`]. With a trusted proxy list
//!   ([`ForwardedConfig`], [`TrustedProxies`]) the client address, scheme and host come from
//!   `Forwarded` or `X-Forwarded-*`; [`Request::client_addr`], [`Request::scheme`] and
//!   [`Request::host`] read them. Forwarded headers are only safe behind a proxy that sets or
//!   strips them; see the README.

mod app;
mod catch;
mod compression;
mod controller;
mod cors;
mod error;
mod filter;
mod forwarded;
#[cfg(feature = "json")]
mod json;
#[cfg(feature = "json")]
mod negotiation;
pub mod order;
mod render;
mod request;
mod request_log;
mod response;
mod route;
mod router;
mod static_files;

pub use app::App;
#[cfg(feature = "di")]
pub use app::AppError;
pub use compression::{CompressionConfig, CompressionFilter, CompressionLevel, codecs};
pub use controller::Controller;
pub use cors::{CorsConfig, CorsError, CorsFilter};
pub use error::{ErrorAttributes, ErrorValue, HttpError};
pub use filter::{Filter, Next, Ordered};
pub use forwarded::{
    ClientInfo, ForwardStrategy, ForwardedConfig, ProxyParseError, Scheme, TrustedProxies,
};
#[cfg(feature = "json")]
pub use json::{Json, JsonBody};
pub use render::{ErrorRenderer, ProblemRenderer, RequestInfo};
pub use request::Request;
pub use request_log::RequestLogFilter;
pub use response::{IntoResponse, Response};
pub use route::{BoxFuture, Handler, Route};
pub use router::{RouteError, Router, RouterConfig};
pub use static_files::{
    DiskResources, EmbeddedResources, Resource, ResourceError, ResourceSource, StaticResources,
};

pub use oxbloom_core::HttpMethod;
pub use oxbloom_macros::{controller, delete, get, head, options, patch, post, put};
