use oxbloom_boot::{ExitStatus, RunListener, StartupError, StartupPhase};
use oxbloom_test::Log;

struct Recorder(Log);

impl RunListener for Recorder {
    fn starting(&self) {
        self.0.push("starting");
    }
    fn stopped(&self, status: &ExitStatus) {
        self.0.push(format!("stopped {}", status.code()));
    }
    fn failed(&self, error: &StartupError) {
        self.0.push(format!("failed {}", error.phase()));
    }
}

#[test]
fn custom_listener_receives_public_callbacks() {
    let log = Log::new();
    let l: &dyn RunListener = &Recorder(log.clone());
    l.starting();
    l.stopping();
    l.stopped(&ExitStatus::new(3));
    l.failed(&StartupError::new(StartupPhase::Runner, "x"));
    assert_eq!(log.entries(), ["starting", "stopped 3", "failed runner"]);
    assert!(l.name().ends_with("Recorder"));
}
