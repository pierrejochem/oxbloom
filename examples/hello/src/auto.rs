//! Auto-configuration: a default that steps aside when the application has its
//! own bean.
//!
//! `GuestAutoConfig` offers a default [`GuestGreeter`]. `lib.rs` registers it
//! with `.auto_configuration::<GuestAutoConfig>()`, so boot adds it after the
//! application's own `configure` wiring. The application registers
//! [`CustomGuestGreeter`] itself only when `hello.custom-greeter = true` (see
//! [`custom_guest_greeter`]); the default then finds a `dyn GuestGreeter`
//! already registered and backs off.
//!
//! To see which registrations were kept or skipped, and why, run
//!
//! ```text
//! cargo run -p hello -- --server.port=0 --oxbloom.main.conditions-report=console
//! cargo run -p hello -- --server.port=0 --hello.custom-greeter=true --oxbloom.main.conditions-report=console
//! ```
//!
//! The conditions evaluation report is printed right after the container is
//! built, before the runners.

use std::sync::Arc;

use oxbloom_boot::auto_configuration;
use oxbloom_di::{ContainerBuilder, component};

/// Welcomes a visitor who did not give a name.
pub trait GuestGreeter: Send + Sync {
    /// The welcome text.
    fn welcome(&self) -> String;
}

/// The default guest greeter of [`GuestAutoConfig`].
pub struct StandardGuestGreeter;

impl GuestGreeter for StandardGuestGreeter {
    fn welcome(&self) -> String {
        "Welcome, guest!".to_owned()
    }
}

/// The application's own guest greeter, registered with
/// `hello.custom-greeter = true`.
pub struct CustomGuestGreeter;

impl GuestGreeter for CustomGuestGreeter {
    fn welcome(&self) -> String {
        "Make yourself at home, guest!".to_owned()
    }
}

/// Feature: an auto-configuration. The struct is a `#[component]` because boot
/// registers its instance first in the configuration's group, so the `#[bean]`
/// methods get `&self`.
#[component]
pub struct GuestAutoConfig;

/// Feature: a class-level condition. `on_property("hello.greeting")` is decided
/// once for the whole configuration; when the key is absent, none of its beans
/// is registered. `application.toml` sets it, so it matches.
#[auto_configuration(on_property("hello.greeting"))]
impl GuestAutoConfig {
    /// Feature: back-off. `on_missing_bean(dyn GuestGreeter)` keeps this default
    /// only when no registration before it provides `dyn GuestGreeter`.
    #[bean(provides(dyn GuestGreeter), on_missing_bean(dyn GuestGreeter))]
    fn standard_guest_greeter(&self) -> StandardGuestGreeter {
        StandardGuestGreeter
    }
}

/// Feature: a condition on a user registration. The application's own guest
/// greeter is kept only when `hello.custom-greeter` is exactly `true`; the
/// conditions of `configure` closures read `Config`, so the flag works from
/// `application.toml`, the environment (`APP_HELLO__CUSTOM_GREETER=true`) and
/// the command line (`--hello.custom-greeter=true`).
pub fn custom_guest_greeter(builder: ContainerBuilder) -> ContainerBuilder {
    builder
        .bean(|| CustomGuestGreeter)
        .provides(|g: Arc<CustomGuestGreeter>| -> Arc<dyn GuestGreeter> { g })
        .on_property_value("hello.custom-greeter", "true")
}
