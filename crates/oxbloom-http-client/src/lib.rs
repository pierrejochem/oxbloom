//! The HTTP client abstraction of Oxbloom: the [`HttpClient`] trait, the [`Request`] and
//! [`Response`] types, the fixed-text [`HttpClientError`], and [`Interceptor`]s that run
//! before a request is sent.
//!
//! A [`Request`] has a method, a URL, headers, a [`Body`] and an optional timeout. A
//! [`Response`] has the status, the headers and the body bytes; a status that is not 2xx is
//! not an error unless you call [`Response::error_for_status`]. No type here prints a URL, a
//! header value or a body in `Debug` or in error text. The cargo feature `json` (on by
//! default) adds [`Body::Json`].
//!
//! # Examples
//!
//! ```
//! use oxbloom_http_client::{HttpClient, HttpClientError, Request, Response};
//!
//! struct Stub;
//!
//! #[async_trait::async_trait]
//! impl HttpClient for Stub {
//!     async fn execute(&self, request: Request) -> Result<Response, HttpClientError> {
//!         assert_eq!(request.method(), http::Method::GET);
//!         Ok(Response::new(http::StatusCode::OK, http::HeaderMap::new(), b"pong".to_vec()))
//!     }
//! }
//!
//! let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! let response = rt.block_on(Stub.execute(Request::get("https://api.example.test/ping")))?;
//! assert_eq!(response.text()?, "pong");
//! # Ok::<(), HttpClientError>(())
//! ```

mod client;
mod error;
mod interceptor;
mod request;
mod response;

pub use client::HttpClient;
pub use error::HttpClientError;
pub use interceptor::{InterceptedClient, Interceptor};
pub use request::{Body, Request};
pub use response::Response;
