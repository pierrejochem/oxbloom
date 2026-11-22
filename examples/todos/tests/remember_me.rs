//! Remember-me of the todos example, driven through a `TestClient` against the real application.
//! Each cookie jar is one browser: the test keeps the cookies by hand, as a browser would, and
//! reads the CSRF token from the hidden field of the page.

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

/// The `name=value` pair of the `Set-Cookie` header that sets `name`.
fn pair(reply: &TestResponse, name: &str) -> String {
    reply
        .header_all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("a Set-Cookie header for {name}"))
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

const SESSION: &str = "OXBLOOM_SESSION";
const REMEMBER: &str = "OXBLOOM_REMEMBER_ME";

/// Logs in as `admin` with or without the remember-me checkbox. Returns the session cookie pair
/// and the remember-me cookie pair, when the login set one.
async fn login(client: &TestClient, checkbox: bool) -> (String, Option<String>) {
    let reply = client.get("/me").header("accept", "text/html").send().await;
    assert_eq!(reply.status(), 302);
    let anonymous = pair(&reply, SESSION);

    let login_page = page(client, "/login", &anonymous).await;
    assert_eq!(login_page.status(), 200);
    let text = login_page.text();
    // The default login page carries the checkbox because remember-me is on.
    assert!(text.contains("name=\"remember-me\""), "{text}");
    let token = field_token(&text);

    let extra = if checkbox { "&remember-me=on" } else { "" };
    let reply = post_form(
        client,
        "/login",
        &anonymous,
        format!("_csrf={token}&username=admin&password=changeit{extra}"),
    )
    .await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/me"));
    let remember = reply
        .header_all("set-cookie")
        .into_iter()
        .any(|line| line.starts_with(&format!("{REMEMBER}=")))
        .then(|| pair(&reply, REMEMBER));
    (pair(&reply, SESSION), remember)
}

#[tokio::test]
async fn a_login_with_the_checkbox_sets_a_cookie_that_logs_in_again() {
    let (client, handle, run) = start().await;
    let (session, remember) = login(&client, true).await;
    let remember = remember.expect("the checkbox sets the remember-me cookie");
    assert_eq!(page(&client, "/me", &session).await.status(), 200);

    // A browser that lost its session cookie is logged in again by the remember-me cookie.
    let reply = page(&client, "/me", &remember).await;
    assert_eq!(reply.status(), 200);
    assert!(reply.text().contains("admin"));
    // The auto login starts a new session and sets its cookie.
    assert!(
        reply
            .header_all("set-cookie")
            .iter()
            .any(|line| line.starts_with("OXBLOOM_SESSION="))
    );

    stop(handle, run).await;
}

#[tokio::test]
async fn a_login_without_the_checkbox_sets_no_cookie() {
    let (client, handle, run) = start().await;
    let (session, remember) = login(&client, false).await;
    assert!(remember.is_none());
    assert_eq!(page(&client, "/me", &session).await.status(), 200);
    stop(handle, run).await;
}

#[tokio::test]
async fn the_cookie_is_http_only_lax_and_lasts_fourteen_days() {
    let (client, handle, run) = start().await;
    let reply = client.get("/me").header("accept", "text/html").send().await;
    let anonymous = pair(&reply, SESSION);
    let token = field_token(&page(&client, "/login", &anonymous).await.text());
    let reply = post_form(
        &client,
        "/login",
        &anonymous,
        format!("_csrf={token}&username=admin&password=changeit&remember-me=true"),
    )
    .await;
    let line = reply
        .header_all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(&format!("{REMEMBER}=")))
        .expect("a remember-me cookie")
        .to_owned();
    assert!(line.contains("HttpOnly"), "{line}");
    assert!(line.contains("SameSite=Lax"), "{line}");
    assert!(line.contains("Max-Age=1209600"), "{line}");
    stop(handle, run).await;
}

#[tokio::test]
async fn a_tampered_cookie_is_refused_and_cleared() {
    let (client, handle, run) = start().await;
    let (_, remember) = login(&client, true).await;
    let remember = remember.expect("a remember-me cookie");

    // Changing the last character breaks the signature.
    let mut tampered = remember.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'A' { 'B' } else { 'A' });
    let reply = page(&client, "/me", &tampered).await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login"));
    let clearing = reply
        .header_all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(&format!("{REMEMBER}=")))
        .expect("a clearing cookie")
        .to_owned();
    assert!(clearing.contains("Max-Age=0"), "{clearing}");

    // A cookie for another user name does not verify either.
    let value = remember.split_once('=').unwrap().1;
    let mut parts: Vec<&str> = value.split('.').collect();
    parts[1] = "dmlld2Vy"; // base64url of "viewer"
    let forged = format!("{REMEMBER}={}", parts.join("."));
    let reply = page(&client, "/me", &forged).await;
    assert_eq!(reply.status(), 302);
    let clearing = reply
        .header_all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(&format!("{REMEMBER}=")))
        .expect("a clearing cookie")
        .to_owned();
    assert!(clearing.contains("Max-Age=0"), "{clearing}");

    // The genuine cookie still works.
    assert_eq!(page(&client, "/me", &remember).await.status(), 200);

    stop(handle, run).await;
}

#[tokio::test]
async fn any_authorization_header_blocks_the_auto_login() {
    let (client, handle, run) = start().await;
    let (_, remember) = login(&client, true).await;
    let remember = remember.expect("a remember-me cookie");

    // The valid cookie alone logs in.
    assert_eq!(page(&client, "/me", &remember).await.status(), 200);

    // The same valid cookie with a header of a scheme that the chain does not authenticate is
    // not logged in: the header is what blocks the cookie, and the browser goes to the login page.
    let reply = client
        .get("/me")
        .header("accept", "text/html")
        .header("cookie", &remember)
        .header("authorization", "Digest x")
        .send()
        .await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login"));

    // A wrong Basic credential is a 401.
    let reply = client
        .get("/me")
        .header("accept", "text/html")
        .header("cookie", &remember)
        .header("authorization", "Basic YWRtaW46d3Jvbmc=")
        .send()
        .await;
    assert_eq!(reply.status(), 401);

    stop(handle, run).await;
}

#[tokio::test]
async fn logout_clears_the_cookie() {
    let (client, handle, run) = start().await;
    let (session, remember) = login(&client, true).await;
    let remember = remember.expect("a remember-me cookie");
    let both = format!("{session}; {remember}");
    let token = field_token(&page(&client, "/me", &both).await.text());

    let reply = post_form(&client, "/logout", &both, format!("_csrf={token}")).await;
    assert_eq!(reply.status(), 302);
    assert_eq!(reply.header("location"), Some("/login?logout"));
    let clearing = reply
        .header_all("set-cookie")
        .into_iter()
        .find(|line| line.starts_with(&format!("{REMEMBER}=")))
        .expect("a clearing remember-me cookie")
        .to_owned();
    assert!(clearing.contains("Max-Age=0"), "{clearing}");

    stop(handle, run).await;
}
