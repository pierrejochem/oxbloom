//! `GET /api/visits`: a counter in the session, driven through a `TestClient` against the
//! real application.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use oxbloom_boot::{RunListener, WebServer};
use oxbloom_di::Container;
use oxbloom_test::TestClient;
use serde_json::Value;
use tokio::sync::oneshot;

/// Sends the server's address once the application is ready.
struct Ready(Mutex<Option<oneshot::Sender<SocketAddr>>>);

impl RunListener for Ready {
    fn ready(&self, container: &Container) {
        let addr = container.get::<WebServer>().unwrap().local_addr().unwrap();
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send(addr);
        }
    }
}

/// One answer of `GET /api/visits`: the count and the `Set-Cookie` header, if any.
struct Visit {
    visits: u64,
    set_cookie: Option<String>,
}

/// Asks `GET /api/visits` with the given `Cookie` header.
async fn visit(client: &TestClient, cookie: Option<&str>) -> Visit {
    let mut request = client.get("/api/visits");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    let reply = request.send().await;
    assert_eq!(reply.status(), 200);
    let body: Value = serde_json::from_slice(reply.bytes()).expect("JSON");
    Visit {
        visits: body["visits"].as_u64().expect("a number"),
        set_cookie: reply.header("set-cookie").map(str::to_owned),
    }
}

/// The `name=value` pair of a `Set-Cookie` header.
fn pair(set_cookie: &str) -> &str {
    set_cookie.split(';').next().unwrap()
}

#[tokio::test]
async fn the_counter_lives_in_the_session() {
    let (tx, rx) = oneshot::channel();
    let builder = todos::application(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(["--server.port=0"])
        .logging(false)
        .output(std::io::sink())
        .listener(Ready(Mutex::new(Some(tx))));
    let handle = builder.shutdown_handle();
    let run = tokio::spawn(builder.run());
    let addr = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("test timed out")
        .expect("application ready");
    let client = TestClient::connect(addr);

    let first = visit(&client, None).await;
    assert_eq!(first.visits, 1);
    let set_cookie = first.set_cookie.expect("the first answer sets the cookie");
    assert!(set_cookie.starts_with("OXBLOOM_SESSION="));
    assert!(set_cookie.contains("HttpOnly"));
    assert!(set_cookie.contains("SameSite=Lax"));

    let cookie = pair(&set_cookie);
    assert_eq!(visit(&client, Some(cookie)).await.visits, 2);
    assert_eq!(visit(&client, Some(cookie)).await.visits, 3);

    // A request without the cookie is a new session.
    assert_eq!(visit(&client, None).await.visits, 1);

    handle.trigger();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("test timed out")
        .unwrap()
        .unwrap();
}
