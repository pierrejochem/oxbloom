//! The form login flow of the `/me` page, driven through a `TestClient` against the real
//! application. The test keeps the cookie by hand, as a browser would.

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

/// Starts the application and returns its client and the way to stop it.
async fn start() -> (
    TestClient,
    oxbloom_boot::ShutdownHandle,
    tokio::task::JoinHandle<Result<oxbloom_boot::ExitStatus, oxbloom_boot::StartupError>>,
) {
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
    (TestClient::connect(addr), handle, run)
}

/// The `name=value` pair of a `Set-Cookie` header.
fn pair(set_cookie: &str) -> &str {
    set_cookie.split(';').next().unwrap()
}

/// Posts the login form.
async fn login(client: &TestClient, user: &str, password: &str) -> oxbloom_test::TestResponse {
    client
        .post("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("username={user}&password={password}"))
        .send()
        .await
}

#[tokio::test]
async fn the_login_flow_of_the_me_page() {
    let (client, handle, run) = start().await;

    // A browser without a session is sent to the login page, and the target is saved.
    let reply = client.get("/me").header("accept", "text/html").send().await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login"));
    let anonymous = pair(
        reply
            .header("set-cookie")
            .expect("the saved request needs a session"),
    )
    .to_owned();

    // An API client gets 401 and no session.
    let reply = client
        .get("/me")
        .header("accept", "application/json")
        .send()
        .await;
    assert_eq!(reply.status(), 401);
    assert!(reply.header("set-cookie").is_none());

    // The default login page is served by the framework.
    let reply = client
        .get("/login")
        .header("accept", "text/html")
        .header("cookie", &anonymous)
        .send()
        .await;
    assert_eq!(reply.status(), 200);
    assert!(
        reply
            .text()
            .contains("<form method=\"post\" action=\"/login\">")
    );

    // A wrong password goes back to the login page with the error flag.
    let reply = login(&client, "admin", "wrong").await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login?error"));

    // The right password returns to the saved page and rotates the session id.
    let reply = client
        .post("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", &anonymous)
        .body("username=admin&password=changeit")
        .send()
        .await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/me"));
    let set_cookie = reply.header("set-cookie").expect("login sets a new cookie");
    let session = pair(set_cookie).to_owned();
    assert_ne!(session, anonymous);

    // The cookie now opens the page.
    let reply = client
        .get("/me")
        .header("accept", "text/html")
        .header("cookie", &session)
        .send()
        .await;
    assert_eq!(reply.status(), 200);
    assert!(reply.text().contains("admin"));

    // Logout ends the session.
    let reply = client
        .post("/logout")
        .header("cookie", &session)
        .send()
        .await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login?logout"));
    let reply = client
        .get("/me")
        .header("accept", "text/html")
        .header("cookie", &session)
        .send()
        .await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login"));

    handle.trigger();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("test timed out")
        .unwrap()
        .unwrap();
}
