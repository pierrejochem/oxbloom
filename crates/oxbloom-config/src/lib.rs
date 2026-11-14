//! Typed runtime configuration for Oxbloom applications.
//!
//! Values come from `application.toml` (and its `oxbloom.config.import` files), the
//! `application-{profile}.toml` overlays of the active profiles (and their imports, profiles
//! expanded through `oxbloom.profiles.group`), environment variables and command-line arguments, in
//! that order of increasing precedence.
//!
//! Keys match relaxed (case, `-` and `_` are ignored), lists can be comma-separated or indexed,
//! `Duration` and [`DataSize`] fields read strings such as `1.5s` and `10MB`, and `${name}` /
//! `${name:default}` placeholders in file values resolve against other keys and then the
//! environment. Environment and command-line values are taken literally; placeholders are
//! resolved only in values from files. Every leaf knows its [`Origin`]; errors and
//! [`Config::explain`] name keys and origins, never values. The one exception is a placeholder
//! in a file value: its name, or at most 32 characters of an unterminated one, appears in
//! the error.
//! The example below writes one file to a temporary directory and injects environment
//! variables and arguments.
//!
//! ```
//! use std::time::Duration;
//!
//! use oxbloom_config::{Config, Origin};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Server {
//!     port: u16,
//!     hosts: Vec<String>,
//!     timeout: Duration,
//!     url: String,
//! }
//!
//! let dir = tempfile::tempdir()?;
//! std::fs::write(
//!     dir.path().join("application.toml"),
//!     "[server]\nurl = \"http://${server.name}:${server.port}\"",
//! )?;
//!
//! let config = Config::builder()
//!     .dir(dir.path())
//!     .env_from([
//!         ("APP_SERVER__PORT".to_string(), "9000".to_string()),
//!         ("APP_SERVER__HOSTS".to_string(), "a.example, b.example".to_string()),
//!         ("APP_SERVER__TIMEOUT".to_string(), "1.5s".to_string()),
//!         ("APP_SERVER__NAME".to_string(), "api.example".to_string()),
//!     ])
//!     .args(["--server.port=9001"]) // the command line wins over the environment
//!     .build()?;
//!
//! let server: Server = config.get("server")?;
//! assert_eq!(server.port, 9001);
//! assert_eq!(server.hosts, ["a.example", "b.example"]);
//! assert_eq!(server.timeout, Duration::from_millis(1500));
//! assert_eq!(server.url, "http://api.example:9001");
//!
//! assert_eq!(config.origin("server.port"), Some(&Origin::Cli { arg: "--server.port".into() }));
//! assert_eq!(
//!     config.origin("SERVER.timeout"),
//!     Some(&Origin::Env { name: "APP_SERVER__TIMEOUT".into() })
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod config;
mod error;
mod introspect;
mod lenient;
mod loader;
mod migrate;
mod node;
mod origin;
mod placeholder;
mod secret;
mod sources;
mod units;

pub use config::{Config, ConfigBuilder};
pub use error::ConfigError;
pub use introspect::Entry;
pub use migrate::{ConfigWarning, WarningKind};
pub use origin::Origin;
pub use secret::Secret;
pub use units::DataSize;

#[cfg(feature = "di")]
mod di;

#[cfg(feature = "di")]
pub use di::ContainerBuilderExt;
