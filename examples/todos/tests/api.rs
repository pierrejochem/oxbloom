//! Runs the todos application in-process on a free port and drives it with
//! a `TestClient`, ended by a `ShutdownHandle` instead of a signal. Every app here
//! runs with `.logging(false)`: only one app per process can install the
//! global subscriber, and `tests/logging.rs` owns that.

// These tests keep their own Running/Ready/start harness: `TestApp` cannot take the
// application's own `OxbloomApplicationBuilder`.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use oxbloom_boot::{ExitStatus, RunListener, StartupError, WebServer};
use oxbloom_di::Container;
use oxbloom_test::TestClient;
use serde_json::{Value, json};
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

async fn start() -> Running {
    let (tx, rx) = oneshot::channel();
    let builder = todos::application(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args(["--server.port=0"])
        .logging(false)
        .output(std::io::sink())
        .listener(Ready(Mutex::new(Some(tx))));
    let handle = builder.shutdown_handle();
    let run = tokio::spawn(builder.run());
    let addr = within(rx).await.expect("application ready");
    Running { addr, handle, run }
}

/// One response: status, the raw header block (lower-cased) and the body.
struct Reply {
    status: u16,
    headers: String,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {:?}", self.body))
    }
}

async fn send(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> Reply {
    send_with(addr, method, path, body, Some("application/json")).await
}

/// Like `send`, but with an explicit `Content-Type` (or none) for a body.
async fn send_with(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
    content_type: Option<&str>,
) -> Reply {
    let client = TestClient::connect(addr);
    let body = body.unwrap_or_default();
    let mut request = match method {
        "GET" => client.get(path),
        "POST" => client.post(path),
        "PUT" => client.put(path),
        "PATCH" => client.patch(path),
        "DELETE" => client.delete(path),
        other => panic!("unsupported method {other}"),
    };
    if let (false, Some(value)) = (body.is_empty(), content_type) {
        request = request.header("content-type", value);
    }
    let reply = request.body(body.to_owned()).send().await.into_inner();
    let headers = reply
        .headers()
        .iter()
        .map(|(name, value)| format!("{name}: {}", value.to_str().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\r\n")
        .to_ascii_lowercase();
    Reply {
        status: reply.status().as_u16(),
        headers,
        body: String::from_utf8_lossy(reply.body()).into_owned(),
    }
}

async fn create(app: &Running, title: &str) -> Value {
    let body = json!({ "title": title }).to_string();
    send(app.addr, "POST", "/api/todos", Some(&body))
        .await
        .json()
}

#[tokio::test]
async fn create_returns_201_location_and_body() {
    let app = start().await;
    let res = send(app.addr, "POST", "/api/todos", Some(r#"{"title":"a"}"#)).await;
    assert_eq!(res.status, 201);
    assert!(
        res.headers.contains("location: /api/todos/1"),
        "{}",
        res.headers
    );
    assert!(res.headers.contains("content-type: application/json"));
    assert_eq!(res.json(), json!({ "id": 1, "title": "a", "done": false }));
    app.stop().await;
}

#[tokio::test]
async fn get_returns_created_todo() {
    let app = start().await;
    create(&app, "a").await;
    let res = send(app.addr, "GET", "/api/todos/1", None).await;
    assert_eq!(res.status, 200);
    assert!(res.headers.contains("content-type: application/json"));
    assert_eq!(res.json(), json!({ "id": 1, "title": "a", "done": false }));
    app.stop().await;
}

#[tokio::test]
async fn list_returns_all_and_filters_by_done() {
    let app = start().await;
    create(&app, "a").await;
    create(&app, "b").await;
    send(app.addr, "PATCH", "/api/todos/2", Some(r#"{"done":true}"#)).await;

    let all = send(app.addr, "GET", "/api/todos", None).await;
    assert_eq!(all.status, 200);
    assert_eq!(all.json().as_array().unwrap().len(), 2);
    let done = send(app.addr, "GET", "/api/todos?done=true", None)
        .await
        .json();
    assert_eq!(done, json!([{ "id": 2, "title": "b", "done": true }]));
    let open = send(app.addr, "GET", "/api/todos?done=false", None)
        .await
        .json();
    assert_eq!(open, json!([{ "id": 1, "title": "a", "done": false }]));
    assert_eq!(
        send(app.addr, "GET", "/api/todos?done=maybe", None)
            .await
            .status,
        400
    );
    app.stop().await;
}

#[tokio::test]
async fn put_replaces_all_fields() {
    let app = start().await;
    create(&app, "a").await;
    let res = send(
        app.addr,
        "PUT",
        "/api/todos/1",
        Some(r#"{"title":"b","done":true}"#),
    )
    .await;
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "id": 1, "title": "b", "done": true }));
    // A body without `done` is not a full replacement.
    let res = send(app.addr, "PUT", "/api/todos/1", Some(r#"{"title":"c"}"#)).await;
    assert_eq!(res.status, 400);
    app.stop().await;
}

#[tokio::test]
async fn patch_changes_only_given_fields() {
    let app = start().await;
    create(&app, "a").await;
    let res = send(app.addr, "PATCH", "/api/todos/1", Some(r#"{"done":true}"#)).await;
    assert_eq!(res.json(), json!({ "id": 1, "title": "a", "done": true }));
    let res = send(app.addr, "PATCH", "/api/todos/1", Some(r#"{"title":"z"}"#)).await;
    assert_eq!(res.json(), json!({ "id": 1, "title": "z", "done": true }));
    app.stop().await;
}

#[tokio::test]
async fn delete_returns_204_then_get_is_404() {
    let app = start().await;
    create(&app, "a").await;
    let res = send(app.addr, "DELETE", "/api/todos/1", None).await;
    assert_eq!(res.status, 204);
    assert_eq!(res.body, "");
    assert_eq!(
        send(app.addr, "GET", "/api/todos/1", None).await.status,
        404
    );
    // Ids are not reused.
    assert_eq!(create(&app, "b").await["id"], 2);
    app.stop().await;
}

#[tokio::test]
async fn unknown_id_is_404_for_get_put_patch_delete() {
    let app = start().await;
    let put = r#"{"title":"x","done":false}"#;
    for (method, body) in [
        ("GET", None),
        ("PUT", Some(put)),
        ("PATCH", Some("{}")),
        ("DELETE", None),
    ] {
        let res = send(app.addr, method, "/api/todos/99", body).await;
        assert_eq!(res.status, 404, "{method}");
        assert!(
            res.headers
                .contains("content-type: application/problem+json"),
            "{method}"
        );
        let body = res.json();
        assert_eq!(body["detail"], "no todo with this id", "{method}");
        assert_eq!(body["instance"], "/api/todos/99", "{method}");
    }
    // PUT and PATCH never create.
    let all = send(app.addr, "GET", "/api/todos", None).await;
    assert_eq!(all.json(), json!([]));
    app.stop().await;
}

#[tokio::test]
async fn non_numeric_id_is_400() {
    let app = start().await;
    for method in ["GET", "DELETE"] {
        let res = send(app.addr, method, "/api/todos/abc", None).await;
        assert_eq!(res.status, 400, "{method}");
        assert!(
            res.headers
                .contains("content-type: application/problem+json"),
            "{method}"
        );
        assert!(res.json()["detail"].is_string(), "{method}");
    }
    app.stop().await;
}

#[tokio::test]
async fn bad_json_is_400_problem_json() {
    let app = start().await;
    let res = send(app.addr, "POST", "/api/todos", Some("{not json")).await;
    assert_eq!(res.status, 400);
    assert!(
        res.headers
            .contains("content-type: application/problem+json")
    );
    assert!(res.json()["detail"].is_string());
    assert_eq!(
        send(app.addr, "POST", "/api/todos", Some("{}"))
            .await
            .status,
        400
    );
    app.stop().await;
}

#[tokio::test]
async fn post_without_content_type_is_415() {
    let app = start().await;
    let res = send_with(
        app.addr,
        "POST",
        "/api/todos",
        Some(r#"{"title":"a"}"#),
        None,
    )
    .await;
    assert_eq!(res.status, 415);
    assert!(
        res.headers
            .contains("content-type: application/problem+json")
    );
    assert_eq!(
        send(app.addr, "GET", "/api/todos", None).await.json(),
        json!([])
    );
    app.stop().await;
}

#[tokio::test]
async fn patch_with_wrong_content_type_is_415() {
    let app = start().await;
    create(&app, "a").await;
    let res = send_with(
        app.addr,
        "PATCH",
        "/api/todos/1",
        Some(r#"{"done":true}"#),
        Some("text/plain"),
    )
    .await;
    assert_eq!(res.status, 415);
    assert!(
        res.headers
            .contains("content-type: application/problem+json")
    );
    app.stop().await;
}

#[tokio::test]
async fn empty_title_is_422() {
    let app = start().await;
    for title in ["", "   "] {
        let body = json!({ "title": title }).to_string();
        let res = send(app.addr, "POST", "/api/todos", Some(&body)).await;
        assert_eq!(res.status, 422, "{title:?}");
        assert!(
            res.headers
                .contains("content-type: application/problem+json")
        );
        assert_eq!(res.json()["detail"], "the title must not be empty");
    }
    create(&app, "a").await;
    let res = send(app.addr, "PATCH", "/api/todos/1", Some(r#"{"title":" "}"#)).await;
    assert_eq!(res.status, 422);
    let res = send(
        app.addr,
        "PUT",
        "/api/todos/1",
        Some(r#"{"title":"","done":false}"#),
    )
    .await;
    assert_eq!(res.status, 422);
    app.stop().await;
}

#[tokio::test]
async fn unprotected_path_is_401() {
    let app = start().await;
    assert_eq!(send(app.addr, "GET", "/api/other", None).await.status, 401);
    assert_eq!(send(app.addr, "GET", "/nope", None).await.status, 401);
    app.stop().await;
}

#[tokio::test]
async fn open_path_stops_at_the_todos_segment() {
    // `/api/todos/**` matches `/api/todos` and below, not a longer segment.
    let app = start().await;
    assert_eq!(send(app.addr, "GET", "/api/todosx", None).await.status, 401);
    app.stop().await;
}

#[tokio::test]
async fn actuator_health_is_up_without_authentication() {
    let app = start().await;
    let res = send(app.addr, "GET", "/actuator/health", None).await;
    assert_eq!(res.status, 200, "{}", res.body);
    assert_eq!(res.json(), json!({ "status": "UP" }));
    app.stop().await;
}

#[tokio::test]
async fn actuator_info_needs_authentication_and_reports_the_build() {
    let app = start().await;
    // Only the health probes are open; without a `security.jwt` the info is a 401.
    let res = send(app.addr, "GET", "/actuator/info", None).await;
    assert_eq!(res.status, 401, "{}", res.body);
    app.stop().await;
    // The build facts come from `build.rs`; the git fields may be absent.
    let build = serde_json::to_value(todos::build_info()).unwrap();
    assert_eq!(build["name"], "todos");
    assert!(build["version"].is_string());
}
