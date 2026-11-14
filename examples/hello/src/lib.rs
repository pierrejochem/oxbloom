//! A small Oxbloom application that shows the features available today.
//!
//! | File | Shows |
//! |---|---|
//! | `greeting.rs` | `#[configuration]` with `#[bean]` methods, typed configuration, `primary` and `order` |
//! | `web.rs` | `#[component]` with `Vec<Arc<dyn ..>>` injection, `#[controller]`, `dyn Controller`, a `SecurityCustomizer`; the chain and `WebServer` are auto-configured |
//! | `lifecycle.rs` | a lazy bean with `pre_destroy`, `ApplicationRunner`, `RunListener`, `ExitCodeGenerator` |
//! | `failure.rs` | a custom `FailureAnalyzer` |
//! | `auto.rs` | an `#[auto_configuration]` with a class-level `on_property`, a default bean with `on_missing_bean`, and a user override behind `hello.custom-greeter` |
//! | `application.toml`, `application-dev.toml` | configuration with `server.port` and a `dev` profile |
//!
//! [`application`] wires everything into an `OxbloomApplicationBuilder`;
//! `main.rs` adds the command line and runs it, the tests add a
//! `ShutdownHandle` instead of a signal.

pub mod auto;
pub mod failure;
pub mod greeting;
pub mod lifecycle;
pub mod web;

use std::path::PathBuf;
use std::sync::Arc;

use oxbloom_boot::{
    ExitCodeGenerator, OxbloomApplication, OxbloomApplicationBuilder, SecurityCustomizer,
};
use oxbloom_security::SECRET_KEY;
use oxbloom_web::Controller;

use crate::auto::GuestAutoConfig;
use crate::failure::EmptyGreetingAnalyzer;
use crate::greeting::GreetingConfig;
use crate::lifecycle::{Announce, ConfiguredExitCode, StartupSummary, VisitCounter};
use crate::web::{HelloController, PublicPaths};

/// The application, reading `application.toml` (and, with an active
/// profile, `application-{profile}.toml`) from `config_dir`.
///
/// The caller adds the arguments, for example `--server.port=0`,
/// `--oxbloom.profiles.active=dev`, `--hello.custom-greeter=true` or
/// `--oxbloom.main.conditions-report=console`, and runs it.
pub fn application(config_dir: impl Into<PathBuf>) -> OxbloomApplicationBuilder {
    let config_dir = config_dir.into();
    OxbloomApplication::builder()
        // Feature: config from files, environment (`APP_...`) and arguments.
        // `secret_key` refuses the JWT secret on the command line.
        .config(move |b| b.dir(config_dir).secret_key(SECRET_KEY))
        .listener(Announce)
        .failure_analyzer(EmptyGreetingAnalyzer)
        // Feature: auto-configuration, registered after every `configure`
        // closure, so its `on_missing_bean` default sees the user's beans.
        .auto_configuration::<GuestAutoConfig>()
        .configure(|b| {
            b.instance(GreetingConfig)
                .configuration::<GreetingConfig>()
                // Feature: lazy bean with lifecycle hooks.
                .bean(VisitCounter::default)
                .lazy()
                .lifecycle::<VisitCounter>()
                .component::<HelloController>()
                // Feature: web contributors. The auto-configurations below
                // build the filter chain and the `WebServer` from them.
                .provides(|c: Arc<HelloController>| -> Arc<dyn Controller> { c })
                .component::<StartupSummary>()
                .bean(ConfiguredExitCode::new)
                .provides(|c: Arc<ConfiguredExitCode>| -> Arc<dyn ExitCodeGenerator> { c })
                .bean(|| PublicPaths)
                .provides(|p: Arc<PublicPaths>| -> Arc<dyn SecurityCustomizer> { p })
        })
        // Feature: the web server, security chain and JWT decoder
        // auto-configurations of `oxbloom-boot`.
        .auto_configurations(oxbloom_boot::auto_configurations())
        // Feature: a conditional user bean that overrides the default.
        .configure(auto::custom_guest_greeter)
}
