//! The form login flow of the `/me` page, driven through a `TestClient` against the real
//! application. The test keeps the cookie by hand, as a browser would, and sends the CSRF token
//! it reads from the pages.

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

/// The value of the hidden `_csrf` field of a page.
fn field_token(page: &str) -> String {
    let marker = "name=\"_csrf\" value=\"";
    let start = page.find(marker).expect("the page has the hidden field") + marker.len();
    let end = start + page[start..].find('"').unwrap();
    page[start..end].to_owned()
}

/// Posts the login form with the session cookie and the token of the login page.
async fn login(
    client: &TestClient,
    cookie: &str,
    token: &str,
    user: &str,
    password: &str,
) -> oxbloom_test::TestResponse {
    client
        .post("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", cookie)
        .body(format!("_csrf={token}&username={user}&password={password}"))
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
    let login_page = reply.text();
    assert!(login_page.contains("<form method=\"post\" action=\"/login\">"));
    // The page carries the CSRF token of the session and cannot be framed.
    let token = field_token(&login_page);
    assert_eq!(reply.header("x-frame-options"), Some("DENY"));

    // A login post without the token is refused before the password is looked at.
    let reply = client
        .post("/login")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", &anonymous)
        .body("username=admin&password=changeit")
        .send()
        .await;
    assert_eq!(reply.status(), 403);

    // A wrong password goes back to the login page with the error flag.
    let reply = login(&client, &anonymous, &token, "admin", "wrong").await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login?error"));

    // The right password returns to the saved page and rotates the session id.
    let reply = login(&client, &anonymous, &token, "admin", "changeit").await;
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
    let page = reply.text();
    assert!(page.contains("admin"));
    // The login replaced the token, and the page carries the new one.
    let session_token = field_token(&page);
    assert_ne!(session_token, token);

    // Logout needs the token, and the form of the page posts it.
    let reply = client
        .post("/logout")
        .header("cookie", &session)
        .send()
        .await;
    assert_eq!(reply.status(), 403);
    let reply = client
        .post("/logout")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", &session)
        .body(format!("_csrf={session_token}"))
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
