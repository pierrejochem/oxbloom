//! Lifecycle features: a lazy bean with a `pre_destroy` hook, an
//! `ApplicationRunner`, a `RunListener` and an `ExitCodeGenerator`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxbloom_boot::{
    ApplicationArguments, ApplicationRunner, ExitCodeGenerator, RunListener, RunnerError,
    WebServer, async_trait,
};
use oxbloom_config::Config;
use oxbloom_di::{Container, Lifecycle, component};

use crate::auto::GuestGreeter;
use crate::greeting::{Greeter, GreetingProperties};

/// Feature: a lazy bean with lifecycle hooks. `lazy` creates it on the first
/// request for it (here: when the controller is built), and `pre_destroy`
/// runs when the container shuts down after the web server stopped.
#[derive(Default)]
pub struct VisitCounter {
    visits: AtomicUsize,
}

impl VisitCounter {
    /// Counts one greeting request.
    pub fn record(&self) {
        self.visits.fetch_add(1, Ordering::Relaxed);
    }

    /// The number of greeting requests so far.
    pub fn count(&self) -> usize {
        self.visits.load(Ordering::Relaxed)
    }
}

impl Lifecycle for VisitCounter {
    fn pre_destroy(&self) {
        println!("served {} greetings, goodbye", self.count());
    }
}

/// Feature: an `ApplicationRunner` bean. It runs once after the container is
/// built and before the web server starts. `guest` is the auto-configured
/// default or the application's override (see `auto.rs`).
#[component(provides(dyn ApplicationRunner))]
pub struct StartupSummary {
    config: Arc<Config>,
    greeters: Vec<Arc<dyn Greeter>>,
    guest: Arc<dyn GuestGreeter>,
}

#[async_trait]
impl ApplicationRunner for StartupSummary {
    async fn run(&self, _args: &ApplicationArguments) -> Result<(), RunnerError> {
        let languages: Vec<&str> = self.greeters.iter().map(|g| g.language()).collect();
        let profiles = self.config.active_profiles();
        println!(
            "greeters: {}; active profiles: {}",
            languages.join(", "),
            if profiles.is_empty() {
                "none".to_owned()
            } else {
                profiles.join(", ")
            }
        );
        println!("guest greeter: {}", self.guest.welcome());
        Ok(())
    }
}

/// Feature: a `RunListener`. It prints the address once the server listens.
pub struct Announce;

impl RunListener for Announce {
    fn ready(&self, container: &Container) {
        if let Some(addr) = container
            .get::<WebServer>()
            .ok()
            .and_then(|s| s.local_addr())
        {
            println!("listening on http://{addr}");
        }
    }
}

/// Feature: an `ExitCodeGenerator`. The process exits with `hello.exit-code`
/// after a clean shutdown.
pub struct ConfiguredExitCode(i32);

impl ConfiguredExitCode {
    /// Reads the code from the typed settings.
    pub fn new(properties: Arc<GreetingProperties>) -> Self {
        ConfiguredExitCode(properties.exit_code)
    }
}

impl ExitCodeGenerator for ConfiguredExitCode {
    fn exit_code(&self) -> i32 {
        self.0
    }
}
