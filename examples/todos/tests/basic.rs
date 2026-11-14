//! HTTP Basic on `GET /api/admin/stats`: the configured demo users `admin` (role `ADMIN`)
//! and `viewer` (role `USER`), driven through a `TestClient` against the real application.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use oxbloom_boot::{RunListener, WebServer};
use oxbloom_di::Container;
use oxbloom_test::TestClient;
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

/// The base64 of `user:password`, for the `Authorization` header.
fn basic(user: &str, password: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = format!("{user}:{password}").into_bytes();
    let mut out = String::from("Basic ");
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Starts the application, asks `GET /api/admin/stats` with the given credentials and
/// returns the status and the `WWW-Authenticate` header.
async fn stats(credentials: Option<(&str, &str)>) -> (u16, Option<String>, String) {
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
    let mut request = client.get("/api/admin/stats");
    if let Some((user, password)) = credentials {
        request = request.header("authorization", basic(user, password));
    }
    let reply = request.send().await.into_inner();
    let challenge = reply
        .headers()
        .get("www-authenticate")
        .map(|v| v.to_str().unwrap().to_owned());
    let result = (
        reply.status().as_u16(),
        challenge,
        String::from_utf8_lossy(reply.body()).into_owned(),
    );

    handle.trigger();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("test timed out")
        .unwrap()
        .unwrap();
    result
}

#[tokio::test]
async fn anonymous_gets_401_with_the_basic_challenge() {
    let (status, challenge, _) = stats(None).await;
    assert_eq!(status, 401);
    assert!(challenge.unwrap().starts_with("Basic"));
}

#[tokio::test]
async fn admin_with_the_demo_password_gets_200() {
    let (status, _, body) = stats(Some(("admin", "changeit"))).await;
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"total":0,"done":0}"#);
}

#[tokio::test]
async fn a_wrong_password_gets_401() {
    let (status, challenge, _) = stats(Some(("admin", "wrong"))).await;
    assert_eq!(status, 401);
    assert!(challenge.unwrap().starts_with("Basic"));
}

#[tokio::test]
async fn a_user_without_the_admin_role_gets_403() {
    let (status, _, _) = stats(Some(("viewer", "viewonly"))).await;
    assert_eq!(status, 403);
}
