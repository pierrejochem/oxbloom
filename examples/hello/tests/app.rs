//! Runs the hello application in-process on a free port and drives it with
//! a `TestClient`, ended by a `ShutdownHandle` instead of a signal.

// These tests keep their own Running/Ready/start harness: `TestApp` cannot take the
// application's own `OxbloomApplicationBuilder`.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use jsonwebtoken::{EncodingKey, Header, encode};
use oxbloom_boot::{ExitStatus, OxbloomApplicationBuilder, RunListener, StartupError, WebServer};
use oxbloom_di::Container;
use oxbloom_security::Algorithm;
use oxbloom_test::{Captured, TestClient};
use tokio::sync::oneshot;

const SECRET: &[u8] = b"a-test-only-secret-of-32-bytes!!";

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

/// A running application: its address, its shutdown handle and the run.
struct Running {
    addr: SocketAddr,
    handle: oxbloom_boot::ShutdownHandle,
    run: tokio::task::JoinHandle<Result<ExitStatus, StartupError>>,
}

impl Running {
    async fn stop(self) -> ExitStatus {
        self.handle.trigger();
        within(self.run).await.unwrap().unwrap()
    }
}

async fn within<F: std::future::Future>(f: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(5), f)
        .await
        .expect("test timed out")
}

async fn start(dir: &Path, args: &[&str]) -> Running {
    let (tx, rx) = oneshot::channel();
    let builder: OxbloomApplicationBuilder = hello::application(dir)
        .args(args.iter().copied())
        .output(std::io::sink())
        .listener(Ready(Mutex::new(Some(tx))));
    let handle = builder.shutdown_handle();
    let run = tokio::spawn(builder.run());
    let addr = within(rx).await.expect("application ready");
    Running { addr, handle, run }
}

/// Sends one GET request and returns the status code and body.
async fn get(addr: SocketAddr, path: &str, token: Option<&str>) -> (u16, String) {
    let client = TestClient::connect(addr);
    let mut request = client.get(path);
    if let Some(t) = token {
        request = request.header("authorization", format!("Bearer {t}"));
    }
    let reply = request.send().await;
    (reply.status().as_u16(), reply.text())
}

fn example_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[tokio::test]
async fn greets_and_protects_the_admin_route() {
    let app = start(example_dir(), &["--server.port=0"]).await;

    assert_eq!(
        get(app.addr, "/api/hello", None).await,
        (200, "Hello, world!".into())
    );
    assert_eq!(
        get(app.addr, "/api/hello/Ada", None).await,
        (200, "Hello, Ada!".into())
    );
    // Every greeter, by `order`.
    assert_eq!(
        get(app.addr, "/api/greetings/Ada", None).await,
        (200, "configured: Hello, Ada!\ndutch: Hallo, Ada!".into())
    );
    // No token: 401, also for an unknown path.
    assert_eq!(get(app.addr, "/api/admin/visits", None).await.0, 401);
    assert_eq!(get(app.addr, "/nope", None).await.0, 401);

    assert_eq!(app.stop().await.code(), 0);
}

#[tokio::test]
async fn dev_profile_changes_the_greeting() {
    let app = start(
        example_dir(),
        &["--server.port=0", "--oxbloom.profiles.active=dev"],
    )
    .await;
    assert_eq!(
        get(app.addr, "/api/hello", None).await,
        (200, "Hi from dev, world!".into())
    );
    assert_eq!(app.stop().await.code(), 0);
}

#[tokio::test]
async fn exit_code_comes_from_the_configuration() {
    let app = start(example_dir(), &["--server.port=0", "--hello.exit-code=3"]).await;
    assert_eq!(app.stop().await.code(), 3);
}

#[tokio::test]
async fn a_valid_token_reaches_the_admin_route() {
    let dir = tempfile::tempdir().unwrap();
    let secret = std::str::from_utf8(SECRET).unwrap();
    std::fs::write(
        dir.path().join("application.toml"),
        format!(
            "[hello]\ngreeting = \"Hello\"\n[server]\nport = 0\n\
             [security.jwt]\nalgorithms = [\"HS256\"]\nsecret = \"{secret}\"\n"
        ),
    )
    .unwrap();
    let app = start(dir.path(), &[]).await;

    let exp = jsonwebtoken::get_current_timestamp() + 300;
    let claims = serde_json::json!({ "sub": "alice", "exp": exp });
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(SECRET).unwrap(),
    )
    .unwrap();
    get(app.addr, "/api/hello", None).await;
    assert_eq!(
        get(app.addr, "/api/admin/visits", Some(&token)).await,
        (200, "1 visits so far, asked by alice".into())
    );
    assert_eq!(
        get(app.addr, "/api/admin/visits", Some("not-a-token"))
            .await
            .0,
        401
    );
    assert_eq!(app.stop().await.code(), 0);
}

#[tokio::test]
async fn empty_greeting_gets_the_custom_report() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("application.toml"),
        "[hello]\ngreeting = \"\"\n[server]\nport = 0\n",
    )
    .unwrap();
    let out = Captured::new();
    let error = within(
        hello::application(dir.path())
            .output(out.clone())
            .failure_report(true)
            .run(),
    )
    .await
    .unwrap_err();

    let description =
        "The configuration key `hello.greeting` is empty, so there is nothing to greet with.";
    assert_eq!(error.analysis().unwrap().description(), description);
    let report = out.contents();
    assert!(report.contains("APPLICATION FAILED TO START"), "{report}");
    assert!(report.contains(description), "{report}");
    assert!(
        report.ends_with(
            "Action:\n\nSet `hello.greeting` in `application.toml` or the active profile file \
             to a non-empty text.\n"
        ),
        "{report}"
    );
}

#[tokio::test]
async fn bad_security_pattern_gets_the_security_report() {
    use oxbloom_di::DiError;
    use oxbloom_security::{HttpSecurity, SecurityFilterChain};

    // A test-only second chain with a pattern that `build()` rejects.
    struct BadChain(#[allow(dead_code)] SecurityFilterChain);

    let out = Captured::new();
    let error = within(
        hello::application(example_dir())
            .args(["--server.port=0"])
            .output(out.clone())
            .failure_report(true)
            .configure(|b| {
                b.try_bean(|| -> Result<BadChain, DiError> {
                    let chain = HttpSecurity::new()
                        .authorize(|a| a.get("/users/{id}").permit_all())
                        .build()?;
                    Ok(BadChain(chain))
                })
            })
            .run(),
    )
    .await
    .unwrap_err();

    let description = error.analysis().unwrap().description();
    assert!(
        description.starts_with("A security matcher pattern is not valid."),
        "{description}"
    );
    let report = out.contents();
    assert!(
        report.ends_with("Use `/users/*` or `/users/**` and match the rest in a handler.\n"),
        "{report}"
    );
}
