//! Session management of the todos example, driven through a `TestClient` against the real
//! application. Each cookie jar is one browser: the test keeps the cookie by hand, as a browser
//! would, and reads the CSRF token from the hidden field of the page.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use oxbloom_boot::{RunListener, WebServer};
use oxbloom_di::Container;
use oxbloom_test::{TestClient, TestResponse};
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

type Run = tokio::task::JoinHandle<Result<oxbloom_boot::ExitStatus, oxbloom_boot::StartupError>>;

/// Starts the application and returns its client and the way to stop it.
async fn start() -> (TestClient, oxbloom_boot::ShutdownHandle, Run) {
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

async fn stop(handle: oxbloom_boot::ShutdownHandle, run: Run) {
    handle.trigger();
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("test timed out")
        .unwrap()
        .unwrap();
}

/// The `name=value` pair of a `Set-Cookie` header.
fn pair(reply: &TestResponse) -> String {
    reply
        .header("set-cookie")
        .expect("a Set-Cookie header")
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

/// The value of the hidden `_csrf` field of a page.
fn field_token(page: &str) -> String {
    let marker = "name=\"_csrf\" value=\"";
    let start = page.find(marker).expect("the page has the hidden field") + marker.len();
    let end = start + page[start..].find('"').unwrap();
    page[start..end].to_owned()
}

/// Posts a urlencoded form.
async fn post_form(client: &TestClient, path: &str, cookie: &str, body: String) -> TestResponse {
    client
        .post(path)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", cookie)
        .body(body)
        .send()
        .await
}

/// Reads a page of the browser flow.
async fn page(client: &TestClient, path: &str, cookie: &str) -> TestResponse {
    client
        .get(path)
        .header("accept", "text/html")
        .header("cookie", cookie)
        .send()
        .await
}

/// Logs in as `admin` the way a browser does. Returns the session cookie and the login token
/// that was used.
async fn login(client: &TestClient) -> (String, String) {
    let reply = client.get("/me").header("accept", "text/html").send().await;
    assert_eq!(reply.status(), 302);
    let anonymous = pair(&reply);

    // The token comes from the hidden field of the default login page.
    let login_page = page(client, "/login", &anonymous).await;
    assert_eq!(login_page.status(), 200);
    assert_eq!(login_page.header("x-frame-options"), Some("DENY"));
    let token = field_token(&login_page.text());

    // The login post without the token is refused.
    let reply = post_form(
        client,
        "/login",
        &anonymous,
        "username=admin&password=changeit".to_owned(),
    )
    .await;
    assert_eq!(reply.status(), 403);

    let reply = post_form(
        client,
        "/login",
        &anonymous,
        format!("_csrf={token}&username=admin&password=changeit"),
    )
    .await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/me"));
    (pair(&reply), token)
}

#[tokio::test]
async fn a_page_carries_the_default_security_headers() {
    let (client, handle, run) = start().await;
    let (session, _) = login(&client).await;

    let me = page(&client, "/me", &session).await;
    assert_eq!(me.status(), 200);
    assert_eq!(me.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(me.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(me.header("x-frame-options"), Some("DENY"));
    assert_eq!(me.header("cache-control"), Some("no-store"));
    // The request is plain HTTP, so there is no HSTS.
    assert_eq!(me.header("strict-transport-security"), None);

    stop(handle, run).await;
}

#[tokio::test]
async fn a_third_login_ends_the_oldest_session() {
    let (client, handle, run) = start().await;
    let (first, _) = login(&client).await;
    let (second, _) = login(&client).await;
    assert_eq!(page(&client, "/me", &first).await.status(), 200);
    assert_eq!(page(&client, "/me", &second).await.status(), 200);

    // The limit is 2, so the third login evicts the session that was touched longest ago. The
    // page reads above touched both, `second` last.
    let (third, _) = login(&client).await;
    let reply = page(&client, "/me", &first).await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login"));
    assert_eq!(page(&client, "/me", &second).await.status(), 200);
    assert_eq!(page(&client, "/me", &third).await.status(), 200);

    stop(handle, run).await;
}

#[tokio::test]
async fn logout_everywhere_keeps_only_the_current_session() {
    let (client, handle, run) = start().await;
    let (other, _) = login(&client).await;
    let (current, _) = login(&client).await;
    let token = field_token(&page(&client, "/me", &current).await.text());

    // Without the token the route is refused and nothing ends.
    let reply = post_form(&client, "/me/logout-everywhere", &current, "x=1".to_owned()).await;
    assert_eq!(reply.status(), 403);
    assert_eq!(page(&client, "/me", &other).await.status(), 200);

    // With the token the other session ends and the current one stays.
    let reply = post_form(
        &client,
        "/me/logout-everywhere",
        &current,
        format!("_csrf={token}"),
    )
    .await;
    assert_eq!(reply.status(), 303);
    assert_eq!(reply.header("location"), Some("/me"));
    let reply = page(&client, "/me", &other).await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login"));
    assert_eq!(page(&client, "/me", &current).await.status(), 200);

    stop(handle, run).await;
}
