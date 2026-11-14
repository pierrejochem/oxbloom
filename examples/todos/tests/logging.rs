//! The one test that lets the application install the global `tracing`
//! subscriber. It lives in its own binary because only one app per process
//! can; `tests/api.rs` runs with `.logging(false)`.

// This test keeps its own `Ready` listener and run harness: `TestApp` cannot take the
// application's own `OxbloomApplicationBuilder`.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use oxbloom_boot::{RunListener, WebServer};
use oxbloom_di::Container;
use oxbloom_test::{Captured, TestClient};
use tokio::sync::oneshot;

struct Ready(Mutex<Option<oneshot::Sender<SocketAddr>>>);

impl RunListener for Ready {
    fn ready(&self, container: &Container) {
        let addr = container.get::<WebServer>().unwrap().local_addr().unwrap();
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send(addr);
        }
    }
}

async fn within<F: std::future::Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

#[tokio::test]
async fn startup_logs_through_oxbloom_logging() {
    let logs = Captured::new();
    let writer = logs.clone();
    let (tx, rx) = oneshot::channel();
    // The example's own `application.toml` (levels root=info, todos=debug) plus
    // plain console output.
    let builder = todos::application(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(["--server.port=0", "--logging.console.color=never"])
        .logging_writer(move || writer.clone())
        .output(std::io::sink())
        .listener(Ready(Mutex::new(Some(tx))));
    let handle = builder.shutdown_handle();
    let run = tokio::spawn(builder.run());
    let addr = within(rx).await.expect("application ready");

    let body = r#"{"title":"logged"}"#;
    let reply = TestClient::connect(addr)
        .post("/api/todos")
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await;
    assert_eq!(reply.status().as_u16(), 201, "{}", reply.text());

    handle.trigger();
    within(run).await.unwrap().unwrap();

    let text = logs.contents();
    assert!(
        text.contains("oxbloom::boot"),
        "boot events missing: {text}"
    );
    assert!(text.contains("todos"), "{text}");
    assert!(
        text.contains("created todo"),
        "handler event missing: {text}"
    );
}
