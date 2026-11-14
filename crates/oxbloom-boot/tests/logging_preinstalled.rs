//! A foreign `tracing` subscriber installed before the run: boot keeps it and tells it so.
//! A binary of its own, because the subscriber is global.
#![cfg(feature = "logging")]

use std::fmt;
use std::sync::{Arc, Mutex};

use oxbloom_boot::{Banner, OxbloomApplication, RunListener};
use oxbloom_config::Config;
use oxbloom_di::Container;
use oxbloom_logging::LoggingSystem;
use oxbloom_test::Captured;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Records every event as `(level, target, message)`.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(Level, String, String)>>>);

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        let meta = event.metadata();
        self.0
            .lock()
            .unwrap()
            .push((*meta.level(), meta.target().to_owned(), message.0));
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[derive(Clone, Default)]
struct Bean(Arc<Mutex<Option<bool>>>);

impl RunListener for Bean {
    fn context_ready(&self, container: &Container) {
        *self.0.lock().unwrap() = Some(container.get::<LoggingSystem>().is_ok());
    }
}

#[tokio::test]
async fn already_installed_is_a_warning() {
    let recorder = Recorder::default();
    tracing::dispatcher::set_global_default(tracing::Dispatch::new(recorder.clone())).unwrap();

    let (out, logs, bean) = (Captured::new(), Captured::new(), Bean::default());
    let writer = logs.clone();
    let status = OxbloomApplication::builder()
        .with_config(Config::from_str("[logging.console]\ncolor = \"never\"").unwrap())
        .banner(Banner::off())
        .output(out.clone())
        .logging_writer(move || writer.clone())
        .listener(bean.clone())
        .run()
        .await
        .unwrap();
    assert_eq!(status.code(), 0);
    assert_eq!(
        *bean.0.lock().unwrap(),
        Some(false),
        "no LoggingSystem bean"
    );
    assert_eq!(out.contents(), "", "nothing on the output target");
    assert_eq!(logs.contents(), "", "the logging writer is never used");

    let events = recorder.0.lock().unwrap().clone();
    let boot: Vec<_> = events.iter().filter(|e| e.1 == "oxbloom::boot").collect();
    // The startup events reach the foreign subscriber too, after the one `warn`.
    let messages: Vec<&str> = boot.iter().map(|e| e.2.as_str()).collect();
    assert_eq!(messages.len(), 3, "{events:?}");
    assert_eq!(boot[0].0, Level::WARN);
    assert!(
        boot[0].2.contains(
            "logging not installed: a global tracing subscriber or `log` logger is already installed"
        ),
        "{events:?}"
    );
    assert_eq!(boot.iter().filter(|e| e.0 == Level::WARN).count(), 1);
    assert_eq!(messages[1], "Starting application");
    assert!(
        messages[2].starts_with("Started application in "),
        "{events:?}"
    );
}
