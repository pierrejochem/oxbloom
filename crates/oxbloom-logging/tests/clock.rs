//! The JSON timestamp follows the clock given to the logging system, against a scoped
//! subscriber (`with_default`). Nothing here installs the global subscriber.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxbloom_config::Config;
use oxbloom_core::Clock;
use oxbloom_logging::__private::{scoped, scoped_with_clock};
use tracing_subscriber::fmt::MakeWriter;

/// Collects the console output.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn take(&self) -> String {
        String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
    }
}

impl io::Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;

    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

/// A clock that always answers the same instant.
struct Fixed(SystemTime);

impl Clock for Fixed {
    fn now(&self) -> SystemTime {
        self.0
    }
}

const JSON: &str = "[logging.console]\nformat = \"json\"\n";

fn timestamp_of(dispatch: &tracing::Dispatch, out: &Capture) -> String {
    tracing::dispatcher::with_default(dispatch, || tracing::info!(target: "app", "tick"));
    let text = out.take();
    let line: serde_json::Value = serde_json::from_str(text.trim_end()).unwrap();
    line["timestamp"].as_str().unwrap().to_string()
}

#[test]
fn json_timestamps_follow_the_clock() {
    let out = Capture::default();
    let clock: Arc<dyn Clock> = Arc::new(Fixed(UNIX_EPOCH + Duration::from_secs(86_400)));
    let (dispatch, _system) =
        scoped_with_clock(&Config::from_str(JSON).unwrap(), out.clone(), clock).unwrap();
    assert_eq!(timestamp_of(&dispatch, &out), "1970-01-02T00:00:00.000Z");
}

#[test]
fn json_timestamps_use_the_system_time_without_a_clock() {
    let out = Capture::default();
    let (dispatch, _system) = scoped(&Config::from_str(JSON).unwrap(), out.clone()).unwrap();
    let year = 1970
        + SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            / 31_556_952;
    let stamp = timestamp_of(&dispatch, &out);
    // The current year is enough to tell the system time from a fixed clock near the epoch.
    let printed: u64 = stamp[..4].parse().unwrap();
    assert!(printed + 1 >= year && printed <= year + 1, "{stamp}");
}
