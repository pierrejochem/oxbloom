//! CSRF tokens in the form login flow, driven through a `TestClient` against the real
//! application. The test keeps the cookie by hand, as a browser would, and reads the token the
//! way a browser does: from the hidden field of the page.

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
async fn a_logged_in_post_needs_the_token() {
    let (client, handle, run) = start().await;
    let (session, login_token) = login(&client).await;

    // The page of the logged-in user carries the token in both forms.
    let me = page(&client, "/me", &session).await;
    assert_eq!(me.status(), 200);
    let text = me.text();
    assert_eq!(text.matches("name=\"_csrf\"").count(), 2);
    let token = field_token(&text);
    // The login replaced the token, so the one of the login page is dead.
    assert_ne!(token, login_token);

    // A POST of the JSON API without the token is refused, with the wrong token too.
    let reply = client
        .post("/api/todos")
        .header("content-type", "application/json")
        .header("cookie", &session)
        .body(r#"{"title":"no token"}"#)
        .send()
        .await;
    assert_eq!(reply.status(), 403);
    let reply = client
        .post("/api/todos")
        .header("content-type", "application/json")
        .header("cookie", &session)
        .header("x-csrf-token", &login_token)
        .body(r#"{"title":"old token"}"#)
        .send()
        .await;
    assert_eq!(reply.status(), 403);

    // With the header it succeeds.
    let reply = client
        .post("/api/todos")
        .header("content-type", "application/json")
        .header("cookie", &session)
        .header("x-csrf-token", &token)
        .body(r#"{"title":"with header"}"#)
        .send()
        .await;
    assert_eq!(reply.status(), 201);

    // Reads need no token.
    let reply = client
        .get("/api/todos")
        .header("cookie", &session)
        .send()
        .await;
    assert_eq!(reply.status(), 200);

    // A request without the session cookie is anonymous and not checked.
    let reply = client
        .post("/api/todos")
        .header("content-type", "application/json")
        .body(r#"{"title":"anonymous"}"#)
        .send()
        .await;
    assert_eq!(reply.status(), 201);

    stop(handle, run).await;
}

#[tokio::test]
async fn the_form_field_of_the_page_works() {
    let (client, handle, run) = start().await;
    let (session, _) = login(&client).await;
    let token = field_token(&page(&client, "/me", &session).await.text());

    // The reset form of the page posts the field, and the route answers a redirect.
    let reply = post_form(&client, "/me/reset-visits", &session, "x=1".to_owned()).await;
    assert_eq!(reply.status(), 403);
    let reply = post_form(
        &client,
        "/me/reset-visits",
        &session,
        format!("_csrf={token}"),
    )
    .await;
    assert_eq!(reply.status(), 303);
    assert_eq!(reply.header("location"), Some("/me"));

    // The token in the query string does not count.
    let reply = client
        .post(&format!("/me/reset-visits?_csrf={token}"))
        .header("cookie", &session)
        .send()
        .await;
    assert_eq!(reply.status(), 403);

    stop(handle, run).await;
}

#[tokio::test]
async fn logout_needs_the_token() {
    let (client, handle, run) = start().await;
    let (session, _) = login(&client).await;
    let token = field_token(&page(&client, "/me", &session).await.text());

    // Without the token the session stays.
    let reply = client
        .post("/logout")
        .header("cookie", &session)
        .send()
        .await;
    assert_eq!(reply.status(), 403);
    assert_eq!(page(&client, "/me", &session).await.status(), 200);

    // With the token the logout form works and the session ends.
    let reply = post_form(&client, "/logout", &session, format!("_csrf={token}")).await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login?logout"));
    let reply = page(&client, "/me", &session).await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login"));

    stop(handle, run).await;
}
