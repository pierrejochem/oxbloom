//! Greeters and their typed settings.
//!
//! Shows a `#[configuration]` with `#[bean]` methods, typed configuration
//! bound from `application.toml`, trait beans with `primary` and `order`, and a
//! fallible bean whose typed error a custom failure analyzer recognises.

use std::fmt;
use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_di::{DiError, configuration};
use serde::Deserialize;

/// Says hello in one language. Several beans provide it.
pub trait Greeter: Send + Sync {
    /// The language name, for listings.
    fn language(&self) -> &'static str;
    /// The greeting for `name`.
    fn greet(&self, name: &str) -> String;
}

/// The `hello` section of the configuration.
#[derive(Debug, Deserialize)]
pub struct GreetingProperties {
    /// The text of the primary greeter, `hello.greeting`.
    pub greeting: String,
    /// The exit code after a clean shutdown, `hello.exit-code` (default 0).
    #[serde(default)]
    pub exit_code: i32,
}

/// `hello.greeting` is empty. The text names the key, never a value.
#[derive(Debug)]
pub struct EmptyGreeting;

impl fmt::Display for EmptyGreeting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`hello.greeting` must not be empty")
    }
}

impl std::error::Error for EmptyGreeting {}

/// The configured greeter.
pub struct Configured {
    text: String,
}

impl Greeter for Configured {
    fn language(&self) -> &'static str {
        "configured"
    }

    fn greet(&self, name: &str) -> String {
        format!("{}, {name}!", self.text)
    }
}

/// A fixed Dutch greeter.
pub struct Dutch;

impl Greeter for Dutch {
    fn language(&self) -> &'static str {
        "dutch"
    }

    fn greet(&self, name: &str) -> String {
        format!("Hallo, {name}!")
    }
}

/// Feature: `#[configuration]`. Each `#[bean]` method registers one bean; its
/// parameters are injected. The configuration itself is registered as an
/// instance in `lib.rs`.
pub struct GreetingConfig;

#[configuration]
impl GreetingConfig {
    /// Feature: typed configuration and a fallible bean. `?` turns a
    /// `ConfigError` into a `DiError` that keeps it as its source, and
    /// `creation_from` keeps `EmptyGreeting` for the failure analyzer.
    #[bean(fallible)]
    fn properties(&self, config: Arc<Config>) -> Result<GreetingProperties, DiError> {
        let properties: GreetingProperties = config.get("hello")?;
        if properties.greeting.trim().is_empty() {
            return Err(DiError::creation_from::<GreetingProperties>(EmptyGreeting));
        }
        Ok(properties)
    }

    /// Feature: `provides`, `primary` and `order`. A single `Arc<dyn Greeter>`
    /// gets this one; `Vec<Arc<dyn Greeter>>` lists it first.
    #[bean(provides(dyn Greeter), primary, order = 1)]
    fn configured(&self, properties: Arc<GreetingProperties>) -> Configured {
        Configured {
            text: properties.greeting.clone(),
        }
    }

    /// A second provider of `dyn Greeter`, listed after the first.
    #[bean(provides(dyn Greeter), order = 2)]
    fn dutch(&self) -> Dutch {
        Dutch
    }
}
