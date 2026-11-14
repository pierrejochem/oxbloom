use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use oxbloom_config::Config;
use oxbloom_di::Container;

use crate::error::{ExitStatus, StartupError};
use crate::output::Output;

/// Observes the lifecycle of an Oxbloom application run.
///
/// Every method has an empty default, so implement only what you need.
/// Listeners must not panic: a panic is caught and reported as
/// `listener {name} panicked` (the panic message is never printed by Oxbloom),
/// but the process-wide panic hook still prints to stderr and the remaining
/// callbacks of that listener for the event are skipped.
///
/// ```
/// use oxbloom_boot::{ExitStatus, RunListener};
///
/// struct Exit;
///
/// impl RunListener for Exit {
///     fn stopped(&self, status: &ExitStatus) {
///         println!("exit code {}", status.code());
///     }
/// }
///
/// Exit.stopped(&ExitStatus::new(0));
/// assert!(Exit.name().ends_with("Exit"));
/// ```
pub trait RunListener: Send + Sync + 'static {
    /// The run is beginning; nothing has been loaded yet.
    fn starting(&self) {}
    /// Configuration is loaded.
    fn environment_prepared(&self, _config: &Config) {}
    /// The container is built.
    fn context_ready(&self, _container: &Container) {}
    /// Runners and services have started.
    fn started(&self, _container: &Container) {}
    /// The application is ready.
    fn ready(&self, _container: &Container) {}
    /// Shutdown is beginning. Fires on every successful run, also when no
    /// service ran.
    fn stopping(&self) {}
    /// The run finished with `status`.
    fn stopped(&self, _status: &ExitStatus) {}
    /// Startup failed with `error`.
    fn failed(&self, _error: &StartupError) {}
    /// The listener's name, by default its type name.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Ordered listeners with panic-isolated dispatch.
pub(crate) struct Listeners(pub(crate) Vec<Arc<dyn RunListener>>);

impl Listeners {
    pub(crate) fn fire(&self, out: &Output, f: impl Fn(&dyn RunListener)) {
        for l in &self.0 {
            if catch_unwind(AssertUnwindSafe(|| f(l.as_ref()))).is_err() {
                out.line(&format!("listener {} panicked", l.name()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Buf {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    type Log = Arc<Mutex<Vec<&'static str>>>;

    struct Named(&'static str, Log);
    impl RunListener for Named {
        fn starting(&self) {
            self.1.lock().unwrap().push(self.0);
        }
    }

    struct Panicker;
    impl RunListener for Panicker {
        fn starting(&self) {
            panic!("secret-marker");
        }
    }

    struct Quiet;
    impl RunListener for Quiet {}

    #[test]
    fn listeners_called_in_order() {
        let log = Log::default();
        let ls = Listeners(vec![
            Arc::new(Named("a", log.clone())),
            Arc::new(Named("b", log.clone())),
        ]);
        let buf = Buf::default();
        ls.fire(&Output::new(buf.clone()), |l| l.starting());
        assert_eq!(*log.lock().unwrap(), ["a", "b"]);
        assert_eq!(buf.text(), "");
    }

    #[test]
    fn panicking_listener_is_skipped_and_named_in_output() {
        let log = Log::default();
        let ls = Listeners(vec![
            Arc::new(Panicker),
            Arc::new(Named("after", log.clone())),
        ]);
        let buf = Buf::default();
        ls.fire(&Output::new(buf.clone()), |l| l.starting());
        assert_eq!(*log.lock().unwrap(), ["after"]);
        let text = buf.text();
        assert!(text.contains("Panicker"), "{text}");
        assert!(text.contains(" panicked"), "{text}");
        assert!(!text.contains("secret-marker"), "{text}");
    }

    #[test]
    fn default_methods_do_nothing() {
        let ls = Listeners(vec![Arc::new(Quiet)]);
        let buf = Buf::default();
        let out = Output::new(buf.clone());
        ls.fire(&out, |l| {
            l.starting();
            l.stopping();
            l.stopped(&ExitStatus::new(0));
            l.failed(&StartupError::new(crate::StartupPhase::Config, "x"));
        });
        assert_eq!(buf.text(), "");
    }
}
